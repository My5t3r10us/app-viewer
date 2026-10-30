//! Launches the configured command, streams its output, and watches the port.

use crate::{
    job::{self, Job},
    procs::{self, ProcInfo},
    state::{emit_state, push_log, AppState, Status},
    views,
};
use std::{
    io::{BufRead, BufReader, Read},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PORT_WARNING_AFTER: Duration = Duration::from_secs(30);

pub fn port_open(port: u16) -> bool {
    let timeout = Duration::from_millis(200);
    [SocketAddr::from((Ipv4Addr::LOCALHOST, port)), SocketAddr::from((Ipv6Addr::LOCALHOST, port))]
        .iter()
        .any(|addr| TcpStream::connect_timeout(addr, timeout).is_ok())
}

fn set_status(app: &AppHandle, id: &str, status: Status, error: Option<String>) {
    let st = app.state::<AppState>();
    {
        let mut rts = st.runtimes.lock().unwrap();
        let rt = rts.entry(id.to_string()).or_default();
        rt.status = status;
        rt.error = error;
    }
    emit_state(app);
    views::sync(app);
}

pub fn start(app: &AppHandle, id: &str) -> Result<(), String> {
    let st = app.state::<AppState>();
    let cfg = st.app(id).ok_or("Application inconnue")?;
    {
        // Check-and-claim under one lock so a double click can't launch twice.
        let mut rts = st.runtimes.lock().unwrap();
        let rt = rts.entry(id.to_string()).or_default();
        if matches!(rt.status, Status::Starting | Status::Running | Status::External) {
            return Ok(());
        }
        rt.status = Status::Starting;
        rt.error = None;
    }

    if !cfg.is_native() && port_open(cfg.port) {
        push_log(app, id, format!("● Le port {} répond déjà : application déjà lancée hors d'App Viewer, affichage direct.", cfg.port));
        set_status(app, id, Status::External, None);
        return Ok(());
    }

    st.logs.lock().unwrap().remove(id);
    push_log(app, id, format!("▶ {}  (dans {})", cfg.command, cfg.path));

    // chcp 65001 so cmd.exe's own messages ("... n'est pas reconnu ...") come out as UTF-8.
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/D", "/S", "/C"])
        .raw_arg(format!("\"chcp 65001 >nul & {}\"", cfg.command))
        .current_dir(&cfg.path)
        .env("BROWSER", "none")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | job::CREATE_SUSPENDED);
    if cfg.set_port_env {
        cmd.env("PORT", cfg.port.to_string());
    } else {
        cmd.env_remove("PORT");
    }

    let fail = |msg: String| {
        push_log(app, id, format!("✖ {msg}"));
        set_status(app, id, Status::Error, Some(msg.clone()));
        Err(msg)
    };

    let job = match Job::new() {
        Ok(j) => Arc::new(j),
        Err(e) => return fail(format!("Création du job Windows impossible : {e}")),
    };
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("Lancement impossible : {e}")),
    };
    // The child is suspended until it's in the job, so nothing it spawns can escape it.
    if let Err(e) = job.assign(&child) {
        let _ = child.kill();
        return fail(format!("Rattachement au job impossible : {e}"));
    }
    if let Err(e) = job::resume(&child) {
        job.terminate();
        return fail(format!("Reprise du processus impossible : {e}"));
    }

    let generation = {
        let mut rts = st.runtimes.lock().unwrap();
        let rt = rts.entry(id.to_string()).or_default();
        rt.generation += 1;
        // A native app has no port to wait for: it is up as soon as it runs.
        rt.status = if cfg.is_native() { Status::Running } else { Status::Starting };
        rt.error = None;
        rt.stopping = false;
        rt.job = Some(job.clone());
        rt.generation
    };
    emit_state(app);

    if let Some(out) = child.stdout.take() {
        pump(app.clone(), id.to_string(), out);
    }
    if let Some(err) = child.stderr.take() {
        pump(app.clone(), id.to_string(), err);
    }

    if cfg.is_native() {
        push_log(app, id, "● Lancée : sa fenêtre sera affichée dans App Viewer dès qu'elle apparaît.".into());
        let (app, id) = (app.clone(), id.to_string());
        // A launcher may exit at once and leave the real program running: the app is over only
        // when nothing is left in its job.
        thread::spawn(move || {
            let mut code = None;
            loop {
                if code.is_none() {
                    match child.try_wait() {
                        Ok(Some(exit)) => code = Some(exit.code()),
                        Err(_) => code = Some(None),
                        Ok(None) => {}
                    }
                }
                if code.is_some() && job.active_processes() == 0 {
                    crate::embed::forget(&id);
                    return on_exit(&app, &id, generation, code.flatten());
                }
                // Its window shows up some time after launch (or is replaced, splash → main):
                // keep looking for it so it lands in App Viewer.
                if !crate::embed::is_embedded(&id) {
                    views::sync(&app);
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
        return Ok(());
    }

    let (app, id, port) = (app.clone(), id.to_string(), cfg.port);
    thread::spawn(move || {
        let started = Instant::now();
        let mut warned = false;
        loop {
            match child.try_wait() {
                Ok(Some(exit)) => return on_exit(&app, &id, generation, exit.code()),
                Err(_) => return on_exit(&app, &id, generation, None),
                Ok(None) => {}
            }
            let current = {
                let st = app.state::<AppState>();
                let rts = st.runtimes.lock().unwrap();
                rts.get(&id).filter(|r| r.generation == generation).map(|r| r.status)
            };
            if current == Some(Status::Starting) {
                if port_open(port) {
                    push_log(&app, &id, format!("● Prêt sur http://localhost:{port} ({:.1}s)", started.elapsed().as_secs_f32()));
                    set_status(&app, &id, Status::Running, None);
                } else if !warned && started.elapsed() > PORT_WARNING_AFTER {
                    warned = true;
                    push_log(&app, &id, format!("⚠ Le port {port} ne répond toujours pas. Vérifiez le port configuré (ou que l'app n'a pas pris un autre port)."));
                }
            }
            thread::sleep(Duration::from_millis(300));
        }
    });
    Ok(())
}

fn on_exit(app: &AppHandle, id: &str, generation: u64, code: Option<i32>) {
    let st = app.state::<AppState>();
    let stopping = {
        let mut rts = st.runtimes.lock().unwrap();
        let Some(rt) = rts.get_mut(id).filter(|r| r.generation == generation) else { return };
        rt.job = None; // closing the job kills anything the command left behind
        std::mem::take(&mut rt.stopping)
    };
    let code_txt = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    if stopping {
        push_log(app, id, "■ Arrêté.".into());
        set_status(app, id, Status::Stopped, None);
    } else if code == Some(0) {
        push_log(app, id, "■ Le processus s'est terminé (code 0).".into());
        set_status(app, id, Status::Stopped, None);
    } else {
        let msg = format!("Le processus s'est arrêté (code {code_txt})");
        push_log(app, id, format!("✖ {msg}"));
        set_status(app, id, Status::Error, Some(msg));
    }
}

pub fn stop(app: &AppHandle, id: &str) {
    let st = app.state::<AppState>();
    let job = {
        let mut rts = st.runtimes.lock().unwrap();
        let Some(rt) = rts.get_mut(id) else { return };
        match rt.status {
            // Not ours: just stop showing it.
            Status::External => {
                rt.status = Status::Stopped;
                None
            }
            Status::Error => {
                rt.status = Status::Stopped;
                rt.error = None;
                None
            }
            _ => {
                rt.stopping = true;
                rt.job.clone()
            }
        }
    };
    match job {
        Some(job) => job.terminate(), // the watcher thread reports the exit
        None => {
            emit_state(app);
            views::sync(app);
        }
    }
}

/// Stops then waits (up to 5s) for the process to be gone before relaunching.
pub fn restart(app: &AppHandle, id: &str) -> Result<(), String> {
    stop(app, id);
    let st = app.state::<AppState>();
    for _ in 0..50 {
        if !matches!(st.status(id), Status::Starting | Status::Running) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    start(app, id)
}

/// The process holding an app's port, for the "take over" confirmation.
pub fn external_info(app: &AppHandle, id: &str) -> Option<ProcInfo> {
    let cfg = app.state::<AppState>().app(id).filter(|c| !c.is_native())?;
    procs::listener(cfg.port)
}

/// Frees the app's port by ending the process (tree) that listens on it, then launches the app
/// ourselves. For an app shown as "Externe", or one whose port is taken before we ever attached.
/// Returns what was ended, e.g. "node.exe (PID 1234)".
/// `expected_pid`: the process the user was shown and confirmed; if something else listens now, nothing is killed.
pub fn take_over(app: &AppHandle, id: &str, expected_pid: Option<u32>) -> Result<String, String> {
    let st = app.state::<AppState>();
    let cfg = st.app(id).ok_or("Application inconnue")?;
    if cfg.is_native() {
        return Err("Une app native n'a pas de port à reprendre.".into());
    }
    if matches!(st.status(id), Status::Starting | Status::Running) {
        return Err("L'app est déjà lancée par App Viewer.".into());
    }
    let target = procs::listener(cfg.port).ok_or_else(|| format!("Aucun process n'écoute sur le port {}.", cfg.port))?;
    if expected_pid.is_some_and(|pid| pid != target.pid) {
        return Err(format!("Le process sur le port {} a changé ({}) : recommencez.", cfg.port, target.label()));
    }
    let killed = procs::kill_tree(&target)?;
    if st.status(id) == Status::External {
        // Not "up" anymore: the webview closes, and start() below launches for real.
        set_status(app, id, Status::Stopped, None);
    }
    for _ in 0..50 {
        if !port_open(cfg.port) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if port_open(cfg.port) {
        return Err(format!(
            "{} est arrêté mais le port {} répond encore : un process parent le relance peut-être.",
            target.label(),
            cfg.port
        ));
    }
    start(app, id)?;
    let label = target.label();
    push_log(app, id, format!("■ Process externe arrêté : {label}{}", if killed > 1 { format!(" et ses {} sous-process", killed - 1) } else { String::new() }));
    Ok(label)
}

/// Apps we only attached to have no process to watch: poll their port instead.
pub fn watch_external(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(3));
        let st = app.state::<AppState>();
        let external: Vec<String> = st
            .runtimes
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, rt)| rt.status == Status::External)
            .map(|(id, _)| id.clone())
            .collect();
        for id in external {
            let Some(cfg) = st.app(&id) else { continue };
            if !port_open(cfg.port) && st.status(&id) == Status::External {
                push_log(&app, &id, format!("■ Le port {} ne répond plus.", cfg.port));
                set_status(&app, &id, Status::Stopped, None);
            }
        }
    });
}

/// Kills every process tree we launched and waits (up to 3s) until the jobs are empty.
/// Idempotent: called from every exit path. Apps we only attached to are left alone.
pub fn stop_all(app: &AppHandle) {
    let st = app.state::<AppState>();
    let jobs: Vec<Arc<Job>> = {
        let mut rts = st.runtimes.lock().unwrap();
        rts.values_mut()
            .filter_map(|rt| {
                rt.stopping = true;
                rt.job.take()
            })
            .collect()
    };
    for job in &jobs {
        job.terminate();
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    for job in &jobs {
        if !job.wait_empty(deadline) {
            eprintln!("App Viewer : un job n'est pas vide après l'arrêt ({} process)", job.active_processes());
        }
    }
    // Dropping the last handles closes the jobs: KILL_ON_JOB_CLOSE finishes off any straggler.
}

fn pump(app: AppHandle, id: String, stream: impl Read + Send + 'static) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = strip_ansi(String::from_utf8_lossy(&buf).trim_end());
                    push_log(&app, &id, line);
                }
            }
        }
    });
}

/// Removes terminal escape sequences (colors, cursor moves) from a log line.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}
