use crate::{
    catalog::{self, Extension, ItemKind},
    importer::bounded_file,
};
use rusqlite::Connection;
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zip::ZipArchive;

const ARCHIVE_LIMIT: u64 = 8 * 1024 * 1024;
const EXPANDED_LIMIT: u64 = 24 * 1024 * 1024;
const FIXTURE_MANIFEST: &str = include_str!("../../../fixture/extension/manifest.json");
const FIXTURE_SCRIPT: &str = include_str!("../../../fixture/extension/badge.js");

#[derive(Deserialize)]
struct Manifest {
    manifest_version: u32,
    name: String,
    version: String,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    host_permissions: Vec<String>,
    #[serde(default)]
    optional_permissions: Vec<String>,
    #[serde(default)]
    optional_host_permissions: Vec<String>,
    #[serde(default)]
    content_scripts: Vec<ContentScript>,
}
#[derive(Deserialize)]
struct ContentScript {
    matches: Vec<String>,
}

fn package_path(name: &str) -> Result<PathBuf, String> {
    let name = name.strip_suffix('/').unwrap_or(name);
    if name.is_empty()
        || name.len() > 200
        || name.split('/').any(|part| {
            let stem = part
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part
                    .chars()
                    .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4)
        })
    {
        return Err("The extension archive contains an unsafe path.".into());
    }
    Ok(PathBuf::from(name))
}

fn unpack(bytes: &[u8]) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| {
        "Choose a ZIP with a manifest.json at its root. CRX packages are not supported."
    })?;
    if archive.len() > 512 {
        return Err("Use at most 512 files per extension ZIP.".into());
    }
    let mut expanded = 0;
    let mut names = HashSet::new();
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| "Could not read extension ZIP entry.")?;
        let path = package_path(
            &file
                .name()
                .map_err(|_| "Invalid extension ZIP entry name.")?,
        )?;
        if file.enclosed_name().is_none()
            || file.is_symlink()
            || !names.insert(path.to_string_lossy().to_lowercase())
        {
            return Err("The extension ZIP contains a symlink, unsafe or repeated path.".into());
        }
        if file.is_dir() {
            continue;
        }
        let remaining = EXPANDED_LIMIT - expanded;
        if file.size() > remaining {
            return Err("Expanded extension ZIP data exceeds 24 MiB.".into());
        }
        let mut data = Vec::new();
        file.take(remaining + 1)
            .read_to_end(&mut data)
            .map_err(|_| "Could not read extension ZIP content.")?;
        expanded += data.len() as u64;
        if expanded > EXPANDED_LIMIT {
            return Err("Expanded extension ZIP data exceeds 24 MiB.".into());
        }
        files.push((path, data));
    }
    Ok(files)
}

fn manifest(files: &[(PathBuf, Vec<u8>)]) -> Result<Manifest, String> {
    let manifest = files
        .iter()
        .find(|(path, _)| path == Path::new("manifest.json"))
        .ok_or("The ZIP needs manifest.json at its root.")?;
    let decoded: Manifest =
        serde_json::from_slice(&manifest.1).map_err(|_| "The extension manifest is invalid.")?;
    if decoded.manifest_version != 3
        || decoded.version.is_empty()
        || decoded.version.len() > 32
        || !decoded
            .version
            .bytes()
            .all(|c| c.is_ascii_digit() || c == b'.')
    {
        return Err("Choose a Manifest V3 package with a numeric dotted version.".into());
    }
    Ok(decoded)
}

pub fn stage(
    db: &Connection,
    root: &Path,
    account: &str,
    id: Option<String>,
    source: &str,
    archive: Option<&Path>,
    fixture_update: bool,
) -> Result<String, String> {
    let files = match source {
        "fixture" => {
            let manifest = if fixture_update {
                FIXTURE_MANIFEST.replace("\"1.0.0\"", "\"1.0.1\"")
            } else {
                FIXTURE_MANIFEST.to_owned()
            };
            vec![
                ("manifest.json".into(), manifest.into_bytes()),
                ("badge.js".into(), FIXTURE_SCRIPT.as_bytes().to_vec()),
            ]
        }
        "zip" => unpack(&bounded_file(
            archive.ok_or("Choose an extension ZIP.")?,
            ARCHIVE_LIMIT,
        )?)?,
        _ => return Err("Unsupported extension package source.".into()),
    };
    let manifest = manifest(&files)?;
    let parent = root.join("extension-packages");
    fs::create_dir_all(&parent)
        .map_err(|e| format!("Could not create the extension cache: {e}"))?;
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    if dunce::canonicalize(&parent).map_err(|e| e.to_string())? != root.join("extension-packages") {
        return Err("The extension cache path is not app-owned.".into());
    }
    let directory = parent.join(Uuid::new_v4().to_string());
    fs::create_dir(&directory)
        .map_err(|e| format!("Could not create the extension staging folder: {e}"))?;
    let result = (|| {
        for (path, content) in files {
            let target = directory.join(path);
            fs::create_dir_all(target.parent().ok_or("Invalid package path.")?)
                .map_err(|e| format!("Could not create a package folder: {e}"))?;
            fs::write(target, content)
                .map_err(|e| format!("Could not stage extension file: {e}"))?;
        }
        let mut permissions = manifest.permissions;
        permissions.extend(manifest.host_permissions);
        permissions.extend(
            manifest
                .optional_permissions
                .into_iter()
                .map(|p| format!("Optional: {p}")),
        );
        permissions.extend(
            manifest
                .optional_host_permissions
                .into_iter()
                .map(|p| format!("Optional: {p}")),
        );
        permissions.extend(manifest.content_scripts.into_iter().flat_map(|c| c.matches));
        permissions.sort();
        permissions.dedup();
        catalog::save_item(
            db,
            account,
            ItemKind::Extension,
            id,
            &Extension {
                name: catalog::name(&manifest.name)?.into(),
                browser_id: String::new(),
                source: source.into(),
                version: manifest.version,
                permissions,
                update_mode: "Manual reload or replace in Chrome".into(),
                install_url: None,
                unpacked_path: Some(
                    directory
                        .to_str()
                        .ok_or("The extension folder path is not valid Unicode.")?
                        .into(),
                ),
            },
        )
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(directory);
    }
    result
}

pub fn save_store(
    db: &Connection,
    account: &str,
    id: Option<String>,
    name: &str,
    browser_id: &str,
    version: &str,
    permissions: Vec<String>,
) -> Result<String, String> {
    if browser_id.len() != 32 || !browser_id.bytes().all(|c| (b'a'..=b'p').contains(&c)) {
        return Err("Enter the 32-character Chrome Web Store extension ID.".into());
    }
    if version.len() > 32 || permissions.len() > 50 || permissions.iter().any(|p| p.len() > 200) {
        return Err("Extension metadata is too large.".into());
    }
    catalog::save_item(
        db,
        account,
        ItemKind::Extension,
        id,
        &Extension {
            name: catalog::name(name)?.into(),
            browser_id: browser_id.into(),
            source: "store".into(),
            version: version.into(),
            permissions,
            update_mode: "Browser-managed after user approval".into(),
            install_url: Some(format!(
                "https://chromewebstore.google.com/detail/{browser_id}"
            )),
            unpacked_path: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::Manager;

    #[test]
    fn package_paths_reject_escape_and_windows_aliases() {
        for path in [
            "../manifest.json",
            "/manifest.json",
            "x/../manifest.json",
            "C:/bad",
            "dir/CON.js",
            "dir\\bad",
            "dir/evil:ads",
        ] {
            assert!(package_path(path).is_err(), "{path}");
        }
        assert_eq!(
            package_path("assets/icon.png").unwrap(),
            Path::new("assets/icon.png")
        );
    }

    #[test]
    fn fixture_update_preserves_catalog_id_and_persists_version() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::new(root.path().into()).unwrap();
        let id = stage(
            manager.db().unwrap(),
            root.path(),
            "local",
            None,
            "fixture",
            None,
            false,
        )
        .unwrap();
        let first: Extension =
            catalog::item(manager.db().unwrap(), "local", ItemKind::Extension, &id).unwrap();
        assert_eq!(first.version, "1.0.0");
        let updated = stage(
            manager.db().unwrap(),
            root.path(),
            "local",
            Some(id.clone()),
            "fixture",
            None,
            true,
        )
        .unwrap();
        assert_eq!(updated, id);
        drop(manager);
        let manager = Manager::new(root.path().into()).unwrap();
        let saved: Extension =
            catalog::item(manager.db().unwrap(), "local", ItemKind::Extension, &id).unwrap();
        assert_eq!(saved.version, "1.0.1");
        assert!(Path::new(&saved.unpacked_path.unwrap())
            .join("badge.js")
            .exists());
    }
}
