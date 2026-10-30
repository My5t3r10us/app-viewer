//! Driving the page of an app on behalf of an agent (`appv ui ...`), without a headless browser,
//! a debugging port or a PowerShell script: App Viewer already owns the app's WebView2.
//!
//! Safeguards: nothing happens unless the user switched "contrôle par agent" on for that app, from
//! the shell UI (no CLI command can) and for the session only; the actions are a fixed list run by
//! `agent.js` on elements designated by reference, never agent-supplied code; every action is
//! written in the app's logs, and the UI shows when an agent is active.

use crate::{
    capture,
    state::{push_log, AppState},
    views,
};
use serde_json::{json, Map, Value};
use std::{
    sync::{mpsc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, ExecuteScriptCompletedHandler};
use windows::core::HSTRING;

const AGENT_JS: &str = include_str!("agent.js");
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_WAIT_SECS: u64 = 60;

/// One agent action at a time: two agents typing in the same page would corrupt each other.
static SERIAL: Mutex<()> = Mutex::new(());

pub fn handle(app: &AppHandle, req: &Value) -> Result<Value, String> {
    let id = req.get("app").and_then(Value::as_str).ok_or("application manquante")?;
    let action = req.get("action").and_then(Value::as_str).ok_or("action manquante")?;
    let st = app.state::<AppState>();
    let cfg = st.app(id).ok_or("Application inconnue.")?;
    if !st.agent_control.lock().unwrap().contains(id) {
        return Err(format!(
            "Le contrôle par agent est désactivé pour « {} ». Seul l'utilisateur peut l'activer, avec le bouton robot de la barre d'outils d'App Viewer : demandez-le-lui, ne cherchez pas à le contourner.",
            cfg.name
        ));
    }
    let _one_at_a_time = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    capture::bring_forward(app, id)?;

    // The action's own arguments: everything but the envelope.
    let mut args: Map<String, Value> = req.as_object().cloned().unwrap_or_default();
    for key in ["v", "cmd", "app", "action"] {
        args.remove(key);
    }
    let args = Value::Object(args);

    let _ = app.emit_to(views::UI_LABEL, "agent-activity", json!({ "id": id }));
    let result = match action {
        "navigate" => navigate(app, id, &args),
        "wait" => wait(app, id, &args),
        "snapshot" | "text" | "click" | "type" | "select" | "press" | "scroll" => {
            let mut out = match action {
                "click" => click(app, id, &args)?,
                "type" => type_text(app, id, &args)?,
                "press" => press(app, id, &args)?,
                _ => exec(app, id, action, &args)?,
            };
            // These may have changed the page: tell the agent where it is now.
            if matches!(action, "click" | "press" | "type") {
                thread::sleep(Duration::from_millis(350));
                if let Ok(info) = exec(app, id, "info", &Value::Null) {
                    out["page"] = info;
                }
            }
            Ok(out)
        }
        other => Err(format!("action inconnue : « {other} »")),
    };
    push_log(app, id, log_line(action, &args, &result));
    result
}

/// What goes in the app's logs. Text typed into a password field is never written.
fn log_line(action: &str, args: &Value, result: &Result<Value, String>) -> String {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
    let secret = result.as_ref().ok().and_then(|r| r.get("secret")).and_then(Value::as_bool).unwrap_or(false);
    let detail = match action {
        "click" | "select" => format!("{} {}", arg("ref"), arg("value")),
        "type" => format!("{} ← {}", arg("ref"), if secret { "••••••".to_string() } else { format!("{:?}", arg("text")) }),
        "press" => arg("key").to_string(),
        "navigate" => arg("target").to_string(),
        "wait" => format!("{:?}", arg("text")),
        "scroll" => format!("{}{}", arg("to"), args.get("by").map(|b| b.to_string()).unwrap_or_default()),
        _ => String::new(),
    };
    match result {
        Ok(_) => format!("🤖 {action} {detail}").trim_end().to_string(),
        Err(e) => format!("🤖 {action} {detail} ✖ {e}"),
    }
}

fn navigate(app: &AppHandle, id: &str, args: &Value) -> Result<Value, String> {
    let target = args.get("target").and_then(Value::as_str).ok_or("adresse manquante")?;
    views::navigate(app, id, target)?;
    // Give the page a moment, then report where we landed.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        thread::sleep(Duration::from_millis(250));
        if let Ok(info) = exec(app, id, "info", &Value::Null) {
            if info.get("ready").and_then(Value::as_str) == Some("complete") || Instant::now() > deadline {
                return Ok(json!({ "page": info }));
            }
        } else if Instant::now() > deadline {
            return Ok(json!({}));
        }
    }
}

/// Waits (polling) for a text to appear, or with `gone` to disappear.
fn wait(app: &AppHandle, id: &str, args: &Value) -> Result<Value, String> {
    let text = args.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or("texte manquant")?;
    let gone = args.get("gone").and_then(Value::as_bool).unwrap_or(false);
    let secs = args.get("timeout").and_then(Value::as_u64).unwrap_or(10).min(MAX_WAIT_SECS);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(secs) {
        let found = exec(app, id, "has-text", &json!({ "text": text }))
            .ok()
            .and_then(|r| r.get("found").and_then(Value::as_bool));
        if found == Some(!gone) {
            return Ok(json!({ "waited": started.elapsed().as_millis() as u64 }));
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("délai dépassé ({secs} s) : le texte {text:?} est toujours {}", if gone { "présent" } else { "absent" }))
}

// ------------------------------------------------------------ real input
// Events made by script are untrusted: the page gets no user activation, so a second download,
// a popup or a clipboard write is refused. The embedded DevTools protocol of the WebView2 (an
// in-process call, no debugging port is opened) injects genuine input instead. Whenever it
// fails, the script-made equivalent is the fallback.

/// A real left click on the element.
fn click(app: &AppHandle, id: &str, args: &Value) -> Result<Value, String> {
    let target = exec(app, id, "locate", args)?;
    let (x, y) = (target["x"].as_f64().unwrap_or(0.0), target["y"].as_f64().unwrap_or(0.0));
    // Page zoom scales the viewport: the protocol wants device-independent pixels.
    let z = app.state::<AppState>().settings.lock().unwrap().zoom.get(id).copied().unwrap_or(1.0);
    let (x, y) = (x * z, y * z);
    let mouse = |kind: &str| json!({ "type": kind, "x": x, "y": y, "button": "left", "buttons": u8::from(kind == "mousePressed"), "clickCount": u8::from(kind != "mouseMoved") });
    let sent = cdp(app, id, "Input.dispatchMouseEvent", &mouse("mouseMoved"))
        .and_then(|_| cdp(app, id, "Input.dispatchMouseEvent", &mouse("mousePressed")))
        .and_then(|_| cdp(app, id, "Input.dispatchMouseEvent", &mouse("mouseReleased")));
    match sent {
        Ok(_) => Ok(json!({ "desc": target["desc"], "covered": target["covered"] })),
        Err(_) => exec(app, id, "click", args), // script-made click
    }
}

/// Real typing: the field gets focus with its content selected, the text replaces it.
fn type_text(app: &AppHandle, id: &str, args: &Value) -> Result<Value, String> {
    let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
    let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
    let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
    let field = exec(app, id, "focus-field", args)?;
    let typed = if text.is_empty() {
        // Nothing to insert: replacing with nothing means deleting the selection.
        if append || !field["hasText"].as_bool().unwrap_or(false) { Ok(Value::Null) } else { key_press(app, id, "Backspace") }
    } else {
        cdp(app, id, "Input.insertText", &json!({ "text": text }))
    };
    if typed.is_err() {
        return exec(app, id, "type", args); // script-made typing (value setter + events)
    }
    if submit {
        key_press(app, id, "Enter")?;
    }
    Ok(json!({ "desc": field["desc"], "secret": field["secret"] }))
}

fn press(app: &AppHandle, id: &str, args: &Value) -> Result<Value, String> {
    let combo = args.get("key").and_then(Value::as_str).ok_or("touche manquante")?;
    match key_press(app, id, combo) {
        Ok(_) => Ok(json!({ "desc": combo })),
        Err(_) => exec(app, id, "press", args),
    }
}

/// (key, code, virtual key code, text typed by it) of the keys an agent is likely to press.
fn key_def(name: &str) -> Option<(String, String, u32, Option<String>)> {
    let named = |key: &str, code: &str, vk: u32, text: Option<&str>| Some((key.to_string(), code.to_string(), vk, text.map(String::from)));
    match name {
        "Enter" => named("Enter", "Enter", 13, Some("\r")),
        "Escape" | "Esc" => named("Escape", "Escape", 27, None),
        "Tab" => named("Tab", "Tab", 9, None),
        "Backspace" => named("Backspace", "Backspace", 8, None),
        "Delete" => named("Delete", "Delete", 46, None),
        "ArrowUp" => named("ArrowUp", "ArrowUp", 38, None),
        "ArrowDown" => named("ArrowDown", "ArrowDown", 40, None),
        "ArrowLeft" => named("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowRight" => named("ArrowRight", "ArrowRight", 39, None),
        "Home" => named("Home", "Home", 36, None),
        "End" => named("End", "End", 35, None),
        "PageUp" => named("PageUp", "PageUp", 33, None),
        "PageDown" => named("PageDown", "PageDown", 34, None),
        "Space" | " " => named(" ", "Space", 32, Some(" ")),
        single if single.chars().count() == 1 => {
            let c = single.chars().next()?;
            let upper = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() { format!("Key{upper}") } else if c.is_ascii_digit() { format!("Digit{c}") } else { String::new() };
            Some((single.to_string(), code, upper as u32, Some(single.to_string())))
        }
        _ => None,
    }
}

/// "Ctrl+Shift+a", "Enter"... as real key down / key up events.
fn key_press(app: &AppHandle, id: &str, combo: &str) -> Result<Value, String> {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let name = if combo.ends_with('+') && combo.len() > 1 { parts.pop(); parts.pop(); "+" } else { parts.pop().unwrap_or_default() };
    let mut modifiers = 0u8;
    for m in &parts {
        modifiers |= match m.to_lowercase().as_str() {
            "alt" => 1,
            "ctrl" | "control" => 2,
            "meta" | "cmd" | "win" => 4,
            "shift" => 8,
            other => return Err(format!("modificateur inconnu : {other}")),
        };
    }
    let (key, code, vk, text) = key_def(name).ok_or_else(|| format!("touche inconnue : {name}"))?;
    // With Ctrl or Alt held, a letter is a shortcut, not text.
    let text = text.filter(|_| modifiers & 3 == 0);
    let mut down = json!({ "type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers });
    if let Some(t) = &text {
        down["text"] = json!(t);
    }
    cdp(app, id, "Input.dispatchKeyEvent", &down)?;
    cdp(app, id, "Input.dispatchKeyEvent", &json!({ "type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers }))
}

/// One DevTools protocol call on the app's page (CallDevToolsProtocolMethod).
fn cdp(app: &AppHandle, id: &str, method: &str, params: &Value) -> Result<Value, String> {
    let wv = views::webview(app, id).ok_or("L'app n'est pas ouverte.")?;
    let (method, params) = (HSTRING::from(method), HSTRING::from(json_literal(params)));
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    wv.with_webview(move |platform| unsafe {
        let started = (|| -> Result<(), String> {
            let core = platform.controller().CoreWebView2().map_err(|e| e.to_string())?;
            let done = tx.clone();
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
                let _ = done.send(result.map(|_| json).map_err(|e| e.to_string()));
                Ok(())
            }));
            core.CallDevToolsProtocolMethod(&method, &params, &handler).map_err(|e| e.to_string())
        })();
        if let Err(e) = started {
            let _ = tx.send(Err(e));
        }
    })
    .map_err(|e| e.to_string())?;
    let raw = rx.recv_timeout(SCRIPT_TIMEOUT).map_err(|_| "La page n'a pas répondu à temps.".to_string())??;
    serde_json::from_str(&raw).map_err(|_| "Réponse DevTools illisible.".to_string())
}

/// Runs one agent.js action in the app's page and returns its JSON result.
fn exec(app: &AppHandle, id: &str, action: &str, args: &Value) -> Result<Value, String> {
    let wv = views::webview(app, id).ok_or("L'app n'est pas ouverte.")?;
    let raw = run_script(&wv, script_for(action, args))?;
    parse_result(&raw)
}

/// Debug builds only (see ipc.rs): evaluates a script in the shell UI, to check the interface by hand.
#[cfg(debug_assertions)]
pub fn debug_eval_ui(app: &AppHandle, script: &str) -> Result<Value, String> {
    let wv = views::ui_webview(app).ok_or("UI introuvable")?;
    let raw = run_script(&wv, script.to_string())?;
    Ok(serde_json::from_str(&raw).unwrap_or(Value::Null))
}

/// Debug builds only: one DevTools protocol call on an app's page (e.g. a Ctrl+wheel to check the zoom).
#[cfg(debug_assertions)]
pub fn debug_cdp(app: &AppHandle, id: &str, method: &str, params: &Value) -> Result<Value, String> {
    if method == "IsZoomControlEnabled" {
        let wv = views::webview(app, id).ok_or("L'app n'est pas ouverte.")?;
        let (tx, rx) = mpsc::channel();
        wv.with_webview(move |p| unsafe {
            let mut on = windows::core::BOOL(0);
            let r = p.controller().CoreWebView2().and_then(|c| c.Settings()).and_then(|s| s.IsZoomControlEnabled(&mut on));
            let _ = tx.send(r.map(|_| on.as_bool()).map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
        return rx.recv_timeout(SCRIPT_TIMEOUT).map_err(|e| e.to_string())?.map(|on| json!({ "isZoomControlEnabled": on }));
    }
    cdp(app, id, method, params)
}

/// ExecuteScript on a webview: the JSON text of the script's result.
fn run_script(wv: &tauri::Webview, script: String) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    wv.with_webview(move |platform| unsafe {
        let started = (|| -> Result<(), String> {
            let core = platform.controller().CoreWebView2().map_err(|e| e.to_string())?;
            let done = tx.clone();
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |result, json| {
                let _ = done.send(result.map(|_| json).map_err(|e| e.to_string()));
                Ok(())
            }));
            core.ExecuteScript(&HSTRING::from(script), &handler).map_err(|e| e.to_string())
        })();
        if let Err(e) = started {
            let _ = tx.send(Err(e));
        }
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(SCRIPT_TIMEOUT).map_err(|_| "La page n'a pas répondu à temps.".to_string())?
}

/// The page-side call. `action` and `args` are embedded as JSON data (a serialised JSON value is a
/// valid JS literal): whatever the agent sent stays a value and can't become code.
fn script_for(action: &str, args: &Value) -> String {
    format!(
        "(function(){{try{{return JSON.stringify(({AGENT_JS})({},{}));}}catch(e){{return JSON.stringify({{error:String((e&&e.message)||e)}});}}}})()",
        json_literal(&Value::String(action.to_string())),
        json_literal(args),
    )
}

/// JSON text that is also safe inside a script: U+2028 / U+2029 are escaped for older engines.
fn json_literal(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into()).replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// ExecuteScript hands back the JSON of what the script returned: here a JSON *string* holding our
/// own JSON. `{"error": ...}` is how agent.js reports a failure.
fn parse_result(raw: &str) -> Result<Value, String> {
    let inner: String = serde_json::from_str(raw)
        .map_err(|_| "Réponse de la page illisible (la page est peut-être en train de changer).".to_string())?;
    let value: Value = serde_json::from_str(&inner).map_err(|_| "Réponse de la page illisible.".to_string())?;
    match value.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_results_are_unwrapped_and_errors_surface() {
        let ok = parse_result(r#""{\"text\":\"salut\"}""#).unwrap();
        assert_eq!(ok["text"], "salut");
        let err = parse_result(r#""{\"error\":\"référence e9 inconnue\"}""#).unwrap_err();
        assert!(err.contains("e9"));
        assert!(parse_result("null").is_err());
    }

    #[test]
    fn password_text_never_reaches_the_logs() {
        let args = json!({ "ref": "e3", "text": "hunter2" });
        let secret = Ok(json!({ "secret": true }));
        let plain = Ok(json!({ "secret": false }));
        assert!(!log_line("type", &args, &secret).contains("hunter2"));
        assert!(log_line("type", &args, &plain).contains("hunter2"));
    }

    #[test]
    fn hostile_input_stays_inside_a_json_literal() {
        let evil = json!({ "text": "\"});alert(1);({\"\u{2028}" });
        let script = script_for("type", &evil);
        // The quote that would close the string is escaped, and the line separator too.
        assert!(script.contains(r#"{"text":"\"});alert(1);({\"\u2028"}"#));
        assert!(!script.contains('\u{2028}'));
    }
}
