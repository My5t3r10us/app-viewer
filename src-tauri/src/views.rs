//! One native WebView2 per running app, laid over the shell UI's content area.
//! Hidden webviews stay alive, so switching apps keeps their page state.

use crate::{
    downloads, embed, settings,
    state::{AppState, Status},
};
use std::collections::HashSet;
use tauri::{
    webview::DownloadEvent, AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewBuilder, WebviewUrl,
};
use webview2_com::ZoomFactorChangedEventHandler;

pub const MIN_ZOOM: f64 = 0.25;
pub const MAX_ZOOM: f64 = 5.0;

pub const UI_LABEL: &str = "ui";
const PREFIX: &str = "app-";

pub fn label(id: &str) -> String {
    format!("{PREFIX}{id}")
}

pub fn webview(app: &AppHandle, id: &str) -> Option<Webview> {
    app.get_webview(&label(id))
}

/// The shell UI webview (sidebar, toolbar), under the hosted apps.
pub fn ui_webview(app: &AppHandle) -> Option<Webview> {
    app.get_webview(UI_LABEL)
}

pub fn close(app: &AppHandle, id: &str) {
    if let Some(wv) = webview(app, id) {
        let _ = wv.close();
    }
}

/// Reconciles the native webviews with the current state: closes those of stopped apps,
/// hides inactive ones, and creates/places the active one.
pub fn sync(app: &AppHandle) {
    let st = app.state::<AppState>();
    let _guard = st.view_sync.lock().unwrap();

    let (active, rect, visible) = {
        let v = st.view.lock().unwrap();
        (v.active.clone(), v.rect, v.visible)
    };
    // Native apps run in their own windows: no webview for them.
    let native: HashSet<String> = st.apps.lock().unwrap().iter().filter(|a| a.is_native()).map(|a| a.id.clone()).collect();
    let up: HashSet<String> = st
        .runtimes
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, rt)| rt.status.is_up() && !native.contains(*id))
        .map(|(id, _)| id.clone())
        .collect();

    for (lbl, wv) in app.webviews() {
        let Some(id) = lbl.strip_prefix(PREFIX) else { continue };
        if !up.contains(id) {
            let _ = wv.close();
        } else if active.as_deref() != Some(id) || !visible {
            let _ = wv.hide();
        }
    }

    sync_native(app, active.as_deref().filter(|id| native.contains(*id)), rect, visible);

    let Some(id) = active.filter(|id| up.contains(id)) else { return };
    if !visible || rect.width < 1.0 || rect.height < 1.0 {
        return;
    }
    let pos = LogicalPosition::new(rect.x, rect.y);
    let size = LogicalSize::new(rect.width, rect.height);

    if let Some(wv) = webview(app, &id) {
        let _ = wv.set_position(pos);
        let _ = wv.set_size(size);
        let _ = wv.show();
        return;
    }

    let Some(cfg) = st.app(&id) else { return };
    let Ok(url) = cfg.url().parse::<Url>() else { return };
    let Some(window) = app.get_window("main") else { return };
    // WebView2 then handles Ctrl+wheel, Ctrl +/-/0 and pinch itself.
    let (dl_app, dl_id) = (app.clone(), id.clone());
    let builder = WebviewBuilder::new(label(&id), WebviewUrl::External(url))
        .zoom_hotkeys_enabled(true)
        .on_download(move |_, event| {
            // Observed only: the file goes where WebView2 puts it (the Downloads folder).
            match event {
                DownloadEvent::Requested { url, destination } => downloads::started(&dl_app, &dl_id, &url, destination),
                DownloadEvent::Finished { url, path, success } => downloads::finished(&dl_app, &dl_id, &url, path, success),
                _ => {}
            }
            true
        });
    if let Ok(wv) = window.add_child(builder, pos, size) {
        let saved = st.settings.lock().unwrap().zoom.get(&id).copied().unwrap_or(1.0);
        track_zoom(app, &wv, &id, saved);
        let _ = wv.set_focus();
    }
}

/// Native apps: the active one's window is shown over the content area, the others hidden.
/// Returns whether the active native app's window is in place.
fn sync_native(app: &AppHandle, active: Option<&str>, rect: crate::state::Rect, visible: bool) -> bool {
    embed::hide_others(active);
    let Some(id) = active else { return false };
    let st = app.state::<AppState>();
    let job = {
        let rts = st.runtimes.lock().unwrap();
        rts.get(id).filter(|rt| rt.status == Status::Running).and_then(|rt| rt.job.clone())
    };
    let Some(job) = job else {
        embed::forget(id);
        return false;
    };
    if !visible || rect.width < 1.0 || rect.height < 1.0 {
        embed::hide(id);
        return embed::is_embedded(id);
    }
    let Some(window) = app.get_window("main") else { return false };
    let (Ok(hwnd), Ok(scale)) = (window.hwnd(), window.scale_factor()) else { return false };
    let px = |v: f64| (v * scale).round() as i32;
    embed::show(id, &job.pids(), hwnd.0 as isize, (px(rect.x), px(rect.y), px(rect.width), px(rect.height)))
}

/// Restores the app's saved zoom and follows later changes (hotkeys, wheel, buttons): they are
/// remembered per app and pushed to the UI ("app-zoom").
fn track_zoom(app: &AppHandle, wv: &Webview, id: &str, saved: f64) {
    let (app, id) = (app.clone(), id.to_string());
    let _ = wv.with_webview(move |platform| unsafe {
        let controller = platform.controller();
        if (saved - 1.0).abs() > 1e-6 {
            let _ = controller.SetZoomFactor(saved);
        }
        let mut token = 0i64;
        let handler = ZoomFactorChangedEventHandler::create(Box::new(move |controller, _| {
            let mut factor = 1.0f64;
            if let Some(controller) = controller {
                let _ = controller.ZoomFactor(&mut factor);
            }
            zoom_changed(&app, &id, factor);
            Ok(())
        }));
        if let Err(e) = controller.add_ZoomFactorChanged(&handler, &mut token) {
            eprintln!("App Viewer : suivi du zoom impossible ({e})");
        }
    });
}

/// Remembers and announces a zoom factor. WebView2 raises ZoomFactorChanged for the user's own
/// zooming (wheel, hotkeys) but not when the app sets the factor itself, hence the direct call
/// from `set_zoom`.
pub fn zoom_changed(app: &AppHandle, id: &str, factor: f64) {
    let st = app.state::<AppState>();
    let snapshot = {
        let mut settings = st.settings.lock().unwrap();
        let changed = if (factor - 1.0).abs() < 1e-6 {
            settings.zoom.remove(id).is_some()
        } else {
            settings.zoom.insert(id.to_string(), factor).map_or(true, |old| (old - factor).abs() > 1e-6)
        };
        changed.then(|| settings.clone())
    };
    if let Some(snapshot) = snapshot {
        let _ = settings::save(&st.settings_path, &snapshot);
    }
    let _ = app.emit_to(UI_LABEL, "app-zoom", ZoomEvent { id, factor });
}

#[derive(Clone, serde::Serialize)]
struct ZoomEvent<'a> {
    id: &'a str,
    factor: f64,
}

/// Goes to `target`: a path of the app ("/admin", "admin?x=1") or a full URL (http / https only).
pub fn navigate(app: &AppHandle, id: &str, target: &str) -> Result<(), String> {
    let cfg = app.state::<AppState>().app(id).ok_or("Application inconnue.")?;
    let base: Url = cfg.url().parse().map_err(|_| "URL de l'app invalide.")?;
    let target = target.trim();
    let url = if target.contains("://") {
        target.parse::<Url>()
    } else if target.starts_with("localhost:") || target.starts_with("127.0.0.1:") {
        format!("http://{target}").parse::<Url>()
    } else {
        let path = if target.starts_with('/') { target.to_string() } else { format!("/{target}") };
        base.join(&path)
    }
    .map_err(|_| format!("URL invalide : {target}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Seules les URL http(s) sont acceptées.".into());
    }
    let wv = webview(app, id).ok_or("L'app n'est pas ouverte.")?;
    wv.navigate(url).map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize)]
struct UrlEvent<'a> {
    id: &'a str,
    url: &'a str,
}

/// Follows the address of every hosted webview, client-side routing included (pushState
/// fires no page load), and pushes changes to the UI ("app-url").
pub fn watch_urls(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let st = app.state::<AppState>();
        let mut seen = HashSet::new();
        for (lbl, wv) in app.webviews() {
            let Some(id) = lbl.strip_prefix(PREFIX) else { continue };
            let Ok(url) = wv.url() else { continue };
            let url = url.to_string();
            seen.insert(id.to_string());
            let changed = st.urls.lock().unwrap().insert(id.to_string(), url.clone()).as_ref() != Some(&url);
            if changed {
                let _ = app.emit_to(UI_LABEL, "app-url", UrlEvent { id, url: &url });
            }
        }
        st.urls.lock().unwrap().retain(|id, _| seen.contains(id));
    });
}
