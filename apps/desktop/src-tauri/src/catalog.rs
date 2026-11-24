use crate::manager::{db_error, validate_url};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashSet,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub icon: String,
    pub notes: String,
    pub launch_urls: Vec<String>,
    pub folder_id: Option<String>,
    pub tag_ids: Vec<String>,
    pub pinned_tag_id: Option<String>,
    pub bookmark_ids: Vec<String>,
    pub extension_ids: Vec<String>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    pub search: String,
    pub folder: String,
    pub tags: Vec<String>,
    pub status: String,
    pub sort: String,
    pub page_size: u16,
    pub columns: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogView {
    pub active_account: String,
    pub accounts: Vec<Item<Label>>,
    pub folders: Vec<Item<Label>>,
    pub tags: Vec<Item<Tag>>,
    pub bookmarks: Vec<Item<Bookmark>>,
    pub templates: Vec<Item<Template>>,
    pub extensions: Vec<Item<Extension>>,
    pub tasks: Vec<Task>,
    pub default_template: Option<String>,
    pub view: View,
}

#[derive(Serialize)]
pub struct Item<T> {
    pub id: String,
    #[serde(flatten)]
    pub data: T,
}

#[derive(Serialize, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub settings: Settings,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    pub name: String,
    pub browser_id: String,
    pub source: String,
    pub version: String,
    pub permissions: Vec<String>,
    pub update_mode: String,
    pub install_url: Option<String>,
    pub unpacked_path: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    Folder,
    Tag,
    Bookmark,
    Template,
    Extension,
}

impl ItemKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Tag => "tag",
            Self::Bookmark => "bookmark",
            Self::Template => "template",
            Self::Extension => "extension",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: String,
    pub due_at: i64,
    pub fired_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub workspace_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub due_at: i64,
    pub workspace_ids: Vec<String>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| format!("Could not encode catalog data: {e}"))
}

pub fn decode<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| format!("Could not read catalog data: {e}"))
}

pub fn name(input: &str) -> Result<&str> {
    let input = input.trim();
    if input.is_empty() || input.chars().count() > 100 || input.chars().any(char::is_control) {
        return Err("Enter a name of 1–100 characters without control characters.".into());
    }
    Ok(input)
}

pub fn account(db: &Connection) -> Result<String> {
    db.query_row(
        "SELECT id FROM current_account WHERE singleton = 1",
        [],
        |row| row.get(0),
    )
    .map_err(db_error)
}

pub fn list_accounts(db: &Connection) -> Result<Vec<Item<Label>>> {
    let mut query = db
        .prepare("SELECT id, name FROM local_accounts ORDER BY rowid")
        .map_err(db_error)?;
    let rows = query
        .query_map([], |row| {
            Ok(Item {
                id: row.get(0)?,
                data: Label { name: row.get(1)? },
            })
        })
        .map_err(db_error)?;
    rows.collect::<rusqlite::Result<_>>().map_err(db_error)
}

pub fn add_account(db: &mut Connection, input: &str) -> Result<String> {
    let name = name(input)?;
    if list_accounts(db)?
        .iter()
        .any(|item| item.data.name.to_lowercase() == name.to_lowercase())
    {
        return Err("A local account already has this name.".into());
    }
    let id = Uuid::new_v4().to_string();
    let tx = db.transaction().map_err(db_error)?;
    tx.execute(
        "INSERT INTO local_accounts VALUES (?1, ?2)",
        params![id, name],
    )
    .map_err(db_error)?;
    tx.execute(
        "INSERT INTO account_options (account_id) VALUES (?1)",
        [&id],
    )
    .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok(id)
}

pub fn has_workspace(db: &Connection, account: &str, id: &str, trashed: bool) -> Result<bool> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE account_id=?1 AND id=?2 AND (trashed_at IS NOT NULL)=?3)", params![account, id, trashed], |row| row.get(0)).map_err(db_error)
}

pub fn item<T: DeserializeOwned>(
    db: &Connection,
    account: &str,
    kind: ItemKind,
    id: &str,
) -> Result<T> {
    let value: Option<String> = db
        .query_row(
            "SELECT data FROM catalog_items WHERE account_id=?1 AND kind=?2 AND id=?3",
            params![account, kind.key(), id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    decode(&value.ok_or_else(|| {
        format!(
            "This {} no longer exists in the current account.",
            kind.key()
        )
    })?)
}

pub fn items<T: DeserializeOwned>(
    db: &Connection,
    account: &str,
    kind: ItemKind,
) -> Result<Vec<Item<T>>> {
    let mut query = db
        .prepare("SELECT id,data FROM catalog_items WHERE account_id=?1 AND kind=?2 ORDER BY rowid")
        .map_err(db_error)?;
    let rows = query
        .query_map(params![account, kind.key()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?;
    rows.map(|row| {
        let (id, data) = row.map_err(db_error)?;
        Ok(Item {
            id,
            data: decode(&data)?,
        })
    })
    .collect()
}

pub fn save_item<T: Serialize>(
    db: &Connection,
    account: &str,
    kind: ItemKind,
    id: Option<String>,
    value: &T,
) -> Result<String> {
    let data = encode(value)?;
    if let Some(id) = id {
        let changed = db
            .execute(
                "UPDATE catalog_items SET data=?4 WHERE account_id=?1 AND kind=?2 AND id=?3",
                params![account, kind.key(), id, data],
            )
            .map_err(db_error)?;
        if changed != 1 {
            return Err("This catalog entry no longer exists.".into());
        }
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    db.execute(
        "INSERT INTO catalog_items VALUES (?1,?2,?3,?4)",
        params![account, kind.key(), id, data],
    )
    .map_err(db_error)?;
    Ok(id)
}

pub fn remove_item(db: &mut Connection, account: &str, kind: ItemKind, id: &str) -> Result<()> {
    let tx = db.transaction().map_err(db_error)?;
    if tx
        .execute(
            "DELETE FROM catalog_items WHERE account_id=?1 AND kind=?2 AND id=?3",
            params![account, kind.key(), id],
        )
        .map_err(db_error)?
        != 1
    {
        return Err("This catalog entry no longer exists.".into());
    }
    if matches!(kind, ItemKind::Template) {
        tx.execute("UPDATE account_options SET default_template=NULL WHERE account_id=?1 AND default_template=?2", params![account, id]).map_err(db_error)?;
    } else {
        let profiles: Vec<(String, String)> = {
            let mut query = tx
                .prepare("SELECT id,settings FROM workspaces WHERE account_id=?1")
                .map_err(db_error)?;
            let rows = query
                .query_map([account], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(db_error)?;
            rows.collect::<rusqlite::Result<_>>().map_err(db_error)?
        };
        for (profile, value) in profiles {
            let mut settings: Settings = decode(&value)?;
            if clear_reference(&mut settings, kind, id) {
                tx.execute(
                    "UPDATE workspaces SET settings=?2 WHERE id=?1",
                    params![profile, encode(&settings)?],
                )
                .map_err(db_error)?;
            }
        }
        for mut template in items::<Template>(&tx, account, ItemKind::Template)? {
            if clear_reference(&mut template.data.settings, kind, id) {
                save_item(
                    &tx,
                    account,
                    ItemKind::Template,
                    Some(template.id),
                    &template.data,
                )?;
            }
        }
        let mut view = view_options(&tx, account)?.1;
        match kind {
            ItemKind::Folder if view.folder == id => view.folder.clear(),
            ItemKind::Tag => view.tags.retain(|tag| tag != id),
            _ => {}
        }
        save_view(&tx, account, &view)?;
    }
    tx.commit().map_err(db_error)
}

fn clear_reference(settings: &mut Settings, kind: ItemKind, id: &str) -> bool {
    let before = settings.clone();
    match kind {
        ItemKind::Folder if settings.folder_id.as_deref() == Some(id) => settings.folder_id = None,
        ItemKind::Tag => {
            settings.tag_ids.retain(|tag| tag != id);
            if settings.pinned_tag_id.as_deref() == Some(id) {
                settings.pinned_tag_id = None;
            }
        }
        ItemKind::Bookmark => settings.bookmark_ids.retain(|entry| entry != id),
        ItemKind::Extension => settings.extension_ids.retain(|entry| entry != id),
        _ => {}
    }
    *settings != before
}

pub fn settings(db: &Connection, account: &str, mut value: Settings) -> Result<Settings> {
    if value.icon.chars().count() > 12
        || value.icon.chars().any(char::is_control)
        || value.notes.chars().count() > 8_000
    {
        return Err(
            "Use a short icon (up to 12 characters) and notes under 8,000 characters.".into(),
        );
    }
    if value.launch_urls.len() > 12 {
        return Err("Use up to 12 launch URLs.".into());
    }
    value.launch_urls = value
        .launch_urls
        .iter()
        .map(|url| validate_url(url))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    for (kind, ids) in [
        (ItemKind::Folder, value.folder_id.iter().collect::<Vec<_>>()),
        (ItemKind::Tag, value.tag_ids.iter().collect()),
        (ItemKind::Bookmark, value.bookmark_ids.iter().collect()),
        (ItemKind::Extension, value.extension_ids.iter().collect()),
    ] {
        if ids.len() > 100 || ids.iter().collect::<HashSet<_>>().len() != ids.len() {
            return Err("Too many or repeated catalog assignments.".into());
        }
        for id in ids {
            item::<serde_json::Value>(db, account, kind, id)?;
        }
    }
    if value
        .pinned_tag_id
        .as_ref()
        .is_some_and(|tag| !value.tag_ids.contains(tag))
    {
        return Err("Choose a pinned tag from this workspace's assigned tags.".into());
    }
    Ok(value)
}

pub fn read_settings(value: &str, legacy_url: Option<&str>) -> Result<Settings> {
    let mut settings: Settings = decode(value)?;
    if settings.launch_urls.is_empty() {
        settings.launch_urls.extend(legacy_url.map(str::to_owned));
    }
    Ok(settings)
}

pub fn view_options(db: &Connection, account: &str) -> Result<(Option<String>, View)> {
    let (template, view): (Option<String>, String) = db
        .query_row(
            "SELECT default_template,view FROM account_options WHERE account_id=?1",
            [account],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(db_error)?;
    Ok((template, decode(&view)?))
}

pub fn save_view(db: &Connection, account: &str, view: &View) -> Result<()> {
    if view.search.chars().count() > 200
        || view.tags.len() > 100
        || !matches!(view.page_size, 0 | 5 | 10 | 20 | 50)
        || view.columns.as_ref().is_some_and(|c| c.len() > 8)
    {
        return Err("Invalid catalog view options.".into());
    }
    db.execute(
        "UPDATE account_options SET view=?2 WHERE account_id=?1",
        params![account, encode(view)?],
    )
    .map_err(db_error)?;
    Ok(())
}

pub fn set_default_template(db: &Connection, account: &str, id: Option<String>) -> Result<()> {
    if let Some(id) = &id {
        item::<Template>(db, account, ItemKind::Template, id)?;
    }
    db.execute(
        "UPDATE account_options SET default_template=?2 WHERE account_id=?1",
        params![account, id],
    )
    .map_err(db_error)?;
    Ok(())
}

pub fn due_tasks(db: &Connection, account: &str, time: i64) -> Result<usize> {
    db.execute("UPDATE tasks SET fired_at=?2 WHERE account_id=?1 AND completed_at IS NULL AND fired_at IS NULL AND due_at<=?2", params![account, time]).map_err(db_error)
}

pub fn create_task(db: &mut Connection, account: &str, draft: TaskDraft) -> Result<String> {
    let title = name(&draft.title)?;
    if draft.due_at <= 0
        || draft.due_at > 253_402_300_799
        || draft.description.chars().count() > 8_000
        || draft.workspace_ids.len() > 500
    {
        return Err("Enter a valid date, description and up to 500 workspaces.".into());
    }
    let tx = db.transaction().map_err(db_error)?;
    for id in &draft.workspace_ids {
        if !has_workspace(&tx, account, id, false)? {
            return Err("A task workspace is not active in this account.".into());
        }
    }
    let id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO tasks VALUES (?1,?2,?3,?4,?5,NULL,NULL)",
        params![account, id, title, draft.description, draft.due_at],
    )
    .map_err(db_error)?;
    for workspace in draft.workspace_ids.iter().collect::<HashSet<_>>() {
        tx.execute(
            "INSERT INTO task_workspaces VALUES (?1,?2,?3)",
            params![account, id, workspace],
        )
        .map_err(db_error)?;
    }
    tx.commit().map_err(db_error)?;
    Ok(id)
}

pub fn list_tasks(db: &Connection, account: &str) -> Result<Vec<Task>> {
    let mut query = db.prepare(
        "SELECT id,title,description,due_at,fired_at,completed_at,
        (SELECT json_group_array(tw.workspace_id) FROM task_workspaces tw WHERE tw.account_id=t.account_id AND tw.task_id=t.id)
        FROM tasks t WHERE account_id=?1 ORDER BY completed_at IS NOT NULL, due_at,id",
    ).map_err(db_error)?;
    let rows = query
        .query_map([account], |row| {
            let json: String = row.get(6)?;
            let workspace_ids = serde_json::from_str(&json).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?;
            Ok(Task {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                due_at: row.get(3)?,
                fired_at: row.get(4)?,
                completed_at: row.get(5)?,
                workspace_ids,
            })
        })
        .map_err(db_error)?;
    rows.collect::<rusqlite::Result<_>>().map_err(db_error)
}

pub fn finish_task(db: &Connection, account: &str, id: &str, snooze: Option<i64>) -> Result<()> {
    let time = now();
    let changed = if let Some(until) = snooze {
        if until <= time || until > 253_402_300_799 { return Err("Choose a future snooze time.".into()); }
        db.execute("UPDATE tasks SET due_at=?3,fired_at=NULL WHERE account_id=?1 AND id=?2 AND completed_at IS NULL", params![account, id, until])
    } else {
        db.execute("UPDATE tasks SET completed_at=?3 WHERE account_id=?1 AND id=?2 AND completed_at IS NULL", params![account, id, time])
    }.map_err(db_error)?;
    if changed != 1 {
        return Err("This reminder is already complete or does not exist.".into());
    }
    Ok(())
}

pub fn snapshot(db: &Connection, account: String) -> Result<CatalogView> {
    let (default_template, view) = view_options(db, &account)?;
    due_tasks(db, &account, now())?;
    Ok(CatalogView {
        accounts: list_accounts(db)?,
        folders: items(db, &account, ItemKind::Folder)?,
        tags: items(db, &account, ItemKind::Tag)?,
        bookmarks: items(db, &account, ItemKind::Bookmark)?,
        templates: items(db, &account, ItemKind::Template)?,
        extensions: items(db, &account, ItemKind::Extension)?,
        tasks: list_tasks(db, &account)?,
        active_account: account,
        default_template,
        view,
    })
}
