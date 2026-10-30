//! Downloads started from the hosted apps: a short history (status + file), kept in
//! downloads.json next to apps.json and pushed to the UI ("downloads").
//! WebView2 saves into the user's Downloads folder; we only observe.

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, Url};

const MAX_ENTRIES: usize = 100;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Downloading,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub id: String,
    pub app_id: String,
    pub app_name: String,
    pub file_name: String,
    pub url: String,
    pub path: String,
    pub status: Status,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub finished_at: Option<u64>,
    /// Bytes on disk once finished.
    pub size: Option<u64>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

/// Loads the history. Downloads that were still running when App Viewer stopped are over.
pub fn load(path: &Path) -> Vec<Download> {
    let mut list: Vec<Download> =
        fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for d in list.iter_mut().filter(|d| d.status == Status::Downloading) {
        d.status = Status::Failed;
        d.finished_at = Some(d.started_at);
    }
    list
}

fn save(path: &Path, list: &[Download]) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let Ok(json) = serde_json::to_string_pretty(list) else { return };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Applies `change` to the list (newest first), then saves it and tells the UI.
fn update(app: &AppHandle, change: impl FnOnce(&mut Vec<Download>)) {
    let st = app.state::<AppState>();
    let snapshot = {
        let mut list = st.downloads.lock().unwrap();
        change(&mut list);
        list.truncate(MAX_ENTRIES);
        list.clone()
    };
    save(&st.downloads_path, &snapshot);
    let _ = app.emit("downloads", snapshot);
}

pub fn list(app: &AppHandle) -> Vec<Download> {
    app.state::<AppState>().downloads.lock().unwrap().clone()
}

pub fn started(app: &AppHandle, app_id: &str, url: &Url, destination: &Path) {
    let st = app.state::<AppState>();
    let app_name = st.app(app_id).map(|a| a.name).unwrap_or_default();
    let entry = Download {
        id: crate::config::new_id(),
        app_id: app_id.to_string(),
        app_name,
        file_name: file_name(destination),
        url: url.to_string(),
        path: destination.display().to_string(),
        status: Status::Downloading,
        started_at: now_ms(),
        finished_at: None,
        size: None,
    };
    update(app, |list| list.insert(0, entry));
}

pub fn finished(app: &AppHandle, app_id: &str, url: &Url, path: Option<PathBuf>, success: bool) {
    let url = url.to_string();
    let size = path.as_ref().filter(|_| success).and_then(|p| fs::metadata(p).ok()).map(|m| m.len());
    update(app, |list| {
        // The event only carries the URL: it belongs to the newest running download of that app and URL.
        let Some(d) = list.iter_mut().find(|d| d.status == Status::Downloading && d.app_id == app_id && d.url == url)
        else {
            return;
        };
        d.status = if success { Status::Done } else { Status::Failed };
        d.finished_at = Some(now_ms());
        d.size = size;
        if let Some(p) = path {
            d.file_name = file_name(&p);
            d.path = p.display().to_string();
        }
    });
}

/// Forgets the finished entries (files stay on disk); running ones are kept.
pub fn clear_finished(app: &AppHandle) {
    update(app, |list| list.retain(|d| d.status == Status::Downloading));
}

pub fn remove(app: &AppHandle, id: &str) {
    update(app, |list| list.retain(|d| d.id != id));
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn find(app: &AppHandle, id: &str) -> Result<Download, String> {
    list(app).into_iter().find(|d| d.id == id).ok_or_else(|| "Téléchargement inconnu.".to_string())
}

/// Opens the downloaded file with its default program.
pub fn open(app: &AppHandle, id: &str) -> Result<(), String> {
    let d = find(app, id)?;
    if d.status != Status::Done || !Path::new(&d.path).is_file() {
        return Err("Le fichier n'existe plus.".into());
    }
    Command::new("explorer.exe").arg(&d.path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Shows the file selected in an Explorer window (or its folder if it is gone).
pub fn reveal(app: &AppHandle, id: &str) -> Result<(), String> {
    let d = find(app, id)?;
    let path = Path::new(&d.path);
    let mut cmd = Command::new("explorer.exe");
    if path.exists() {
        cmd.raw_arg(format!("/select,\"{}\"", d.path));
    } else if let Some(dir) = path.parent().filter(|p| p.is_dir()) {
        cmd.arg(dir);
    } else {
        return Err("Le dossier n'existe plus.".into());
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_downloads_become_failed_on_load() {
        let dir = std::env::temp_dir().join(format!("av-dl-test-{}", std::process::id()));
        let file = dir.join("downloads.json");
        let entry = |id: &str, status| Download {
            id: id.into(),
            app_id: "a".into(),
            app_name: "App".into(),
            file_name: "f.zip".into(),
            url: "http://localhost/f.zip".into(),
            path: "C:\\f.zip".into(),
            status,
            started_at: 5,
            finished_at: None,
            size: None,
        };
        save(&file, &[entry("1", Status::Downloading), entry("2", Status::Done)]);
        let loaded = load(&file);
        assert_eq!(loaded[0].status, Status::Failed);
        assert_eq!(loaded[1].status, Status::Done);
        let _ = fs::remove_dir_all(dir);
    }
}
