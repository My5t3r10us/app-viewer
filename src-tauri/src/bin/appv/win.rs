//! Windows bits of the CLI: is the GUI running, and the user PATH (set by the installer).

use std::{ffi::OsString, mem, os::windows::ffi::OsStringExt, path::{Path, PathBuf}, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
        Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION},
        Registry::{RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_EXPAND_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ},
    },
    UI::WindowsAndMessaging::{AllowSetForegroundWindow, SendMessageTimeoutW, ASFW_ANY, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Whether a process with this exe name (e.g. "app-viewer.exe") is running.
pub fn process_running(exe: &str) -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            if OsString::from_wide(&entry.szExeFile[..len]).to_string_lossy().eq_ignore_ascii_case(exe) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// Full path of the executable of a running process.
pub fn process_image(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

/// Lets the process we launch next bring its window to the front.
pub fn allow_foreground() {
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

const ENV_KEY: &str = "Environment";

fn read_user_path() -> Result<(String, REG_VALUE_TYPE), String> {
    let (key, name) = (wide(ENV_KEY), wide("Path"));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    unsafe {
        let mut kind: REG_VALUE_TYPE = 0;
        let mut size: u32 = 0;
        let err = RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), flags, &mut kind, ptr::null_mut(), &mut size);
        if err == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        if err != ERROR_SUCCESS {
            return Err(format!("lecture du PATH impossible (erreur {err})"));
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        let mut size = (buf.len() * 2) as u32;
        let err = RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), flags, &mut kind, buf.as_mut_ptr().cast(), &mut size);
        if err != ERROR_SUCCESS {
            return Err(format!("lecture du PATH impossible (erreur {err})"));
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok((String::from_utf16_lossy(&buf[..len]), kind))
    }
}

fn write_user_path(value: &str, kind: REG_VALUE_TYPE) -> Result<(), String> {
    let (key, name, data) = (wide(ENV_KEY), wide("Path"), wide(value));
    unsafe {
        let err = RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), kind, data.as_ptr().cast(), (data.len() * 2) as u32);
        if err != ERROR_SUCCESS {
            return Err(format!("écriture du PATH impossible (erreur {err})"));
        }
        // Tell Explorer (and so newly opened terminals) that the environment changed.
        let env = wide(ENV_KEY);
        SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, env.as_ptr() as isize, SMTO_ABORTIFHUNG, 3000, ptr::null_mut());
    }
    Ok(())
}

fn same_dir(entry: &str, dir: &Path) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
    norm(entry) == norm(&dir.to_string_lossy())
}

/// Adds `dir` to the user PATH if it isn't there yet. Returns whether it changed anything.
pub fn add_to_user_path(dir: &Path) -> Result<bool, String> {
    let (path, kind) = read_user_path()?;
    if path.split(';').any(|e| same_dir(e, dir)) {
        return Ok(false);
    }
    let dir = dir.to_string_lossy();
    // Keep a trailing ';' where it was, so uninstalling gives back the exact same value.
    let value = match path.as_str() {
        "" => dir.to_string(),
        p if p.ends_with(';') => format!("{p}{dir};"),
        p => format!("{p};{dir}"),
    };
    write_user_path(&value, kind)?;
    Ok(true)
}

/// Removes `dir` from the user PATH. Returns whether it was there.
pub fn remove_from_user_path(dir: &Path) -> Result<bool, String> {
    let (path, kind) = read_user_path()?;
    if !path.split(';').any(|e| same_dir(e, dir)) {
        return Ok(false);
    }
    // Only our entry goes: the rest of the value is left exactly as it was.
    let kept: Vec<&str> = path.split(';').filter(|e| !same_dir(e, dir)).collect();
    write_user_path(&kept.join(";"), kind)?;
    Ok(true)
}
