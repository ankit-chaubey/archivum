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

use anyhow::{Result, bail};
use colored::Colorize;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::checksum::HashingReader;
use crate::compress::CompressionAlgo;
use crate::fsutil::INDEX_NAME;
use crate::index::{ArchivumIndex, INDEX_VERSION, IndexEntry, IndexHeader, Loc, PartRef};
use crate::output::OutputCtx;
use crate::scan::EntryType;
use crate::utils::{fmt_time, human, now};

fn list_parts(dir: &Path, algo: &CompressionAlgo) -> Result<Vec<(u32, PathBuf)>> {
    let ext = algo.extension();
    let mut parts = vec![];
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("data.part") else {
            continue;
        };
        let Some(num) = rest.strip_suffix(ext) else {
            continue;
        };
        if let Ok(n) = num.parse::<u32>() {
            parts.push((n, entry.path()));
        }
    }
    parts.sort();
    Ok(parts)
}

/// Pick the compression from the part files present (and their magic bytes).
fn detect_algo(dir: &Path) -> Result<CompressionAlgo> {
    let mut found = vec![];
    for algo in [
        CompressionAlgo::Zstd,
        CompressionAlgo::Gzip,
        CompressionAlgo::Bzip2,
        CompressionAlgo::Lz4,
        CompressionAlgo::None,
    ] {
        if !list_parts(dir, &algo)?.is_empty() {
            found.push(algo);
        }
    }
    match found.len() {
        0 => bail!("No data.partNNN.* files found in {}", dir.display()),
        1 => Ok(found.remove(0)),
        _ => bail!(
            "Parts with several compression types found ({}); pass --compression",
            found
                .iter()
                .map(|a| a.name())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Rebuild a missing index from the tar parts. Checksums are computed from the
/// archive content, so they describe the data as it is now (a baseline for future
/// `verify`), not as it was when the archive was created. Directories, symlinks and
/// dedup relations are not stored in the tar and cannot be recovered.
pub fn repair(dir: &Path, compression: Option<&str>, out: &OutputCtx) -> Result<()> {
    out.println(&format!(
        "{} {}",
        "Repairing archive in:".cyan().bold(),
        dir.display().to_string().yellow()
    ));

    let index_path = dir.join(INDEX_NAME);
    if index_path.exists() {
        bail!(
            "{} already exists; move it away first if you really want to rebuild it",
            index_path.display()
        );
    }

    let algo = match compression {
        None | Some("auto") => detect_algo(dir)?,
        Some(s) => CompressionAlgo::parse(s)?,
    };
    let parts = list_parts(dir, &algo)?;
    if parts.is_empty() {
        bail!(
            "No data.partNNN{} files found in {}",
            algo.extension(),
            dir.display()
        );
    }
    out.println(&format!(
        "  Found {} part(s), compression: {}",
        parts.len(),
        algo.name().green()
    ));

    if out.dry_run {
        out.dry("would rebuild index.arc.json from the parts");
        return Ok(());
    }

    let mut entries: BTreeMap<PathBuf, IndexEntry> = BTreeMap::new();
    let mut part_refs: Vec<PartRef> = vec![];
    let mut damaged = 0usize;

    for (pos, (_, path)) in parts.iter().enumerate() {
        part_refs.push(PartRef {
            path: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            compression: algo.clone(),
            size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        });
        let reader = match algo.open_stream(path, 0, 0) {
            Ok(r) => r,
            Err(e) => {
                out.eprintln(&format!(
                    "  {} {}: {:#}",
                    "skip".yellow(),
                    path.display(),
                    e
                ));
                damaged += 1;
                continue;
            }
        };
        let mut archive = tar::Archive::new(reader);
        let members = match archive.entries() {
            Ok(m) => m,
            Err(e) => {
                out.eprintln(&format!("  {} {}: {}", "skip".yellow(), path.display(), e));
                damaged += 1;
                continue;
            }
        };
        for item in members {
            let mut item = match item {
                Ok(i) => i,
                Err(e) => {
                    out.eprintln(&format!(
                        "  {} {} is damaged after this point: {}",
                        "warn".yellow(),
                        path.display(),
                        e
                    ));
                    damaged += 1;
                    break;
                }
            };
            if !item.header().entry_type().is_file() {
                continue;
            }
            let Ok(member) = item.path().map(|p| p.into_owned()) else {
                continue;
            };
            let mtime = item.header().mtime().ok();
            let mode = item.header().mode().ok();

            let mut hr = HashingReader::new(&mut item);
            let copied = io::copy(&mut hr, &mut io::sink());
            let (hash, bytes) = hr.finalize();
            if let Err(e) = copied {
                out.eprintln(&format!(
                    "  {} {} cannot be read: {}",
                    "warn".yellow(),
                    member.display(),
                    e
                ));
                damaged += 1;
                break;
            }

            entries.insert(
                member.clone(),
                IndexEntry {
                    path: member,
                    entry_type: EntryType::File,
                    size: bytes,
                    mtime,
                    unix_mode: mode,
                    sha256: Some(hash),
                    symlink_target: None,
                    dedup_of: None,
                    // frame boundaries are unknown: the whole part is decoded to find it
                    loc: Some(Loc {
                        part: pos as u32,
                        off: 0,
                        len: 0,
                    }),
                },
            );
        }
    }

    if entries.is_empty() {
        bail!("No readable files found in the parts; nothing to rebuild");
    }

    let entries: Vec<IndexEntry> = entries.into_values().collect();
    let ts = now();
    let mut idx = ArchivumIndex {
        header: IndexHeader {
            version: INDEX_VERSION,
            created_at_unix: ts,
            created_at_human: fmt_time(ts),
            total_files: 0,
            total_dirs: 0,
            total_symlinks: 0,
            total_size: 0,
            zstd_level: 3,
            notes: "Rebuilt by `archivum repair`: checksums computed from archive content; \
                    directories and symlinks not recoverable; frame offsets unknown"
                .to_string(),
            parts: part_refs,
        },
        entries,
    };
    idx.recount();
    let total_size = idx.header.total_size;
    idx.write(&index_path)?;

    out.println(&format!(
        "  {} {} files ({}) recovered into {}",
        "Done:".green().bold(),
        idx.header.total_files,
        human(total_size),
        index_path.display().to_string().yellow()
    ));
    if damaged > 0 {
        out.eprintln(&format!(
            "  {} {} problem(s) while reading the parts; some files may be missing from the index",
            "warning:".yellow(),
            damaged
        ));
    }
    Ok(())
}
