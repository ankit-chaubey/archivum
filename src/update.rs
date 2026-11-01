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

use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::checksum::{compute_checksums, hash_file};
use crate::compress::CompressionAlgo;
use crate::fsutil::{INDEX_NAME, ensure_clean_output, relative_path, same_path};
use crate::index::{ArchivumIndex, INDEX_VERSION, IndexEntry, IndexHeader, PartRef};
use crate::output::OutputCtx;
use crate::scan::{EntryType, ScanEntry, scan_directory_with};
use crate::tar_writer::{WriteOpts, write_archive};
use crate::utils::{fmt_time, human, now};

pub struct UpdateParams<'a> {
    pub old_index_path: &'a Path,
    pub source: &'a Path,
    pub output_dir: &'a Path,
    pub split_bytes: u64,
    pub split_files: usize,
    /// `None` uses the compression of the newest part of the old archive. It may differ
    /// from the old parts: every part records its own compression.
    pub algo: Option<CompressionAlgo>,
    pub sign_key: Option<&'a ed25519_dalek::SigningKey>,
    pub zstd_level: i32,
    pub zstd_threads: u32,
    pub threads: usize,
    pub exclude: &'a [String],
    pub use_checksum: bool,
    pub dedup: bool,
}

pub fn update(p: UpdateParams, out: &OutputCtx) -> Result<()> {
    out.println(&format!(
        "{} {} -> {}",
        "Incremental update:".cyan().bold(),
        p.source.display().to_string().yellow(),
        p.output_dir.display().to_string().yellow()
    ));
    out.println("");

    let old_idx = ArchivumIndex::read(p.old_index_path)
        .with_context(|| format!("Cannot read old index: {}", p.old_index_path.display()))?;
    let old_index_dir = match p.old_index_path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };

    let algo = p.algo.clone().unwrap_or_else(|| {
        old_idx
            .header
            .parts
            .first()
            .map(|x| x.compression.clone())
            .unwrap_or_default()
    });

    if same_path(p.output_dir, old_index_dir) {
        bail!("The output directory must differ from the directory of the old archive");
    }
    if same_path(p.output_dir, p.source) {
        bail!("The output directory must differ from the source directory");
    }
    ensure_clean_output(p.output_dir, false)?;
    if !out.dry_run {
        std::fs::create_dir_all(p.output_dir)
            .with_context(|| format!("Cannot create output dir {}", p.output_dir.display()))?;
    }
    let skip: Vec<PathBuf> = p.output_dir.canonicalize().into_iter().collect();

    let scan = scan_directory_with(p.source, p.exclude, &skip)
        .with_context(|| format!("Failed to scan {}", p.source.display()))?;
    for w in &scan.warnings {
        out.eprintln(&format!("  {} {}", "warning:".yellow(), w));
    }

    let old_files: HashMap<&Path, &IndexEntry> = old_idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File)
        .map(|e| (e.path.as_path(), e))
        .collect();

    let mut unchanged: Vec<IndexEntry> = vec![];
    let mut rearchive: HashSet<PathBuf> = HashSet::new();
    let (mut n_changed, mut n_new) = (0usize, 0usize);
    let mut seen_old: HashSet<&Path> = HashSet::new();

    for se in scan
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File)
    {
        match old_files.get(se.relative_path.as_path()) {
            Some(old) => {
                seen_old.insert(old.path.as_path());
                let size_match = se.size == old.size;
                let same = if size_match && se.mtime == old.mtime {
                    true
                } else if p.use_checksum && size_match && old.sha256.is_some() {
                    hash_file(&p.source.join(&se.relative_path))
                        .map(|h| Some(h.as_str()) == old.sha256.as_deref())
                        .unwrap_or(false)
                } else {
                    false
                };
                if same {
                    // content is unchanged: keep the data reference, refresh metadata
                    let mut e = (*old).clone();
                    e.mtime = se.mtime;
                    e.unix_mode = se.unix_mode;
                    unchanged.push(e);
                } else {
                    rearchive.insert(se.relative_path.clone());
                    n_changed += 1;
                }
            }
            None => {
                rearchive.insert(se.relative_path.clone());
                n_new += 1;
            }
        }
    }
    let deleted = old_files.len() - seen_old.len();

    // A dedup alias is only valid while the file it points to still holds the same
    // content. If that origin changed or disappeared, the alias must carry its own data.
    let old_map = old_idx.path_map();
    let kept: HashSet<PathBuf> = unchanged.iter().map(|e| e.path.clone()).collect();
    let mut demoted = 0usize;
    unchanged.retain_mut(|e| {
        if e.dedup_of.is_none() {
            return true;
        }
        let origin = old_map
            .get(e.path.as_path())
            .map(|o| old_idx.origin_of(&old_map, o));
        match origin {
            Some(o) if o.dedup_of.is_none() && kept.contains(&o.path) => {
                e.dedup_of = Some(o.path.clone());
                true
            }
            _ => {
                rearchive.insert(e.path.clone());
                demoted += 1;
                false
            }
        }
    });

    out.println(&format!(
        "  Unchanged: {}   Changed: {}   New: {}   Deleted: {}",
        unchanged.len().to_string().green(),
        n_changed.to_string().yellow(),
        n_new.to_string().cyan(),
        deleted.to_string().red()
    ));
    if demoted > 0 {
        out.println(&format!(
            "  {} duplicate file(s) lost their dedup origin and will be stored again",
            demoted
        ));
    }

    let delta_scan: Vec<ScanEntry> = scan
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File && rearchive.contains(&e.relative_path))
        .cloned()
        .collect();
    let delta_size: u64 = delta_scan.iter().map(|e| e.size).sum();

    if out.dry_run {
        out.dry(&format!(
            "would re-archive {} file(s) ({})",
            delta_scan.len(),
            human(delta_size)
        ));
        return Ok(());
    }

    // the old parts that unchanged files still need must exist
    let mut missing_parts = BTreeSet::new();
    for loc in unchanged
        .iter()
        .filter(|e| e.dedup_of.is_none())
        .filter_map(|e| e.loc)
    {
        let part = old_idx.part_path(old_index_dir, loc.part);
        if !part.exists() {
            missing_parts.insert(part);
        }
    }
    if !missing_parts.is_empty() {
        bail!(
            "Parts of the old archive are missing, the update would be unusable:\n  {}",
            missing_parts
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }

    out.println(&format!(
        "  Re-archiving {} file(s) ({})",
        delta_scan.len().to_string().yellow(),
        human(delta_size)
    ));

    let mut delta_idx = ArchivumIndex::build(delta_scan, p.zstd_level);
    compute_checksums(p.source, &mut delta_idx, p.threads, p.dedup)?;
    write_archive(
        p.source,
        &mut delta_idx,
        &WriteOpts {
            out_dir: p.output_dir.to_path_buf(),
            algo: algo.clone(),
            zstd_level: p.zstd_level,
            zstd_threads: p.zstd_threads,
            split_bytes: p.split_bytes,
            split_files: p.split_files,
            workers: p.threads,
        },
    )?;

    // New parts first, then every part of the old archive, re-expressed relative to
    // the new directory so chains of updates keep resolving correctly.
    let new_parts = delta_idx.header.parts.len() as u32;
    let mut parts: Vec<PartRef> = delta_idx.header.parts.clone();
    for old in &old_idx.header.parts {
        let abs = old_index_dir.join(&old.path);
        parts.push(PartRef {
            path: relative_path(p.output_dir, &abs)
                .to_string_lossy()
                .replace('\\', "/"),
            compression: old.compression.clone(),
            size: old.size,
        });
    }

    let mut all_entries: Vec<IndexEntry> = vec![];
    for mut e in unchanged {
        if let Some(l) = e.loc.as_mut() {
            l.part += new_parts;
        }
        all_entries.push(e);
    }
    all_entries.extend(delta_idx.entries);

    // directories and symlinks always come from the fresh scan
    let others: Vec<ScanEntry> = scan
        .entries
        .iter()
        .filter(|e| e.entry_type != EntryType::File)
        .cloned()
        .collect();
    all_entries.extend(ArchivumIndex::build(others, p.zstd_level).entries);
    all_entries.sort_by(|a, b| a.path.cmp(&b.path));

    let ts = now();
    let mut new_idx = ArchivumIndex {
        header: IndexHeader {
            version: INDEX_VERSION,
            created_at_unix: ts,
            created_at_human: fmt_time(ts),
            total_files: 0,
            total_dirs: 0,
            total_symlinks: 0,
            total_size: 0,
            zstd_level: p.zstd_level,
            notes: format!("Incremental update from {}", p.old_index_path.display()),
            parts,
        },
        entries: all_entries,
    };
    new_idx.recount();

    let new_index_path = p.output_dir.join(INDEX_NAME);
    new_idx.write_signed(&new_index_path, p.sign_key)?;

    out.println("");
    out.println(&format!(
        "  {} {}",
        "Incremental archive created:".green().bold(),
        new_index_path.display().to_string().yellow()
    ));
    out.println(&format!(
        "  Older parts referenced from: {}",
        old_index_dir.display().to_string().dimmed()
    ));

    Ok(())
}
