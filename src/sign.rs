/*
 * Copyright 2026 Ankit Chaubey <ankitchaubey.dev@gmail.com>
 * github.com/ankit-chaubey
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Ed25519 signing of the index. The index lists the SHA-256 of every file, so a
//! valid signature on it protects all of the archive's content transitively.

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const SECRET_PREFIX: &str = "archivum-secret-key";
const PUBLIC_PREFIX: &str = "archivum-public-key";
const SIG_PREFIX: &str = "archivum-sig-ed25519";

static TRUSTED: OnceLock<VerifyingKey> = OnceLock::new();

/// Require every index that is read to carry a valid signature from `key`.
pub fn set_trusted(key: VerifyingKey) {
    let _ = TRUSTED.set(key);
}

pub fn trusted() -> Option<&'static VerifyingKey> {
    TRUSTED.get()
}

pub fn fingerprint(key: &VerifyingKey) -> String {
    hex::encode(&blake3::hash(key.as_bytes()).as_bytes()[..8])
}

fn read_keyfile(path: &Path, prefix: &str) -> Result<[u8; 32]> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Cannot read key file {}", path.display()))?;
    let hexed = text
        .trim()
        .strip_prefix(prefix)
        .with_context(|| format!("{} is not an {} file", path.display(), prefix))?;
    let bytes = hex::decode(hexed.trim()).context("Key file is not valid hex")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Key in {} must be 32 bytes", path.display()))
}

pub fn load_signing_key(path: &Path) -> Result<SigningKey> {
    Ok(SigningKey::from_bytes(&read_keyfile(path, SECRET_PREFIX)?))
}

pub fn load_public_key(path: &Path) -> Result<VerifyingKey> {
    VerifyingKey::from_bytes(&read_keyfile(path, PUBLIC_PREFIX)?)
        .map_err(|e| anyhow::anyhow!("Invalid public key {}: {}", path.display(), e))
}

/// Create `<base>.key` (secret, mode 0600) and `<base>.pub`.
pub fn keygen(base: &Path) -> Result<(PathBuf, PathBuf)> {
    let mut sec_name = base.as_os_str().to_owned();
    sec_name.push(".key");
    let mut pub_name = base.as_os_str().to_owned();
    pub_name.push(".pub");
    let (sec_path, pub_path) = (PathBuf::from(sec_name), PathBuf::from(pub_name));
    if sec_path.exists() || pub_path.exists() {
        bail!(
            "{} or {} already exists; refusing to overwrite a key",
            sec_path.display(),
            pub_path.display()
        );
    }

    let signing = SigningKey::generate(&mut rand_core::OsRng);
    let secret = format!("{} {}\n", SECRET_PREFIX, hex::encode(signing.to_bytes()));
    let public = format!(
        "{} {}\n",
        PUBLIC_PREFIX,
        hex::encode(signing.verifying_key().as_bytes())
    );

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&sec_path)?;
        f.write_all(secret.as_bytes())?;
    }
    #[cfg(not(unix))]
    fs::write(&sec_path, secret)?;
    fs::write(&pub_path, public)?;
    Ok((sec_path, pub_path))
}

/// Signature file content for `data`.
pub fn sign(key: &SigningKey, data: &[u8]) -> String {
    let sig = key.sign(data);
    format!(
        "{} {} {}\n",
        SIG_PREFIX,
        hex::encode(key.verifying_key().as_bytes()),
        hex::encode(sig.to_bytes())
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum SigStatus {
    /// No signature file next to the index.
    Absent,
    /// Signature is mathematically valid; `trusted` tells whether the signer is
    /// the key the caller asked for (always true when no key was requested).
    Valid {
        signer: String,
        trusted: bool,
    },
    Invalid(String),
}

pub fn check(sig_text: &str, data: &[u8], expected: Option<&VerifyingKey>) -> SigStatus {
    let parts: Vec<&str> = sig_text.split_whitespace().collect();
    let [prefix, pk_hex, sig_hex] = parts[..] else {
        return SigStatus::Invalid("malformed signature file".into());
    };
    if prefix != SIG_PREFIX {
        return SigStatus::Invalid("unknown signature type".into());
    }
    let Ok(pk_bytes) = hex::decode(pk_hex)
        .map_err(|_| ())
        .and_then(|b| <[u8; 32]>::try_from(b).map_err(|_| ()))
    else {
        return SigStatus::Invalid("bad public key in signature file".into());
    };
    let Ok(key) = VerifyingKey::from_bytes(&pk_bytes) else {
        return SigStatus::Invalid("bad public key in signature file".into());
    };
    let Ok(sig_bytes) = hex::decode(sig_hex)
        .map_err(|_| ())
        .and_then(|b| <[u8; 64]>::try_from(b).map_err(|_| ()))
    else {
        return SigStatus::Invalid("bad signature encoding".into());
    };
    if key
        .verify(data, &Signature::from_bytes(&sig_bytes))
        .is_err()
    {
        return SigStatus::Invalid("signature does not match the index".into());
    }
    SigStatus::Valid {
        signer: fingerprint(&key),
        trusted: expected.is_none_or(|e| e.as_bytes() == key.as_bytes()),
    }
}
