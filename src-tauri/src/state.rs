use crate::{config::AppConfig, downloads::Download, job::Job, settings::Settings};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tauri::{AppHandle, Emitter, Manager};

const MAX_LOG_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    /// Process launched, waiting for the port to answer.
    Starting,
    Running,
    /// The port was already answering before we launched anything: shown, not managed.
    External,
    Error,
}

impl Status {
    pub fn is_up(self) -> bool {
        matches!(self, Status::Running | Status::External)
    }
}

#[derive(Default)]
pub struct Runtime {
    pub status: Status,
    pub error: Option<String>,
    pub job: Option<Arc<Job>>,
    pub stopping: bool,
    /// Bumped on every launch so watcher threads of a previous run are ignored.
    pub generation: u64,
}

#[derive(Clone, Copy, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Default)]
pub struct ViewState {
    pub active: Option<String>,
    pub rect: Rect,
    pub visible: bool,
}

pub struct AppState {
    pub config_path: PathBuf,
    pub settings_path: PathBuf,
    pub settings: Mutex<Settings>,
    pub apps: Mutex<Vec<AppConfig>>,
    /// apps.json's mtime as of our last load/save: a different one means the CLI changed it.
    pub config_mtime: Mutex<Option<SystemTime>>,
    pub runtimes: Mutex<HashMap<String, Runtime>>,
    pub logs: Mutex<HashMap<String, VecDeque<String>>>,
    pub view: Mutex<ViewState>,
    /// Apps whose terminal is currently detached in its own window.
    pub terminals: Mutex<HashSet<String>>,
    /// Serialises webview creation/placement.
    pub view_sync: Mutex<()>,
    /// Current address of each open app webview (it follows the in-app navigation).
    pub urls: Mutex<HashMap<String, String>>,
    /// Apps an agent may drive through `appv ui`. Switched on by the user from the shell UI only,
    /// kept in memory only: every launch of App Viewer starts with all of them off.
    pub agent_control: Mutex<HashSet<String>>,
    /// Downloads started from the hosted apps, newest first.
    pub downloads: Mutex<Vec<Download>>,
    pub downloads_path: PathBuf,
}

impl AppState {
    pub fn new(
        config_path: PathBuf,
        apps: Vec<AppConfig>,
        settings_path: PathBuf,
        settings: Settings,
        downloads_path: PathBuf,
        downloads: Vec<Download>,
    ) -> Self {
        let active = apps.first().map(|a| a.id.clone());
        let mtime = crate::config::modified(&config_path);
        Self {
            config_path,
            settings_path,
            settings: Mutex::new(settings),
            config_mtime: Mutex::new(mtime),
            apps: Mutex::new(apps),
            runtimes: Mutex::default(),
            logs: Mutex::default(),
            view: Mutex::new(ViewState { active, ..Default::default() }),
            terminals: Mutex::default(),
            view_sync: Mutex::default(),
            urls: Mutex::default(),
            agent_control: Mutex::default(),
            downloads: Mutex::new(downloads),
            downloads_path,
        }
    }

    /// Writes apps.json and remembers its new mtime, so our own save isn't taken for an outside edit.
    pub fn save_apps(&self, apps: &[AppConfig]) -> Result<(), String> {
        crate::config::save(&self.config_path, apps)?;
        *self.config_mtime.lock().unwrap() = crate::config::modified(&self.config_path);
        Ok(())
    }

    pub fn app(&self, id: &str) -> Option<AppConfig> {
        self.apps.lock().unwrap().iter().find(|a| a.id == id).cloned()
    }

    pub fn status(&self, id: &str) -> Status {
        self.runtimes.lock().unwrap().get(id).map(|r| r.status).unwrap_or_default()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    #[serde(flatten)]
    pub config: AppConfig,
    pub status: Status,
    pub error: Option<String>,
    pub url: String,
    /// Where its webview is now, if it is open.
    pub current_url: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub apps: Vec<AppView>,
    pub active: Option<String>,
    pub settings: Settings,
    /// Ids of the apps whose terminal is detached.
    pub detached: Vec<String>,
    /// Ids of the apps an agent may currently drive.
    pub agent_control: Vec<String>,
}

pub fn snapshot(app: &AppHandle) -> Snapshot {
    let st = app.state::<AppState>();
    let apps = st.apps.lock().unwrap().clone();
    let active = st.view.lock().unwrap().active.clone();
    let settings = st.settings.lock().unwrap().clone();
    let detached = st.terminals.lock().unwrap().iter().cloned().collect();
    let agent_control = st.agent_control.lock().unwrap().iter().cloned().collect();
    let urls = st.urls.lock().unwrap().clone();
    let runtimes = st.runtimes.lock().unwrap();
    Snapshot {
        apps: apps
            .into_iter()
            .map(|config| {
                let rt = runtimes.get(&config.id);
                AppView {
                    url: config.url(),
                    current_url: urls.get(&config.id).cloned(),
                    status: rt.map(|r| r.status).unwrap_or_default(),
                    error: rt.and_then(|r| r.error.clone()),
                    config,
                }
            })
            .collect(),
        active,
        settings,
        detached,
        agent_control,
    }
}

// Events reach every webview that registered a listener: the shell UI and the detached
// terminals. Hosted apps are remote origins without IPC, so they can't listen.

pub fn emit_state(app: &AppHandle) {
    let snap = snapshot(app);
    if let Some(tray) = app.tray_by_id("main") {
        let up = snap.apps.iter().filter(|a| a.status.is_up() || a.status == Status::Starting).count();
        let tip = match up {
            0 => "App Viewer".to_string(),
            1 => "App Viewer — 1 app en cours".to_string(),
            n => format!("App Viewer — {n} apps en cours"),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
    let _ = app.emit("state", snap);
}

#[derive(Clone, Serialize)]
struct LogEvent<'a> {
    id: &'a str,
    line: &'a str,
}

pub fn push_log(app: &AppHandle, id: &str, line: String) {
    let st = app.state::<AppState>();
    {
        let mut logs = st.logs.lock().unwrap();
        let buf = logs.entry(id.to_string()).or_default();
        if buf.len() >= MAX_LOG_LINES {
            buf.pop_front();
        }
        buf.push_back(line.clone());
    }
    let _ = app.emit("app-log", LogEvent { id, line: &line });
}
