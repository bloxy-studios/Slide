use crate::runtime::{Chromium, Identity, Observation, RuntimeInfo, Session};
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
    status: Status,
    detail: Option<String>,
    #[serde(skip)]
    identity: Option<Identity>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    workspaces: Vec<Workspace>,
    runtime: Option<RuntimeInfo>,
    runtime_error: Option<String>,
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
        let version = db
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .map_err(db_error)?;
        match version {
            0 => {
                let tx = db.transaction().map_err(db_error)?;
                tx.execute_batch(include_str!("../migrations/001_workspaces.sql"))
                    .map_err(db_error)?;
                tx.pragma_update(None, "user_version", 1)
                    .map_err(db_error)?;
                tx.commit().map_err(db_error)?;
            }
            1 => {}
            _ => return Err(
                "The catalog belongs to a newer version. Install that version before opening it."
                    .into(),
            ),
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
        })
    }

    pub fn unavailable(error: String) -> Self {
        Self {
            catalog: Err(error),
            chromium: Chromium::new(),
            runtime: Err("Catalog unavailable".into()),
            children: HashMap::new(),
            transitions: HashMap::new(),
        }
    }

    fn catalog(&self) -> Result<&Catalog, String> {
        self.catalog.as_ref().map_err(Clone::clone)
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

    pub fn create(&mut self, name: &str, start_url: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err("Enter a workspace name between 1 and 100 characters.".into());
        }
        let start_url = validate_url(start_url)?;
        self.catalog()?
            .db
            .execute(
                "INSERT INTO workspaces (id, name, start_url) VALUES (?1, ?2, ?3)",
                params![Uuid::new_v4().to_string(), name, start_url],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        let rows = {
            let mut statement = self.catalog()?.db.prepare(
                "SELECT id, name, start_url, pid, started, exit_note FROM workspaces ORDER BY rowid",
            ).map_err(db_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(Workspace {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        start_url: row.get(2)?,
                        identity: row
                            .get::<_, Option<u32>>(3)?
                            .zip(row.get::<_, Option<u64>>(4)?)
                            .map(|(pid, started)| Identity { pid, started }),
                        status: Status::Closed,
                        detail: row.get(5)?,
                    })
                })
                .map_err(db_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db_error)?
        };
        self.chromium.refresh();
        let mut workspaces = Vec::with_capacity(rows.len());
        for mut workspace in rows {
            self.reconcile(&mut workspace)?;
            workspaces.push(workspace);
        }
        Ok(Snapshot {
            workspaces,
            runtime: self.runtime.as_ref().ok().cloned(),
            runtime_error: self.runtime.as_ref().err().cloned(),
        })
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
        let snapshot = self.snapshot()?;
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|row| row.id == id)
            .ok_or("Workspace no longer exists.")?;
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
                let executable = if reopen {
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
                let child = Chromium::launch(
                    &executable,
                    &directory,
                    workspace.start_url.as_deref(),
                    reopen,
                )?;
                self.children
                    .entry(id.into())
                    .or_default()
                    .push(ChildProcess {
                        process: child,
                        initial: !reopen,
                    });
                self.transitions.insert(
                    id.into(),
                    Transition {
                        status: Status::Starting,
                        since: Instant::now(),
                    },
                );
                Ok(())
            }
        }
    }
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

fn db_error(error: rusqlite::Error) -> String {
    format!("Could not read or save the workspace catalog: {error}. Retry or check local storage.")
}

fn validate_url(input: &str) -> Result<Option<String>, String> {
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
