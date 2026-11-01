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
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::checksum::HashingReader;
use crate::compress::CompressionAlgo;
use crate::fsutil::{INDEX_NAME, ensure_clean_output, same_path};
use crate::index::{ArchivumIndex, INDEX_VERSION, IndexEntry, IndexHeader, Loc};
use crate::output::OutputCtx;
use crate::scan::EntryType;
use crate::tar_writer::{ArchiveSink, WriteOpts};
use crate::utils::{fmt_time, human, now};

pub struct MergeParams<'a> {
    pub indexes: &'a [PathBuf],
    pub output_dir: &'a Path,
    pub split_bytes: u64,
    pub algo: CompressionAlgo,
    pub zstd_level: i32,
    pub zstd_threads: u32,
    pub workers: usize,
    pub overwrite: bool,
    pub sign_key: Option<&'a ed25519_dalek::SigningKey>,
}

/// (source archive, frame inside it)
type FrameKey = (usize, Loc);

/// Merge several archives into one. When the same path appears in more than one
/// input, the archive with the newest creation time wins (later arguments win ties).
/// Every source part is read exactly once and streamed, so memory use stays flat.
pub fn merge(p: MergeParams, out: &OutputCtx) -> Result<()> {
    out.println(&format!(
        "{} {} archive(s) -> {}",
        "Merging:".cyan().bold(),
        p.indexes.len(),
        p.output_dir.display().to_string().yellow()
    ));
    out.println("");

    if p.indexes.len() < 2 {
        bail!("Merge needs at least two indexes");
    }

    let mut sources: Vec<(PathBuf, ArchivumIndex)> = vec![];
    for path in p.indexes {
        let idx = ArchivumIndex::read(path)
            .with_context(|| format!("Cannot read index: {}", path.display()))?;
        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        if same_path(&dir, p.output_dir) {
            bail!("The output directory must differ from every input archive directory");
        }
        sources.push((dir, idx));
    }

    // path -> (source, entry) of the winning version
    let mut winner: HashMap<&Path, (usize, usize)> = HashMap::new();
    for (si, (_, idx)) in sources.iter().enumerate() {
        for (ei, e) in idx.entries.iter().enumerate() {
            let older = winner.get(e.path.as_path()).is_some_and(|&(osi, _)| {
                sources[osi].1.header.created_at_unix > idx.header.created_at_unix
            });
            if !older {
                winner.insert(e.path.as_path(), (si, ei));
            }
        }
    }

    // Plan: which tar members to copy where, and the entries of the merged index.
    let mut out_entries: Vec<IndexEntry> = vec![];
    let mut jobs: BTreeMap<FrameKey, HashMap<PathBuf, usize>> = BTreeMap::new();
    let mut total_bytes = 0u64;

    for (si, (_, idx)) in sources.iter().enumerate() {
        let map = idx.path_map();
        let index_of: HashMap<&Path, usize> = idx
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.as_path(), i))
            .collect();
        // origin path -> first alias that became a real copy
        let mut materialized: HashMap<PathBuf, PathBuf> = HashMap::new();

        for (ei, e) in idx.entries.iter().enumerate() {
            if winner[e.path.as_path()] != (si, ei) {
                continue;
            }
            let mut ne = e.clone();
            ne.loc = None;

            if e.entry_type != EntryType::File {
                out_entries.push(ne);
                continue;
            }

            let origin = idx.origin_of(&map, e);
            let origin_wins = e.dedup_of.is_some()
                && index_of
                    .get(origin.path.as_path())
                    .is_some_and(|&oi| winner[origin.path.as_path()] == (si, oi));

            if e.dedup_of.is_some() && origin_wins {
                ne.dedup_of = Some(origin.path.clone());
                out_entries.push(ne);
            } else if let Some(first) = materialized
                .get(&origin.path)
                .filter(|_| e.dedup_of.is_some())
            {
                ne.dedup_of = Some(first.clone());
                out_entries.push(ne);
            } else {
                // this entry carries real data into the merged archive
                ne.dedup_of = None;
                ne.sha256 = origin.sha256.clone();
                ne.size = origin.size;
                let member = origin.path.clone();
                if e.dedup_of.is_some() {
                    materialized.insert(origin.path.clone(), e.path.clone());
                }
                total_bytes += ne.size;
                let loc = origin
                    .loc
                    .with_context(|| format!("{} has no data location", origin.path.display()))?;
                jobs.entry((si, loc))
                    .or_default()
                    .insert(member, out_entries.len());
                out_entries.push(ne);
            }
        }
    }

    let n_files = out_entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File)
        .count();
    out.println(&format!(
        "  {} file(s), {} data to copy",
        n_files.to_string().yellow(),
        human(total_bytes)
    ));

    if out.dry_run {
        out.dry(&format!(
            "would write merged archive to {}",
            p.output_dir.display()
        ));
        return Ok(());
    }

    ensure_clean_output(p.output_dir, p.overwrite)?;
    std::fs::create_dir_all(p.output_dir)
        .with_context(|| format!("Cannot create {}", p.output_dir.display()))?;

    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} Merging    [{bar:40.cyan/blue}] {bytes}/{total_bytes}  ETA {eta}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let mut sink = ArchiveSink::new(&WriteOpts {
        out_dir: p.output_dir.to_path_buf(),
        algo: p.algo.clone(),
        zstd_level: p.zstd_level,
        zstd_threads: p.zstd_threads,
        split_bytes: p.split_bytes,
        split_files: 0,
        workers: p.workers,
    })?;
    let mut tickets: Vec<(usize, usize)> = vec![];

    for ((si, loc), mut wants) in jobs {
        let (dir, idx) = &sources[si];
        let where_ = idx.part_path(dir, loc.part);
        let reader = idx.open_frame(dir, loc)?;
        let mut archive = tar::Archive::new(reader);

        for item in archive
            .entries()
            .with_context(|| format!("Cannot read {}", where_.display()))?
        {
            let mut item =
                item.with_context(|| format!("Damaged archive data in {}", where_.display()))?;
            let member = item.path()?.into_owned();
            let Some(oi) = wants.remove(&member) else {
                continue;
            };
            let (dest, size, mode, mtime, expected) = {
                let ne = &out_entries[oi];
                (
                    ne.path.clone(),
                    ne.size,
                    ne.unix_mode,
                    ne.mtime,
                    ne.sha256.clone(),
                )
            };
            let mut hr = HashingReader::new(&mut item);
            let ticket = sink.append(&dest, size, mode, mtime, &mut hr)?;
            let (hash, bytes) = hr.finalize();
            if bytes != size || expected.is_some_and(|h| h != hash) {
                bail!(
                    "{} is corrupt in {} (size/checksum mismatch); merge aborted",
                    member.display(),
                    where_.display()
                );
            }
            tickets.push((oi, ticket));
            pb.inc(size);
        }

        if !wants.is_empty() {
            let names: Vec<String> = wants
                .keys()
                .take(10)
                .map(|p| p.display().to_string())
                .collect();
            bail!(
                "{} file(s) listed in the index are missing from {}:\n  {}",
                wants.len(),
                where_.display(),
                names.join("\n  ")
            );
        }
    }

    let committed = sink.commit()?;
    for (oi, t) in tickets {
        out_entries[oi].loc = Some(committed.frames[t]);
    }
    pb.finish_with_message("merge done".green().to_string());

    out_entries.sort_by(|a, b| a.path.cmp(&b.path));
    let ts = now();
    let mut merged = ArchivumIndex {
        header: IndexHeader {
            version: INDEX_VERSION,
            created_at_unix: ts,
            created_at_human: fmt_time(ts),
            total_files: 0,
            total_dirs: 0,
            total_symlinks: 0,
            total_size: 0,
            zstd_level: p.zstd_level,
            notes: format!("Merged from {} archives", sources.len()),
            parts: committed.parts,
        },
        entries: out_entries,
    };
    merged.recount();
    let index_path = p.output_dir.join(INDEX_NAME);
    merged.write_signed(&index_path, p.sign_key)?;
    let (files, dirs, symlinks, total_parts) = (
        merged.header.total_files,
        merged.header.total_dirs,
        merged.header.total_symlinks,
        merged.header.parts.len(),
    );

    out.println(&format!(
        "  Merged: {} files, {} dirs, {} symlinks in {} parts",
        files, dirs, symlinks, total_parts
    ));
    out.println(&format!(
        "\n  {} {}",
        "Merged index written to:".green().bold(),
        index_path.display().to_string().yellow()
    ));
    Ok(())
}
