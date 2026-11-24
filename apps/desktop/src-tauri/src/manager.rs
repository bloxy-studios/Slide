use crate::{
    catalog::{self, CatalogView, ItemKind, Settings},
    runtime::{Chromium, Identity, Observation, RuntimeInfo, Session},
};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Child,
    time::{Duration, Instant},
};
use url::Url;
use uuid::Uuid;

const TRANSITION_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Closed,
    Running,
    Background,
    Starting,
    Closing,
    Locked,
    Crashed,
}

impl Status {
    fn close_unavailable_reason(self) -> Option<&'static str> {
        match self {
            Self::Running => None,
            Self::Background => Chromium::BACKGROUND_CLOSE_HINT,
            _ => Some("No active browser session can be closed."),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Open,
    Focus,
    Close,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Workspace {
    id: String,
    name: String,
    start_url: Option<String>,
    settings: Settings,
    created_at: i64,
    trashed_at: Option<i64>,
    status: Status,
    detail: Option<String>,
    can_close: bool,
    #[serde(skip)]
    identity: Option<Identity>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    workspaces: Vec<Workspace>,
    runtime: Option<RuntimeInfo>,
    runtime_error: Option<String>,
    catalog: CatalogView,
}

struct ChildProcess {
    process: Child,
    initial: bool,
}

struct Transition {
    status: Status,
    since: Instant,
}

struct Catalog {
    root: PathBuf,
    db: Connection,
    _lock: File,
}

pub struct Manager {
    catalog: Result<Catalog, String>,
    chromium: Chromium,
    runtime: Result<RuntimeInfo, String>,
    children: HashMap<String, Vec<ChildProcess>>,
    transitions: HashMap<String, Transition>,
    pub import_preview: Option<(String, crate::importer::Preview)>,
}

impl Manager {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root)
            .map_err(|e| format!("Could not create the app data folder: {e}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("manager.lock"))
            .map_err(|e| format!("Could not open the manager lock: {e}"))?;
        FileExt::try_lock_exclusive(&lock).map_err(|_| "Another Slide Workspaces manager is already using this catalog. Quit it, then restart this window.")?;
        let mut db = Connection::open(root.join("catalog.sqlite3")).map_err(db_error)?;
        db.pragma_update(None, "foreign_keys", true)
            .map_err(db_error)?;
        let version = db
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .map_err(db_error)?;
        let migrations = [
            include_str!("../migrations/001_workspaces.sql"),
            include_str!("../migrations/002_catalog.sql"),
        ];
        if version as usize > migrations.len() {
            return Err(
                "The catalog belongs to a newer version. Install that version before opening it."
                    .into(),
            );
        }
        for (index, migration) in migrations.iter().enumerate().skip(version as usize) {
            let tx = db.transaction().map_err(db_error)?;
            tx.execute_batch(migration).map_err(db_error)?;
            tx.pragma_update(None, "user_version", index + 1)
                .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
        }
        let path = db
            .query_row(
                "SELECT path FROM runtime_settings WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        let runtime = path.map_or_else(Chromium::discover, |path| {
            Chromium::inspect(Path::new(&path))
        });
        Ok(Self {
            catalog: Ok(Catalog {
                root,
                db,
                _lock: lock,
            }),
            chromium: Chromium::new(),
            runtime,
            children: HashMap::new(),
            transitions: HashMap::new(),
            import_preview: None,
        })
    }

    pub fn unavailable(error: String) -> Self {
        Self {
            catalog: Err(error),
            chromium: Chromium::new(),
            runtime: Err("Catalog unavailable".into()),
            children: HashMap::new(),
            transitions: HashMap::new(),
            import_preview: None,
        }
    }

    fn catalog(&self) -> Result<&Catalog, String> {
        self.catalog.as_ref().map_err(Clone::clone)
    }

    pub fn db(&self) -> Result<&Connection, String> {
        Ok(&self.catalog()?.db)
    }

    pub fn db_mut(&mut self) -> Result<&mut Connection, String> {
        Ok(&mut self.catalog.as_mut().map_err(|error| error.clone())?.db)
    }

    pub fn root(&self) -> Result<&Path, String> {
        Ok(&self.catalog()?.root)
    }

    pub fn import_row(
        &mut self,
        account: &str,
        fingerprint: &str,
        row: usize,
    ) -> Result<&'static str, String> {
        self.require_account(account)?;
        let (_, preview) = self
            .import_preview
            .as_ref()
            .filter(|(owner, preview)| owner == account && preview.fingerprint == fingerprint)
            .ok_or("Preview the original input file again before retrying.")?;
        let db = &mut self.catalog.as_mut().map_err(|error| error.clone())?.db;
        crate::importer::import_row(db, account, preview, row)
    }

    pub fn assign(
        &self,
        account: &str,
        id: &str,
        kind: ItemKind,
        value: Option<String>,
    ) -> Result<(), String> {
        let workspace = self
            .rows(account, Some(id))?
            .pop()
            .filter(|row| row.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        let mut settings = workspace.settings;
        match kind {
            ItemKind::Folder => settings.folder_id = value,
            ItemKind::Tag => {
                let tag = value.ok_or("Choose a tag.")?;
                if !settings.tag_ids.contains(&tag) {
                    settings.tag_ids.push(tag);
                }
            }
            _ => return Err("Unsupported batch assignment.".into()),
        }
        self.edit_workspace(account, id, &workspace.name, settings)
    }

    pub fn open_workspace_url(
        &mut self,
        account: &str,
        id: &str,
        url: String,
    ) -> Result<(), String> {
        self.require_account(account)?;
        let url = validate_url(&url)?.ok_or("A destination URL is required.")?;
        self.perform_with_url(id, Operation::Open, Some(url))
    }

    pub fn choose_runtime(&mut self, path: PathBuf) -> Result<RuntimeInfo, String> {
        let runtime = Chromium::inspect(&path)?;
        self.catalog()?.db.execute(
            "INSERT INTO runtime_settings (singleton, path) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET path = excluded.path",
            [runtime.path.to_str().ok_or("The browser path is not valid Unicode.")?],
        ).map_err(db_error)?;
        self.runtime = Ok(runtime.clone());
        Ok(runtime)
    }

    #[cfg(test)]
    fn create(&mut self, name: &str, start_url: &str) -> Result<(), String> {
        let db = self.db()?;
        insert_workspace(
            db,
            &catalog::account(db)?,
            name,
            Settings {
                launch_urls: validate_url(start_url)?.into_iter().collect(),
                ..Settings::default()
            },
        )?;
        Ok(())
    }

    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        let account = catalog::account(self.db()?)?;
        let mut workspaces = self.rows(&account, None)?;
        self.chromium.refresh();
        for workspace in &mut workspaces {
            self.reconcile(workspace)?;
        }
        Ok(Snapshot {
            workspaces,
            runtime: self.runtime.as_ref().ok().cloned(),
            runtime_error: self.runtime.as_ref().err().cloned(),
            catalog: catalog::snapshot(self.db()?, account)?,
        })
    }

    fn rows(&self, account: &str, id: Option<&str>) -> Result<Vec<Workspace>, String> {
        let mut statement = self.db()?.prepare(
            "SELECT id,name,start_url,pid,started,exit_note,settings,created_at,trashed_at FROM workspaces
             WHERE account_id=?1 AND (?2 IS NULL OR id=?2) ORDER BY rowid",
        ).map_err(db_error)?;
        let rows = statement
            .query_map(params![account, id], |row| {
                Ok((
                    Workspace {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        start_url: row.get(2)?,
                        identity: row
                            .get::<_, Option<u32>>(3)?
                            .zip(row.get::<_, Option<u64>>(4)?)
                            .map(|(pid, started)| Identity { pid, started }),
                        status: Status::Closed,
                        detail: row.get(5)?,
                        can_close: false,
                        settings: Settings::default(),
                        created_at: row.get(7)?,
                        trashed_at: row.get(8)?,
                    },
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(db_error)?;
        rows.map(|row| {
            let (mut workspace, data) = row.map_err(db_error)?;
            workspace.settings = catalog::read_settings(&data, workspace.start_url.as_deref())?;
            Ok(workspace)
        })
        .collect()
    }

    pub fn require_account(&self, expected: &str) -> Result<(), String> {
        if catalog::account(self.db()?)? != expected {
            return Err("The active local account changed. Retry in the intended account.".into());
        }
        Ok(())
    }

    pub fn switch_account(&mut self, id: &str) -> Result<(), String> {
        let db = self.db()?;
        if !catalog::list_accounts(db)?
            .iter()
            .any(|account| account.id == id)
        {
            return Err("This local account does not exist.".into());
        }
        let current = catalog::account(db)?;
        if id == current {
            return Ok(());
        }
        let mut workspaces = self.rows(&current, None)?;
        self.chromium.refresh();
        let mut waiting = false;
        for row in &mut workspaces {
            if row.trashed_at.is_some() {
                continue;
            }
            self.reconcile(row)?;
            match row.status {
                Status::Closed | Status::Crashed => {}
                Status::Running | Status::Background if row.can_close => {
                    self.perform(&row.id, Operation::Close)?;
                    waiting = true;
                }
                Status::Closing | Status::Starting => waiting = true,
                _ => {
                    return Err(format!(
                        "Close or unlock “{}” before switching local accounts.",
                        row.name
                    ))
                }
            }
        }
        if waiting {
            return Err(
                "Wait for the outgoing account's browsers to close, then switch again.".into(),
            );
        }
        self.db()?
            .execute("UPDATE current_account SET id=?1 WHERE singleton=1", [id])
            .map_err(db_error)?;
        Ok(())
    }

    pub fn clone_workspace(&self, account: &str, id: &str, name: &str) -> Result<String, String> {
        let workspace = self
            .rows(account, Some(id))?
            .pop()
            .filter(|row| row.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        insert_workspace(self.db()?, account, name, workspace.settings)
    }

    pub fn edit_workspace(
        &self,
        account: &str,
        id: &str,
        name: &str,
        settings: Settings,
    ) -> Result<(), String> {
        let db = self.db()?;
        let settings = catalog::settings(db, account, settings)?;
        if db.execute("UPDATE workspaces SET name=?3,start_url=?4,settings=?5 WHERE account_id=?1 AND id=?2 AND trashed_at IS NULL",
            params![account, id, catalog::name(name)?, settings.launch_urls.first(), catalog::encode(&settings)?],
        ).map_err(db_error)? != 1 {
            return Err("Workspace no longer exists.".into());
        }
        Ok(())
    }

    pub fn trash_workspace(
        &mut self,
        account: &str,
        id: &str,
        action: TrashAction,
    ) -> Result<(), String> {
        let mut row = self
            .rows(account, Some(id))?
            .pop()
            .ok_or("Workspace no longer exists.")?;
        match action {
            TrashAction::Restore if row.trashed_at.is_some() => {
                self.db()?
                    .execute("UPDATE workspaces SET trashed_at=NULL WHERE id=?1", [id])
                    .map_err(db_error)?;
            }
            TrashAction::Trash if row.trashed_at.is_none() => {
                self.require_inactive(&mut row)?;
                self.db()?
                    .execute(
                        "UPDATE workspaces SET trashed_at=?2 WHERE id=?1",
                        params![id, catalog::now()],
                    )
                    .map_err(db_error)?;
            }
            TrashAction::Purge if row.trashed_at.is_some() => {
                self.require_inactive(&mut row)?;
                let directory = self.directory(id)?;
                match fs::symlink_metadata(&directory) {
                    Ok(_) => {
                        let directory = checked_directory(&self.catalog()?.root, &directory)?;
                        fs::remove_dir_all(directory)
                            .map_err(|e| format!("Could not remove local browser data: {e}"))?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("Could not inspect local browser data: {error}"))
                    }
                }
                self.db()?
                    .execute("DELETE FROM workspaces WHERE id=?1", [id])
                    .map_err(db_error)?;
            }
            _ => {
                return Err(
                    "This action is not available for the workspace's current trash state.".into(),
                )
            }
        }
        Ok(())
    }

    fn require_inactive(&mut self, row: &mut Workspace) -> Result<(), String> {
        self.chromium.refresh();
        self.reconcile(row)?;
        if !matches!(row.status, Status::Closed | Status::Crashed) {
            return Err("Close the workspace browser before removing it. If it is locked, resolve the lock first.".into());
        }
        Ok(())
    }

    fn directory(&self, id: &str) -> Result<PathBuf, String> {
        let id = Uuid::parse_str(id).map_err(|_| "Invalid workspace ID.")?;
        Ok(self.catalog()?.root.join("profiles").join(id.to_string()))
    }

    fn reconcile(&mut self, workspace: &mut Workspace) -> Result<(), String> {
        let directory = self.directory(&workspace.id)?;
        let directory = match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => directory,
            _ => match checked_directory(&self.catalog()?.root, &directory) {
                Ok(directory) => directory,
                Err(error) => {
                    workspace.status = Status::Locked;
                    workspace.detail = Some(error);
                    return Ok(());
                }
            },
        };
        let old_identity = workspace.identity;
        let old_note = workspace.detail.take();
        let mut exit_note = old_note.clone();
        if let Some(children) = self.children.get_mut(&workspace.id) {
            children.retain_mut(|child| match child.process.try_wait() {
                Ok(Some(exit)) => {
                    if child.initial && !exit.success() {
                        exit_note = Some(format!(
                            "Browser exited ({exit}). Open the workspace to recover."
                        ));
                    }
                    false
                }
                _ => true,
            });
            if children.is_empty() {
                self.children.remove(&workspace.id);
            }
        }
        let transition = self.transitions.get(&workspace.id);
        let observation = self.chromium.observe(&directory);
        let (status, identity) = match &observation {
            Observation::Session(session) => {
                let (status, ended) = session_status(session, transition);
                if ended {
                    if transition.is_some_and(|t| t.status == Status::Closing && session.window) {
                        workspace.detail = Some(
                            "Browser did not close. Close its windows manually or retry.".into(),
                        );
                    }
                    self.transitions.remove(&workspace.id);
                }
                exit_note = None;
                (status, Some(session.identity))
            }
            Observation::Locked(reason) => {
                if let Some(t) = transition.filter(|t| t.since.elapsed() < TRANSITION_TIMEOUT) {
                    (t.status, old_identity)
                } else {
                    self.transitions.remove(&workspace.id);
                    workspace.detail = Some(reason.clone());
                    (Status::Locked, None)
                }
            }
            Observation::Absent => {
                if transition.is_some_and(|t| {
                    t.status == Status::Starting && t.since.elapsed() < TRANSITION_TIMEOUT
                }) && exit_note.is_none()
                {
                    (Status::Starting, old_identity)
                } else {
                    let requested_close = transition.is_some_and(|t| t.status == Status::Closing);
                    if old_identity.is_some() && !requested_close && exit_note.is_none() {
                        workspace.detail = Some("The previous browser process ended.".into());
                    }
                    if transition.is_some_and(|t| t.status == Status::Starting)
                        && old_identity.is_none()
                        && exit_note.is_none()
                    {
                        exit_note = Some("The browser did not open a verified session. Check the runtime and retry.".into());
                    }
                    self.transitions.remove(&workspace.id);
                    if exit_note.is_some() {
                        workspace.detail = exit_note.clone();
                    }
                    (
                        if exit_note.is_some() {
                            Status::Crashed
                        } else {
                            Status::Closed
                        },
                        None,
                    )
                }
            }
        };
        workspace.status = status;
        workspace.identity = identity;
        let close_unavailable = status.close_unavailable_reason();
        workspace.can_close = close_unavailable.is_none();
        if status == Status::Background {
            workspace.detail = workspace
                .detail
                .take()
                .or_else(|| close_unavailable.map(str::to_owned));
        }
        if identity != old_identity || exit_note != old_note {
            self.catalog()?
                .db
                .execute(
                    "UPDATE workspaces SET pid = ?1, started = ?2, exit_note = ?3 WHERE id = ?4",
                    params![
                        identity.map(|i| i.pid),
                        identity.map(|i| i.started),
                        exit_note,
                        workspace.id
                    ],
                )
                .map_err(db_error)?;
        }
        Ok(())
    }

    pub fn perform(&mut self, id: &str, operation: Operation) -> Result<(), String> {
        self.perform_with_url(id, operation, None)
    }

    fn perform_with_url(
        &mut self,
        id: &str,
        operation: Operation,
        url: Option<String>,
    ) -> Result<(), String> {
        let account = catalog::account(self.db()?)?;
        let mut workspace = self
            .rows(&account, Some(id))?
            .pop()
            .filter(|row| row.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        self.chromium.refresh();
        self.reconcile(&mut workspace)?;
        if matches!(workspace.status, Status::Starting | Status::Closing) {
            return Ok(());
        }
        if workspace.status == Status::Locked {
            return Err(workspace
                .detail
                .clone()
                .unwrap_or_else(|| "Workspace is locked.".into()));
        }
        let directory = self.directory(id)?;
        if workspace.status == Status::Running
            && matches!(operation, Operation::Open | Operation::Focus)
            && url.is_none()
        {
            let directory = dunce::canonicalize(directory)
                .map_err(|e| format!("Profile directory is unavailable: {e}"))?;
            return self.chromium.focus(
                &directory,
                workspace
                    .identity
                    .ok_or("No verified browser window is open.")?,
            );
        }
        match operation {
            Operation::Focus => {
                Err("No verified browser window is open. Use Open to reopen this workspace.".into())
            }
            Operation::Close => {
                if let Some(reason) = workspace.status.close_unavailable_reason() {
                    return Err(reason.into());
                }
                let identity = workspace.identity.ok_or("The browser is already closed.")?;
                let directory = dunce::canonicalize(directory)
                    .map_err(|e| format!("Profile directory is unavailable: {e}"))?;
                self.chromium.close(&directory, identity)?;
                self.transitions.insert(
                    id.into(),
                    Transition {
                        status: Status::Closing,
                        since: Instant::now(),
                    },
                );
                Ok(())
            }
            Operation::Open => {
                let root = self.catalog()?.root.clone();
                let directory = prepare_directory(&root, &directory)?;
                let reopen = workspace.status == Status::Background;
                let active = reopen || workspace.status == Status::Running;
                let executable = if active {
                    let Observation::Session(session) = self.chromium.observe(&directory) else {
                        return Err("The browser changed. Retry opening the workspace.".into());
                    };
                    self.chromium.verified(&directory, session.identity)?;
                    session.executable
                } else {
                    let cached = self.runtime.as_ref().map_err(Clone::clone)?;
                    let runtime = Chromium::inspect(&cached.path)?;
                    let executable = runtime.path.clone();
                    self.runtime = Ok(runtime);
                    executable
                };
                self.catalog()?
                    .db
                    .execute("UPDATE workspaces SET exit_note = NULL WHERE id = ?1", [id])
                    .map_err(db_error)?;
                let urls = url.as_ref().map_or(
                    workspace.settings.launch_urls.as_slice(),
                    std::slice::from_ref,
                );
                let child = Chromium::launch(&executable, &directory, urls, reopen)?;
                self.children
                    .entry(id.into())
                    .or_default()
                    .push(ChildProcess {
                        process: child,
                        initial: !active,
                    });
                if workspace.status != Status::Running {
                    self.transitions.insert(
                        id.into(),
                        Transition {
                            status: Status::Starting,
                            since: Instant::now(),
                        },
                    );
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrashAction {
    Trash,
    Restore,
    Purge,
}

pub fn insert_workspace(
    db: &Connection,
    account: &str,
    name: &str,
    settings: Settings,
) -> Result<String, String> {
    let settings = catalog::settings(db, account, settings)?;
    let id = Uuid::new_v4().to_string();
    db.execute("INSERT INTO workspaces (id,name,start_url,account_id,settings,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        params![id, catalog::name(name)?, settings.launch_urls.first(), account, catalog::encode(&settings)?, catalog::now()],
    ).map_err(db_error)?;
    Ok(id)
}

fn session_status(session: &Session, transition: Option<&Transition>) -> (Status, bool) {
    if let Some(transition) = transition.filter(|t| t.since.elapsed() < TRANSITION_TIMEOUT) {
        if transition.status == Status::Closing || !session.window {
            return (transition.status, false);
        }
    }
    (
        if session.window {
            Status::Running
        } else {
            Status::Background
        },
        true,
    )
}

fn prepare_directory(root: &Path, directory: &Path) -> Result<PathBuf, String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .recursive(true)
        .create(directory)
        .map_err(|e| format!("Could not create the workspace directory: {e}"))?;
    checked_directory(root, directory)
}

fn checked_directory(root: &Path, directory: &Path) -> Result<PathBuf, String> {
    let canonical = dunce::canonicalize(directory)
        .map_err(|e| format!("Could not resolve the workspace directory: {e}"))?;
    let root =
        dunce::canonicalize(root).map_err(|e| format!("Could not resolve app data folder: {e}"))?;
    if canonical
        != root
            .join("profiles")
            .join(directory.file_name().ok_or("Invalid workspace folder.")?)
    {
        return Err("The workspace directory points outside its assigned location. Restore the original folder before opening.".into());
    }
    Ok(canonical)
}

pub(crate) fn db_error(error: rusqlite::Error) -> String {
    format!("Could not read or save the workspace catalog: {error}. Retry or check local storage.")
}

pub(crate) fn validate_url(input: &str) -> Result<Option<String>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let url = Url::parse(input).map_err(|_| "Enter a full HTTP or HTTPS start URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an HTTP or HTTPS start URL without embedded credentials.".into());
    }
    Ok(Some(url.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_accounts_isolate_catalog_and_clones_never_share_browser_data() {
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        let original = insert_workspace(
            manager.db().unwrap(),
            "local",
            "Original",
            Settings {
                notes: "Keep settings".into(),
                launch_urls: vec![
                    "http://localhost:4310".into(),
                    "http://localhost:4311".into(),
                ],
                ..Settings::default()
            },
        )
        .unwrap();
        let directory = manager.directory(&original).unwrap();
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("synthetic-session"), "Creator").unwrap();
        let clone = manager
            .clone_workspace("local", &original, "Clone")
            .unwrap();
        assert!(!manager.directory(&clone).unwrap().exists());
        let rows = manager.snapshot().unwrap().workspaces;
        assert_eq!(rows[0].settings, rows[1].settings);
        let other = catalog::add_account(manager.db_mut().unwrap(), "QA").unwrap();
        manager.switch_account(&other).unwrap();
        assert!(manager.snapshot().unwrap().workspaces.is_empty());
        assert!(manager.perform(&original, Operation::Open).is_err());
        manager.switch_account("local").unwrap();
        assert_eq!(manager.snapshot().unwrap().workspaces.len(), 2);
    }

    #[test]
    fn folder_removal_preserves_profiles_and_trash_purge_removes_owned_data() {
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        let folder = catalog::save_item(
            manager.db().unwrap(),
            "local",
            ItemKind::Folder,
            None,
            &catalog::Label { name: "QA".into() },
        )
        .unwrap();
        let id = insert_workspace(
            manager.db().unwrap(),
            "local",
            "Example",
            Settings {
                folder_id: Some(folder.clone()),
                ..Settings::default()
            },
        )
        .unwrap();
        catalog::remove_item(
            manager.db_mut().unwrap(),
            "local",
            ItemKind::Folder,
            &folder,
        )
        .unwrap();
        assert!(manager.snapshot().unwrap().workspaces[0]
            .settings
            .folder_id
            .is_none());
        let directory = manager.directory(&id).unwrap();
        fs::create_dir_all(&directory).unwrap();
        manager
            .trash_workspace("local", &id, TrashAction::Trash)
            .unwrap();
        assert!(directory.exists());
        assert!(manager.perform(&id, Operation::Open).is_err());
        manager
            .trash_workspace("local", &id, TrashAction::Restore)
            .unwrap();
        assert!(manager.snapshot().unwrap().workspaces[0]
            .trashed_at
            .is_none());
        manager
            .trash_workspace("local", &id, TrashAction::Trash)
            .unwrap();
        manager
            .trash_workspace("local", &id, TrashAction::Purge)
            .unwrap();
        assert!(!directory.exists());
        assert!(manager.snapshot().unwrap().workspaces.is_empty());
    }

    #[test]
    fn missed_shared_task_fires_once_across_relaunch() {
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        manager.create("Creator", "").unwrap();
        manager.create("Viewer", "").unwrap();
        let ids = manager
            .rows("local", None)
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        catalog::create_task(
            manager.db_mut().unwrap(),
            "local",
            catalog::TaskDraft {
                title: "Check fixture".into(),
                description: "".into(),
                due_at: 1,
                workspace_ids: ids.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            catalog::due_tasks(manager.db().unwrap(), "local", 2).unwrap(),
            1
        );
        drop(manager);
        let manager = Manager::new(test.path().into()).unwrap();
        assert_eq!(
            catalog::due_tasks(manager.db().unwrap(), "local", 3).unwrap(),
            0
        );
        let mut task = catalog::list_tasks(manager.db().unwrap(), "local")
            .unwrap()
            .remove(0);
        task.workspace_ids.sort();
        let mut ids = ids;
        ids.sort();
        assert_eq!(task.workspace_ids, ids);
        assert_eq!(task.fired_at, Some(2));
    }

    #[test]
    fn catalog_persists_after_relaunch_and_rejects_parallel_manager() {
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        manager.create("Creator", "http://127.0.0.1:4310").unwrap();
        manager.create("Viewer", "").unwrap();
        assert!(Manager::new(test.path().into()).is_err());
        drop(manager);
        let snapshot = Manager::new(test.path().into())
            .unwrap()
            .snapshot()
            .unwrap();
        assert_eq!(
            snapshot.workspaces[0].start_url.as_deref(),
            Some("http://127.0.0.1:4310/")
        );
        assert_eq!(snapshot.workspaces[1].name, "Viewer");
        assert_eq!(snapshot.workspaces[1].status, Status::Closed);
        assert_ne!(snapshot.workspaces[0].id, snapshot.workspaces[1].id);
    }

    #[test]
    fn a_live_process_without_a_window_is_not_a_crash_or_second_launch() {
        let mut session = Session {
            identity: Identity {
                pid: 1,
                started: 42,
            },
            executable: "Chrome".into(),
            window: false,
        };
        let opening = Transition {
            status: Status::Starting,
            since: Instant::now(),
        };
        let closing = Transition {
            status: Status::Closing,
            since: Instant::now(),
        };
        assert_eq!(session_status(&session, None), (Status::Background, true));
        assert_eq!(
            session_status(&session, Some(&opening)),
            (Status::Starting, false)
        );
        session.window = true;
        assert_eq!(
            session_status(&session, Some(&opening)),
            (Status::Running, true)
        );
        assert_eq!(
            session_status(&session, Some(&closing)),
            (Status::Closing, false)
        );
    }

    #[test]
    fn background_close_availability_matches_native_runtime() {
        assert!(Status::Running.close_unavailable_reason().is_none());
        assert_eq!(
            Status::Background.close_unavailable_reason().is_some(),
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn a_second_open_does_not_launch_while_the_initial_request_is_pending() {
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        manager.create("Workspace", "").unwrap();
        let id = manager.snapshot().unwrap().workspaces[0].id.clone();
        manager.transitions.insert(
            id.clone(),
            Transition {
                status: Status::Starting,
                since: Instant::now(),
            },
        );
        assert!(manager.perform(&id, Operation::Open).is_ok());
        assert!(!manager.directory(&id).unwrap().exists());
        assert_eq!(
            manager.snapshot().unwrap().workspaces[0].status,
            Status::Starting
        );
    }

    #[cfg(unix)]
    #[test]
    fn records_a_crash_but_not_a_successful_browser_exit() {
        use std::process::{Command, Stdio};
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        for (code, expected_status) in [(0, Status::Closed), (9, Status::Crashed)] {
            manager.create(&format!("Workspace {code}"), "").unwrap();
            let id = manager
                .snapshot()
                .unwrap()
                .workspaces
                .last()
                .unwrap()
                .id
                .clone();
            let mut process = Command::new("/bin/sh")
                .args(["-c", &format!("exit {code}")])
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            process.wait().unwrap();
            manager.children.insert(
                id,
                vec![ChildProcess {
                    process,
                    initial: true,
                }],
            );
            assert_eq!(
                manager
                    .snapshot()
                    .unwrap()
                    .workspaces
                    .last()
                    .unwrap()
                    .status,
                expected_status
            );
        }
    }

    #[test]
    fn profile_folder_cannot_point_to_another_workspace() {
        #[cfg(unix)]
        {
            let test = tempfile::tempdir().unwrap();
            let profiles = test.path().join("profiles");
            let other = profiles.join("other");
            fs::create_dir_all(&other).unwrap();
            let directory = profiles.join("workspace");
            std::os::unix::fs::symlink(other, &directory).unwrap();
            assert!(prepare_directory(test.path(), &directory).is_err());
        }
    }

    #[test]
    fn start_urls_do_not_allow_browser_switches_or_embedded_credentials() {
        for invalid in [
            "--incognito",
            "file:///private",
            "javascript:alert(1)",
            "http://user:password@example.local",
        ] {
            assert!(validate_url(invalid).is_err());
        }
        assert_eq!(validate_url(" ").unwrap(), None);
    }
}
