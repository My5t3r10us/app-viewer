//! Control pipe: lets the `appv` CLI ask the running App Viewer for things that need an answer
//! (screenshots, driving an app's page, taking a port over). One JSON line in, one JSON line out.
//!
//! Security: the pipe is named after the user's SID, its DACL grants access to that user only,
//! remote clients are rejected, a request is capped in size and must arrive within seconds.
//! What a request may do is decided per command in `dispatch`; nothing here runs arbitrary code.

use crate::{
    agent, capture,
    ipc_common::{pipe_path, user_sid, MAX_MESSAGE},
    runner,
};
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    mem,
    os::windows::io::{AsRawHandle, FromRawHandle},
    ptr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::AppHandle;
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE},
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
    System::{
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
        IO::CancelIoEx,
    },
};

pub const PROTOCOL_VERSION: u64 = 1;
const MAX_CONNECTIONS: usize = 16;
/// Time a client has to deliver its request, and to take the response.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub type Handler = dyn Fn(&Value) -> Result<Value, String> + Send + Sync;

/// Starts the control pipe of the running App Viewer.
pub fn serve(app: AppHandle) {
    let handler: Arc<Handler> = Arc::new(move |req| dispatch(&app, req));
    match pipe_path().and_then(|name| serve_named(name, handler)) {
        Ok(()) => {}
        Err(e) => eprintln!("App Viewer : canal de contrôle indisponible ({e})"),
    }
}

/// What the CLI may ask. Every command checks its own arguments.
fn dispatch(app: &AppHandle, req: &Value) -> Result<Value, String> {
    let str_arg = |key: &str| req.get(key).and_then(Value::as_str);
    match str_arg("cmd").unwrap_or_default() {
        "ping" => Ok(json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() })),
        "screenshot" => {
            let whole_window = req.get("window").and_then(Value::as_bool).unwrap_or(false);
            capture::screenshot(app, str_arg("app"), whole_window)
        }
        "ui" => agent::handle(app, req),
        "status" => {
            let id = str_arg("app").ok_or("application manquante")?;
            Ok(json!({ "status": tauri::Manager::state::<crate::state::AppState>(app).status(id) }))
        }
        // Debug builds only, never shipped: lets tests inspect and drive the shell UI.
        #[cfg(debug_assertions)]
        #[cfg(debug_assertions)]
        "debug-cdp" => agent::debug_cdp(app, str_arg("app").unwrap_or_default(), str_arg("method").unwrap_or_default(), req.get("params").unwrap_or(&Value::Null)),
        #[cfg(debug_assertions)]
        "debug-eval-ui" => agent::debug_eval_ui(app, str_arg("script").unwrap_or("null")).map(|v| json!({ "value": v })),
        "takeover" => {
            let id = str_arg("app").ok_or("application manquante")?;
            let killed = runner::take_over(app, id, None)?;
            Ok(json!({ "killed": killed }))
        }
        other => Err(format!("commande inconnue : « {other} »")),
    }
}

/// Creates the first pipe instance (it fails if the name is already taken, so a squatter can't
/// shadow us), then accepts clients on a background thread.
pub fn serve_named(name: String, handler: Arc<Handler>) -> Result<(), String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || accept_loop(&name, handler, ready_tx));
    ready_rx.recv().map_err(|_| "le canal n'a pas démarré".to_string())?
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn accept_loop(name: &str, handler: Arc<Handler>, ready: mpsc::Sender<Result<(), String>>) {
    let sid = match user_sid() {
        Ok(s) => s,
        Err(e) => return drop(ready.send(Err(e))),
    };
    // Protected DACL: generic-all for the current user and nobody else.
    let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
    let mut descriptor = ptr::null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut()) } == 0 {
        return drop(ready.send(Err("droits du canal impossibles à définir".into())));
    }
    // The descriptor is intentionally never freed: it lives as long as the pipe server.
    let attrs = SECURITY_ATTRIBUTES { nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
    let name_w = wide(name);
    let active = Arc::new(AtomicUsize::new(0));
    let mut ready = Some(ready);
    let mut first = true;
    loop {
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let pipe = unsafe {
            CreateNamedPipeW(
                name_w.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &attrs,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            let err = unsafe { GetLastError() };
            if let Some(tx) = ready.take() {
                return drop(tx.send(Err(format!("création du canal impossible (erreur {err})"))));
            }
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        first = false;
        if let Some(tx) = ready.take() {
            let _ = tx.send(Ok(()));
        }
        let connected = unsafe { ConnectNamedPipe(pipe, ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if !connected {
            unsafe { CloseHandle(pipe) };
            continue;
        }
        // Owned from here on: dropping the File closes the handle.
        let file = unsafe { File::from_raw_handle(pipe as _) };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            continue; // too many at once: hang up on this one
        }
        let (handler, active) = (handler.clone(), active.clone());
        thread::spawn(move || {
            serve_client(file, handler.as_ref());
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Cancels the blocking I/O of a connection that takes too long. Dropping it (the normal case)
/// disarms it and waits for its thread, so the handle is never touched after it is closed.
struct Watchdog {
    disarm: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    fn start(handle: HANDLE, timeout: Duration) -> Self {
        let handle = handle as usize;
        let (tx, rx) = mpsc::channel::<()>();
        let thread = thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(timeout) {
                unsafe { CancelIoEx(handle as HANDLE, ptr::null()) };
            }
        });
        Self { disarm: Some(tx), thread: Some(thread) }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        drop(self.disarm.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve_client(mut file: File, handler: &Handler) {
    let raw = file.as_raw_handle() as HANDLE;
    let request = {
        let _guard = Watchdog::start(raw, IO_TIMEOUT);
        read_request(&file)
    };
    let response = match request.and_then(|req| handler(&req)) {
        Ok(mut value) => {
            if let Some(map) = value.as_object_mut() {
                map.insert("ok".into(), true.into());
            }
            value
        }
        Err(error) => json!({ "ok": false, "error": error }),
    };
    let mut line = serde_json::to_vec(&response).unwrap_or_else(|_| br#"{"ok":false,"error":"reponse illisible"}"#.to_vec());
    line.push(b'\n');
    let _guard = Watchdog::start(raw, IO_TIMEOUT);
    if file.write_all(&line).is_ok() {
        let _ = file.sync_all(); // returns once the client has read it
    }
}

fn read_request(file: &File) -> Result<Value, String> {
    let mut line = Vec::new();
    // One byte past the limit tells "exactly at the limit" from "too long".
    BufReader::new(file.take(MAX_MESSAGE as u64 + 1))
        .read_until(b'\n', &mut line)
        .map_err(|_| "requête non reçue à temps".to_string())?;
    if line.len() > MAX_MESSAGE {
        return Err("requête trop longue".into());
    }
    let req: Value = serde_json::from_slice(&line).map_err(|_| "requête illisible (JSON attendu)".to_string())?;
    match req.get("v").and_then(Value::as_u64) {
        Some(PROTOCOL_VERSION) => Ok(req),
        _ => Err(format!("version du protocole non prise en charge (attendu : {PROTOCOL_VERSION}) : mettez à jour appv et App Viewer ensemble")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    fn echo() -> Arc<Handler> {
        Arc::new(|req| match req["cmd"].as_str() {
            Some("echo") => Ok(json!({ "echo": req["text"] })),
            _ => Err("non".into()),
        })
    }

    fn ask(name: &str, line: &str) -> Value {
        let mut pipe = OpenOptions::new().read(true).write(true).open(name).unwrap();
        pipe.write_all(line.as_bytes()).unwrap();
        pipe.write_all(b"\n").unwrap();
        let mut answer = String::new();
        BufReader::new(&pipe).read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap()
    }

    #[test]
    fn answers_requests_and_rejects_bad_ones() {
        let name = format!(r"\\.\pipe\app-viewer-test-{}", std::process::id());
        serve_named(name.clone(), echo()).unwrap();

        let ok = ask(&name, r#"{"v":1,"cmd":"echo","text":"salut"}"#);
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["echo"], "salut");

        let refused = ask(&name, r#"{"v":1,"cmd":"autre"}"#);
        assert_eq!(refused["ok"], false);

        let wrong_version = ask(&name, r#"{"v":99,"cmd":"echo"}"#);
        assert_eq!(wrong_version["ok"], false);

        let garbage = ask(&name, "pas du json");
        assert_eq!(garbage["ok"], false);
    }

    #[test]
    fn a_second_server_cannot_take_the_same_name() {
        let name = format!(r"\\.\pipe\app-viewer-test-dup-{}", std::process::id());
        serve_named(name.clone(), echo()).unwrap();
        assert!(serve_named(name, echo()).is_err());
    }

    #[test]
    fn oversized_requests_are_refused() {
        let name = format!(r"\\.\pipe\app-viewer-test-big-{}", std::process::id());
        serve_named(name.clone(), echo()).unwrap();
        let mut pipe = OpenOptions::new().read(true).write(true).open(&name).unwrap();
        let big = vec![b'x'; MAX_MESSAGE + 10];
        // The server may hang up before reading everything: only the answer matters.
        let _ = pipe.write_all(&big);
        let _ = pipe.write_all(b"\n");
        let mut answer = String::new();
        let _ = BufReader::new(&pipe).read_line(&mut answer);
        assert!(answer.is_empty() || answer.contains("trop longue"));
    }
}
