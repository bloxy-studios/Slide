use serde::Serialize;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as native;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as native;

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeInfo {
    pub path: PathBuf,
    pub brand: String,
    pub version: String,
    pub architecture: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pid: u32,
    pub started: u64,
}

pub struct Session {
    pub identity: Identity,
    pub executable: PathBuf,
    pub window: bool,
}

pub enum Observation {
    Session(Session),
    Absent,
    Locked(String),
}

pub struct Chromium {
    processes: System,
    windows: Vec<u32>,
}

impl Chromium {
    pub const BACKGROUND_CLOSE_HINT: Option<&'static str> = native::BACKGROUND_CLOSE_HINT;

    pub fn new() -> Self {
        Self {
            processes: System::new(),
            windows: Vec::new(),
        }
    }

    pub fn discover() -> Result<RuntimeInfo, String> {
        native::candidates().into_iter().find_map(|path| Self::inspect(&path).ok()).ok_or_else(|| {
            "No compatible Google Chrome or Chromium installation was found. Install one and choose its application or executable below.".into()
        })
    }

    pub fn inspect(path: &Path) -> Result<RuntimeInfo, String> {
        native::inspect(path).map_err(|step| {
            format!(
                "Cannot use this browser: {step} Choose an installed Google Chrome or Chromium."
            )
        })
    }

    pub fn refresh(&mut self) {
        self.processes.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::OnlyIfNotSet),
        );
        self.windows = native::browser_windows();
    }

    pub fn observe(&self, directory: &Path) -> Observation {
        let mut candidates = self
            .processes
            .processes()
            .values()
            .filter(|process| has_profile(process.cmd(), directory));
        if let Some(process) = candidates.next() {
            if candidates.next().is_some() {
                return Observation::Locked("More than one browser claims this directory. Close the conflicting windows and retry.".into());
            }
            if let Some(session) = self.session(process) {
                return Observation::Session(session);
            }
            return Observation::Locked("The owner of this profile could not be verified. Close its browser manually and retry.".into());
        }
        if let Some(reason) = native::lock_conflict(directory, &self.processes) {
            return Observation::Locked(reason);
        }
        Observation::Absent
    }

    fn session(&self, process: &Process) -> Option<Session> {
        let pid = process.pid().as_u32();
        let executable = process.exe()?.to_path_buf();
        let started = native::birth(pid)?;
        if !native::is_browser_executable(&executable) {
            return None;
        }
        Some(Session {
            identity: Identity { pid, started },
            executable,
            window: self.windows.contains(&pid),
        })
    }

    pub fn launch(
        executable: &Path,
        directory: &Path,
        url: Option<&str>,
        reopen: bool,
    ) -> Result<Child, String> {
        let mut profile = OsString::from("--user-data-dir=");
        profile.push(directory);
        let mut command = Command::new(executable);
        command
            .arg(profile)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if reopen {
            command
                .arg("--new-window")
                .arg(url.unwrap_or("chrome://newtab/"));
        } else if let Some(url) = url {
            command.arg(url);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
        }
        command.spawn().map_err(|error| {
            format!(
                "Could not launch the browser: {error}. Check the selected runtime and try again."
            )
        })
    }

    pub fn focus(&mut self, directory: &Path, expected: Identity) -> Result<(), String> {
        self.verified(directory, expected)?;
        native::focus(expected)
    }

    pub fn close(&mut self, directory: &Path, expected: Identity) -> Result<(), String> {
        self.verified(directory, expected)?;
        native::close(expected)
    }

    pub fn verified(&mut self, directory: &Path, expected: Identity) -> Result<(), String> {
        self.processes.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[Pid::from_u32(expected.pid)]),
            true,
            ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::Always)
                .with_exe(UpdateKind::Always),
        );
        match self.processes.process(Pid::from_u32(expected.pid)) {
            Some(process) if matches_identity(expected, self.session(process).map(|session| session.identity), process.cmd(), directory) => Ok(()),
            _ => Err("The browser process changed before the action. Wait for the status to refresh and retry.".into()),
        }
    }
}

fn matches_identity(
    expected: Identity,
    actual: Option<Identity>,
    args: &[OsString],
    directory: &Path,
) -> bool {
    actual == Some(expected) && has_profile(args, directory)
}

fn has_profile(args: &[OsString], directory: &Path) -> bool {
    let mut args = args.iter().skip(1);
    let mut found = None;
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let Some(arg) = arg.to_str() else {
            continue;
        };
        if arg == "--user-data-dir" {
            let Some(path) = args.next() else {
                return false;
            };
            if found.replace(path.as_os_str()).is_some() {
                return false;
            }
        } else if let Some(path) = arg.strip_prefix("--user-data-dir=") {
            if found.replace(OsStr::new(path)).is_some() {
                return false;
            }
        } else if arg == "--type" || arg.starts_with("--type=") {
            return false;
        }
    }
    found.is_some_and(|path| Path::new(path) == directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_pid_and_unrelated_profile_fail_identity_verification() {
        let expected = Identity { pid: 5, started: 1 };
        let recycled = Identity { pid: 5, started: 2 };
        let args = ["Chrome".into(), "--user-data-dir=/profiles/1".into()];
        assert!(matches_identity(
            expected,
            Some(expected),
            &args,
            Path::new("/profiles/1")
        ));
        assert!(!matches_identity(
            expected,
            Some(recycled),
            &args,
            Path::new("/profiles/1")
        ));
        assert!(!matches_identity(
            expected,
            Some(expected),
            &args,
            Path::new("/profiles/2")
        ));
        assert!(!matches_identity(
            expected,
            None,
            &args,
            Path::new("/profiles/1")
        ));
    }

    #[test]
    fn exact_directory_excludes_normal_chrome_helpers_and_ambiguous_switches() {
        let directory = Path::new("/profiles/1 space");
        let argv = |items: &[&str]| items.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(has_profile(
            &argv(&["Chrome", "--user-data-dir=/profiles/1 space"]),
            directory
        ));
        assert!(has_profile(
            &argv(&["Chrome", "--user-data-dir", "/profiles/1 space"]),
            directory
        ));
        for arguments in [
            &["Chrome", "--user-data-dir=/profiles/1 spaceX"][..],
            &["Chrome", "--user-data-dir=/profiles/2"],
            &["Chrome"],
            &[
                "Chrome",
                "--user-data-dir=/profiles/1 space",
                "--type=renderer",
            ],
            &["Chrome", "--", "--user-data-dir=/profiles/1 space"],
            &[
                "Chrome",
                "--user-data-dir=/profiles/1 space",
                "--user-data-dir=/elsewhere",
            ],
        ] {
            assert!(!has_profile(&argv(arguments), directory));
        }
    }
}
