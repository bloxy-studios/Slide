use crate::{
    catalog::{self, CatalogView, ItemKind, Settings},
    crx,
    runtime::{Chromium, Identity, Observation, RuntimeInfo, Session},
};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::Child,
    time::{Duration, Instant},
};
use url::Url;
use uuid::Uuid;

const TRANSITION_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_PROFILE_SEED_BYTES: u64 = 1024;

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

#[derive(Clone, Copy, Debug, Deserialize)]
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
    automation_cdp: bool,
    detail: Option<String>,
    can_close: bool,
    #[serde(skip)]
    identity: Option<Identity>,
    #[serde(rename = "browserBrand", serialize_with = "serialize_browser_brand")]
    browser: Option<ProfileBrowser>,
}

fn serialize_browser_brand<S: serde::Serializer>(browser: &Option<ProfileBrowser>, serializer: S) -> Result<S::Ok, S::Error> {
    browser.as_ref().map(|browser| browser.brand.as_str()).serialize(serializer)
}

struct ProfileBrowser {
    brand: String,
    path: PathBuf,
    version: Option<String>,
}

impl ProfileBrowser {
    fn accepts(&self, runtime: &RuntimeInfo) -> bool {
        self.brand == runtime.brand
            && self.version.as_deref().is_some_and(|version| runtime.is_at_least(version))
    }

    fn unavailable(&self) -> String {
        match &self.version {
            Some(version) => format!(
                "This workspace last used {} {}. Install or select that version or newer; Slide will not open it in an older browser or a different brand.",
                self.brand, version,
            ),
            None => format!(
                "This workspace last used {}, but its version was not recorded. Slide will not risk its saved sessions. Clone its settings into a new workspace or restore a backup with its original Slide and browser.",
                self.brand,
            ),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    workspaces: Vec<Workspace>,
    runtime: Option<RuntimeInfo>,
    runtime_error: Option<String>,
    has_bundled_runtime: bool,
    platform: &'static str,
    required_http_proxy_available: bool,
    catalog: CatalogView,
}

struct ChildProcess {
    process: Child,
    initial: bool,
    startup_check: Option<crate::runtime::Route>,
}

struct Transition {
    status: Status,
    since: Instant,
}

struct Catalog {
    root: PathBuf,
    db: Connection,
    _lock: CatalogLock,
}

struct CatalogLock(File);

impl Drop for CatalogLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub struct Manager {
    catalog: Result<Catalog, String>,
    chromium: Chromium,
    runtime: Result<RuntimeInfo, String>,
    bundled_path: Option<PathBuf>,
    fingerprint_path: Option<PathBuf>,
    children: HashMap<String, Vec<ChildProcess>>,
    transitions: HashMap<String, Transition>,
    debug_launches: HashMap<String, u32>,
    pub import_preview: Option<(String, crate::importer::Preview)>,
    crx_review: Option<(Uuid, String, String, String, crx::Package)>,
}

impl Manager {
    #[cfg(test)]
    pub fn new(root: PathBuf) -> Result<Self, String> {
        Self::with_bundle(root, None)
    }

    #[cfg(test)]
    pub fn with_bundle(root: PathBuf, bundled_path: Option<PathBuf>) -> Result<Self, String> {
        Self::with_bundles(root, bundled_path, None)
    }

    pub fn with_bundles(root: PathBuf, bundled_path: Option<PathBuf>, fingerprint_path: Option<PathBuf>) -> Result<Self, String> {
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
        let lock = CatalogLock(lock);
        let mut db = Connection::open(root.join("catalog.sqlite3")).map_err(db_error)?;
        db.pragma_update(None, "foreign_keys", true)
            .map_err(db_error)?;
        let version = db
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .map_err(db_error)?;
        let migrations = [
            include_str!("../migrations/001_workspaces.sql"),
            include_str!("../migrations/002_catalog.sql"),
            include_str!("../migrations/003_profile_browser.sql"),
            include_str!("../migrations/004_profile_browser_version.sql"),
            include_str!("../migrations/005_workspace_crx.sql"),
            include_str!("../migrations/006_connections.sql"),
            include_str!("../migrations/007_bridge.sql"),
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
        let runtime = match (path, fingerprint_path.as_deref(), bundled_path.as_deref()) {
            (Some(path), _, _) => Chromium::inspect(Path::new(&path)),
            (None, Some(path), _) => Chromium::inspect_fingerprint(path),
            (None, None, Some(path)) => Chromium::inspect_bundled(path),
            (None, None, None) => Chromium::discover(),
        };
        let mut manager = Self {
            catalog: Ok(Catalog {
                root,
                db,
                _lock: lock,
            }),
            chromium: Chromium::new(),
            runtime,
            bundled_path,
            fingerprint_path,
            children: HashMap::new(),
            transitions: HashMap::new(),
            debug_launches: HashMap::new(),
            import_preview: None,
            crx_review: None,
        };
        manager.runtime = manager.runtime.as_ref().cloned().map(|runtime| manager.identify_bundled(runtime)).map_err(Clone::clone);
        Ok(manager)
    }

    pub fn unavailable(error: String) -> Self {
        Self {
            catalog: Err(error),
            chromium: Chromium::new(),
            runtime: Err("Catalog unavailable".into()),
            bundled_path: None,
            fingerprint_path: None,
            children: HashMap::new(),
            transitions: HashMap::new(),
            debug_launches: HashMap::new(),
            import_preview: None,
            crx_review: None,
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

    pub fn remove_item(&mut self, account: &str, kind: ItemKind, id: &str) -> Result<(), String> {
        self.require_account(account)?;
        if matches!(kind, ItemKind::Extension) {
            self.chromium.refresh();
            for mut workspace in self.rows(account, None)? {
                if crx::installed(self.db()?, &workspace.id, id)?.is_none() {
                    continue;
                }
                self.reconcile(&mut workspace)?;
                if !matches!(workspace.status, Status::Closed | Status::Crashed) {
                    return Err(format!(
                        "Close the browser for “{}” before removing this extension. If the profile is locked, resolve the lock first.",
                        workspace.name
                    ));
                }
            }
        }
        let packages = catalog::remove_item(self.db_mut()?, account, kind, id)?;
        let root = self.root()?;
        let mut failure = None;
        for (workspace, package) in packages {
            if let Err(error) = crx::remove_package(root, &workspace, &package) {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(format!("The catalog entry was removed, but extension cleanup failed: {error}")),
            None => Ok(()),
        }
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
        &mut self,
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
        let runtime = self.identify_bundled(Chromium::inspect(&path)?);
        self.catalog()?.db.execute(
            "INSERT INTO runtime_settings (singleton, path) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET path = excluded.path",
            [runtime.path.to_str().ok_or("The browser path is not valid Unicode.")?],
        ).map_err(db_error)?;
        self.runtime = Ok(runtime.clone());
        Ok(runtime)
    }

    pub fn use_bundled_runtime(&mut self) -> Result<RuntimeInfo, String> {
        let runtime = self.bundled_runtimes().next().ok_or("No bundled Chromium runtime is available.")?;
        self.db()?.execute("DELETE FROM runtime_settings", []).map_err(db_error)?;
        self.runtime = Ok(runtime.clone());
        Ok(runtime)
    }

    fn extension_target(&mut self, account: &str, id: &str, extension: &str) -> Result<(Workspace, catalog::Extension, RuntimeInfo), String> {
        self.require_account(account)?;
        let mut workspace = self.rows(account, Some(id))?.pop()
            .filter(|row| row.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        if !workspace.settings.extension_ids.iter().any(|entry| entry == extension) {
            return Err("Assign the extension to this workspace first.".into());
        }
        let entry = catalog::item(self.db()?, account, ItemKind::Extension, extension)?;
        let directory = self.directory(id)?;
        self.chromium.refresh();
        self.reconcile(&mut workspace)?;
        let runtime = match workspace.status {
            Status::Closed | Status::Crashed => {
                let directory = prepare_directory(self.root()?, &directory)?;
                self.profile_runtime(&workspace, &directory)?
            }
            Status::Running | Status::Background => {
                let directory = checked_directory(self.root()?, &directory)?;
                let Observation::Session(session) = self.chromium.observe(&directory) else {
                    return Err("The browser changed. Try again.".into());
                };
                self.chromium.verified(&directory, session.identity)?;
                self.identify_bundled(Chromium::inspect(&session.executable)?)
            }
            _ => return Err("Wait for this workspace to finish opening or closing, or resolve its profile lock.".into()),
        };
        Ok((workspace, entry, runtime))
    }

    pub fn extension_install_options(&mut self, account: &str, id: &str, extension: &str) -> Result<crx::InstallOptions, String> {
        let (_, entry, runtime) = self.extension_target(account, id, extension)?;
        let installed = crx::installed(self.db()?, id, extension)?
            .filter(|install| install.browser_id == crx_id(&entry));
        let update_available = installed.as_ref().and_then(|install| crx::version(&install.version))
            .zip(crx::version(&entry.version)).is_some_and(|(installed, pin)| pin > installed);
        Ok(crx::InstallOptions { bundled: runtime.bundled, installed_version: installed.map(|install| install.version), update_available })
    }

    pub fn review_crx(&mut self, account: &str, id: &str, extension: &str, path: Option<&Path>) -> Result<(Uuid, crx::Review), String> {
        self.crx_review = None;
        let (workspace, entry, runtime) = self.extension_target(account, id, extension)?;
        if !runtime.bundled || !matches!(workspace.status, Status::Closed | Status::Crashed) {
            return Err("Close the bundled browser workspace before reviewing a CRX.".into());
        }
        let package = match path {
            Some(path) => crx::inspect_file(path, crx_id(&entry), &entry.version)?,
            None if entry.source == "fixture" => crx::fixture(&entry.version)?,
            None => return Err("Choose a downloaded CRX file.".into()),
        };
        package.require_compatible_runtime(&runtime.version)?;
        let token = Uuid::new_v4();
        let review = package.review.clone();
        self.crx_review = Some((token, account.into(), id.into(), extension.into(), package));
        Ok((token, review))
    }

    pub fn approve_crx(&mut self, account: &str, token: Uuid) -> Result<(), String> {
        let (_, owner, id, extension, package) = self.crx_review.take()
            .filter(|(reviewed, owner, ..)| *reviewed == token && owner == account)
            .ok_or("Review this extension again before installing it.")?;
        let (workspace, entry, runtime) = self.extension_target(&owner, &id, &extension)?;
        if !runtime.bundled || !matches!(workspace.status, Status::Closed | Status::Crashed)
            || package.review.browser_id != crx_id(&entry) || package.review.version != entry.version
        {
            return Err("The workspace or catalog changed. Review this extension again.".into());
        }
        package.require_compatible_runtime(&runtime.version)?;
        if crx::installed(self.db()?, &id, &extension)?
            .filter(|installed| installed.browser_id == package.review.browser_id)
            .and_then(|installed| crx::version(&installed.version))
            .zip(crx::version(&package.review.version))
            .is_some_and(|(installed, proposed)| installed > proposed)
        {
            return Err("Slide will not downgrade the installed extension.".into());
        }
        crx::install(self.db()?, self.root()?, &id, &extension, package)
    }

    fn approved_extensions(&self, account: &str, workspace: &Workspace) -> Result<Vec<PathBuf>, String> {
        let db = self.db()?;
        let mut paths = Vec::new();
        for id in &workspace.settings.extension_ids {
            if let Some(install) = crx::installed(db, &workspace.id, id)? {
                let entry = catalog::item(db, account, ItemKind::Extension, id)?;
                if install.browser_id == crx_id(&entry) {
                    paths.push(crx::approved_path(self.root()?, &workspace.id, &install)?);
                }
            }
        }
        Ok(paths)
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
            has_bundled_runtime: self.bundled_path.is_some() || self.fingerprint_path.is_some(),
            platform: std::env::consts::OS,
            required_http_proxy_available: self.runtime.as_ref().is_ok_and(RuntimeInfo::supports_required_http_proxy),
            catalog: catalog::snapshot(self.db()?, account)?,
        })
    }

    pub fn default_runtime(&self) -> Option<&RuntimeInfo> {
        self.runtime.as_ref().ok()
    }

    fn rows(&self, account: &str, id: Option<&str>) -> Result<Vec<Workspace>, String> {
        let mut statement = self.db()?.prepare(
            "SELECT id,name,start_url,pid,started,exit_note,settings,created_at,trashed_at,browser_brand,browser_path,browser_version,automation_cdp FROM workspaces
             WHERE account_id=?1 AND (?2 IS NULL OR id=?2) ORDER BY rowid",
        ).map_err(db_error)?;
        let rows = statement
            .query_map(params![account, id], |row| {
                let version = row.get(11)?;
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
                        automation_cdp: row.get(12)?,
                        detail: row.get(5)?,
                        can_close: false,
                        settings: Settings::default(),
                        created_at: row.get(7)?,
                        trashed_at: row.get(8)?,
                        browser: row
                            .get::<_, Option<String>>(9)?
                            .zip(row.get::<_, Option<String>>(10)?)
                            .map(|(brand, path)| ProfileBrowser {
                                brand,
                                path: PathBuf::from(path),
                                version,
                            }),
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
        let mut settings = workspace.settings;
        if let Some(config) = &mut settings.fingerprint {
            config.seed = None;
        }
        insert_workspace(self.db()?, account, name, settings)
    }

    pub fn edit_workspace(
        &mut self,
        account: &str,
        id: &str,
        name: &str,
        settings: Settings,
    ) -> Result<(), String> {
        let mut settings = catalog::settings(self.db()?, account, settings)?;
        let mut current = self.rows(account, Some(id))?.pop()
            .filter(|workspace| workspace.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        if let Some(config) = &mut settings.fingerprint {
            config.seed = config.seed.or_else(|| current.settings.fingerprint.as_ref().and_then(|saved| saved.seed));
            config.ensure_seed();
        }
        if current.settings.connection != settings.connection || current.settings.fingerprint != settings.fingerprint {
            self.chromium.refresh();
            self.reconcile(&mut current)?;
            if !matches!(current.status, Status::Closed | Status::Crashed) {
                return Err("Close this workspace before changing its connection or fingerprint.".into());
            }
        }
        let db = self.db()?;
        if db.execute("UPDATE workspaces SET name=?3,start_url=?4,settings=?5 WHERE account_id=?1 AND id=?2 AND trashed_at IS NULL",
            params![account, id, catalog::name(name)?, settings.launch_urls.first(), catalog::encode(&settings)?],
        ).map_err(db_error)? != 1 {
            return Err("Workspace no longer exists.".into());
        }
        Ok(())
    }

    pub fn workspace_route(&self, account: &str, id: &str) -> Result<crate::runtime::Route, String> {
        let workspace = self.rows(account, Some(id))?.pop()
            .filter(|workspace| workspace.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        crate::runtime::Route::resolve(self.db()?, account, &workspace.settings.connection)
    }

    pub fn export_sync_metadata(&mut self, account: &str, id: &str) -> Result<crate::sync::Metadata, String> {
        self.require_account(account)?;
        let mut row = self.rows(account, Some(id))?.pop().filter(|row| row.trashed_at.is_none())
            .ok_or("Workspace no longer exists.")?;
        self.chromium.refresh();
        self.reconcile(&mut row)?;
        if !matches!(row.status, Status::Closed | Status::Crashed) {
            return Err("Close the workspace before snapshotting its metadata.".into());
        }
        let bookmarks = row.settings.bookmark_ids.iter()
            .map(|id| catalog::item(self.db()?, account, ItemKind::Bookmark, id))
            .collect::<Result<_, _>>()?;
        Ok(crate::sync::Metadata {
            version: 1, name: row.name, icon: row.settings.icon, notes: row.settings.notes,
            launch_urls: row.settings.launch_urls, bookmarks,
            runtime: row.browser.and_then(|browser| browser.version.map(|version| crate::sync::RuntimeVersion {
                brand: browser.brand, version,
            })),
        })
    }

    pub fn import_sync_metadata(&mut self, account: &str, metadata: crate::sync::Metadata) -> Result<String, String> {
        self.require_account(account)?;
        if metadata.bookmarks.len() > 100 { return Err("Too many snapshot bookmarks.".into()); }
        let tx = self.db_mut()?.transaction().map_err(db_error)?;
        let mut bookmark_ids = Vec::with_capacity(metadata.bookmarks.len());
        for entry in metadata.bookmarks {
            let bookmark = catalog::Bookmark {
                name: catalog::name(&entry.name)?.into(),
                url: validate_url(&entry.url)?.ok_or("Snapshot bookmark is missing a URL.")?,
            };
            bookmark_ids.push(catalog::save_item(&tx, account, ItemKind::Bookmark, None, &bookmark)?);
        }
        let id = insert_workspace(&tx, account, &metadata.name, Settings {
            icon: metadata.icon, notes: metadata.notes, launch_urls: metadata.launch_urls, bookmark_ids,
            ..Settings::default()
        })?;
        tx.commit().map_err(db_error)?;
        Ok(id)
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
                crx::remove_workspace(self.root()?, id)?;
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
                let needs_record = workspace.browser.as_ref().is_none_or(|browser| browser.version.is_none())
                    || old_identity != Some(session.identity)
                    || transition.is_some_and(|t| t.status == Status::Starting);
                if matches!(status, Status::Running | Status::Background)
                    && needs_record
                    && self.chromium.verified(&directory, session.identity).is_ok()
                {
                    if let Ok(runtime) = Chromium::inspect(&session.executable).map(|runtime| self.identify_bundled(runtime)) {
                        if workspace.browser.as_ref().is_none_or(|browser| {
                            browser.brand == runtime.brand
                                && (browser.version.is_none() || browser.accepts(&runtime))
                        }) {
                            self.record_browser(&workspace.id, &runtime)?;
                            workspace.browser = Some(ProfileBrowser {
                                brand: runtime.brand,
                                path: runtime.path,
                                version: Some(runtime.version),
                            });
                        }
                    }
                }
                if ended {
                    if transition.is_some_and(|t| t.status == Status::Closing && session.window) {
                        workspace.detail = Some(
                            "Browser did not close. Close its windows manually or retry.".into(),
                        );
                    }
                    self.transitions.remove(&workspace.id);
                }
                exit_note = None;
                (
                    status,
                    if status == Status::Starting {
                        old_identity
                    } else {
                        Some(session.identity)
                    },
                )
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
        if let (Status::Running, Observation::Session(session)) = (status, observation) {
            let startup_check = self.children.get_mut(&workspace.id)
                .and_then(|children| children.iter_mut().find(|child| child.process.id() == session.identity.pid))
                .and_then(|child| child.startup_check.take());
            if let Some(route) = startup_check {
                if self.chromium.verified(&directory, session.identity).is_ok() {
                    match Chromium::launch(&session.executable, &directory, &[crate::startup::check_url()], false, &[], &route, false, &[]) {
                        Ok(process) => self.children.get_mut(&workspace.id).unwrap().push(ChildProcess {
                            process, initial: false, startup_check: None,
                        }),
                        Err(error) => workspace.detail = Some(error),
                    }
                }
            }
        }
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

    pub fn set_automation_cdp(&mut self, account: &str, id: &str, enabled: bool) -> Result<(), String> {
        self.require_account(account)?;
        let mut workspace = self.rows(account, Some(id))?.pop().filter(|row| row.trashed_at.is_none())
            .ok_or("Workspace unavailable.")?;
        self.chromium.refresh();
        self.reconcile(&mut workspace)?;
        if !matches!(workspace.status, Status::Closed | Status::Crashed) {
            return Err("Close the workspace before changing its CDP setting.".into());
        }
        self.db()?.execute("UPDATE workspaces SET automation_cdp=?1 WHERE id=?2 AND account_id=?3",
            params![enabled, id, account]).map_err(db_error)?;
        Ok(())
    }

    pub fn automation_list(&mut self, id: Option<&str>, include_cdp: bool) -> Result<Vec<serde_json::Value>, String> {
        let account = catalog::account(self.db()?)?;
        let mut rows = self.rows(&account, id)?;
        self.chromium.refresh();
        rows.retain(|row| row.trashed_at.is_none());
        let mut results = Vec::with_capacity(rows.len());
        for row in &mut rows {
            self.reconcile(row)?;
            let mut value = serde_json::json!({ "id": row.id, "name": row.name, "status": row.status,
                "cdpOptIn": row.automation_cdp });
            if include_cdp && row.automation_cdp && matches!(row.status, Status::Running | Status::Background) {
                if let Some(identity) = row.identity.filter(|identity| self.debug_launches.get(&row.id) == Some(&identity.pid)
                    && self.children.get(&row.id).is_some_and(|children| children.iter().any(|child| child.process.id() == identity.pid))) {
                    let directory = checked_directory(self.root()?, &self.directory(&row.id)?)?;
                    self.chromium.verified(&directory, identity)?;
                    let active_port = directory.join("DevToolsActivePort");
                    if fs::symlink_metadata(&active_port).is_ok_and(|meta| meta.is_file() && meta.len() <= 256) {
                        if let Some(port) = fs::read_to_string(active_port).ok()
                            .and_then(|text| text.lines().next()?.parse::<std::num::NonZeroU16>().ok()) {
                            value["cdpEndpoint"] = serde_json::json!(format!("http://127.0.0.1:{port}"));
                        }
                    }
                }
            }
            results.push(value);
        }
        Ok(results)
    }

    pub fn automation_perform(&mut self, id: &str, operation: Operation) -> Result<(), String> {
        self.perform_inner(id, operation, None, true)
    }

    fn record_browser(&self, id: &str, runtime: &RuntimeInfo) -> Result<(), String> {
        self.db()?.execute(
            "UPDATE workspaces SET browser_brand=?1, browser_path=?2, browser_version=?3 WHERE id=?4",
            params![runtime.brand, runtime.path.to_str().ok_or("The browser path is not valid Unicode.")?, runtime.version, id],
        ).map_err(db_error)?;
        Ok(())
    }

    fn identify_bundled(&self, runtime: RuntimeInfo) -> RuntimeInfo {
        self.bundled_runtimes()
            .find(|bundled| bundled.path == runtime.path)
            .unwrap_or(runtime)
    }

    fn bundled_runtimes(&self) -> impl Iterator<Item = RuntimeInfo> + '_ {
        [
            self.fingerprint_path.as_deref().map(Chromium::inspect_fingerprint),
            self.bundled_path.as_deref().map(Chromium::inspect_bundled),
        ].into_iter().flatten().filter_map(Result::ok)
    }

    fn profile_runtime(&self, workspace: &Workspace, directory: &Path) -> Result<RuntimeInfo, String> {
        let selected = self.runtime.as_ref().map_err(Clone::clone).and_then(|cached| {
            Chromium::inspect(&cached.path).map(|runtime| self.identify_bundled(runtime))
        });
        if let Some(browser) = &workspace.browser {
            if browser.version.is_none() {
                return Err(browser.unavailable());
            }
            return Chromium::inspect(&browser.path)
                .ok()
                .map(|runtime| self.identify_bundled(runtime))
                .filter(|runtime| browser.accepts(runtime))
                .map(|runtime| match &selected {
                    Ok(selected) if selected.path == runtime.path => selected.clone(),
                    _ => self.identify_bundled(runtime),
                })
                .or_else(|| selected.ok().filter(|runtime| browser.accepts(runtime)))
                .or_else(|| self.bundled_runtimes().find(|runtime| browser.accepts(runtime)))
                .or_else(|| Chromium::discover_matching(|runtime| browser.accepts(runtime)))
                .ok_or_else(|| browser.unavailable());
        }
        if !profile_is_fresh(directory)? {
            return Err("This existing profile predates browser tracking. Slide cannot verify its browser and will not risk its saved sessions. Keep the original profile and clone its settings into a new workspace, or restore a pre-upgrade backup with the original Slide and browser.".into());
        }
        selected
    }

    fn perform_with_url(
        &mut self,
        id: &str,
        operation: Operation,
        url: Option<String>,
    ) -> Result<(), String> {
        self.perform_inner(id, operation, url, false)
    }

    fn perform_inner(&mut self, id: &str, operation: Operation, url: Option<String>, automation: bool) -> Result<(), String> {
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
                let (runtime, should_seed) = if active {
                    let Observation::Session(session) = self.chromium.observe(&directory) else {
                        return Err("The browser changed. Retry opening the workspace.".into());
                    };
                    self.chromium.verified(&directory, session.identity)?;
                    let runtime = self.identify_bundled(Chromium::inspect(&session.executable)?);
                    if let Some(browser) = &workspace.browser {
                        if !browser.accepts(&runtime) {
                            return Err(browser.unavailable());
                        }
                    }
                    (runtime, false)
                } else {
                    let runtime = self.profile_runtime(&workspace, &directory)?;
                    let should_seed = runtime.bundled;
                    (runtime, should_seed)
                };
                let route = crate::runtime::Route::resolve(self.db()?, &account, &workspace.settings.connection)?;
                if let crate::runtime::Route::Proxy(proxy) = &route {
                    if !runtime.supports_required_http_proxy() {
                        return Err("Required HTTP proxy launch is only enabled for bundled Chromium on macOS arm64. No direct fallback was attempted.".into());
                    }
                    proxy.require_reachable()?;
                }
                self.catalog()?
                    .db
                    .execute("UPDATE workspaces SET exit_note = NULL WHERE id = ?1", [id])
                    .map_err(db_error)?;
                let urls = url.as_ref().map_or(
                    workspace.settings.launch_urls.as_slice(),
                    std::slice::from_ref,
                );
                let extensions = if should_seed { self.approved_extensions(&account, &workspace)? } else { Vec::new() };
                let debugging = automation && workspace.automation_cdp && !active;
                if debugging && !runtime.bundled {
                    return Err("CDP is available only with bundled Chromium.".into());
                }
                if !active {
                    self.debug_launches.remove(id);
                    if debugging {
                        match fs::remove_file(directory.join("DevToolsActivePort")) {
                            Ok(()) => (),
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                            Err(_) => return Err("Could not clear the previous CDP endpoint.".into()),
                        }
                    }
                }
                let (urls, defer_check) = crate::startup::launch_tabs(urls, !automation && !active && url.is_none());
                let flags = if active {
                    Vec::new()
                } else {
                    let config = workspace.settings.fingerprint.get_or_insert_with(Default::default);
                    let added_seed = config.ensure_seed();
                    let flags = config.flags(runtime.brand == "fingerprint-chromium");
                    if added_seed {
                        self.db()?.execute(
                            "UPDATE workspaces SET settings = ?1 WHERE id = ?2",
                            params![catalog::encode(&workspace.settings)?, id],
                        ).map_err(db_error)?;
                    }
                    flags
                };
                let seeded = should_seed && seed_profile(&directory, &workspace.name)?;
                let child = Chromium::launch(&runtime.path, &directory, &urls, reopen, &extensions, &route, debugging, &flags).map_err(|error| {
                    if seeded {
                        remove_profile_seed(&directory.join("Default"));
                    }
                    error
                })?;
                if debugging {
                    self.debug_launches.insert(id.into(), child.id());
                }
                self.children
                    .entry(id.into())
                    .or_default()
                    .push(ChildProcess {
                        process: child,
                        initial: !active,
                        startup_check: defer_check.then_some(route),
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

fn crx_id(entry: &catalog::Extension) -> &str {
    if entry.source == "fixture" { crx::FIXTURE_ID } else { &entry.browser_id }
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
    let mut settings = catalog::settings(db, account, settings)?;
    settings.fingerprint.get_or_insert_with(Default::default).ensure_seed();
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

fn profile_is_fresh(directory: &Path) -> Result<bool, String> {
    let inspect = || -> std::io::Result<bool> {
        let mut root = fs::read_dir(directory)?;
        let Some(profile) = root.next().transpose()? else {
            return Ok(true);
        };
        if profile.file_name() != "Default"
            || !profile.file_type()?.is_dir()
            || root.next().transpose()?.is_some()
        {
            return Ok(false);
        }
        let mut defaults = fs::read_dir(profile.path())?;
        let Some(preferences) = defaults.next().transpose()? else {
            return Ok(false);
        };
        if preferences.file_name() != "Preferences"
            || !preferences.file_type()?.is_file()
            || defaults.next().transpose()?.is_some()
        {
            return Ok(false);
        }

        let mut bytes = Vec::new();
        File::open(preferences.path())?
            .take(MAX_PROFILE_SEED_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PROFILE_SEED_BYTES {
            return Ok(false);
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Ok(false);
        };
        let Some(name) = value["profile"]["name"].as_str() else {
            return Ok(false);
        };
        Ok(bytes == profile_seed(name).to_string().as_bytes())
    };
    inspect().map_err(|error| format!("Could not inspect the workspace directory: {error}"))
}

fn profile_seed(name: &str) -> serde_json::Value {
    serde_json::json!({ "profile": { "name": name } })
}

fn seed_profile(directory: &Path, name: &str) -> Result<bool, String> {
    let first_run = (|| -> std::io::Result<bool> {
        if fs::read_dir(directory)?.next().transpose()?.is_some() {
            return Ok(false);
        }
        let profile = directory.join("Default");
        fs::create_dir(&profile)?;
        let result = (|| -> std::io::Result<()> {
            let file = OpenOptions::new().write(true).create_new(true).open(profile.join("Preferences"))?;
            serde_json::to_writer(file, &profile_seed(name))?;
            Ok(())
        })();
        if result.is_err() {
            remove_profile_seed(&profile);
        }
        result.map(|()| true)
    })();
    first_run.map_err(|error| format!("Could not initialize the Chromium profile: {error}"))
}

fn remove_profile_seed(profile: &Path) {
    let _ = fs::remove_file(profile.join("Preferences"));
    let _ = fs::remove_dir(profile);
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
    fn bundled_profile_seeding_never_changes_existing_preferences() {
        let test = tempfile::tempdir().unwrap();
        let directory = test.path();
        seed_profile(directory, "Creator").unwrap();
        let preferences = directory.join("Default/Preferences");
        assert_eq!(fs::read_to_string(&preferences).unwrap(), r#"{"profile":{"name":"Creator"}}"#);
        fs::write(&preferences, "{}").unwrap();
        seed_profile(directory, "Viewer").unwrap();
        assert_eq!(fs::read_to_string(preferences).unwrap(), "{}");
    }

    #[test]
    fn untracked_nonempty_profile_is_not_opened_in_a_new_browser() {
        let test = tempfile::tempdir().unwrap();
        let db = Connection::open(test.path().join("catalog.sqlite3")).unwrap();
        db.execute_batch(include_str!("../migrations/001_workspaces.sql")).unwrap();
        db.execute_batch(include_str!("../migrations/002_catalog.sql")).unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        let id = insert_workspace(&db, "local", "Legacy", Settings::default()).unwrap();
        drop(db);
        let directory = prepare_directory(test.path(), &test.path().join("profiles").join(&id))
            .unwrap();
        let session = directory.join("synthetic-session");
        fs::write(&session, "Creator").unwrap();

        let mut manager = Manager::new(test.path().into()).unwrap();
        let error = manager.perform(&id, Operation::Open).unwrap_err();
        assert!(error.contains("predates browser tracking"));
        assert_eq!(fs::read_to_string(session).unwrap(), "Creator");
    }

    #[cfg(target_os = "macos")]
    fn mac_browser(root: &Path, brand: &str, identifier: &str, version: &str) -> PathBuf {
        let app = root.join(format!("{brand}.app"));
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        let plist = plist::Dictionary::from_iter([
            ("CFBundleIdentifier", plist::Value::String(identifier.into())),
            ("CFBundleExecutable", plist::Value::String(brand.into())),
            ("CFBundleShortVersionString", plist::Value::String(version.into())),
        ]);
        plist::Value::Dictionary(plist)
            .to_file_xml(app.join("Contents/Info.plist"))
            .unwrap();
        let arch: u32 = if cfg!(target_arch = "aarch64") { 0x0100000c } else { 0x01000007 };
        fs::write(
            app.join("Contents/MacOS").join(brand),
            [0xfeedfacfu32.to_be_bytes(), arch.to_be_bytes()].concat(),
        )
        .unwrap();
        app
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn legacy_fingerprint_seed_is_persisted_on_edit_and_cold_launch() {
        let test = tempfile::tempdir().unwrap();
        let patched = mac_browser(&test.path().join("patched"), "Chromium",
            "org.chromium.Chromium", "148.0.7778.215");
        let mut manager = Manager::with_bundles(test.path().into(), None, Some(patched)).unwrap();
        let settings = Settings {
            fingerprint: Some(crate::fingerprint::Config {
                hardware_concurrency: Some(7), ..Default::default()
            }),
            ..Default::default()
        };
        for edited in [true, false] {
            let id = insert_workspace(manager.db().unwrap(), "local", "Legacy", Settings::default()).unwrap();
            manager.db().unwrap().execute("UPDATE workspaces SET settings = ?1 WHERE id = ?2",
                params![catalog::encode(&if edited { Settings::default() } else { settings.clone() }).unwrap(), id],
            ).unwrap();
            if edited {
                manager.edit_workspace("local", &id, "Legacy", settings.clone()).unwrap();
            } else {
                let error = manager.perform(&id, Operation::Open).unwrap_err();
                assert!(error.contains("Could not launch the browser"), "{error}");
            }
            let stored = || manager.rows("local", Some(&id)).unwrap().pop().unwrap().settings.fingerprint.unwrap();
            let config = stored();
            let seed = config.seed.unwrap();
            assert!(config.flags(true).contains(&format!("--fingerprint={seed}")));
            assert!(config.flags(true).contains(&"--fingerprint-hardware-concurrency=7".into()));
            manager.edit_workspace("local", &id, "Legacy", settings.clone()).unwrap();
            assert_eq!(manager.rows("local", Some(&id)).unwrap()[0].settings.fingerprint.as_ref().unwrap().seed, Some(seed));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fingerprint_is_default_but_does_not_replace_a_recorded_runtime() {
        let test = tempfile::tempdir().unwrap();
        let stock = mac_browser(&test.path().join("stock"), "Chromium",
            "io.ungoogled-software.ungoogled-chromium", "157.0.8103.78");
        let patched = mac_browser(&test.path().join("patched"), "Chromium",
            "org.chromium.Chromium", "148.0.7778.215");
        let manager = Manager::with_bundles(test.path().into(), Some(stock.clone()), Some(patched.clone())).unwrap();
        let a = insert_workspace(manager.db().unwrap(), "local", "A", Settings::default()).unwrap();
        let b = insert_workspace(manager.db().unwrap(), "local", "B", Settings::default()).unwrap();
        manager.record_browser(&b, &Chromium::inspect(&stock).unwrap()).unwrap();
        let a_workspace = manager.rows("local", Some(&a)).unwrap().pop().unwrap();
        let b_workspace = manager.rows("local", Some(&b)).unwrap().pop().unwrap();
        let a_directory = manager.directory(&a).unwrap();
        fs::create_dir_all(&a_directory).unwrap();
        let a_runtime = manager.profile_runtime(&a_workspace, &a_directory).unwrap();
        let b_runtime = manager.profile_runtime(&b_workspace, &manager.directory(&b).unwrap()).unwrap();
        assert_eq!((&*a_runtime.brand, &*a_runtime.version), ("fingerprint-chromium", "148.0.7778.215"));
        assert_eq!((&*b_runtime.brand, &*b_runtime.version), ("ungoogled-chromium", "157.0.8103.78"));
        assert_ne!(a_workspace.settings.fingerprint.unwrap().seed, b_workspace.settings.fingerprint.unwrap().seed);
        manager.record_browser(&a, &a_runtime).unwrap();
        fs::remove_dir_all(patched).unwrap();
        let a_workspace = manager.rows("local", Some(&a)).unwrap().pop().unwrap();
        assert!(manager.profile_runtime(&a_workspace, &manager.directory(&a).unwrap()).unwrap_err().contains("different brand"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fresh_profile_uses_ungoogled_bundle_without_switching_existing_chromium_profile() {
        let test = tempfile::tempdir().unwrap();
        let original = mac_browser(test.path(), "Chromium", "org.chromium.Chromium", "157.0.8103.78");
        let bundled = mac_browser(
            &test.path().join("bundle"),
            "Chromium",
            "io.ungoogled-software.ungoogled-chromium",
            "157.0.8103.78",
        );
        let manager = Manager::with_bundle(test.path().into(), Some(bundled)).unwrap();
        let id = insert_workspace(manager.db().unwrap(), "local", "Creator", Settings::default())
            .unwrap();
        let directory = prepare_directory(test.path(), &manager.directory(&id).unwrap()).unwrap();
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        let runtime = manager.profile_runtime(&workspace, &directory).unwrap();
        assert!(runtime.bundled);
        assert_eq!(runtime.brand, "ungoogled-chromium");

        let original = Chromium::inspect(&original).unwrap();
        manager.record_browser(&id, &original).unwrap();
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        assert_eq!(
            manager.profile_runtime(&workspace, &directory).unwrap().path,
            original.path
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fixture_crx_requires_workspace_approval_for_install_and_update() {
        let test = tempfile::tempdir().unwrap();
        let browser = mac_browser(test.path(), "Chromium", "io.ungoogled-software.ungoogled-chromium", "157.0.8103.78");
        let mut manager = Manager::with_bundle(test.path().into(), Some(browser)).unwrap();
        let extension = crate::extensions::stage(manager.db().unwrap(), test.path(), "local", None, "fixture", None, false).unwrap();
        let settings = Settings { extension_ids: vec![extension.clone()], ..Default::default() };
        let id = insert_workspace(manager.db().unwrap(), "local", "Creator", settings.clone()).unwrap();
        let other = insert_workspace(manager.db().unwrap(), "local", "Viewer", settings).unwrap();
        let (token, review) = manager.review_crx("local", &id, &extension, None).unwrap();
        assert_eq!(review.version, "1.0.0");
        assert!(crx::installed(manager.db().unwrap(), &id, &extension).unwrap().is_none());
        manager.approve_crx("local", token).unwrap();
        assert!(manager.approve_crx("local", token).is_err());
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        let path = manager.approved_extensions("local", &workspace).unwrap().pop().unwrap();
        assert!(path.join("badge.js").is_file());
        let other_row = manager.rows("local", Some(&other)).unwrap().pop().unwrap();
        assert!(manager.approved_extensions("local", &other_row).unwrap().is_empty());

        crate::extensions::stage(manager.db().unwrap(), test.path(), "local", Some(extension.clone()), "fixture", None, true).unwrap();
        let options = manager.extension_install_options("local", &id, &extension).unwrap();
        assert_eq!(options.installed_version.as_deref(), Some("1.0.0"));
        assert!(options.update_available);
        let (token, review) = manager.review_crx("local", &id, &extension, None).unwrap();
        assert_eq!(review.version, "1.0.1");
        manager.crx_review.as_mut().unwrap().4.minimum_chrome_version = Some("158".into());
        let error = manager.approve_crx("local", token).unwrap_err();
        assert!(error.contains("requires Chromium 158 or newer"));
        assert!(path.join("badge.js").is_file());
        assert_eq!(crx::installed(manager.db().unwrap(), &id, &extension).unwrap().unwrap().version, "1.0.0");

        manager.db().unwrap().execute_batch(
            "CREATE TRIGGER reject_crx_update BEFORE UPDATE ON workspace_crx
             BEGIN SELECT RAISE(ABORT, 'test: replacement rejected'); END;"
        ).unwrap();
        let (token, _) = manager.review_crx("local", &id, &extension, None).unwrap();
        assert!(manager.approve_crx("local", token).unwrap_err().contains("test: replacement rejected"));
        assert!(path.join("badge.js").is_file());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(crx::installed(manager.db().unwrap(), &id, &extension).unwrap().unwrap().version, "1.0.0");
        manager.db().unwrap().execute_batch("DROP TRIGGER reject_crx_update;").unwrap();

        let (token, _) = manager.review_crx("local", &id, &extension, None).unwrap();
        manager.crx_review.as_mut().unwrap().4.minimum_chrome_version = Some("157.0".into());
        assert!(path.is_dir());
        manager.approve_crx("local", token).unwrap();
        assert!(!path.exists());
        assert!(!manager.extension_install_options("local", &id, &extension).unwrap().update_available);

        let directory = manager.directory(&id).unwrap();
        let runtime = manager.profile_runtime(&workspace, &directory).unwrap();
        manager.record_browser(&id, &runtime).unwrap();
        let chrome = mac_browser(test.path(), "Google Chrome", "com.google.Chrome", "158.0.8127.36");
        manager.choose_runtime(chrome).unwrap();
        assert!(manager.extension_install_options("local", &id, &extension).unwrap().bundled);
        assert!(!manager.extension_install_options("local", &other, &extension).unwrap().bundled);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn removing_catalog_extension_releases_approvals_and_allows_reapproval() {
        let test = tempfile::tempdir().unwrap();
        let browser = mac_browser(test.path(), "Chromium", "io.ungoogled-software.ungoogled-chromium", "157.0.8103.78");
        let mut manager = Manager::with_bundle(test.path().into(), Some(browser)).unwrap();
        let extension = crate::extensions::stage(manager.db().unwrap(), test.path(), "local", None, "fixture", None, false).unwrap();
        let settings = Settings { extension_ids: vec![extension.clone()], ..Default::default() };
        let first = insert_workspace(manager.db().unwrap(), "local", "Creator", settings.clone()).unwrap();
        let second = insert_workspace(manager.db().unwrap(), "local", "Viewer", settings).unwrap();
        let paths: Vec<_> = [&first, &second].into_iter().map(|workspace| {
            let (token, _) = manager.review_crx("local", workspace, &extension, None).unwrap();
            manager.approve_crx("local", token).unwrap();
            let install = crx::installed(manager.db().unwrap(), workspace, &extension).unwrap().unwrap();
            crx::approved_path(test.path(), workspace, &install).unwrap()
        }).collect();
        manager.trash_workspace("local", &second, TrashAction::Trash).unwrap();

        manager.transitions.insert(first.clone(), Transition {
            status: Status::Starting,
            since: Instant::now(),
        });
        let error = manager.remove_item("local", ItemKind::Extension, &extension).unwrap_err();
        assert!(error.contains("Close the browser for “Creator”"));
        assert!(paths.iter().all(|path| path.join("badge.js").is_file()));
        assert!(crx::installed(manager.db().unwrap(), &first, &extension).unwrap().is_some());
        assert!(catalog::item::<catalog::Extension>(manager.db().unwrap(), "local", ItemKind::Extension, &extension).is_ok());
        manager.transitions.remove(&first);

        manager.remove_item("local", ItemKind::Extension, &extension).unwrap();
        for (workspace, path) in [first.as_str(), second.as_str()].into_iter().zip(paths) {
            assert!(crx::installed(manager.db().unwrap(), workspace, &extension).unwrap().is_none());
            assert!(!path.exists());
            assert!(manager.rows("local", Some(workspace)).unwrap()[0].settings.extension_ids.is_empty());
        }

        let new_entry = crate::extensions::stage(manager.db().unwrap(), test.path(), "local", None, "fixture", None, false).unwrap();
        assert_ne!(new_entry, extension);
        let settings = Settings { extension_ids: vec![new_entry.clone()], ..Default::default() };
        manager.db().unwrap().execute(
            "UPDATE workspaces SET settings=?2 WHERE id=?1",
            params![first, catalog::encode(&settings).unwrap()],
        ).unwrap();
        let (token, _) = manager.review_crx("local", &first, &new_entry, None).unwrap();
        manager.approve_crx("local", token).unwrap();
        assert!(crx::installed(manager.db().unwrap(), &first, &new_entry).unwrap().is_some());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn required_proxy_survives_manager_restart_and_blocks_launch_when_unreachable() {
        let test = tempfile::tempdir().unwrap();
        let browser = mac_browser(test.path(), "Chromium", "io.ungoogled-software.ungoogled-chromium", "157.0.8103.78");
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let manager = Manager::with_bundle(test.path().into(), Some(browser.clone())).unwrap();
        let saved = crate::connections::save(manager.db().unwrap(), "local", "Fixture proxy", &endpoint).unwrap();
        let connection = crate::connections::Selection::Saved { id: saved };
        let id = insert_workspace(manager.db().unwrap(), "local", "Proxied", Settings {
            connection: connection.clone(), ..Default::default()
        }).unwrap();
        drop(manager);
        drop(listener);

        let mut manager = Manager::with_bundle(test.path().into(), Some(browser)).unwrap();
        assert_eq!(manager.rows("local", Some(&id)).unwrap()[0].settings.connection, connection);
        assert!(manager.perform(&id, Operation::Open).unwrap_err().contains("No direct fallback"));
        assert!(!manager.children.contains_key(&id));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn legacy_upgrade_defaults_to_bundle_unless_a_browser_was_chosen() {
        for chosen in [false, true] {
            let test = tempfile::tempdir().unwrap();
            let bundled = mac_browser(test.path(), "Chromium", "org.chromium.Chromium", "160.0.8182.0");
            let chrome = mac_browser(test.path(), "Google Chrome", "com.google.Chrome", "158.0.8127.36");
            let chrome = Chromium::inspect(&chrome).unwrap();
            let db = Connection::open(test.path().join("catalog.sqlite3")).unwrap();
            db.execute_batch(include_str!("../migrations/001_workspaces.sql")).unwrap();
            db.execute_batch(include_str!("../migrations/002_catalog.sql")).unwrap();
            db.pragma_update(None, "user_version", 2).unwrap();
            if chosen {
                db.execute(
                    "INSERT INTO runtime_settings (singleton, path) VALUES (1, ?1)",
                    [chrome.path.to_str().unwrap()],
                )
                .unwrap();
            }
            drop(db);

            let manager = Manager::with_bundle(test.path().into(), Some(bundled)).unwrap();
            let runtime = manager.runtime.as_ref().unwrap();
            if chosen {
                assert_eq!(runtime.path, chrome.path);
            } else {
                assert!(runtime.bundled);
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn persisted_chrome_profile_keeps_chrome_when_bundled_is_selected() {
        let test = tempfile::tempdir().unwrap();
        let bundled = mac_browser(test.path(), "Chromium", "org.chromium.Chromium", "160.0.8182.0");
        let chrome = mac_browser(test.path(), "Google Chrome", "com.google.Chrome", "154.0.8037.93");
        let manager = Manager::with_bundle(test.path().into(), Some(bundled.clone())).unwrap();
        let id = insert_workspace(manager.db().unwrap(), "local", "Creator", Settings::default())
            .unwrap();
        let directory = prepare_directory(test.path(), &manager.directory(&id).unwrap()).unwrap();
        let chrome = Chromium::inspect(&chrome).unwrap();
        manager.record_browser(&id, &chrome).unwrap();
        drop(manager);

        let manager = Manager::with_bundle(test.path().into(), Some(bundled)).unwrap();
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        assert!(manager.runtime.as_ref().unwrap().bundled);
        assert_eq!(manager.profile_runtime(&workspace, &directory).unwrap().path, chrome.path);

        let clone = manager.clone_workspace("local", &id, "Viewer").unwrap();
        let workspace = manager.rows("local", Some(&clone)).unwrap().pop().unwrap();
        let directory = prepare_directory(test.path(), &manager.directory(&clone).unwrap()).unwrap();
        assert!(manager.profile_runtime(&workspace, &directory).unwrap().bundled);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_browser_only_falls_back_to_same_or_newer_profile_version() {
        let test = tempfile::tempdir().unwrap();
        let manager = Manager::new(test.path().into()).unwrap();
        let id = insert_workspace(manager.db().unwrap(), "local", "Creator", Settings::default())
            .unwrap();
        let directory = prepare_directory(test.path(), &manager.directory(&id).unwrap()).unwrap();
        let version = "4000000000.0.10.0";
        let previous = mac_browser(test.path(), "Chromium", "org.chromium.Chromium", version);
        manager.record_browser(&id, &Chromium::inspect(&previous).unwrap()).unwrap();
        fs::rename(&previous, test.path().join("Removed.app")).unwrap();
        drop(manager);

        let bundle_root = test.path().join("bundle");
        let bundled = mac_browser(&bundle_root, "Chromium", "org.chromium.Chromium", "4000000000.0.9.0");
        let manager = Manager::with_bundle(test.path().into(), Some(bundled)).unwrap();
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        let error = manager.profile_runtime(&workspace, &directory).unwrap_err();
        assert!(error.contains(version));

        for available in [version, "4000000000.0.11.0"] {
            mac_browser(&bundle_root, "Chromium", "org.chromium.Chromium", available);
            let runtime = manager.profile_runtime(&workspace, &directory).unwrap();
            assert_eq!(runtime.version, available);
            assert!(runtime.bundled);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn failed_launch_keeps_the_last_verified_browser_version() {
        let test = tempfile::tempdir().unwrap();
        let previous = mac_browser(test.path(), "Chromium", "org.chromium.Chromium", "160.0.8182.0");
        let bundled = mac_browser(
            &test.path().join("bundle"),
            "Chromium",
            "org.chromium.Chromium",
            "161.0.8200.0",
        );
        let mut manager = Manager::with_bundle(test.path().into(), Some(bundled)).unwrap();
        let id = insert_workspace(manager.db().unwrap(), "local", "Creator", Settings::default())
            .unwrap();
        let runtime = Chromium::inspect(&previous).unwrap();
        manager.record_browser(&id, &runtime).unwrap();
        let removed = test.path().join("Removed.app");
        fs::rename(&previous, &removed).unwrap();

        let error = manager.perform(&id, Operation::Open).unwrap_err();
        assert!(error.contains("Could not launch the browser"));

        fs::rename(&removed, &previous).unwrap();
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        let directory = manager.directory(&id).unwrap();
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        let recovered = manager.profile_runtime(&workspace, &directory).unwrap();
        assert_eq!(recovered.version, runtime.version);
        assert_eq!(recovered.path, runtime.path);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn untracked_seed_only_profile_can_retry_but_browser_state_blocks_it() {
        let test = tempfile::tempdir().unwrap();
        let bundled = mac_browser(test.path(), "Chromium", "org.chromium.Chromium", "160.0.8182.0");
        let manager = Manager::with_bundle(test.path().into(), Some(bundled)).unwrap();
        let id = insert_workspace(manager.db().unwrap(), "local", "Creator", Settings::default())
            .unwrap();
        let directory = prepare_directory(test.path(), &manager.directory(&id).unwrap()).unwrap();
        let workspace = manager.rows("local", Some(&id)).unwrap().pop().unwrap();
        assert!(workspace.browser.is_none());
        assert!(seed_profile(&directory, &workspace.name).unwrap());
        assert!(manager.profile_runtime(&workspace, &directory).unwrap().bundled);

        for state in ["Local State", "Default/Cookies"] {
            let state = directory.join(state);
            fs::write(&state, "browser data").unwrap();
            let error = manager.profile_runtime(&workspace, &directory).unwrap_err();
            assert!(error.contains("predates browser tracking"));
            fs::remove_file(state).unwrap();
        }
        fs::write(directory.join("Default/Preferences"), r#"{"profile":{"name":"Creator","created_by_version":"160.0.8182.0"}}"#).unwrap();
        assert!(manager.profile_runtime(&workspace, &directory).is_err());
    }

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
        let mut rows = manager.snapshot().unwrap().workspaces;
        let original_seed = rows[0].settings.fingerprint.as_ref().unwrap().seed;
        let clone_fingerprint = rows[1].settings.fingerprint.as_mut().unwrap();
        assert_ne!(original_seed, clone_fingerprint.seed);
        clone_fingerprint.seed = original_seed;
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
    fn snoozing_a_future_reminder_extends_its_deadline() {
        let test = tempfile::tempdir().unwrap();
        let mut manager = Manager::new(test.path().into()).unwrap();
        let due_at = catalog::now() + 3600;
        let id = catalog::create_task(
            manager.db_mut().unwrap(),
            "local",
            catalog::TaskDraft {
                title: "Check later".into(),
                description: "".into(),
                due_at,
                workspace_ids: vec![],
            },
        )
        .unwrap();
        let db = manager.db().unwrap();
        catalog::finish_task(db, "local", &id, true).unwrap();
        assert_eq!(catalog::list_tasks(db, "local").unwrap()[0].due_at, due_at + 600);
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
    #[cfg(unix)]
    fn catalog_releases_lock_even_with_a_duplicated_descriptor() {
        let test = tempfile::tempdir().unwrap();
        let manager = Manager::new(test.path().into()).unwrap();
        let duplicated = manager.catalog().unwrap()._lock.0.try_clone().unwrap();
        assert!(Manager::new(test.path().into()).is_err());
        drop(manager);
        let reopened = Manager::new(test.path().into()).unwrap();
        drop(duplicated);
        assert!(Manager::new(test.path().into()).is_err());
        drop(reopened);
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
                    startup_check: None,
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
