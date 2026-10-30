//! Client of App Viewer's control pipe (see `ipc.rs` of the GUI for the protocol and its safeguards).

use crate::{
    ipc_common::{pipe_path, MAX_MESSAGE},
    win,
};
use serde_json::{json, Map, Value};
use std::{
    env,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    os::windows::io::AsRawHandle,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::ERROR_FILE_NOT_FOUND, System::Pipes::GetNamedPipeServerProcessId};

const PROTOCOL_VERSION: u64 = 1;
const ERROR_PIPE_BUSY: i32 = 231;

pub const NOT_RUNNING: &str = "App Viewer n'est pas lancé (démarrez-le avec `appv open <app>`)";

/// Sends one command to the running App Viewer and returns its answer (without the `ok` flag).
pub fn request(cmd: &str, args: Value) -> Result<Value, String> {
    let mut msg = match args {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    msg.insert("v".into(), PROTOCOL_VERSION.into());
    msg.insert("cmd".into(), cmd.into());

    let path = pipe_path()?;
    let pipe = open_pipe(&path)?;
    check_server(&pipe)?;

    let mut line = serde_json::to_vec(&Value::Object(msg)).map_err(|e| e.to_string())?;
    line.push(b'\n');
    (&pipe).write_all(&line).map_err(|e| format!("envoi à App Viewer impossible : {e}"))?;

    let mut answer = Vec::new();
    BufReader::new((&pipe).take(MAX_MESSAGE as u64 + 1))
        .read_until(b'\n', &mut answer)
        .map_err(|e| format!("réponse d'App Viewer illisible : {e}"))?;
    if answer.is_empty() {
        return Err("App Viewer n'a pas répondu (version incompatible ou requête refusée)".into());
    }
    let mut reply: Value = serde_json::from_slice(&answer).map_err(|_| "réponse d'App Viewer illisible".to_string())?;
    if reply["ok"] == json!(true) {
        if let Some(map) = reply.as_object_mut() {
            map.remove("ok");
        }
        Ok(reply)
    } else {
        Err(reply["error"].as_str().unwrap_or("erreur inconnue").to_string())
    }
}

fn open_pipe(path: &str) -> Result<std::fs::File, String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(f) => return Ok(f),
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Err(NOT_RUNNING.into()),
            // All instances are busy serving other clients: wait for a free one.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("canal d'App Viewer inaccessible : {e}")),
        }
    }
}

/// The pipe must be served by the app-viewer.exe installed next to this appv.exe: never send
/// a request (which may carry typed text) to whatever process happens to own the name.
fn check_server(pipe: &std::fs::File) -> Result<(), String> {
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as _, &mut pid) } == 0 {
        return Err("serveur du canal introuvable".into());
    }
    let server = win::process_image(pid).ok_or("serveur du canal illisible")?;
    let expected: PathBuf = env::current_exe().map_err(|e| e.to_string())?.with_file_name(crate::GUI_EXE);
    let same = |a: &PathBuf, b: &PathBuf| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    if !same(&server, &expected) {
        return Err(format!("le canal n'est pas servi par App Viewer ({})", server.display()));
    }
    Ok(())
}
