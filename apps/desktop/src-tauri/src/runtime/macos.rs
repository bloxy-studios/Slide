use super::{Identity, RuntimeInfo};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use std::{
    ffi::{c_void, CStr},
    fs::{self, File},
    io::Read,
    mem::{size_of, MaybeUninit},
    path::{Path, PathBuf},
};
use sysinfo::{Pid, System};

pub fn candidates() -> Vec<PathBuf> {
    let mut locations = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        locations.push(PathBuf::from(home).join("Applications"));
    }
    locations
        .iter()
        .flat_map(|root| ["Google Chrome.app", "Chromium.app"].map(|name| root.join(name)))
        .collect()
}

pub fn inspect(selected: &Path) -> Result<RuntimeInfo, String> {
    let path = dunce::canonicalize(selected).map_err(|_| "The selected path does not exist.")?;
    let bundle = path
        .ancestors()
        .find(|part| part.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Select a browser application bundle.")?;
    let plist = plist::Value::from_file(bundle.join("Contents/Info.plist"))
        .map_err(|_| "The application's Info.plist could not be read.")?;
    let value = |key| plist.as_dictionary()?.get(key)?.as_string();
    let (brand, expected) = match value("CFBundleIdentifier") {
        Some("com.google.Chrome") => ("Google Chrome", "Google Chrome"),
        Some("org.chromium.Chromium") => ("Chromium", "Chromium"),
        _ => return Err("The selected application is not a supported browser.".into()),
    };
    if value("CFBundleExecutable") != Some(expected) {
        return Err("The browser's bundle executable is not recognised.".into());
    }
    let executable = bundle.join("Contents/MacOS").join(expected);
    if path != bundle && path != executable {
        return Err("Select the browser bundle or its main executable.".into());
    }
    let architecture =
        architectures(&executable).ok_or("The browser executable is missing or unreadable.")?;
    if !architecture.contains(if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    }) {
        return Err("The browser has no native executable slice for this Mac.".into());
    }
    let version = value("CFBundleShortVersionString")
        .ok_or("The browser version is missing.")?
        .to_owned();
    Ok(RuntimeInfo {
        path: executable,
        brand: brand.into(),
        version,
        architecture,
    })
}

fn architectures(path: &Path) -> Option<String> {
    let mut header = [0u8; 4096];
    let count = File::open(path).ok()?.read(&mut header).ok()?;
    let bytes = &header[..count];
    let read = |offset, little| {
        let word: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(word)
        } else {
            u32::from_be_bytes(word)
        })
    };
    let mut architectures = Vec::new();
    let mut add = |cpu| match cpu {
        Some(0x0100000c) => architectures.push("arm64"),
        Some(0x01000007) => architectures.push("x86_64"),
        _ => {}
    };
    match read(0, false)? {
        0xcafebabe | 0xcafebabf => {
            let stride = if read(0, false)? == 0xcafebabe {
                20
            } else {
                32
            };
            for index in 0..usize::try_from(read(4, false)?).ok()?.min(64) {
                add(read(8 + index * stride, false));
            }
        }
        0xcffaedfe => add(read(4, true)),
        0xfeedfacf => add(read(4, false)),
        _ => return None,
    }
    Some(architectures.join(" + "))
}

pub fn is_browser_executable(path: &Path) -> bool {
    let stem = path.file_name().and_then(|name| name.to_str());
    matches!(stem, Some("Google Chrome" | "Chromium"))
        && path
            .parent()
            .is_some_and(|parent| parent.ends_with("Contents/MacOS"))
}

pub fn birth(pid: u32) -> Option<u64> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = size_of::<libc::proc_bsdinfo>() as i32;
    unsafe {
        if libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        ) != size
        {
            return None;
        }
        let info = info.assume_init();
        if info.pbi_uid != libc::geteuid() || info.pbi_status == 5 {
            return None;
        }
        Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
}

pub fn lock_conflict(directory: &Path, processes: &System) -> Option<String> {
    let lock = directory.join("SingletonLock");
    let detail = "Chrome's profile lock has an unverified owner. Close any browser using this workspace and retry.";
    if let Err(error) = lock.symlink_metadata() {
        return (error.kind() != std::io::ErrorKind::NotFound).then(|| detail.into());
    }
    let Some(target) = fs::read_link(lock)
        .ok()
        .and_then(|link| link.to_str().map(str::to_owned))
    else {
        return Some(detail.into());
    };
    let Some((host, pid)) = target
        .rsplit_once('-')
        .and_then(|(host, pid)| Some((host, pid.parse::<u32>().ok()?)))
    else {
        return Some(detail.into());
    };
    let mut hostname = [0; 256];
    let local = unsafe {
        libc::gethostname(hostname.as_mut_ptr(), hostname.len() - 1) == 0
            && CStr::from_ptr(hostname.as_ptr()).to_bytes() == host.as_bytes()
    };
    if !local {
        return Some(detail.into());
    }
    // Chromium owns the singleton files. Let it recover an abandoned local lock.
    match processes.process(Pid::from_u32(pid)) {
        None => None,
        Some(process) if process.exe().is_some_and(|exe| !is_browser_executable(exe)) => None,
        Some(_) => Some(detail.into()),
    }
}

pub fn focus(identity: Identity) -> Result<(), String> {
    let application =
        NSRunningApplication::runningApplicationWithProcessIdentifier(identity.pid as i32)
            .ok_or("The browser window is not available to activate yet.")?;
    if birth(identity.pid) != Some(identity.started) {
        return Err("The browser changed before it could be focused.".into());
    }
    if application.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows) {
        Ok(())
    } else {
        Err(
            "macOS did not activate the browser. Select its window in the Dock or try again."
                .into(),
        )
    }
}

pub fn close(identity: Identity) -> Result<(), String> {
    let application =
        NSRunningApplication::runningApplicationWithProcessIdentifier(identity.pid as i32)
            .ok_or("The browser process is no longer available to close.")?;
    if birth(identity.pid) != Some(identity.started) {
        return Err("The browser changed before it could be closed.".into());
    }
    if application.terminate() {
        Ok(())
    } else {
        Err(
            "Chrome did not accept the quit request. Quit that browser window manually or retry."
                .into(),
        )
    }
}

type Cf = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(options: u32, relative_to_window: u32) -> Cf;
    static kCGWindowOwnerPID: Cf;
    static kCGWindowLayer: Cf;
    static kCGWindowBounds: Cf;
    fn CGRectMakeWithDictionaryRepresentation(dictionary: Cf, rect: *mut Rect) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: Cf) -> isize;
    fn CFArrayGetValueAtIndex(array: Cf, index: isize) -> Cf;
    fn CFDictionaryGetValue(dictionary: Cf, key: Cf) -> Cf;
    fn CFNumberGetValue(number: Cf, kind: isize, value: *mut i32) -> bool;
    fn CFRelease(value: Cf);
}

#[repr(C)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub fn browser_windows() -> Vec<u32> {
    unsafe {
        let list = CGWindowListCopyWindowInfo(1 << 4, 0); // exclude desktop elements; include other Spaces
        if list.is_null() {
            return Vec::new();
        }
        let mut pids = Vec::new();
        for index in 0..CFArrayGetCount(list) {
            let window = CFArrayGetValueAtIndex(list, index);
            let number = |key| {
                let mut value = 0i32;
                let ptr = CFDictionaryGetValue(window, key);
                if !ptr.is_null() && CFNumberGetValue(ptr, 3, &mut value) {
                    Some(value)
                } else {
                    None
                }
            };
            if number(kCGWindowLayer) != Some(0) {
                continue;
            }
            let Some(pid) = number(kCGWindowOwnerPID) else {
                continue;
            };
            let mut rect = MaybeUninit::uninit();
            let bounds = CFDictionaryGetValue(window, kCGWindowBounds);
            if !bounds.is_null()
                && CGRectMakeWithDictionaryRepresentation(bounds, rect.as_mut_ptr())
            {
                let rect = rect.assume_init();
                if rect.width > 100.0 && rect.height > 100.0 {
                    pids.push(pid as u32);
                }
            }
        }
        CFRelease(list);
        pids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_local_lock_is_left_for_chromium_but_foreign_lock_blocks_launch() {
        let test = tempfile::tempdir().unwrap();
        let lock = test.path().join("SingletonLock");
        let mut host = [0; 256];
        assert_eq!(
            unsafe { libc::gethostname(host.as_mut_ptr(), host.len() - 1) },
            0
        );
        let host = unsafe { CStr::from_ptr(host.as_ptr()) }.to_str().unwrap();
        std::os::unix::fs::symlink(format!("{host}-2147483647"), &lock).unwrap();
        let processes = System::new();
        assert_eq!(lock_conflict(test.path(), &processes), None);
        assert!(lock.symlink_metadata().is_ok());
        fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink("another-runner.invalid-1", &lock).unwrap();
        assert!(lock_conflict(test.path(), &processes).is_some());
    }
}
