#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod embed;
mod capture;
mod config;
mod downloads;
mod ipc;
#[path = "ipc_common.rs"]
mod ipc_common;
mod job;
mod procs;
mod runner;
mod settings;
mod state;
mod stats;
mod views;

use config::{AppConfig, FolderInfo};
use settings::Settings;
use state::{emit_state, snapshot, AppState, Rect, Snapshot};
use std::{path::Path, process::Command};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    window::WindowBuilder,
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, RunEvent, Url, WebviewBuilder, WebviewUrl, WindowEvent,
};
use tauri_plugin_dialog::DialogExt;

const TERM_PREFIX: &str = "term-";

// Commands are async so they run off the main thread: creating webviews from the
// main thread inside a command would deadlock on Windows.

#[tauri::command]
async fn get_state(app: AppHandle) -> Snapshot {
    snapshot(&app)
}

#[tauri::command]
async fn save_app(app: AppHandle, mut config: AppConfig) -> Result<AppConfig, String> {
    config.validate()?;
    // Pick up any CLI edit first, so saving from the UI doesn't overwrite it.
    reload_config(&app);

    let st = app.state::<AppState>();
    let url_changed = {
        let mut apps = st.apps.lock().unwrap();
        let changed = match apps.iter_mut().find(|a| a.id == config.id && !config.id.is_empty()) {
            Some(existing) => {
                let changed = existing.url() != config.url();
                *existing = config.clone();
                changed
            }
            None => {
                config.id = config::new_id();
                apps.push(config.clone());
                false
            }
        };
        st.save_apps(&apps)?;
        changed
    };
    st.view.lock().unwrap().active = Some(config.id.clone());
    if url_changed {
        views::close(&app, &config.id);
    }
    emit_state(&app);
    views::sync(&app);
    Ok(config)
}

#[tauri::command]
async fn delete_app(app: AppHandle, id: String) -> Result<(), String> {
    reload_config(&app);
    let st = app.state::<AppState>();
    {
        let mut apps = st.apps.lock().unwrap();
        apps.retain(|a| a.id != id);
        st.save_apps(&apps)?;
    }
    forget_app(&app, &id);
    emit_state(&app);
    views::sync(&app);
    Ok(())
}

/// Stops and drops everything tied to an app that is no longer configured.
fn forget_app(app: &AppHandle, id: &str) {
    runner::stop(app, id);
    views::close(app, id);
    close_terminal(app, id);
    let st = app.state::<AppState>();
    st.runtimes.lock().unwrap().remove(id);
    st.agent_control.lock().unwrap().remove(id);
    st.logs.lock().unwrap().remove(id);
    let apps = st.apps.lock().unwrap();
    let mut view = st.view.lock().unwrap();
    if view.active.as_deref() == Some(id) {
        view.active = apps.first().map(|a| a.id.clone());
    }
}

/// Reloads apps.json if it changed on disk (edited by the `appv` CLI). Returns whether it did.
fn reload_config(app: &AppHandle) -> bool {
    let st = app.state::<AppState>();
    let mtime = config::modified(&st.config_path);
    {
        let mut known = st.config_mtime.lock().unwrap();
        if *known == mtime {
            return false;
        }
        *known = mtime;
    }
    let fresh = config::load(&st.config_path);
    let (removed, moved): (Vec<String>, Vec<String>) = {
        let mut apps = st.apps.lock().unwrap();
        let removed = apps.iter().filter(|a| !fresh.iter().any(|f| f.id == a.id)).map(|a| a.id.clone()).collect();
        let moved = apps
            .iter()
            .filter(|a| fresh.iter().any(|f| f.id == a.id && f.url() != a.url()))
            .map(|a| a.id.clone())
            .collect();
        *apps = fresh;
        (removed, moved)
    };
    for id in &removed {
        forget_app(app, id);
    }
    for id in &moved {
        views::close(app, id); // recreated on the new URL by views::sync
    }
    {
        let apps = st.apps.lock().unwrap();
        let mut view = st.view.lock().unwrap();
        if view.active.is_none() || !apps.iter().any(|a| Some(&a.id) == view.active.as_ref()) {
            view.active = apps.first().map(|a| a.id.clone());
        }
    }
    emit_state(app);
    views::sync(app);
    true
}

/// Requests passed on the command line, by the `appv` CLI: `--open <id>` starts the app if
/// needed and brings it to the front, `--stop <id>` stops it. Reaches the running instance
/// through the single-instance plugin.
fn handle_args(app: &AppHandle, args: Vec<String>) {
    let mut it = args.into_iter().skip(1);
    let mut requests = Vec::new();
    while let Some(arg) = it.next() {
        if let ("--open" | "--stop", Some(id)) = (arg.as_str(), it.next()) {
            requests.push((arg, id));
        }
    }
    if requests.is_empty() {
        show_main(app);
        return;
    }
    let app = app.clone();
    // Off the main thread: starting an app creates webviews.
    std::thread::spawn(move || {
        reload_config(&app);
        for (action, id) in requests {
            if app.state::<AppState>().app(&id).is_none() {
                continue;
            }
            if action == "--stop" {
                runner::stop(&app, &id);
                continue;
            }
            app.state::<AppState>().view.lock().unwrap().active = Some(id.clone());
            emit_state(&app);
            show_main(&app);
            let _ = runner::start(&app, &id);
            views::sync(&app);
        }
    });
}

#[tauri::command]
async fn start_app(app: AppHandle, id: String) -> Result<(), String> {
    runner::start(&app, &id)
}

#[tauri::command]
async fn stop_app(app: AppHandle, id: String) {
    runner::stop(&app, &id);
}

#[tauri::command]
async fn restart_app(app: AppHandle, id: String) -> Result<(), String> {
    runner::restart(&app, &id)
}

/// Who holds the port of an "Externe" app (shown in the confirmation before taking it over).
#[tauri::command]
async fn external_process(app: AppHandle, id: String) -> Option<procs::ProcInfo> {
    runner::external_info(&app, &id)
}

#[tauri::command]
async fn take_over_app(app: AppHandle, id: String, expected_pid: Option<u32>) -> Result<String, String> {
    runner::take_over(&app, &id, expected_pid)
}

#[tauri::command]
async fn select_app(app: AppHandle, id: Option<String>) {
    app.state::<AppState>().view.lock().unwrap().active = id;
    emit_state(&app);
    views::sync(&app);
}

#[tauri::command]
async fn set_viewport(app: AppHandle, x: f64, y: f64, width: f64, height: f64, visible: bool) {
    {
        let st = app.state::<AppState>();
        let mut view = st.view.lock().unwrap();
        view.rect = Rect { x, y, width, height };
        view.visible = visible;
    }
    views::sync(&app);
}

#[tauri::command]
async fn get_logs(app: AppHandle, id: String) -> Vec<String> {
    let st = app.state::<AppState>();
    let logs = st.logs.lock().unwrap();
    logs.get(&id).map(|l| l.iter().cloned().collect()).unwrap_or_default()
}

#[tauri::command]
async fn clear_logs(app: AppHandle, id: String) {
    app.state::<AppState>().logs.lock().unwrap().remove(&id);
}

#[tauri::command]
async fn reload_app(app: AppHandle, id: String) {
    if let Some(wv) = views::webview(&app, &id) {
        let _ = wv.eval("location.reload()");
    }
}

#[tauri::command]
async fn home_app(app: AppHandle, id: String) {
    let url = app.state::<AppState>().app(&id).and_then(|c| c.url().parse::<Url>().ok());
    if let (Some(wv), Some(url)) = (views::webview(&app, &id), url) {
        let _ = wv.navigate(url);
    }
}

/// Goes to `target`: a path of the app ("/admin", "admin?x=1") or a full URL.
#[tauri::command]
async fn navigate_app(app: AppHandle, id: String, target: String) -> Result<(), String> {
    views::navigate(&app, &id, &target)
}

/// Lets an agent drive an app through `appv ui` (or stops it). Only the shell UI can call this.
#[tauri::command]
async fn set_agent_control(app: AppHandle, id: String, enabled: bool) {
    {
        let st = app.state::<AppState>();
        let mut allowed = st.agent_control.lock().unwrap();
        if enabled && st.app(&id).is_some() {
            allowed.insert(id);
        } else {
            allowed.remove(&id);
        }
    }
    emit_state(&app);
}

/// Sets the page zoom of an open app (1.0 = 100 %); the UI hears about it through "app-zoom".
#[tauri::command]
async fn set_zoom(app: AppHandle, id: String, factor: f64) -> Result<(), String> {
    let factor = factor.clamp(views::MIN_ZOOM, views::MAX_ZOOM);
    let wv = views::webview(&app, &id).ok_or("L'app n'est pas ouverte.")?;
    wv.set_zoom(factor).map_err(|e| e.to_string())?;
    views::zoom_changed(&app, &id, factor);
    Ok(())
}

#[tauri::command]
async fn open_devtools(app: AppHandle, id: String) {
    if let Some(wv) = views::webview(&app, &id) {
        wv.open_devtools();
    }
}

#[tauri::command]
async fn open_in_browser(app: AppHandle, id: String) {
    let st = app.state::<AppState>();
    let current = st.urls.lock().unwrap().get(&id).cloned();
    if let Some(url) = current.or_else(|| st.app(&id).map(|c| c.url())) {
        let _ = Command::new("explorer.exe").arg(url).spawn();
    }
}

#[tauri::command]
async fn get_downloads(app: AppHandle) -> Vec<downloads::Download> {
    downloads::list(&app)
}

#[tauri::command]
async fn clear_downloads(app: AppHandle) {
    downloads::clear_finished(&app);
}

#[tauri::command]
async fn remove_download(app: AppHandle, id: String) {
    downloads::remove(&app, &id);
}

#[tauri::command]
async fn open_download(app: AppHandle, id: String) -> Result<(), String> {
    downloads::open(&app, &id)
}

#[tauri::command]
async fn reveal_download(app: AppHandle, id: String) -> Result<(), String> {
    downloads::reveal(&app, &id)
}

#[tauri::command]
async fn open_folder(app: AppHandle, id: String) {
    if let Some(cfg) = app.state::<AppState>().app(&id) {
        let _ = Command::new("explorer.exe").arg(&cfg.path).spawn();
    }
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    let mut dialog = app.dialog().file().set_title("Dossier de l'application");
    if let Some(win) = app.get_window("main") {
        dialog = dialog.set_parent(&win);
    }
    dialog.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.display().to_string())
}

#[tauri::command]
async fn inspect_folder(path: String) -> FolderInfo {
    config::inspect_folder(Path::new(&path))
}

#[tauri::command]
async fn save_settings(app: AppHandle, mut settings: Settings) -> Result<(), String> {
    let st = app.state::<AppState>();
    let mut current = st.settings.lock().unwrap();
    settings.zoom = current.zoom.clone(); // owned by the backend, the UI copy may be stale
    settings::save(&st.settings_path, &settings)?;
    *current = settings;
    drop(current);
    emit_state(&app);
    Ok(())
}

/// Opens (or focuses) the app's terminal in its own small window, owned by the main one.
#[tauri::command]
async fn detach_logs(app: AppHandle, id: String) -> Result<(), String> {
    let cfg = app.state::<AppState>().app(&id).ok_or("Application inconnue")?;
    let label = format!("{TERM_PREFIX}{id}");
    if let Some(win) = app.get_window(&label) {
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let main = app.get_window("main").ok_or("Fenêtre principale introuvable")?;
    let size = LogicalSize::new(780.0, 440.0);
    let win = WindowBuilder::new(&app, &label)
        .title(format!("Logs — {}", cfg.name))
        .inner_size(size.width, size.height)
        .min_inner_size(420.0, 220.0)
        .owner(&main)
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    // The page finds its app id from its own label.
    win.add_child(
        WebviewBuilder::new(&label, WebviewUrl::App("terminal.html".into())).auto_resize(),
        LogicalPosition::new(0.0, 0.0),
        size,
    )
    .map_err(|e| e.to_string())?;
    app.state::<AppState>().terminals.lock().unwrap().insert(id);
    emit_state(&app);
    Ok(())
}

/// Closes the detached terminal and tells the main window to show the docked panel again.
#[tauri::command]
async fn dock_logs(app: AppHandle, id: String) {
    close_terminal(&app, &id);
    let _ = app.emit_to(views::UI_LABEL, "terminal-docked", &id);
}

fn close_terminal(app: &AppHandle, id: &str) {
    if let Some(win) = app.get_window(&format!("{TERM_PREFIX}{id}")) {
        let _ = win.destroy();
    }
    if app.state::<AppState>().terminals.lock().unwrap().remove(id) {
        emit_state(app);
    }
}

fn close_all_terminals(app: &AppHandle) {
    for (label, win) in app.windows() {
        if label.starts_with(TERM_PREFIX) {
            let _ = win.destroy();
        }
    }
    app.state::<AppState>().terminals.lock().unwrap().clear();
}

fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// The only way out: stops every launched app (and its process tree) before exiting.
fn quit(app: &AppHandle) {
    close_all_terminals(app);
    runner::stop_all(app);
    app.exit(0);
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Afficher App Viewer", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quitter (arrête toutes les apps)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &PredefinedMenuItem::separator(app)?, &quit_item])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("App Viewer")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    // When `appv open` launches us as a second instance, we hold the foreground right: pass it
    // on so the running instance may bring its window to the front.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY) };
    tauri::Builder::default()
        // Must come first: a second launch (e.g. while hidden in the tray) just shows this one,
        // instead of a new instance fighting over the same ports.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| handle_args(app, args)))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_app,
            delete_app,
            start_app,
            stop_app,
            restart_app,
            external_process,
            take_over_app,
            select_app,
            set_viewport,
            get_logs,
            clear_logs,
            reload_app,
            home_app,
            navigate_app,
            set_zoom,
            set_agent_control,
            open_devtools,
            open_in_browser,
            open_folder,
            get_downloads,
            clear_downloads,
            remove_download,
            open_download,
            reveal_download,
            pick_folder,
            inspect_folder,
            save_settings,
            detach_logs,
            dock_logs,
        ])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let config_path = config_dir.join("apps.json");
            let settings_path = config_dir.join("settings.json");
            let downloads_path = config_dir.join("downloads.json");
            let apps = config::load(&config_path);
            let settings = settings::load(&settings_path);
            let boot: Vec<(String, Option<u16>, bool)> =
                apps.iter().map(|a| (a.id.clone(), (!a.is_native()).then_some(a.port), a.autostart)).collect();
            let downloads = downloads::load(&downloads_path);
            app.manage(AppState::new(config_path, apps, settings_path, settings, downloads_path, downloads));
            // Debug builds only: lets automated tests stand in for the robot button of the UI.
            #[cfg(debug_assertions)]
            if std::env::var_os("APPV_DEBUG_AGENT_ALL").is_some() {
                let st = app.state::<AppState>();
                let ids: Vec<String> = st.apps.lock().unwrap().iter().map(|a| a.id.clone()).collect();
                st.agent_control.lock().unwrap().extend(ids);
            }
            build_tray(app.handle())?;
            ipc::serve(app.handle().clone());

            let window = WindowBuilder::new(app, "main")
                .title("App Viewer")
                .inner_size(1440.0, 900.0)
                .min_inner_size(820.0, 520.0)
                .decorations(false)
                .build()?;
            let size = window.inner_size()?;
            window.add_child(
                WebviewBuilder::new(views::UI_LABEL, WebviewUrl::App("index.html".into())).auto_resize(),
                LogicalPosition::new(0.0, 0.0),
                size,
            )?;
            // No title bar (the UI draws its own controls). Setting it again after the webview
            // exists makes Tauri attach its native resize borders to this frameless window.
            window.set_decorations(false)?;

            let handle = app.handle().clone();
            runner::watch_external(handle.clone());
            stats::watch(handle.clone());
            views::watch_urls(handle.clone());
            // Autostart apps get launched; the others are just shown if already running elsewhere.
            std::thread::spawn(move || {
                for (id, port, autostart) in boot {
                    if autostart || port.is_some_and(runner::port_open) {
                        let _ = runner::start(&handle, &id);
                    }
                }
            });
            // Launched by `appv open` while no instance was running.
            handle_args(app.handle(), std::env::args().collect());
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                reload_config(&handle);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            let label = window.label();
            match event {
                WindowEvent::CloseRequested { api, .. } if label == "main" => {
                    api.prevent_close();
                    let app = window.app_handle();
                    if app.state::<AppState>().settings.lock().unwrap().keep_in_background {
                        // Apps keep running; the tray icon brings the window back or quits.
                        close_all_terminals(app);
                        emit_state(app);
                        let _ = window.hide();
                    } else {
                        quit(app);
                    }
                }
                WindowEvent::Destroyed => {
                    if let Some(id) = label.strip_prefix(TERM_PREFIX) {
                        let app = window.app_handle();
                        if app.state::<AppState>().terminals.lock().unwrap().remove(id) {
                            emit_state(app);
                        }
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build App Viewer")
        .run(|app, event| {
            // Every exit path (tray, window close, crash-free shutdown) ends up here: stop_all
            // is idempotent. A hard kill is covered by the jobs' KILL_ON_JOB_CLOSE.
            if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
                runner::stop_all(app);
            }
        });
}
