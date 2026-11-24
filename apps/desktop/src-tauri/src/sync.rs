use crate::catalog::Bookmark;
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{aead, pbkdf2, rand::{SecureRandom, SystemRandom}};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;

const MAX_BYTES: usize = 256 * 1024;
const AAD: &[u8] = b"slide.workspace.metadata.v1";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub version: u8,
    pub name: String,
    pub icon: String,
    pub notes: String,
    pub launch_urls: Vec<String>,
    pub bookmarks: Vec<Bookmark>,
    pub runtime: Option<RuntimeVersion>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeVersion {
    pub brand: String,
    pub version: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    version: u8,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize)]
pub struct Sealed {
    envelope: Envelope,
    sha256: String,
}

fn key(passphrase: &str, salt: &[u8; 16]) -> Result<aead::LessSafeKey, String> {
    if !(12..=256).contains(&passphrase.len()) {
        return Err("Use a snapshot passphrase of 12–256 bytes.".into());
    }
    let mut material = [0; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(600_000).unwrap(), salt,
        passphrase.as_bytes(), &mut material);
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &material)
        .map_err(|_| "Could not prepare snapshot encryption.")?;
    material.fill(0);
    Ok(aead::LessSafeKey::new(key))
}

fn bytes(envelope: &Envelope) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(envelope).map_err(|_| "Invalid snapshot envelope.")?;
    if bytes.len() > MAX_BYTES { return Err("Snapshot metadata is too large.".into()); }
    Ok(bytes)
}

pub fn seal(metadata: Metadata, passphrase: String) -> Result<Sealed, String> {
    let mut ciphertext = serde_json::to_vec(&metadata).map_err(|_| "Invalid workspace metadata.")?;
    if ciphertext.len() > MAX_BYTES { return Err("Snapshot metadata is too large.".into()); }
    let rng = SystemRandom::new();
    let (mut salt, mut nonce) = ([0; 16], [0; 12]);
    rng.fill(&mut salt).and_then(|_| rng.fill(&mut nonce)).map_err(|_| "Could not create snapshot randomness.")?;
    key(&passphrase, &salt)?.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(AAD), &mut ciphertext).map_err(|_| "Could not encrypt the workspace.")?;
    let envelope = Envelope { version: 1, salt: STANDARD.encode(salt), nonce: STANDARD.encode(nonce), ciphertext: STANDARD.encode(ciphertext) };
    let sha256 = format!("{:x}", Sha256::digest(bytes(&envelope)?));
    Ok(Sealed { envelope, sha256 })
}

pub fn open(envelope: Envelope, sha256: String, passphrase: String) -> Result<Metadata, String> {
    if envelope.version != 1 || format!("{:x}", Sha256::digest(bytes(&envelope)?)) != sha256 {
        return Err("Snapshot version or integrity check failed.".into());
    }
    let decode = |value: &str| STANDARD.decode(value).map_err(|_| "Invalid snapshot encoding.");
    let salt: [u8; 16] = decode(&envelope.salt)?.try_into().map_err(|_| "Invalid snapshot salt.")?;
    let nonce: [u8; 12] = decode(&envelope.nonce)?.try_into().map_err(|_| "Invalid snapshot nonce.")?;
    let mut ciphertext = decode(&envelope.ciphertext)?;
    let key = key(&passphrase, &salt)?;
    let plaintext = key.open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::from(AAD), &mut ciphertext)
        .map_err(|_| "Wrong snapshot passphrase or damaged snapshot.")?;
    let metadata: Metadata = serde_json::from_slice(plaintext).map_err(|_| "Invalid workspace metadata.")?;
    if metadata.version != 1 { return Err("Unsupported workspace metadata version.".into()); }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_metadata_round_trips_and_rejects_wrong_passphrase() {
        let password = "fixture-passphrase";
        let metadata = Metadata {
            version: 1, name: "Synthetic workspace".into(), icon: "".into(), notes: "Fixture notes".into(),
            launch_urls: vec!["http://127.0.0.1:4310/".into()], bookmarks: Vec::new(), runtime: None,
        };
        let sealed = seal(metadata, password.into()).unwrap();
        let encoded = serde_json::to_string(&sealed.envelope).unwrap();
        assert!(!encoded.contains("Fixture notes"));
        let envelope = serde_json::from_str(&encoded).unwrap();
        assert!(open(envelope, sealed.sha256.clone(), "wrong-passphrase".into()).err().unwrap().contains("Wrong snapshot passphrase"));
        let recovered = open(sealed.envelope, sealed.sha256, password.into()).unwrap();
        assert_eq!(recovered.notes, "Fixture notes");
    }
}
