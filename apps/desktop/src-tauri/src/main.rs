#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod catalog;
mod extensions;
mod crx;
mod importer;
mod manager;
mod runtime;

use catalog::{Bookmark, ItemKind, Label, Settings, Tag, TaskDraft, Template, View};
use manager::{insert_workspace, Manager, Snapshot, TrashAction};
use runtime::{Chromium, RuntimeInfo};
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
    account: String,
    name: String,
    creation: Creation,
) -> Result<String, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        let db = manager.db()?;
        let settings = match creation {
            Creation::Manual { settings } => settings,
            Creation::Quick => match catalog::view_options(db, &account)?.0 {
                Some(id) => {
                    catalog::item::<Template>(db, &account, ItemKind::Template, &id)?.settings
                }
                None => Settings::default(),
            },
            Creation::Template { id } => {
                catalog::item::<Template>(db, &account, ItemKind::Template, &id)?.settings
            }
            Creation::Clone { id } => return manager.clone_workspace(&account, &id, &name),
        };
        insert_workspace(db, &account, &name, settings)
    })
}

#[derive(serde::Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
enum Creation {
    Manual { settings: Settings },
    Quick,
    Template { id: String },
    Clone { id: String },
}

#[tauri::command]
fn edit_workspace(
    state: State<'_, AppState>,
    account: String,
    id: String,
    name: String,
    settings: Settings,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        manager.edit_workspace(&account, &id, &name, settings)
    })
}

#[tauri::command]
fn trash_workspace(
    state: State<'_, AppState>,
    account: String,
    id: String,
    action: TrashAction,
    confirmed: bool,
) -> Result<(), String> {
    if matches!(action, TrashAction::Purge) && !confirmed {
        return Err("Confirm permanent removal first.".into());
    }
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        manager.trash_workspace(&account, &id, action)
    })
}

#[tauri::command]
fn add_account(state: State<'_, AppState>, name: String) -> Result<String, String> {
    with_manager(state, |manager| {
        catalog::add_account(manager.db_mut()?, &name)
    })
}

#[tauri::command]
fn switch_account(state: State<'_, AppState>, id: String) -> Result<(), String> {
    with_manager(state, |manager| manager.switch_account(&id))
}

#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum CatalogEntry {
    Folder { name: String },
    Tag { name: String, color: String },
    Bookmark { name: String, url: String },
    Template { name: String, settings: Settings },
}

#[tauri::command]
fn save_item(
    state: State<'_, AppState>,
    account: String,
    id: Option<String>,
    entry: CatalogEntry,
) -> Result<String, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        let db = manager.db()?;
        match entry {
            CatalogEntry::Folder { name } => catalog::save_item(
                db,
                &account,
                ItemKind::Folder,
                id,
                &Label {
                    name: catalog::name(&name)?.into(),
                },
            ),
            CatalogEntry::Tag { name, color } => {
                if color.len() != 7
                    || !color.starts_with('#')
                    || !color[1..].bytes().all(|c| c.is_ascii_hexdigit())
                {
                    return Err("Enter a color in #RRGGBB format.".into());
                }
                catalog::save_item(
                    db,
                    &account,
                    ItemKind::Tag,
                    id,
                    &Tag {
                        name: catalog::name(&name)?.into(),
                        color,
                    },
                )
            }
            CatalogEntry::Bookmark { name, url } => catalog::save_item(
                db,
                &account,
                ItemKind::Bookmark,
                id,
                &Bookmark {
                    name: catalog::name(&name)?.into(),
                    url: manager::validate_url(&url)?.ok_or("Enter a bookmark URL.")?,
                },
            ),
            CatalogEntry::Template { name, settings } => catalog::save_item(
                db,
                &account,
                ItemKind::Template,
                id,
                &Template {
                    name: catalog::name(&name)?.into(),
                    settings: catalog::settings(db, &account, settings)?,
                },
            ),
        }
    })
}

#[tauri::command]
fn remove_item(
    state: State<'_, AppState>,
    account: String,
    kind: ItemKind,
    id: String,
) -> Result<(), String> {
    with_manager(state, |manager| manager.remove_item(&account, kind, &id))
}

#[tauri::command]
fn save_view(state: State<'_, AppState>, account: String, view: View) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        catalog::save_view(manager.db()?, &account, &view)
    })
}

#[tauri::command]
fn set_default_template(
    state: State<'_, AppState>,
    account: String,
    id: Option<String>,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        catalog::set_default_template(manager.db()?, &account, id)
    })
}

#[tauri::command]
fn create_task(
    state: State<'_, AppState>,
    account: String,
    draft: TaskDraft,
) -> Result<String, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        catalog::create_task(manager.db_mut()?, &account, draft)
    })
}

#[tauri::command]
fn finish_task(
    state: State<'_, AppState>,
    account: String,
    id: String,
    snooze: bool,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        catalog::finish_task(manager.db()?, &account, &id, snooze)
    })
}

#[tauri::command]
fn preview_import(
    state: State<'_, AppState>,
    account: String,
    path: String,
) -> Result<serde_json::Value, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        let preview = importer::read_preview(manager.db()?, &account, std::path::Path::new(&path))?;
        let display = serde_json::to_value(&preview).map_err(|e| e.to_string())?;
        manager.import_preview = Some((account, preview));
        Ok(display)
    })
}

#[tauri::command]
fn import_profile_row(
    state: State<'_, AppState>,
    account: String,
    fingerprint: String,
    row: usize,
) -> Result<&'static str, String> {
    with_manager(state, |manager| {
        manager.import_row(&account, &fingerprint, row)
    })
}

#[tauri::command]
fn batch_assign(
    state: State<'_, AppState>,
    account: String,
    id: String,
    kind: ItemKind,
    value: Option<String>,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        manager.assign(&account, &id, kind, value)
    })
}

#[tauri::command]
async fn network_test(
    state: State<'_, AppState>,
    account: String,
    id: String,
) -> Result<String, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        if !catalog::has_workspace(manager.db()?, &account, &id, false)? {
            return Err("Workspace no longer exists.".into());
        }
        Ok(())
    })?;
    tauri::async_runtime::spawn_blocking(|| {
        use std::{
            io::{Read, Write},
            net::TcpStream,
            time::Duration,
        };
        let timeout = Duration::from_secs(2);
        let mut socket = TcpStream::connect_timeout(&([127, 0, 0, 1], 4310).into(), timeout)
            .map_err(|_| "The local fixture is not reachable at 127.0.0.1:4310.")?;
        socket
            .set_read_timeout(Some(timeout))
            .and_then(|_| socket.set_write_timeout(Some(timeout)))
            .map_err(|_| "Could not set fixture probe timeouts.")?;
        socket
            .write_all(b"HEAD / HTTP/1.1\r\nHost: 127.0.0.1:4310\r\nConnection: close\r\n\r\n")
            .map_err(|_| "Could not request the local fixture.")?;
        let mut prefix = [0; 12];
        socket
            .read_exact(&mut prefix)
            .map_err(|_| "The local fixture did not respond.")?;
        if !prefix.starts_with(b"HTTP/1.") {
            return Err("The local endpoint did not send an HTTP response.".into());
        }
        Ok("Local HTTP fixture reachable (manager only; no browser or proxy test)".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn stage_extension(
    state: State<'_, AppState>,
    account: String,
    id: Option<String>,
    source: String,
    path: Option<PathBuf>,
    fixture_update: bool,
) -> Result<String, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        extensions::stage(
            manager.db()?,
            manager.root()?,
            &account,
            id,
            &source,
            path.as_deref(),
            fixture_update,
        )
    })
}

#[tauri::command]
fn save_store_extension(
    state: State<'_, AppState>,
    account: String,
    id: Option<String>,
    name: String,
    browser_id: String,
    version: String,
    permissions: Vec<String>,
) -> Result<String, String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        extensions::save_store(
            manager.db()?,
            &account,
            id,
            &name,
            &browser_id,
            &version,
            permissions,
        )
    })
}

#[tauri::command]
fn open_bookmark(
    state: State<'_, AppState>,
    account: String,
    id: String,
    bookmark: String,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        let bookmark: Bookmark =
            catalog::item(manager.db()?, &account, ItemKind::Bookmark, &bookmark)?;
        manager.open_workspace_url(&account, &id, bookmark.url)
    })
}

#[tauri::command]
fn extension_install_options(state: State<'_, AppState>, account: String, id: String, extension: String) -> Result<crx::InstallOptions, String> {
    with_manager(state, |manager| manager.extension_install_options(&account, &id, &extension))
}

#[tauri::command]
fn review_crx(state: State<'_, AppState>, account: String, id: String, extension: String, path: Option<PathBuf>) -> Result<(uuid::Uuid, crx::Review), String> {
    with_manager(state, |manager| manager.review_crx(&account, &id, &extension, path.as_deref()))
}

#[tauri::command]
fn approve_crx(state: State<'_, AppState>, account: String, token: uuid::Uuid) -> Result<(), String> {
    with_manager(state, |manager| manager.approve_crx(&account, token))
}

#[tauri::command]
fn open_extension_page(
    state: State<'_, AppState>,
    account: String,
    id: String,
    extension: String,
) -> Result<(), String> {
    with_manager(state, |manager| {
        if manager.extension_install_options(&account, &id, &extension)?.bundled {
            return Err("Use a downloaded CRX for this bundled browser workspace.".into());
        }
        let extension: catalog::Extension =
            catalog::item(manager.db()?, &account, ItemKind::Extension, &extension)?;
        manager.open_workspace_url(
            &account,
            &id,
            extension
                .install_url
                .ok_or("Open chrome://extensions manually in this workspace.")?,
        )
    })
}

#[tauri::command]
fn choose_runtime(state: State<'_, AppState>, path: String) -> Result<RuntimeInfo, String> {
    with_manager(state, |manager| {
        manager.choose_runtime(PathBuf::from(path.trim()))
    })
}

#[tauri::command]
fn use_bundled_runtime(state: State<'_, AppState>) -> Result<RuntimeInfo, String> {
    with_manager(state, Manager::use_bundled_runtime)
}

#[tauri::command]
fn workspace_action(
    state: State<'_, AppState>,
    account: String,
    id: String,
    operation: manager::Operation,
) -> Result<(), String> {
    with_manager(state, |manager| {
        manager.require_account(&account)?;
        manager.perform(&id, operation)
    })
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            let bundled = Chromium::bundled_path(&app.path().resource_dir()?);
            let bundled = (!cfg!(debug_assertions) || bundled.exists()).then_some(bundled);
            match Manager::with_bundle(root, bundled) {
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
            edit_workspace,
            trash_workspace,
            add_account,
            switch_account,
            save_item,
            remove_item,
            save_view,
            set_default_template,
            create_task,
            finish_task,
            preview_import,
            import_profile_row,
            batch_assign,
            network_test,
            stage_extension,
            save_store_extension,
            open_bookmark,
            open_extension_page,
            extension_install_options,
            review_crx,
            approve_crx,
            choose_runtime,
            use_bundled_runtime,
            workspace_action
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Slide Workspaces");
}
