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

use anyhow::{Context, Result};
use colored::Colorize;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::compress::CompressionAlgo;
use crate::fsutil;
use crate::output::OutputCtx;
use crate::scan::{EntryType, ScanEntry};
use crate::sign::{self, SigStatus};
use crate::utils::{fmt_time, human, now};

/// v4: every part carries its own path and compression; every file points at the
/// frame (`part`, `off`, `len`) that holds it.
pub const INDEX_VERSION: u32 = 4;

/// One tar part on disk. `path` is relative to the directory of the index file and
/// may point into another directory (incremental archives reuse older parts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartRef {
    pub path: String,
    pub compression: CompressionAlgo,
    /// compressed size in bytes, used to spot truncated parts without reading them
    pub size: u64,
}

/// Location of the compressed frame holding a file. `len == 0` means the frame
/// boundaries are unknown (index rebuilt by `repair`): decode the whole part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Loc {
    pub part: u32,
    pub off: u64,
    pub len: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexHeader {
    pub version: u32,
    pub created_at_unix: u64,
    pub created_at_human: String,
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_symlinks: u64,
    pub total_size: u64,
    #[serde(default = "default_zstd_level")]
    pub zstd_level: i32,
    #[serde(default)]
    pub notes: String,
    pub parts: Vec<PartRef>,
}

fn default_zstd_level() -> i32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub entry_type: EntryType,
    pub size: u64,
    pub mtime: Option<u64>,
    pub unix_mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<PathBuf>,
    /// if set, this file is a dedup alias and not stored in any part
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_of: Option<PathBuf>,
    /// where the data lives (regular files that are not aliases)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loc: Option<Loc>,
}

/// How much to trust the `.b3` companion file.
#[derive(Debug, PartialEq, Eq)]
pub enum SealStatus {
    Valid,
    Missing,
    Invalid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchivumIndex {
    pub header: IndexHeader,
    pub entries: Vec<IndexEntry>,
}

impl ArchivumIndex {
    pub fn build(scan: Vec<ScanEntry>, zstd_level: i32) -> Self {
        let mut files = 0u64;
        let mut dirs = 0u64;
        let mut symlinks = 0u64;
        let mut size = 0u64;

        let entries = scan
            .into_iter()
            .map(|e| {
                match e.entry_type {
                    EntryType::File => {
                        files += 1;
                        size += e.size;
                    }
                    EntryType::Directory => dirs += 1,
                    EntryType::Symlink => symlinks += 1,
                }
                IndexEntry {
                    path: e.relative_path,
                    entry_type: e.entry_type,
                    size: e.size,
                    mtime: e.mtime,
                    unix_mode: e.unix_mode,
                    sha256: None,
                    symlink_target: e.symlink_target,
                    dedup_of: None,
                    loc: None,
                }
            })
            .collect();

        let ts = now();
        Self {
            header: IndexHeader {
                version: INDEX_VERSION,
                created_at_unix: ts,
                created_at_human: fmt_time(ts),
                total_files: files,
                total_dirs: dirs,
                total_symlinks: symlinks,
                total_size: size,
                zstd_level,
                notes: String::new(),
                parts: vec![],
            },
            entries,
        }
    }

    /// Recompute the totals in the header from the entries.
    pub fn recount(&mut self) {
        let (mut f, mut d, mut l, mut s) = (0u64, 0u64, 0u64, 0u64);
        for e in &self.entries {
            match e.entry_type {
                EntryType::File => {
                    f += 1;
                    s += e.size;
                }
                EntryType::Directory => d += 1,
                EntryType::Symlink => l += 1,
            }
        }
        self.header.total_files = f;
        self.header.total_dirs = d;
        self.header.total_symlinks = l;
        self.header.total_size = s;
    }

    pub fn compression_summary(&self) -> String {
        let mut names: Vec<&str> = vec![];
        for p in &self.header.parts {
            let n = p.compression.name();
            if !names.contains(&n) {
                names.push(n);
            }
        }
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join("+")
        }
    }

    pub fn part_path(&self, index_dir: &Path, part: u32) -> PathBuf {
        let rel = self
            .header
            .parts
            .get(part as usize)
            .map(|p| p.path.as_str())
            .unwrap_or("");
        index_dir.join(rel)
    }

    /// Decoded tar stream of one frame.
    pub fn open_frame(&self, index_dir: &Path, loc: Loc) -> Result<Box<dyn Read>> {
        let part = self
            .header
            .parts
            .get(loc.part as usize)
            .with_context(|| format!("part {} does not exist", loc.part))?;
        part.compression
            .open_stream(&index_dir.join(&part.path), loc.off, loc.len)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        self.write_signed(path, None)
    }

    pub fn write_signed(&self, path: &Path, key: Option<&ed25519_dalek::SigningKey>) -> Result<()> {
        let json = serde_json::to_vec_pretty(self)?;
        fsutil::write_atomic(path, &json)?;

        // companion .b3 file: catches corruption and accidental edits
        // (an unkeyed hash; use --sign for protection against a deliberate attacker)
        let hash = blake3::hash(&json);
        fsutil::write_atomic(&Self::seal_path(path), hash.to_hex().as_bytes())?;

        let sig_path = Self::sig_path(path);
        match key {
            Some(k) => fsutil::write_atomic(&sig_path, sign::sign(k, &json).as_bytes())?,
            // never leave a signature of an older index next to a new one
            None => {
                let _ = std::fs::remove_file(&sig_path);
            }
        }
        Ok(())
    }

    fn seal_path(path: &Path) -> PathBuf {
        Self::sibling(path, ".b3")
    }

    fn sig_path(path: &Path) -> PathBuf {
        Self::sibling(path, ".sig")
    }

    fn sibling(path: &Path, suffix: &str) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        PathBuf::from(s)
    }

    pub fn seal_status(path: &Path) -> Result<SealStatus> {
        let seal = Self::seal_path(path);
        if !seal.exists() {
            return Ok(SealStatus::Missing);
        }
        let bytes = std::fs::read(path)?;
        let stored = std::fs::read_to_string(&seal)?;
        Ok(if blake3::hash(&bytes).to_hex().as_str() == stored.trim() {
            SealStatus::Valid
        } else {
            SealStatus::Invalid
        })
    }

    /// Signature state of the index next to `path`, judged against the key given
    /// with `--pubkey` (if any).
    pub fn signature_status(path: &Path) -> Result<SigStatus> {
        let sig = Self::sig_path(path);
        if !sig.exists() {
            return Ok(SigStatus::Absent);
        }
        let bytes = std::fs::read(path)?;
        Ok(sign::check(
            &std::fs::read_to_string(sig)?,
            &bytes,
            sign::trusted(),
        ))
    }

    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;

        let b3_path = Self::seal_path(path);
        if b3_path.exists() {
            let stored_hex = std::fs::read_to_string(&b3_path)?;
            let stored_hex = stored_hex.trim();
            let actual = blake3::hash(&bytes);
            if actual.to_hex().as_str() != stored_hex {
                anyhow::bail!(
                    "Index integrity check FAILED for {}.\n  \
                     The index may have been modified or corrupted.\n  \
                     Expected: {}\n  Got:      {}",
                    path.display(),
                    stored_hex,
                    actual.to_hex()
                );
            }
        }

        if sign::trusted().is_some() {
            match Self::signature_status(path)? {
                SigStatus::Valid { trusted: true, .. } => {}
                SigStatus::Valid { signer, .. } => anyhow::bail!(
                    "{} is signed by {}, which is not the key given with --pubkey",
                    path.display(),
                    signer
                ),
                SigStatus::Absent => anyhow::bail!(
                    "{} has no signature, but --pubkey requires one",
                    path.display()
                ),
                SigStatus::Invalid(why) => {
                    anyhow::bail!("Signature check FAILED for {}: {}", path.display(), why)
                }
            }
        }

        let idx: Self = serde_json::from_slice(&bytes)?;
        idx.validate()?;
        Ok(idx)
    }

    /// Reject indexes we cannot safely interpret (other format, unsafe paths,
    /// dangling dedup references, bad part references).
    fn validate(&self) -> Result<()> {
        if self.header.version != INDEX_VERSION {
            anyhow::bail!(
                "Index format v{} is not supported by this build (needs v{}). \
                 Archives written by older versions must be restored with that version.",
                self.header.version,
                INDEX_VERSION
            );
        }
        for p in &self.header.parts {
            if p.path.is_empty() || Path::new(&p.path).is_absolute() {
                anyhow::bail!("Invalid part path in index: '{}'", p.path);
            }
        }

        let map = self.path_map();
        for e in &self.entries {
            if !is_safe_relative(&e.path) {
                anyhow::bail!("Unsafe path in index: {}", e.path.display());
            }
            if e.entry_type != EntryType::File {
                continue;
            }
            match (&e.dedup_of, &e.loc) {
                (Some(o), _) => match map.get(o.as_path()) {
                    Some(t) if t.entry_type == EntryType::File && t.path != e.path => {}
                    _ => anyhow::bail!(
                        "Entry {} is a dedup alias of {} which is not a file in this index",
                        e.path.display(),
                        o.display()
                    ),
                },
                (None, Some(l)) => {
                    if l.part as usize >= self.header.parts.len() {
                        anyhow::bail!(
                            "Entry {} refers to part {} which does not exist",
                            e.path.display(),
                            l.part
                        );
                    }
                }
                (None, None) => anyhow::bail!("Entry {} has no data location", e.path.display()),
            }
        }
        Ok(())
    }

    pub fn path_map(&self) -> HashMap<&Path, &IndexEntry> {
        self.entries.iter().map(|e| (e.path.as_path(), e)).collect()
    }

    /// The entry that physically holds the data for `entry` (follows dedup aliases).
    pub fn origin_of<'a>(
        &'a self,
        map: &HashMap<&Path, &'a IndexEntry>,
        entry: &'a IndexEntry,
    ) -> &'a IndexEntry {
        let mut cur = entry;
        for _ in 0..16 {
            match cur.dedup_of.as_deref().and_then(|p| map.get(p)) {
                Some(next) => cur = next,
                None => break,
            }
        }
        cur
    }

    pub fn print_summary(
        &self,
        verbose: bool,
        filter: Option<&str>,
        out: &OutputCtx,
    ) -> Result<()> {
        let h = &self.header;
        out.println(&"─".repeat(65).dimmed().to_string());
        out.println(&format!(
            "{}  v{}",
            " ▲ Archivum Archive".black().on_cyan().bold(),
            h.version
        ));
        out.println(&"─".repeat(65).dimmed().to_string());
        out.println(&format!("  Created   : {}", h.created_at_human.yellow()));
        if !h.notes.is_empty() {
            out.println(&format!("  Notes     : {}", h.notes.cyan()));
        }
        out.println(&format!(
            "  Files     : {}",
            h.total_files.to_string().cyan()
        ));
        out.println(&format!(
            "  Dirs      : {}",
            h.total_dirs.to_string().cyan()
        ));
        out.println(&format!(
            "  Symlinks  : {}",
            h.total_symlinks.to_string().cyan()
        ));
        out.println(&format!("  Total size: {}", human(h.total_size).cyan()));
        out.println(&format!(
            "  Parts     : {}",
            h.parts.len().to_string().cyan()
        ));
        out.println(&format!(
            "  Compress  : {}",
            self.compression_summary().green()
        ));
        if h.parts
            .iter()
            .any(|p| p.compression == CompressionAlgo::Zstd)
        {
            out.println(&format!(
                "  Zstd lvl  : {}",
                h.zstd_level.to_string().green()
            ));
        }

        let deduped = self.entries.iter().filter(|e| e.dedup_of.is_some()).count();
        if deduped > 0 {
            out.println(&format!(
                "  Deduped   : {} files",
                deduped.to_string().yellow()
            ));
        }

        if verbose || filter.is_some() {
            let globset = filter
                .map(|f| -> Result<GlobSet> {
                    let mut b = GlobSetBuilder::new();
                    b.add(Glob::new(f)?);
                    Ok(b.build()?)
                })
                .transpose()?;

            out.println(&"─".repeat(65).dimmed().to_string());
            out.println(&format!(
                "  {:<8} {:<12} {:<10} {}",
                "PART".dimmed(),
                "SIZE".dimmed(),
                "TYPE".dimmed(),
                "PATH".dimmed()
            ));
            out.println(&"─".repeat(65).dimmed().to_string());

            for e in &self.entries {
                if let Some(gs) = &globset {
                    if !gs.is_match(&e.path) {
                        continue;
                    }
                }
                let type_str = match e.entry_type {
                    EntryType::File => "file".green(),
                    EntryType::Directory => "dir".blue(),
                    EntryType::Symlink => "symlink".yellow(),
                };
                let dedup_tag = if e.dedup_of.is_some() {
                    " [dedup]".dimmed().to_string()
                } else {
                    String::new()
                };
                out.println(&format!(
                    "  {:<8} {:<12} {:<10} {}{}",
                    e.loc
                        .map(|l| format!("part{:03}", l.part))
                        .unwrap_or_else(|| "-".into()),
                    human(e.size),
                    type_str,
                    e.path.display(),
                    dedup_tag
                ));
            }
        }

        out.println(&"─".repeat(65).dimmed().to_string());
        Ok(())
    }

    pub fn print_summary_json(&self) -> Result<()> {
        println!("{}", serde_json::to_string_pretty(self)?);
        Ok(())
    }
}

/// Relative path made only of normal components (no `..`, no root, no drive prefix).
pub fn is_safe_relative(p: &Path) -> bool {
    !p.as_os_str().is_empty()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
