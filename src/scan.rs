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

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub relative_path: PathBuf,
    pub entry_type: EntryType,
    pub size: u64,
    pub mtime: Option<u64>,
    pub unix_mode: Option<u32>,
    pub symlink_target: Option<PathBuf>,
}

/// Result of a directory walk. Anything that could not be read ends up in
/// `warnings` instead of disappearing silently.
pub struct Scan {
    pub entries: Vec<ScanEntry>,
    pub warnings: Vec<String>,
}

/// `skip_dirs` are absolute directories (typically the archive output dir) that must
/// never be archived, even when they live inside `root`.
pub fn scan_directory_with(
    root: &Path,
    excludes: &[String],
    skip_dirs: &[PathBuf],
) -> Result<Scan> {
    let excludeset = build_globset(excludes)?;

    let root_canon = root.canonicalize().ok();
    let skip_rel: Vec<PathBuf> = skip_dirs
        .iter()
        .filter_map(|s| {
            root_canon
                .as_ref()
                .and_then(|rc| s.strip_prefix(rc).ok())
                .map(Path::to_path_buf)
        })
        .filter(|p| !p.as_os_str().is_empty())
        .collect();

    let mut entries = Vec::new();
    let mut warnings = Vec::new();

    // filter_entry prunes whole subtrees, so excluding `node_modules` really
    // skips everything below it.
    let walker = WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            match e.path().strip_prefix(root) {
                Ok(rel) => !(excludeset.is_match(rel) || skip_rel.iter().any(|s| s == rel)),
                Err(_) => true,
            }
        });

    for item in walker {
        let entry = match item {
            Ok(e) => e,
            Err(e) => {
                let where_ = e
                    .path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "?".into());
                warnings.push(format!("cannot read {}: {}", where_, e));
                continue;
            }
        };

        let path = entry.path();
        let rel = path.strip_prefix(root)?.to_path_buf();

        if rel.as_os_str().is_empty() {
            continue;
        }

        if rel.to_str().is_none() {
            warnings.push(format!(
                "skipping {}: path is not valid UTF-8 and cannot be stored in the index",
                path.display()
            ));
            continue;
        }

        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("cannot stat {}: {}", path.display(), e));
                continue;
            }
        };

        #[cfg(unix)]
        let (mtime, mode) = (Some(meta.mtime().max(0) as u64), Some(meta.mode()));

        #[cfg(not(unix))]
        let (mtime, mode) = {
            let m = meta.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
            });
            (m, None)
        };

        if meta.is_symlink() {
            let target = fs::read_link(path).ok();
            entries.push(ScanEntry {
                relative_path: rel,
                entry_type: EntryType::Symlink,
                size: 0,
                mtime,
                unix_mode: mode,
                symlink_target: target,
            });
        } else if meta.is_dir() {
            entries.push(ScanEntry {
                relative_path: rel,
                entry_type: EntryType::Directory,
                size: 0,
                mtime,
                unix_mode: mode,
                symlink_target: None,
            });
        } else if meta.is_file() {
            entries.push(ScanEntry {
                relative_path: rel,
                entry_type: EntryType::File,
                size: meta.len(),
                mtime,
                unix_mode: mode,
                symlink_target: None,
            });
        }
    }

    Ok(Scan { entries, warnings })
}

/// Patterns without a `/` apply at any depth (`.DS_Store` also matches `a/b/.DS_Store`).
fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        builder.add(Glob::new(p)?);
        if !p.contains('/') {
            builder.add(Glob::new(&format!("**/{p}"))?);
        }
    }
    Ok(builder.build()?)
}
