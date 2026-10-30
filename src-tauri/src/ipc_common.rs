//! Shared by App Viewer and `appv` (included in both with `#[path]`): where the control pipe lives.
//!
//! The CLI talks to the running GUI through a named pipe, not a localhost port: the hosted apps
//! are remote origins that could `fetch` a local HTTP port, but they cannot reach a pipe.
//! The pipe is named after the user's SID and only that user may open it.

use std::{mem, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree},
    Security::{
        Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

/// Requests and responses are one JSON line each, at most this long.
pub const MAX_MESSAGE: usize = 256 * 1024;

/// String form of the current user's SID ("S-1-5-21-…").
pub fn user_sid() -> Result<String, String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err("jeton du process illisible".into());
        }
        let result = (|| {
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
            if len == 0 {
                return Err("SID de l'utilisateur illisible".to_string());
            }
            // u64 storage keeps the TOKEN_USER suitably aligned.
            let mut buf = vec![0u64; (len as usize).div_ceil(mem::size_of::<u64>())];
            if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
                return Err("SID de l'utilisateur illisible".to_string());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut wide: *mut u16 = ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 || wide.is_null() {
                return Err("SID de l'utilisateur illisible".to_string());
            }
            let mut n = 0;
            while *wide.add(n) != 0 {
                n += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, n));
            LocalFree(wide.cast());
            Ok(sid)
        })();
        CloseHandle(token);
        result
    }
}

pub fn pipe_path() -> Result<String, String> {
    Ok(format!(r"\\.\pipe\app-viewer-{}", user_sid()?))
}
