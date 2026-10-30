//! Takes a port back from a process App Viewer did not launch ("Externe" apps): identifies the
//! listener, refuses what must never be killed, and ends its process tree.
//! Only the process listening on an app's registered port is ever a target, never an arbitrary PID.

use serde::Serialize;
use std::{collections::HashMap, mem, path::PathBuf};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Threading::{OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE},
    },
};

/// What is listening on a port, as shown in the confirmation and in the logs.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcInfo {
    pub pid: u32,
    /// Executable file name ("node.exe"), empty if it can't be read.
    pub name: String,
    pub path: String,
}

impl ProcInfo {
    pub fn label(&self) -> String {
        let name = if self.name.is_empty() { "process inconnu" } else { &self.name };
        format!("{name} (PID {})", self.pid)
    }
}

/// The process listening on `port`, if any.
pub fn listener(port: u16) -> Option<ProcInfo> {
    let pid = crate::stats::listening_pids().get(&port).copied()?;
    let path = image_path(pid).unwrap_or_default();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(ProcInfo { pid, name, path: path.display().to_string() })
}

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Every (pid, parent pid) pair currently running.
fn process_table() -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            out.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// `root` and all its descendants, deepest first (so a parent can't respawn a child we just killed).
fn tree_leaves_first(root: u32, table: &[(u32, u32)]) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(pid, parent) in table {
        if pid != parent {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut order = vec![root];
    let mut i = 0;
    while i < order.len() {
        if let Some(kids) = children.get(&order[i]) {
            for &k in kids {
                if !order.contains(&k) {
                    order.push(k);
                }
            }
        }
        i += 1;
    }
    order.reverse();
    order
}

/// Ancestors of `pid` (parent, grandparent…), to protect the chain App Viewer itself lives in.
fn ancestors(pid: u32, table: &[(u32, u32)]) -> Vec<u32> {
    let parents: HashMap<u32, u32> = table.iter().copied().collect();
    let mut out = Vec::new();
    let mut cur = pid;
    while let Some(&p) = parents.get(&cur) {
        if p == 0 || out.contains(&p) {
            break;
        }
        out.push(p);
        cur = p;
    }
    out
}

/// Why `target` must not be killed, if so. `me` is App Viewer's own pid.
fn refusal(target: &ProcInfo, me: u32, table: &[(u32, u32)], windows_dir: &str) -> Option<String> {
    if target.pid == 0 || target.pid == 4 {
        return Some("process système".into());
    }
    if target.pid == me || tree_leaves_first(me, table).contains(&target.pid) {
        return Some("App Viewer lui-même (ou un de ses sous-process)".into());
    }
    if ancestors(me, table).contains(&target.pid) {
        return Some("le process qui a lancé App Viewer".into());
    }
    let path = target.path.to_lowercase();
    let windows_dir = windows_dir.to_lowercase();
    if !windows_dir.is_empty() && path.starts_with(&windows_dir) {
        return Some("process Windows".into());
    }
    None
}

/// Ends the process tree of `target` after the safety checks. Returns how many processes were ended.
pub fn kill_tree(target: &ProcInfo) -> Result<usize, String> {
    let table = process_table();
    let windows_dir = std::env::var("SystemRoot").unwrap_or_default();
    if let Some(why) = refusal(target, std::process::id(), &table, &windows_dir) {
        return Err(format!("Refusé : {} est {why}.", target.label()));
    }
    terminate_tree(target, &table)
}

fn terminate_tree(target: &ProcInfo, table: &[(u32, u32)]) -> Result<usize, String> {
    let mut killed = 0;
    let mut denied = Vec::new();
    for pid in tree_leaves_first(target.pid, table) {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                denied.push(pid);
                continue;
            }
            if TerminateProcess(h, 1) != 0 {
                killed += 1;
            } else {
                denied.push(pid);
            }
            CloseHandle(h);
        }
    }
    // A parent often exits by itself once its children are gone: only a survivor is a failure.
    if denied.contains(&target.pid) && process_table().iter().any(|&(pid, _)| pid == target.pid) {
        return Err(format!("Impossible d'arrêter {} (accès refusé : process lancé avec plus de droits ?).", target.label()));
    }
    Ok(killed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(pid: u32, path: &str) -> ProcInfo {
        ProcInfo { pid, name: path.rsplit('\\').next().unwrap_or("").into(), path: path.into() }
    }

    // 1 -> 10 (terminal) -> 20 (App Viewer) -> 30 (webview) ; 10 -> 40 (node) -> 50 (node child)
    const TABLE: [(u32, u32); 6] = [(1, 0), (10, 1), (20, 10), (30, 20), (40, 10), (50, 40)];

    #[test]
    fn tree_is_ordered_leaves_first() {
        assert_eq!(tree_leaves_first(40, &TABLE), vec![50, 40]);
        assert_eq!(tree_leaves_first(10, &TABLE).last(), Some(&10));
    }

    #[test]
    fn refuses_system_self_ancestors_and_windows_binaries() {
        let win = "C:\\Windows";
        assert!(refusal(&info(4, ""), 20, &TABLE, win).is_some());
        assert!(refusal(&info(20, "C:\\x\\app-viewer.exe"), 20, &TABLE, win).is_some());
        assert!(refusal(&info(30, "C:\\x\\msedgewebview2.exe"), 20, &TABLE, win).is_some());
        assert!(refusal(&info(10, "C:\\x\\terminal.exe"), 20, &TABLE, win).is_some());
        assert!(refusal(&info(99, "C:\\Windows\\System32\\svchost.exe"), 20, &TABLE, win).is_some());
    }

    #[test]
    fn allows_an_ordinary_dev_server() {
        assert!(refusal(&info(40, "C:\\Program Files\\nodejs\\node.exe"), 20, &TABLE, "C:\\Windows").is_none());
    }

    #[test]
    fn kills_a_real_process_tree() {
        use std::process::{Command, Stdio};
        // cmd spawns a child ping that lasts long enough to be killed
        let mut child = Command::new("cmd.exe")
            .args(["/C", "ping -n 30 127.0.0.1 >nul"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        // Straight to terminate_tree: it is our own child, which kill_tree would rightly refuse.
        let target = ProcInfo { pid: child.id(), name: "cmd.exe".into(), path: String::new() };
        let n = terminate_tree(&target, &process_table()).unwrap();
        assert!(n >= 1);
        assert!(child.wait().is_ok());
    }
}
