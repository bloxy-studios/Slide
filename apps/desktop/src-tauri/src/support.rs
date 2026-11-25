use crate::{runtime::RuntimeInfo, with_manager, AppState};
use serde::Serialize;
use std::{fs::{self, OpenOptions}, io::Write, path::PathBuf, process::Command};
use tauri::{Manager as _, State};
use tauri_plugin_dialog::DialogExt;

const DOCS: &str = "https://github.com/bloxy-studios/Slide/blob/devin/milestone-f/docs/support.md";

#[derive(Serialize)]
pub struct SupportInfo {
    report: String,
    log_location: Option<PathBuf>,
}

#[derive(Serialize)]
struct Browser {
    brand: &'static str,
    version: Option<String>,
    bundled: bool,
}

fn version(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= 64
        && value.split('.').all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
        .then(|| value.to_owned())
}

fn os_version(os: &str, value: &str) -> Option<String> {
    if os != "windows" {
        return version(value);
    }
    let (major, build) = value.strip_suffix(')')?.split_once(" (")?;
    Some(format!("{} ({})", version(major)?, version(build)?))
}

fn report(app_version: &str, os_version: Option<&str>, runtime: Option<&RuntimeInfo>) -> Result<String, String> {
    let browser = runtime.map(|runtime| Browser {
        brand: match runtime.brand.as_str() {
            "Google Chrome" => "Google Chrome",
            "Chromium" => "Chromium",
            "ungoogled-chromium" => "ungoogled-chromium",
            "fingerprint-chromium" => "fingerprint-chromium",
            _ => "Unknown",
        },
        version: version(&runtime.version),
        bundled: runtime.bundled,
    });
    serde_json::to_string_pretty(&serde_json::json!({
        "schemaVersion": 1,
        "appVersion": version(app_version),
        "os": std::env::consts::OS,
        "osVersion": os_version.and_then(|value| self::os_version(std::env::consts::OS, value)),
        "architecture": std::env::consts::ARCH,
        "defaultBrowser": browser,
        "updates": "manual",
    })).map_err(|_| "Could not prepare diagnostics.".into())
}

fn log_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    Some(app.path().app_log_dir().ok()?.join("support.log"))
}

pub fn log_start(app: &tauri::AppHandle, catalog_available: bool) {
    let Some(path) = log_path(app) else { return };
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_ok() {
        let _ = fs::write(path, if catalog_available { "catalog: ready\n" } else { "catalog: unavailable\n" });
    }
}

fn current_report(app: &tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let runtime = with_manager(state, |manager| Ok(manager.default_runtime().cloned()))?;
    report(&app.package_info().version.to_string(), sysinfo::System::os_version().as_deref(), runtime.as_ref())
}

#[tauri::command]
pub fn support_info(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<SupportInfo, String> {
    Ok(SupportInfo {
        report: current_report(&app, state)?,
        log_location: log_path(&app).filter(|path| path.is_file()),
    })
}

#[tauri::command]
pub async fn export_support(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    let report = current_report(&app, state)?;
    let (send, mut receive) = tauri::async_runtime::channel(1);
    app.dialog().file().set_file_name("slide-diagnostics.json").add_filter("JSON", &["json"])
        .save_file(move |path| { let _ = send.try_send(path); });
    let Some(path) = receive.recv().await.ok_or("The save dialog did not return a result.")? else { return Ok(false) };
    let path = path.into_path().map_err(|_| "Choose a local file for the export.")?;
    OpenOptions::new().write(true).create_new(true).open(path)
        .and_then(|mut file| file.write_all(report.as_bytes()))
        .map_err(|_| "Could not save diagnostics. Choose a new file in a writable folder.")?;
    Ok(true)
}

#[tauri::command]
pub fn open_support_docs() -> Result<(), String> {
    let result = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open").arg(DOCS).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", DOCS]).spawn()
    } else {
        Command::new("xdg-open").arg(DOCS).spawn()
    };
    result.map(|_| ()).map_err(|_| "Could not open the support documentation.".into())
}

#[tauri::command]
pub async fn restart_manager(app: tauri::AppHandle) {
    app.request_restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_export_contains_only_allowlisted_fields() {
        let runtime = RuntimeInfo {
            path: "/Users/example/private-profile".into(),
            brand: "fingerprint-chromium".into(),
            version: "148.0.7778.215".into(),
            architecture: "secret-machine-identifier".into(),
            bundled: true,
        };
        let os = std::env::consts::OS;
        let os_version = if os == "windows" { "11 (26100)" } else { "26.1" };
        let json: serde_json::Value = serde_json::from_str(&report("0.1.0", Some(os_version), Some(&runtime)).unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({
            "schemaVersion": 1, "appVersion": "0.1.0",
            "os": os, "osVersion": os_version,
            "architecture": std::env::consts::ARCH, "updates": "manual",
            "defaultBrowser": {"brand": "fingerprint-chromium", "version": "148.0.7778.215", "bundled": true},
        }));
        let mut untrusted = runtime;
        untrusted.brand = "private-brand".into();
        untrusted.version = "148.0 private-token".into();
        let redacted = report("0.1.0", Some("26.1 private-host"), Some(&untrusted)).unwrap();
        assert!(!redacted.contains("private"));
        assert!(!redacted.contains("secret"));
        assert_eq!(super::os_version("windows", "11 (26100)").as_deref(), Some("11 (26100)"));
        assert_eq!(super::os_version("windows", "11 (26100 secret-token)"), None);
    }
}
