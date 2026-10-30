//! CPU / RAM of the running apps, sampled every 1.5 s and pushed to the UI ("app-stats").
//! Apps we launched are measured as a whole process tree, through their job. External apps
//! have no job: we measure the process listening on their port (its children are unknown).

use crate::{
    job::Job,
    state::{AppState, Status},
};
use serde::Serialize;
use std::{
    collections::HashMap,
    mem, ptr,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    },
    System::{
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ},
    },
};

const INTERVAL: Duration = Duration::from_millis(1500);
const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStats {
    /// Percent of the whole machine (all logical cores = 100), like the Task Manager.
    /// None on the first sample of a run (no previous point to diff against).
    pub cpu: Option<f32>,
    /// Working set, in bytes, summed over the processes.
    pub memory: u64,
    pub processes: u32,
    /// "tree": the whole tree launched by App Viewer; "port": only the process on the port.
    pub scope: &'static str,
}

enum Source {
    Job { job: Arc<Job>, generation: u64 },
    Port(u16),
}

/// What a CPU time sample belongs to: a diff is only meaningful within the same run / process.
#[derive(PartialEq, Clone, Copy)]
enum Key {
    Run(u64),
    Pid(u32),
}

pub fn watch(app: AppHandle) {
    thread::spawn(move || {
        let cores = thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let mut prev: HashMap<String, (Key, u64, Instant)> = HashMap::new();
        loop {
            thread::sleep(INTERVAL);
            // Nobody is looking while the window sits in the tray.
            if app.get_window("main").and_then(|w| w.is_visible().ok()) == Some(false) {
                prev.clear();
                continue;
            }
            let st = app.state::<AppState>();
            let pending: Vec<(String, Option<(Arc<Job>, u64)>)> = st
                .runtimes
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(id, rt)| match rt.status {
                    Status::Starting | Status::Running => rt.job.clone().map(|j| (id.clone(), Some((j, rt.generation)))),
                    Status::External => Some((id.clone(), None)),
                    _ => None,
                })
                .collect();
            if pending.is_empty() && prev.is_empty() {
                continue;
            }
            let sources: Vec<(String, Source)> = pending
                .into_iter()
                .filter_map(|(id, job)| match job {
                    Some((job, generation)) => Some((id, Source::Job { job, generation })),
                    None => st.app(&id).map(|cfg| (id, Source::Port(cfg.port))),
                })
                .collect();
            let listeners = if sources.iter().any(|(_, s)| matches!(s, Source::Port(_))) {
                listening_pids()
            } else {
                HashMap::new()
            };

            let mut next = HashMap::new();
            let mut out: HashMap<String, AppStats> = HashMap::new();
            for (id, source) in sources {
                let (key, cpu_time, pids, scope) = match source {
                    Source::Job { job, generation } => (Key::Run(generation), job.cpu_time(), job.pids(), "tree"),
                    Source::Port(port) => {
                        let Some(&pid) = listeners.get(&port) else { continue };
                        let Some(t) = process_cpu_time(pid) else { continue };
                        (Key::Pid(pid), t, vec![pid], "port")
                    }
                };
                let now = Instant::now();
                let cpu = prev.get(&id).filter(|(k, _, _)| *k == key).map(|(_, t0, at)| {
                    let secs = now.duration_since(*at).as_secs_f64().max(0.001);
                    let used = cpu_time.saturating_sub(*t0) as f64 / 1e7; // 100 ns -> s
                    (used / secs / cores * 100.0).clamp(0.0, 100.0) as f32
                });
                next.insert(id.clone(), (key, cpu_time, now));
                let memory = pids.iter().filter_map(|&pid| working_set(pid)).sum();
                out.insert(id, AppStats { cpu, memory, processes: pids.len() as u32, scope });
            }
            prev = next;
            let _ = app.emit("app-stats", &out);
        }
    });
}

struct Process(HANDLE);

impl Process {
    fn open(pid: u32, access: u32) -> Option<Self> {
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(Process(h))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn working_set(pid: u32) -> Option<u64> {
    let p = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ)
        .or_else(|| Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION))?;
    unsafe {
        let mut mc: PROCESS_MEMORY_COUNTERS = mem::zeroed();
        mc.cb = mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        (K32GetProcessMemoryInfo(p.0, &mut mc, mc.cb) != 0).then_some(mc.WorkingSetSize as u64)
    }
}

fn process_cpu_time(pid: u32) -> Option<u64> {
    let p = Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    unsafe {
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
            (mem::zeroed(), mem::zeroed(), mem::zeroed(), mem::zeroed());
        (GetProcessTimes(p.0, &mut c, &mut e, &mut k, &mut u) != 0).then(|| ft(k) + ft(u))
    }
}

/// Listening TCP port -> owning process id (IPv4 and IPv6).
pub fn listening_pids() -> HashMap<u16, u32> {
    let mut map = HashMap::new();
    for (port, pid) in tcp_listeners::<MIB_TCPROW_OWNER_PID>(AF_INET, |r| (r.dwLocalPort, r.dwOwningPid))
        .into_iter()
        .chain(tcp_listeners::<MIB_TCP6ROW_OWNER_PID>(AF_INET6, |r| (r.dwLocalPort, r.dwOwningPid)))
    {
        // The port is stored in network byte order in the low 16 bits.
        map.entry(u16::from_be(port as u16)).or_insert(pid);
    }
    map
}

/// Reads a MIB_TCP(6)TABLE_OWNER_PID: a u32 row count followed by the rows.
fn tcp_listeners<Row>(family: u32, fields: impl Fn(&Row) -> (u32, u32)) -> Vec<(u32, u32)> {
    let mut size = 0u32;
    for _ in 0..3 {
        // u64 storage keeps the rows suitably aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let res = unsafe {
            GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0)
        };
        if res == 0 {
            let base = buf.as_ptr() as *const u8;
            let count = unsafe { ptr::read(base as *const u32) } as usize;
            let rows = unsafe { base.add(mem::align_of::<Row>().max(4)) } as *const Row;
            return (0..count).map(|i| fields(unsafe { &*rows.add(i) })).collect();
        }
        // ERROR_INSUFFICIENT_BUFFER: `size` now holds the needed size, try again.
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn finds_the_process_listening_on_a_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(listening_pids().get(&port), Some(&std::process::id()));
    }

    #[test]
    fn measures_this_process() {
        let pid = std::process::id();
        assert!(working_set(pid).unwrap() > 1024 * 1024);
        assert!(process_cpu_time(pid).is_some());
    }
}
