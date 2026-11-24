use crate::{extensions, importer::bounded_file, manager::db_error};
use base64::{engine::general_purpose::STANDARD, Engine};
use prost::Message;
use ring::signature;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_HEADER: usize = 128 * 1024;
const MAX_FILE: u64 = 8 * 1024 * 1024;
pub const FIXTURE_ID: &str = "lbiejmdkeemgflijdoepcdkefpgkcbmf";

#[derive(Message)]
struct Header {
    #[prost(message, repeated, tag = "2")]
    rsa: Vec<Proof>,
    #[prost(message, repeated, tag = "3")]
    ecdsa: Vec<Proof>,
    #[prost(bytes = "vec", tag = "10000")]
    signed: Vec<u8>,
}

#[derive(Message)]
struct Proof {
    #[prost(bytes = "vec", tag = "1")]
    key: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    signature: Vec<u8>,
}

#[derive(Message)]
struct SignedData {
    #[prost(bytes = "vec", tag = "1")]
    id: Vec<u8>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub name: String,
    pub version: String,
    pub browser_id: String,
    pub permissions: Vec<String>,
}

pub struct Package {
    pub review: Review,
    pub minimum_chrome_version: Option<String>,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Package {
    pub fn require_compatible_runtime(&self, runtime: &str) -> Result<(), String> {
        fn components(version: &str) -> Option<Vec<u32>> {
            let mut parts = version
                .split('.')
                .map(|part| {
                    if part.is_empty() || !part.bytes().all(|c| c.is_ascii_digit()) {
                        return None;
                    }
                    part.parse().ok()
                })
                .collect::<Option<Vec<_>>>()?;
            while parts.last() == Some(&0) {
                parts.pop();
            }
            Some(parts)
        }
        let Some(minimum) = self.minimum_chrome_version.as_deref() else {
            return Ok(());
        };
        let minimum_parts =
            components(minimum).ok_or("Invalid minimum_chrome_version in the CRX manifest.")?;
        let runtime_parts =
            components(runtime).ok_or("Could not verify the workspace Chromium version.")?;
        if runtime_parts < minimum_parts {
            return Err(format!(
                "This CRX requires Chromium {minimum} or newer; this workspace uses {runtime}."
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOptions {
    pub bundled: bool,
    pub installed_version: Option<String>,
    pub update_available: bool,
}

pub struct Installed {
    pub browser_id: String,
    pub version: String,
    package_id: String,
}

pub fn installed(
    db: &Connection,
    workspace: &str,
    catalog: &str,
) -> Result<Option<Installed>, String> {
    db.query_row(
        "SELECT browser_id,version,package_id FROM workspace_crx WHERE workspace_id=?1 AND catalog_id=?2",
        params![workspace, catalog],
        |row| Ok(Installed { browser_id: row.get(0)?, version: row.get(1)?, package_id: row.get(2)? }),
    ).optional().map_err(db_error)
}

fn component(id: &str) -> Result<String, String> {
    Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| "Invalid extension storage ID.".into())
}

fn workspace_directory(root: &Path, workspace: &str) -> Result<PathBuf, String> {
    Ok(root
        .join("workspace-extensions")
        .join(component(workspace)?))
}

fn expect_directory(path: &Path) -> Result<(), String> {
    if dunce::canonicalize(path).map_err(|e| e.to_string())? != path {
        return Err("The extension folder is not in its app-owned location.".into());
    }
    Ok(())
}

pub fn approved_path(root: &Path, workspace: &str, install: &Installed) -> Result<PathBuf, String> {
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    let path = workspace_directory(&root, workspace)?.join(component(&install.package_id)?);
    expect_directory(&path)?;
    if path.to_string_lossy().contains(',') {
        return Err(
            "Chromium cannot load an extension from a directory containing a comma.".into(),
        );
    }
    Ok(path)
}

pub fn remove_workspace(root: &Path, workspace: &str) -> Result<(), String> {
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    let path = workspace_directory(&root, workspace)?;
    remove_directory(&path)
}

pub fn remove_package(root: &Path, workspace: &str, package: &str) -> Result<(), String> {
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    let path = workspace_directory(&root, workspace)?.join(component(package)?);
    remove_directory(&path)
}

fn remove_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            expect_directory(path)?;
            fs::remove_dir_all(path)
                .map_err(|e| format!("Could not remove workspace extensions: {e}"))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not inspect workspace extensions: {error}")),
    }
}

pub fn install(
    db: &Connection,
    root: &Path,
    workspace: &str,
    catalog: &str,
    package: Package,
) -> Result<(), String> {
    let previous = installed(db, workspace, catalog)?;
    let root = dunce::canonicalize(root).map_err(|e| e.to_string())?;
    if root.to_string_lossy().contains(',') {
        return Err(
            "Chromium cannot load an extension from a directory containing a comma.".into(),
        );
    }
    let parent = workspace_directory(&root, workspace)?;
    fs::create_dir_all(&parent)
        .map_err(|e| format!("Could not create the workspace extension folder: {e}"))?;
    expect_directory(&parent)?;
    let package_id = Uuid::new_v4().to_string();
    let directory = parent.join(&package_id);
    fs::create_dir(&directory)
        .map_err(|e| format!("Could not create the extension package folder: {e}"))?;
    let result = (|| {
        for (path, bytes) in package.files {
            let path = directory.join(path);
            fs::create_dir_all(path.parent().ok_or("Invalid extension path.")?)
                .map_err(|e| e.to_string())?;
            fs::write(path, bytes).map_err(|e| format!("Could not save an extension file: {e}"))?;
        }
        db.execute(
            "INSERT INTO workspace_crx(workspace_id,catalog_id,browser_id,version,package_id) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(workspace_id,catalog_id) DO UPDATE SET browser_id=excluded.browser_id,version=excluded.version,package_id=excluded.package_id",
            params![workspace, catalog, package.review.browser_id, package.review.version, package_id],
        ).map_err(db_error)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(directory);
    } else if let Some(path) =
        previous.and_then(|previous| approved_path(&root, workspace, &previous).ok())
    {
        let _ = fs::remove_dir_all(path);
    }
    result
}

pub fn version(value: &str) -> Option<[u16; 4]> {
    let mut numbers = [0; 4];
    for (i, part) in value.split('.').enumerate() {
        if part.is_empty()
            || !part.bytes().all(|c| c.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return None;
        }
        *numbers.get_mut(i)? = part.parse().ok()?;
    }
    (numbers != [0; 4]).then_some(numbers)
}

fn extension_id(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| {
            [
                char::from(b'a' + (byte >> 4)),
                char::from(b'a' + (byte & 15)),
            ]
        })
        .collect()
}

pub fn inspect_file(path: &Path, id: &str, pin: &str) -> Result<Package, String> {
    inspect(&bounded_file(path, MAX_FILE)?, id, pin)
}

pub fn fixture(pin: &str) -> Result<Package, String> {
    let encoded = match pin {
        "1.0.0" => include_str!("../../../fixture/extension/badge-1.0.0.crx.base64"),
        "1.0.1" => include_str!("../../../fixture/extension/badge-1.0.1.crx.base64"),
        _ => return Err("There is no fixture CRX for this version.".into()),
    };
    let bytes = STANDARD
        .decode(encoded.lines().collect::<String>())
        .map_err(|_| "The fixture CRX could not be decoded.")?;
    inspect(&bytes, FIXTURE_ID, pin)
}

fn inspect(bytes: &[u8], id: &str, pin: &str) -> Result<Package, String> {
    if version(pin).is_none() {
        return Err("Set an exact catalog version before reviewing a CRX.".into());
    }
    if bytes.get(..8) != Some(b"Cr24\x03\0\0\0") {
        return Err("Choose a signed CRX3 extension file.".into());
    }
    let header_length = bytes
        .get(8..12)
        .map(|value| u32::from_le_bytes(value.try_into().unwrap()) as usize)
        .filter(|length| *length <= MAX_HEADER && 12 + length < bytes.len())
        .ok_or("Invalid CRX header length.")?;
    let (header, archive) = bytes[12..].split_at(header_length);
    let header = Header::decode(header).map_err(|_| "Invalid CRX header.")?;
    let signed =
        SignedData::decode(header.signed.as_slice()).map_err(|_| "Invalid signed CRX header.")?;
    if signed.id.len() != 16 || extension_id(&signed.id) != id {
        return Err("The file's extension ID does not match the catalog entry.".into());
    }
    let mut message = b"CRX3 SignedData\0".to_vec();
    message.extend_from_slice(&(header.signed.len() as u32).to_le_bytes());
    message.extend_from_slice(&header.signed);
    message.extend_from_slice(archive);
    let valid = |proof: &Proof, algorithm: &'static dyn signature::VerificationAlgorithm| {
        if Sha256::digest(&proof.key)[..16] != signed.id {
            return false;
        }
        let Ok(spki) = spki::SubjectPublicKeyInfoRef::try_from(proof.key.as_slice()) else {
            return false;
        };
        let Some(key) = spki.subject_public_key.as_bytes() else {
            return false;
        };
        signature::UnparsedPublicKey::new(algorithm, key)
            .verify(&message, &proof.signature)
            .is_ok()
    };
    let proof = header
        .rsa
        .iter()
        .find(|proof| valid(proof, &signature::RSA_PKCS1_2048_8192_SHA256))
        .or_else(|| {
            header
                .ecdsa
                .iter()
                .find(|proof| valid(proof, &signature::ECDSA_P256_SHA256_ASN1))
        })
        .ok_or("The CRX signature does not match the extension's signing key.")?;
    let mut files = extensions::unpack(archive)?;
    files.retain(|(path, _)| !path.starts_with("_metadata"));
    let manifest = extensions::manifest(&files)?;
    if manifest.version != pin {
        return Err(format!(
            "The CRX is version {}, but the catalog pins {pin}.",
            manifest.version
        ));
    }
    let minimum_chrome_version = manifest.minimum_chrome_version.clone();
    let review = Review {
        name: crate::catalog::name(&manifest.name)?.to_owned(),
        version: manifest.version.clone(),
        browser_id: id.to_owned(),
        permissions: manifest.permissions(),
    };
    let (_, bytes) = files
        .iter_mut()
        .find(|(path, _)| path == Path::new("manifest.json"))
        .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Invalid CRX manifest.")?;
    manifest["key"] = STANDARD.encode(&proof.key).into();
    *bytes =
        serde_json::to_vec(&manifest).map_err(|_| "Could not prepare the extension manifest.")?;
    Ok(Package {
        review,
        minimum_chrome_version,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_signed_fixture_crx_and_rejects_other_ids_versions_and_modified_payloads() {
        let package = fixture("1.0.0").unwrap();
        assert_eq!(package.review.browser_id, FIXTURE_ID);
        assert_eq!(package.review.permissions, ["http://127.0.0.1/*"]);
        assert_eq!(fixture("1.0.1").unwrap().review.version, "1.0.1");
        let encoded = include_str!("../../../fixture/extension/badge-1.0.0.crx.base64");
        let mut bytes = STANDARD
            .decode(encoded.lines().collect::<String>())
            .unwrap();
        assert!(inspect(&bytes, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "1.0.0")
            .err()
            .unwrap()
            .contains("extension ID"));
        assert!(inspect(&bytes, FIXTURE_ID, "1.0.1")
            .err()
            .unwrap()
            .contains("catalog pins"));
        *bytes.last_mut().unwrap() ^= 1;
        assert!(inspect(&bytes, FIXTURE_ID, "1.0.0")
            .err()
            .unwrap()
            .contains("signature"));
    }
}
