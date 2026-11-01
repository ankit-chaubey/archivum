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
use colored::Colorize;
use hex::encode;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use crate::index::ArchivumIndex;
use crate::scan::EntryType;

/// Hash every regular file. Any unreadable file aborts the run: an archive with
/// a missing checksum would silently weaken verification later.
/// When `dedup` is set, files with identical content are marked as aliases.
pub fn compute_checksums(
    root: &Path,
    idx: &mut ArchivumIndex,
    num_threads: usize,
    dedup: bool,
) -> Result<()> {
    let work: Vec<(usize, PathBuf, u64)> = idx
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.entry_type == EntryType::File)
        .map(|(i, e)| (i, root.join(&e.path), e.size))
        .collect();

    let total: u64 = work.iter().map(|(_, _, s)| *s).sum();

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} Checksums  [{bar:40.cyan/blue}] {bytes}/{total_bytes}  {elapsed}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads.max(1))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;

    let results: Vec<(usize, Result<String>)> = pool.install(|| {
        work.par_iter()
            .map(|(pos, path, size)| {
                let r = hash_file(path);
                pb.inc(*size);
                (*pos, r)
            })
            .collect()
    });

    pb.finish_with_message("checksums done".green().to_string());

    let mut failures = vec![];
    for (pos, r) in results {
        match r {
            Ok(h) => idx.entries[pos].sha256 = Some(h),
            Err(e) => failures.push(format!("{}: {:#}", idx.entries[pos].path.display(), e)),
        }
    }
    if !failures.is_empty() {
        let shown: Vec<_> = failures.iter().take(10).cloned().collect();
        anyhow::bail!(
            "{} file(s) could not be read:\n  {}{}",
            failures.len(),
            shown.join("\n  "),
            if failures.len() > shown.len() {
                "\n  ..."
            } else {
                ""
            }
        );
    }

    for e in idx.entries.iter_mut() {
        e.dedup_of = None;
    }

    if dedup {
        let mut seen: HashMap<String, PathBuf> = HashMap::new();
        for entry in idx.entries.iter_mut() {
            if entry.entry_type != EntryType::File || entry.size == 0 {
                continue;
            }
            if let Some(hash) = entry.sha256.as_ref() {
                match seen.get(hash) {
                    Some(first) => entry.dedup_of = Some(first.clone()),
                    None => {
                        seen.insert(hash.clone(), entry.path.clone());
                    }
                }
            }
        }
    }

    Ok(())
}

// 128 KiB chunks, no temp files
pub fn hash_file(path: &Path) -> Result<String> {
    let file =
        File::open(path).map_err(|e| anyhow::anyhow!("Cannot open {}: {}", path.display(), e))?;
    let mut reader = BufReader::new(file);
    hash_reader(&mut reader)
}

pub fn hash_reader<R: Read + ?Sized>(reader: &mut R) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 131072];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(encode(hasher.finalize()))
}

/// Pass-through reader that hashes and counts everything read through it.
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R: Read> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
        }
    }

    /// (hex sha256, bytes read)
    pub fn finalize(self) -> (String, u64) {
        (encode(self.hasher.finalize()), self.bytes)
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}
