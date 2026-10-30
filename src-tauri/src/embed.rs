//! Native apps: their main window is taken into App Viewer's window (made a child of it, frame
//! removed) and laid over the content area, like the webviews of the web apps.

use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
};
use windows_sys::Win32::{
    Foundation::{BOOL, HWND, LPARAM, RECT},
    UI::WindowsAndMessaging::*,
};

/// App id -> its embedded window (as isize: HWND is not Send).
fn embedded() -> &'static Mutex<HashMap<String, isize>> {
    static MAP: OnceLock<Mutex<HashMap<String, isize>>> = OnceLock::new();
    MAP.get_or_init(Mutex::default)
}

pub fn is_embedded(id: &str) -> bool {
    let hwnd = embedded().lock().unwrap().get(id).copied();
    hwnd.is_some_and(|h| unsafe { IsWindow(h as HWND) } != 0)
}

pub fn forget(id: &str) {
    embedded().lock().unwrap().remove(id);
}

/// Shows the app's window over `rect` (physical pixels, client coordinates of `parent`),
/// taking it in first if needed. Returns false while the app has no window yet.
pub fn show(id: &str, pids: &[u32], parent: isize, rect: (i32, i32, i32, i32)) -> bool {
    let mut map = embedded().lock().unwrap();
    let hwnd = match map.get(id).copied().filter(|&h| unsafe { IsWindow(h as HWND) } != 0) {
        Some(h) => h,
        None => {
            let Some(h) = find_main_window(pids) else {
                map.remove(id);
                return false;
            };
            adopt(h, parent);
            map.insert(id.to_string(), h);
            h
        }
    };
    let (x, y, w, h) = rect;
    unsafe {
        SetWindowPos(hwnd as HWND, HWND_TOP, x, y, w, h, SWP_SHOWWINDOW | SWP_FRAMECHANGED | SWP_NOACTIVATE);
    }
    true
}

pub fn hide(id: &str) {
    if let Some(&h) = embedded().lock().unwrap().get(id) {
        unsafe { ShowWindow(h as HWND, SW_HIDE) };
    }
}

/// Every embedded app except those in `keep`, hidden.
pub fn hide_others(keep: Option<&str>) {
    let ids: Vec<String> = embedded().lock().unwrap().keys().filter(|id| Some(id.as_str()) != keep).cloned().collect();
    for id in ids {
        hide(&id);
    }
}

fn adopt(hwnd: isize, parent: isize) {
    let hwnd = hwnd as HWND;
    unsafe {
        if IsZoomed(hwnd) != 0 || IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let style = (style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU)) | WS_CHILD;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !(WS_EX_APPWINDOW | WS_EX_DLGMODALFRAME | WS_EX_WINDOWEDGE)) as isize);
        SetParent(hwnd, parent as HWND);
    }
}

struct Search {
    pids: HashSet<u32>,
    best: Option<(isize, i64)>,
}

/// The largest visible, unowned top-level window of the app's processes (consoles excluded).
fn find_main_window(pids: &[u32]) -> Option<isize> {
    let mut search = Search { pids: pids.iter().copied().collect(), best: None };
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    search.best.map(|(h, _)| h)
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam as *mut Search);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if !search.pids.contains(&pid) || IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
        return 1;
    }
    let mut class = [0u16; 64];
    let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) as usize;
    if String::from_utf16_lossy(&class[..n]) == "ConsoleWindowClass" {
        return 1;
    }
    if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut r);
    let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
    if area > 100 * 100 && search.best.map_or(true, |(_, a)| area > a) {
        search.best = Some((hwnd as isize, area));
    }
    1
}
