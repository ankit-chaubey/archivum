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

//! Small filesystem helpers shared by the archive writers.

use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const INDEX_NAME: &str = "index.arc.json";
pub const SEAL_NAME: &str = "index.arc.json.b3";
pub const SIG_NAME: &str = "index.arc.json.sig";

/// `foo/bar.ext` -> `foo/bar.ext.tmp`
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

/// Write `data` to a temp file next to `path`, fsync it, then rename over `path`.
/// A crash can therefore never leave a half-written file behind.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = tmp_path(path);
    let written = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("Cannot write {}", tmp.display()));
    }
    fs::rename(&tmp, path).with_context(|| format!("Cannot move {} into place", path.display()))
}

pub fn is_archive_file(name: &str) -> bool {
    name.starts_with("data.part")
        || name == INDEX_NAME
        || name == SEAL_NAME
        || name == SIG_NAME
        || name == format!("{INDEX_NAME}.tmp")
        || name == format!("{SEAL_NAME}.tmp")
        || name == format!("{SIG_NAME}.tmp")
}

/// Refuse to write a new archive on top of an existing one unless `overwrite` is set.
pub fn ensure_clean_output(dir: &Path, overwrite: bool) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() {
        bail!("Output path is not a directory: {}", dir.display());
    }
    let mut found = vec![];
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if is_archive_file(&entry.file_name().to_string_lossy()) {
            found.push(entry.path());
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    if !overwrite {
        bail!(
            "{} already contains an archive ({} file(s)). Use another directory or pass --overwrite.",
            dir.display(),
            found.len()
        );
    }
    for f in found {
        fs::remove_file(&f).with_context(|| format!("Cannot remove {}", f.display()))?;
    }
    Ok(())
}

/// True when both paths resolve to the same location.
pub fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Path of `target` relative to `base` (both resolved through the filesystem when possible).
pub fn relative_path(base: &Path, target: &Path) -> PathBuf {
    let base_abs = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let target_abs = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());

    let base_comps: Vec<_> = base_abs.components().collect();
    let target_comps: Vec<_> = target_abs.components().collect();

    let common = base_comps
        .iter()
        .zip(target_comps.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut rel = PathBuf::new();
    for _ in 0..(base_comps.len() - common) {
        rel.push("..");
    }
    for comp in &target_comps[common..] {
        rel.push(comp);
    }

    if rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rel
    }
}
