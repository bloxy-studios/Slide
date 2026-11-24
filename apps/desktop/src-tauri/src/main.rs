#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod manager;
mod runtime;

use manager::{Manager, Snapshot};
use runtime::RuntimeInfo;
use std::{path::PathBuf, sync::Mutex};
use tauri::{Manager as _, State};

type AppState = Mutex<Manager>;

fn with_manager<T>(
    state: State<'_, AppState>,
    action: impl FnOnce(&mut Manager) -> Result<T, String>,
) -> Result<T, String> {
    let mut manager = state
        .lock()
        .map_err(|_| "Workspace manager is unavailable. Restart the app.")?;
    action(&mut manager)
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    with_manager(state, Manager::snapshot)
}

#[tauri::command]
fn create_workspace(
    state: State<'_, AppState>,
    name: String,
    start_url: String,
) -> Result<(), String> {
    with_manager(state, |manager| manager.create(&name, &start_url))
}

#[tauri::command]
fn choose_runtime(state: State<'_, AppState>, path: String) -> Result<RuntimeInfo, String> {
    with_manager(state, |manager| {
        manager.choose_runtime(PathBuf::from(path.trim()))
    })
}

#[tauri::command]
fn workspace_action(
    state: State<'_, AppState>,
    id: String,
    operation: manager::Operation,
) -> Result<(), String> {
    with_manager(state, |manager| manager.perform(&id, operation))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            match Manager::new(root) {
                Ok(manager) => {
                    app.manage(Mutex::new(manager));
                }
                Err(error) => {
                    app.manage(Mutex::new(Manager::unavailable(error)));
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            create_workspace,
            choose_runtime,
            workspace_action
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Slide Workspaces");
}
