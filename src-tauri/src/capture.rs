//! Screenshots for `appv screenshot`: the page of an app (WebView2 `CapturePreview`) or the whole
//! App Viewer window (shell UI and app captured separately, then composed). Images go to a dedicated folder and the CLI gets the path,
//! so an agent can open the PNG with its image reader instead of driving a headless browser.

use crate::{state::{emit_state, AppState, Status}, views};
use serde_json::{json, Value};
use std::{
    fs,
    mem,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager};
use webview2_com::{
    CapturePreviewCompletedHandler, Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
};
use windows::Win32::{
    System::Com::{IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET},
    UI::Shell::SHCreateMemStream,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

const KEEP_FILES: usize = 50;
const KEEP_FOR: Duration = Duration::from_secs(24 * 3600);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Takes the screenshot and returns `{ path, width, height, app, url, scope }`. Without an app
/// (whole window only), the window is captured as it is.
pub fn screenshot(app: &AppHandle, id: Option<&str>, whole_window: bool) -> Result<Value, String> {
    let st = app.state::<AppState>();
    let cfg = match id {
        Some(id) => Some(st.app(id).ok_or("Application inconnue.")?),
        None if whole_window => None,
        None => return Err("Précisez l'application à capturer.".into()),
    };
    match &cfg {
        Some(cfg) => bring_forward(app, &cfg.id)?,
        None => drop(show_window(app)?),
    }

    let (png, scope) = match (&cfg, whole_window) {
        (_, true) => (capture_window(app)?, "window"),
        (Some(cfg), false) => (capture_page(app, &cfg.id)?, "page"),
        (None, false) => unreachable!(),
    };
    let (width, height) = png_size(&png).unwrap_or((0, 0));
    let dir = screenshots_dir(app);
    let name = cfg.as_ref().map_or("app-viewer", |c| c.name.as_str());
    let path = save(&dir, name, &png)?;
    purge(&dir);
    let url = cfg.as_ref().map(|c| st.urls.lock().unwrap().get(&c.id).cloned().unwrap_or_else(|| c.url()));
    Ok(json!({ "path": path, "width": width, "height": height, "app": cfg.as_ref().map(|c| c.name.clone()), "url": url, "scope": scope }))
}

/// Makes sure the app is the one on screen: window shown, app selected, no settings page or
/// dialog in the way, page loaded.
pub fn bring_forward(app: &AppHandle, id: &str) -> Result<(), String> {
    let st = app.state::<AppState>();
    let name = st.app(id).map(|a| a.name).unwrap_or_default();
    if !matches!(st.status(id), Status::Running | Status::External) {
        return Err(format!("« {name} » ne tourne pas : lancez-la avec `appv open \"{name}\"`."));
    }
    let mut moved = show_window(app)?;
    let ready = |st: &AppState| {
        let view = st.view.lock().unwrap();
        view.visible && view.active.as_deref() == Some(id)
    };
    if !ready(&st) || views::webview(app, id).is_none() {
        moved = true;
        st.view.lock().unwrap().active = Some(id.to_string());
        emit_state(app);
        let _ = app.emit_to(views::UI_LABEL, "show-app", id); // the UI closes any page/panel over the app
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(ready(&st) && views::webview(app, id).is_some()) {
            if Instant::now() > deadline {
                return Err("L'app n'est pas affichée (une fenêtre de dialogue d'App Viewer la masque ?).".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    // The page must have a real address before it is worth capturing.
    let deadline = Instant::now() + Duration::from_secs(5);
    while views::webview(app, id).and_then(|wv| wv.url().ok()).map_or(true, |u| u.scheme() == "about") {
        if Instant::now() > deadline {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    if moved {
        thread::sleep(Duration::from_millis(300)); // let the page paint
    }
    Ok(())
}

/// Hidden in the tray or minimized: nothing would be drawn. Shown without taking the focus.
/// Returns whether it had to restore the window.
fn show_window(app: &AppHandle) -> Result<bool, String> {
    let main = app.get_window("main").ok_or("Fenêtre principale introuvable.")?;
    if !main.is_visible().unwrap_or(true) || main.is_minimized().unwrap_or(false) {
        let _ = main.show();
        let _ = main.unminimize();
        thread::sleep(Duration::from_millis(400)); // let the panes lay out and paint again
        return Ok(true);
    }
    Ok(false)
}

fn capture_page(app: &AppHandle, id: &str) -> Result<Vec<u8>, String> {
    capture_webview(&views::webview(app, id).ok_or("L'app n'est pas ouverte.")?)
}

/// PNG of what a webview currently shows.
fn capture_webview(wv: &tauri::Webview) -> Result<Vec<u8>, String> {
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
    wv.with_webview(move |platform| unsafe {
        let started = (|| -> Result<(), String> {
            let core = platform.controller().CoreWebView2().map_err(|e| e.to_string())?;
            let stream = SHCreateMemStream(None).ok_or("flux mémoire indisponible")?;
            let (done_tx, done_stream) = (tx.clone(), stream.clone());
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
                let png = result.map_err(|e| e.to_string()).and_then(|_| read_stream(&done_stream));
                let _ = done_tx.send(png);
                Ok(())
            }));
            core.CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler).map_err(|e| e.to_string())
        })();
        if let Err(e) = started {
            let _ = tx.send(Err(e));
        }
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(CAPTURE_TIMEOUT).map_err(|_| "La capture n'a pas abouti à temps.".to_string())?
}

unsafe fn read_stream(stream: &IStream) -> Result<Vec<u8>, String> {
    let e = |e: windows::core::Error| e.to_string();
    stream.Seek(0, STREAM_SEEK_SET, None).map_err(e)?;
    let mut stat = STATSTG::default();
    stream.Stat(&mut stat, STATFLAG_NONAME).map_err(e)?;
    let mut buf = vec![0u8; stat.cbSize as usize];
    let mut read = 0u32;
    stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read)).ok().map_err(e)?;
    buf.truncate(read as usize);
    Ok(buf)
}

/// The whole App Viewer window (sidebar, toolbar and the app inside). The panes are WebView2
/// surfaces that `PrintWindow` / GDI cannot draw, and a screen grab would catch whatever overlaps
/// the window: so the shell UI and the app are captured separately, then composed as they are laid out.
fn capture_window(app: &AppHandle) -> Result<Vec<u8>, String> {
    let ui = views::ui_webview(app).ok_or("L'interface d'App Viewer est introuvable.")?;
    let mut canvas = decode_png(&capture_webview(&ui)?)?;

    let st = app.state::<AppState>();
    let overlay = {
        let view = st.view.lock().unwrap();
        view.active.clone().filter(|_| view.visible).map(|id| (id, view.rect))
    };
    if let Some((id, rect)) = overlay {
        if let Some(wv) = views::webview(app, &id) {
            let scale = app.get_window("main").and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
            let page = decode_png(&capture_webview(&wv)?)?;
            // The UI reports the page's position in logical pixels (shifted right when the sidebar peeks).
            canvas.paste(&page, (rect.x * scale).round() as i64, (rect.y * scale).round() as i64);
        }
    }
    encode_png(&canvas.rgba, canvas.width, canvas.height)
}

/// An RGBA8 image.
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Image {
    /// Draws `other` with its top-left corner at (x, y), clipped to this image.
    fn paste(&mut self, other: &Image, x: i64, y: i64) {
        for row in 0..other.height as i64 {
            let dy = y + row;
            if dy < 0 || dy >= self.height as i64 {
                continue;
            }
            let (from, to) = ((-x).max(0), (other.width as i64).min(self.width as i64 - x));
            if from >= to {
                continue;
            }
            let src = ((row * other.width as i64 + from) * 4) as usize..((row * other.width as i64 + to) * 4) as usize;
            let dst = ((dy * self.width as i64 + x + from) * 4) as usize;
            self.rgba[dst..dst + src.len()].copy_from_slice(&other.rgba[src]);
        }
    }
}

fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        other => return Err(format!("Format d'image inattendu ({other:?}).")),
    };
    Ok(Image { width: info.width, height: info.height, rgba })
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// (width, height) from the PNG header.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    (png.len() > 24 && &png[1..4] == b"PNG").then(|| (be(&png[16..20]), be(&png[20..24])))
}

pub fn screenshots_dir(app: &AppHandle) -> PathBuf {
    let custom = app.state::<AppState>().settings.lock().unwrap().screenshots_dir.trim().to_string();
    if !custom.is_empty() {
        return PathBuf::from(custom);
    }
    app.path().local_data_dir().unwrap_or_else(|_| std::env::temp_dir()).join("App Viewer").join("screenshots")
}

/// `<app>-AAAAMMJJ-HHMMSS.png`, with the app name reduced to safe file-name characters.
fn file_name(app_name: &str, now: (u16, u16, u16, u16, u16, u16)) -> String {
    let slug: String = app_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "app" } else { slug };
    let (y, mo, d, h, mi, s) = now;
    format!("{slug}-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.png")
}

fn save(dir: &Path, app_name: &str, png: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("Dossier des captures inutilisable ({}) : {e}", dir.display()))?;
    let t = unsafe {
        let mut t = mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    let name = file_name(app_name, (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond));
    let mut path = dir.join(&name);
    // Two shots in the same second: add a counter rather than overwrite.
    for n in 2.. {
        if !path.exists() {
            break;
        }
        path = dir.join(name.replace(".png", &format!("-{n}.png")));
    }
    fs::write(&path, png).map_err(|e| format!("Écriture de la capture impossible : {e}"))?;
    Ok(path)
}

/// Keeps the newest KEEP_FILES images that are not older than KEEP_FOR. Only `.png` files in the folder.
fn purge(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
        .filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    let now = SystemTime::now();
    for (i, (modified, path)) in files.iter().enumerate() {
        let too_old = now.duration_since(*modified).map_or(false, |age| age > KEEP_FOR);
        if i >= KEEP_FILES || too_old {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_dated() {
        assert_eq!(file_name("Mon App/../x", (2026, 10, 3, 9, 5, 7)), "mon-app----x-20261003-090507.png");
        assert_eq!(file_name("///", (2026, 1, 2, 3, 4, 5)), "app-20260102-030405.png");
    }

    #[test]
    fn png_round_trip_and_size() {
        let rgba = vec![255u8; 3 * 2 * 4];
        let png = encode_png(&rgba, 3, 2).unwrap();
        assert_eq!(png_size(&png), Some((3, 2)));
        assert_eq!(png_size(b"nope"), None);
    }

    #[test]
    fn paste_places_and_clips_images() {
        let mut canvas = Image { width: 4, height: 3, rgba: vec![0; 4 * 3 * 4] };
        let patch = Image { width: 2, height: 2, rgba: vec![9; 2 * 2 * 4] };
        canvas.paste(&patch, 3, 2); // only one pixel fits
        let px = |c: &Image, x: usize, y: usize| c.rgba[(y * 4 + x) * 4];
        assert_eq!(px(&canvas, 3, 2), 9);
        assert_eq!(px(&canvas, 2, 2), 0);
        assert_eq!(px(&canvas, 3, 1), 0);
        canvas.paste(&patch, -1, -1); // only the bottom-right pixel of the patch fits, at (0, 0)
        assert_eq!(px(&canvas, 0, 0), 9);
        assert_eq!(px(&canvas, 1, 0), 0);
    }

    #[test]
    fn decode_reads_back_what_encode_wrote() {
        let rgba: Vec<u8> = (0..2 * 2 * 4).map(|i| i as u8).collect();
        let img = decode_png(&encode_png(&rgba, 2, 2).unwrap()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.rgba, rgba);
    }

    #[test]
    fn purge_keeps_the_newest_and_only_pngs() {
        let dir = std::env::temp_dir().join(format!("av-shots-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..KEEP_FILES + 5 {
            fs::write(dir.join(format!("a-{i}.png")), b"x").unwrap();
        }
        fs::write(dir.join("notes.txt"), b"keep").unwrap();
        purge(&dir);
        let pngs = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "png")).count();
        assert_eq!(pngs, KEEP_FILES);
        assert!(dir.join("notes.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
