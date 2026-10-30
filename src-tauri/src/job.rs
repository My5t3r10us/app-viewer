//! Windows Job Object wrapper: every launched app lives in its own job, so stopping it
//! kills the whole process tree (cmd.exe -> node -> ...), and if App Viewer dies the OS
//! closes the job handle and kills the children too (no orphaned servers holding ports).
//!
//! No BREAKAWAY_OK / SILENT_BREAKAWAY_OK limit is ever set: a process in the job can't
//! spawn children outside of it, even with CREATE_BREAKAWAY_FROM_JOB.

use std::{
    ffi::c_void,
    io, mem,
    os::windows::io::AsRawHandle,
    process::Child,
    ptr, thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    },
};

/// Pass to `creation_flags` so the child can't run (and spawn anything) before it's in the job.
pub const CREATE_SUSPENDED: u32 = 0x0000_0004;

pub struct Job(HANDLE);

// The handle is only used through thread-safe Win32 calls.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn new() -> io::Result<Self> {
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Job(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                mem::size_of_val(&info) as u32,
            );
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }

    fn accounting(&self) -> Option<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = mem::zeroed();
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut c_void,
                mem::size_of_val(&info) as u32,
                ptr::null_mut(),
            );
            (ok != 0).then_some(info)
        }
    }

    /// Number of processes still alive in the job (0 if it can't be queried).
    pub fn active_processes(&self) -> u32 {
        self.accounting().map(|i| i.ActiveProcesses).unwrap_or(0)
    }

    /// CPU time (user + kernel, in 100 ns units) used by every process that ran in the job.
    pub fn cpu_time(&self) -> u64 {
        self.accounting().map(|i| (i.TotalUserTime + i.TotalKernelTime).max(0) as u64).unwrap_or(0)
    }

    /// Ids of the processes currently in the job.
    pub fn pids(&self) -> Vec<u32> {
        #[repr(C)]
        struct IdList {
            assigned: u32,
            in_list: u32,
            ids: [usize; 1024],
        }
        unsafe {
            let mut list: Box<IdList> = Box::new(mem::zeroed());
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicProcessIdList,
                &mut *list as *mut _ as *mut c_void,
                mem::size_of::<IdList>() as u32,
                ptr::null_mut(),
            );
            if ok == 0 {
                return Vec::new();
            }
            let n = (list.in_list as usize).min(list.ids.len());
            list.ids[..n].iter().map(|&id| id as u32).collect()
        }
    }

    /// Waits until the job is empty or the deadline passes; true if it emptied.
    pub fn wait_empty(&self, deadline: Instant) -> bool {
        loop {
            if self.active_processes() == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Resumes the threads of a child spawned with CREATE_SUSPENDED (std doesn't expose the
/// primary thread handle, so they are found through a thread snapshot).
pub fn resume(child: &Child) -> io::Result<()> {
    let pid = child.id();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = mem::zeroed();
        entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    if ResumeThread(thread) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        if resumed == 0 {
            Err(io::Error::new(io::ErrorKind::Other, "aucun thread à reprendre"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::windows::process::CommandExt, process::Command};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// cmd.exe -> ping.exe: the grandchild must be in the job and die with it.
    #[test]
    fn suspended_launch_keeps_the_whole_tree_in_the_job() {
        let job = Job::new().unwrap();
        let mut child = Command::new("cmd.exe")
            .args(["/D", "/C", "ping -n 30 127.0.0.1 >nul"])
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn()
            .unwrap();
        job.assign(&child).unwrap();
        assert_eq!(job.active_processes(), 1, "only the suspended cmd.exe yet");
        resume(&child).unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while job.active_processes() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(job.active_processes() >= 2, "ping.exe should have been spawned inside the job");
        let pids = job.pids();
        assert!(pids.contains(&child.id()) && pids.len() >= 2, "pids: {pids:?}");

        job.terminate();
        assert!(job.wait_empty(Instant::now() + Duration::from_secs(3)));
        assert_eq!(child.wait().unwrap().code(), Some(1), "killed by TerminateJobObject(1)");
    }

    /// Dropping the job (what happens if App Viewer dies) kills what's inside.
    #[test]
    fn closing_the_job_kills_its_processes() {
        let job = Job::new().unwrap();
        let mut child = Command::new("cmd.exe")
            .args(["/D", "/C", "ping -n 30 127.0.0.1 >nul"])
            .creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED)
            .spawn()
            .unwrap();
        job.assign(&child).unwrap();
        resume(&child).unwrap();
        drop(job);
        let deadline = Instant::now() + Duration::from_secs(3);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "cmd.exe survived the job being closed");
            thread::sleep(Duration::from_millis(50));
        }
    }
}
