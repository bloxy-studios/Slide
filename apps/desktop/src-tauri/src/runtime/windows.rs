use super::{Identity, RuntimeInfo};
use std::{
    ffi::{c_void, OsStr},
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use sysinfo::System;
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HWND, LPARAM},
    Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    },
    System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        PostMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE, WM_CLOSE,
    },
};

const NO_WINDOW: &str = "Open this workspace first to close it from the manager.";
pub const BACKGROUND_CLOSE_HINT: Option<&str> = Some(NO_WINDOW);

pub fn candidates() -> Vec<PathBuf> {
    [
        "ProgramW6432",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .flat_map(|root| {
        let root = PathBuf::from(root);
        [
            "Google/Chrome/Application/chrome.exe",
            "Chromium/Application/chrome.exe",
            "Chromium/Application/chromium.exe",
        ]
        .map(|relative| root.join(relative))
    })
    .collect()
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain([0]).collect()
}

pub fn inspect(selected: &Path) -> Result<RuntimeInfo, String> {
    let path = dunce::canonicalize(selected).map_err(|_| "The selected path does not exist.")?;
    if !is_browser_executable(&path) {
        return Err("Select chrome.exe or chromium.exe.".into());
    }
    let architecture =
        executable_architecture(&path).ok_or("Unrecognised browser executable architecture.")?;
    if !matches!(architecture, "x86_64" | "x86") {
        return Err("This manager build requires a browser compatible with Windows x64.".into());
    }
    let (brand, version) = executable_version(&path)
        .ok_or("The selected file does not identify itself as Google Chrome or Chromium.")?;
    Ok(RuntimeInfo {
        path,
        brand,
        version,
        architecture: architecture.into(),
    })
}

fn executable_architecture(path: &Path) -> Option<&'static str> {
    let mut file = File::open(path).ok()?;
    let mut header = [0; 64];
    file.read_exact(&mut header).ok()?;
    if header[..2] != *b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(header[60..64].try_into().ok()?);
    file.seek(SeekFrom::Start(pe.into())).ok()?;
    let mut signature = [0; 6];
    file.read_exact(&mut signature).ok()?;
    if signature[..4] != *b"PE\0\0" {
        return None;
    }
    match u16::from_le_bytes(signature[4..6].try_into().ok()?) {
        0x8664 => Some("x86_64"),
        0x014c => Some("x86"),
        0xaa64 => Some("arm64"),
        _ => None,
    }
}

fn executable_version(path: &Path) -> Option<(String, String)> {
    let path = wide(path.as_os_str());
    unsafe {
        let mut ignored_handle = 0;
        let size = GetFileVersionInfoSizeW(path.as_ptr(), &mut ignored_handle);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u32; (size as usize).div_ceil(4)];
        if GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let query = |name: &str| {
            let name = wide(OsStr::new(name));
            let mut buffer: *mut c_void = null_mut();
            let mut length = 0;
            (VerQueryValueW(
                data.as_ptr().cast(),
                name.as_ptr(),
                &mut buffer,
                &mut length,
            ) != 0
                && !buffer.is_null())
            .then_some((buffer, length as usize))
        };
        let (fixed, size) = query("\\")?;
        if size < size_of::<VS_FIXEDFILEINFO>() {
            return None;
        }
        let fixed = fixed.cast::<VS_FIXEDFILEINFO>().read_unaligned();
        let (translation, size) = query("\\VarFileInfo\\Translation")?;
        let translations = std::slice::from_raw_parts(translation.cast::<u16>(), size / 2);
        let brand = translations.chunks_exact(2).find_map(|pair| {
            let (value, length) = query(&format!(
                "\\StringFileInfo\\{:04x}{:04x}\\ProductName",
                pair[0], pair[1]
            ))?;
            let value =
                String::from_utf16_lossy(std::slice::from_raw_parts(value.cast::<u16>(), length));
            let value = value.trim_end_matches('\0');
            matches!(value, "Google Chrome" | "Chromium").then(|| value.to_owned())
        })?;
        let version = format!(
            "{}.{}.{}.{}",
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xffff,
            fixed.dwFileVersionLS >> 16,
            fixed.dwFileVersionLS & 0xffff
        );
        Some((brand, version))
    }
}

pub fn is_browser_executable(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("chrome.exe") || name.eq_ignore_ascii_case("chromium.exe")
        })
}

pub fn birth(pid: u32) -> Option<u64> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let valid = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        (valid && exited.dwHighDateTime == 0 && exited.dwLowDateTime == 0)
            .then_some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

pub fn lock_conflict(directory: &Path, _processes: &System) -> Option<String> {
    if directory.join("SingletonLock").symlink_metadata().is_ok() {
        Some("This profile has a singleton lock from another browser or platform. Close the other browser and retry.".into())
    } else {
        None
    }
}

fn windows() -> Vec<(u32, HWND)> {
    unsafe extern "system" fn collect(window: HWND, context: LPARAM) -> i32 {
        let mut class = [0; 64];
        let count = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        if count > 0
            && IsWindowVisible(window) != 0
            && String::from_utf16_lossy(&class[..count as usize]) == "Chrome_WidgetWin_1"
        {
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            (*(context as *mut Vec<(u32, HWND)>)).push((pid, window));
        }
        1
    }
    let mut results = Vec::new();
    unsafe {
        EnumWindows(Some(collect), &mut results as *mut _ as LPARAM);
    }
    results
}

pub fn browser_windows() -> Vec<u32> {
    windows().into_iter().map(|(pid, _)| pid).collect()
}

fn verify_window(window: HWND, identity: Identity) -> bool {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
    }
    pid == identity.pid && birth(pid) == Some(identity.started)
}

pub fn focus(identity: Identity) -> Result<(), String> {
    let window = windows()
        .into_iter()
        .find(|(pid, _)| *pid == identity.pid)
        .map(|(_, window)| window);
    let window = window
        .filter(|window| verify_window(*window, identity))
        .ok_or("The browser window changed. Try again.")?;
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        if SetForegroundWindow(window) != 0 {
            return Ok(());
        }
    }
    Err(
        "Windows did not allow foreground activation. Select this browser window from the taskbar."
            .into(),
    )
}

pub fn close(identity: Identity) -> Result<(), String> {
    let mut requested = false;
    for (pid, window) in windows() {
        if pid == identity.pid && verify_window(window, identity) {
            if unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } == 0 {
                return Err("Windows declined the close request. Close that browser window manually or try again.".into());
            }
            requested = true;
        }
    }
    if requested {
        Ok(())
    } else {
        Err(NO_WINDOW.into())
    }
}
