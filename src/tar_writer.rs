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

//! Frame-based archive writer.
//!
//! A part is a sequence of independently compressed *frames*. Each frame holds a
//! few small files (batched up to `FRAME_TARGET` bytes of tar) or one large file,
//! and the index records where every file's frame starts. Reading one file thus
//! only decompresses its own frame, frames can be written, restored and verified
//! in parallel, and damage stays local to a frame. Concatenating the frames of a
//! part (plus the final end-of-archive frame) gives an ordinary tar stream, so
//! the standard tools still understand the parts.

use anyhow::{Context, Result, bail};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::checksum::HashingReader;
use crate::compress::{CompressionAlgo, effective_zstd_threads};
use crate::fsutil::tmp_path;
use crate::index::{ArchivumIndex, Loc, PartRef};
use crate::scan::EntryType;
use crate::utils::human;

/// Small files are batched until a frame holds at least this much raw tar.
const FRAME_TARGET: usize = 1 << 20;
/// Files at least this large get their own streamed frame (multithreaded for zstd).
const BIG_FILE: u64 = 4 << 20;

#[derive(Clone)]
pub struct WriteOpts {
    pub out_dir: PathBuf,
    pub algo: CompressionAlgo,
    pub zstd_level: i32,
    /// zstd workers used inside large frames (0 = all cores)
    pub zstd_threads: u32,
    pub split_bytes: u64,
    /// 0 = no limit on files per part
    pub split_files: usize,
    /// threads compressing small frames in parallel
    pub workers: usize,
}

pub struct Committed {
    pub parts: Vec<PartRef>,
    /// indexed by the ticket `append` returned
    pub frames: Vec<Loc>,
}

struct PartFile {
    index: u32,
    w: BufWriter<File>,
    pos: u64,
    name: String,
    tmp: PathBuf,
    dest: PathBuf,
}

struct Pending {
    ticket: usize,
    part: u32,
    rx: Receiver<Result<Vec<u8>>>,
}

struct CountW<'a> {
    inner: &'a mut BufWriter<File>,
    n: u64,
}

impl Write for CountW<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.n += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes parts under a `.tmp` name and only renames them into place in
/// `commit()`, so a failed run never leaves a half-written `data.partNNN` behind.
pub struct ArchiveSink {
    opts: WriteOpts,
    pool: rayon::ThreadPool,
    max_queue: usize,

    // planning state: which part the next file belongs to
    plan_part: u32,
    plan_raw: u64,
    plan_files: usize,

    batch: Vec<u8>,
    cur_ticket: Option<usize>,
    queue: VecDeque<Pending>,
    frames: Vec<Option<Loc>>,

    part: Option<PartFile>,
    parts_done: Vec<PartRef>,
    finished: Vec<(PathBuf, PathBuf)>,
}

fn pad_len(size: u64) -> usize {
    ((512 - size % 512) % 512) as usize
}

/// The tar header blocks (including GNU long-name entries) for one file.
fn tar_head(rel: &Path, header: &mut tar::Header) -> Result<Vec<u8>> {
    let mut b = tar::Builder::new(Vec::new());
    b.append_data(header, rel, io::empty())
        .with_context(|| format!("Cannot build tar header for {}", rel.display()))?;
    let mut v = b.into_inner()?;
    // drop the end-of-archive blocks; the part gets its own at the very end
    v.truncate(v.len().saturating_sub(1024));
    Ok(v)
}

/// Estimated bytes a file occupies in an uncompressed tar (header + padded data,
/// plus the extra GNU long-name entry for long paths).
fn tar_overhead(rel: &Path, size: u64) -> u64 {
    let mut n = 512 + size.div_ceil(512) * 512;
    let len = rel.as_os_str().len() as u64;
    if len >= 100 {
        n += 512 + (len + 1).div_ceil(512) * 512;
    }
    n
}

impl ArchiveSink {
    pub fn new(opts: &WriteOpts) -> Result<Self> {
        let workers = opts.workers.max(1);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;
        Ok(Self {
            opts: opts.clone(),
            pool,
            max_queue: workers * 2 + 2,
            plan_part: 0,
            plan_raw: 0,
            plan_files: 0,
            batch: Vec::new(),
            cur_ticket: None,
            queue: VecDeque::new(),
            frames: vec![],
            part: None,
            parts_done: vec![],
            finished: vec![],
        })
    }

    fn new_ticket(&mut self) -> usize {
        self.frames.push(None);
        self.frames.len() - 1
    }

    /// Append one regular file and return the ticket of the frame it landed in.
    /// The caller must check that `data` really yielded `size` unchanged bytes
    /// (see `HashingReader`); this function already rejects short reads.
    pub fn append(
        &mut self,
        rel: &Path,
        size: u64,
        mode: Option<u32>,
        mtime: Option<u64>,
        data: &mut dyn Read,
    ) -> Result<usize> {
        let overhead = tar_overhead(rel, size);
        let byte_overflow = self.plan_raw > 0 && self.plan_raw + overhead > self.opts.split_bytes;
        let file_overflow = self.opts.split_files > 0 && self.plan_files >= self.opts.split_files;
        if byte_overflow || file_overflow {
            self.flush_batch()?;
            self.plan_part += 1;
            self.plan_raw = 0;
            self.plan_files = 0;
        }

        // uid/gid are fixed and everything else comes from the index, so the
        // same tree always produces the same bytes.
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(size);
        header.set_mode(mode.unwrap_or(0o644) & 0o7777);
        header.set_mtime(mtime.unwrap_or(0));
        header.set_uid(0);
        header.set_gid(0);
        let head = tar_head(rel, &mut header)?;

        self.plan_raw += overhead;
        self.plan_files += 1;

        let changed = || anyhow::anyhow!("{} changed while it was being archived", rel.display());

        if size < BIG_FILE {
            let ticket = match self.cur_ticket {
                Some(t) => t,
                None => {
                    let t = self.new_ticket();
                    self.cur_ticket = Some(t);
                    t
                }
            };
            self.batch.extend_from_slice(&head);
            let copied = io::copy(&mut Read::take(&mut *data, size), &mut self.batch)
                .with_context(|| format!("Cannot read {}", rel.display()))?;
            if copied != size {
                return Err(changed());
            }
            self.batch.resize(self.batch.len() + pad_len(size), 0);
            if self.batch.len() >= FRAME_TARGET {
                self.flush_batch()?;
            }
            return Ok(ticket);
        }

        // large file: its own frame, streamed (frames before it go out first)
        self.flush_batch()?;
        self.drain()?;
        self.ensure_part(self.plan_part)?;
        let ticket = self.new_ticket();
        let algo = self.opts.algo.clone();
        let level = self.opts.zstd_level;
        let workers = effective_zstd_threads(self.opts.zstd_threads);

        let part = self.part.as_mut().expect("part is open");
        let start = part.pos;
        let mut cw = CountW {
            inner: &mut part.w,
            n: 0,
        };
        algo.encode_frame(&mut cw, level, workers, |w| {
            w.write_all(&head)?;
            let n = io::copy(&mut Read::take(&mut *data, size), w)
                .with_context(|| format!("Cannot read {}", rel.display()))?;
            if n != size {
                return Err(changed());
            }
            w.write_all(&vec![0u8; pad_len(size)])?;
            Ok(())
        })?;
        let len = cw.n;
        part.pos += len;
        self.frames[ticket] = Some(Loc {
            part: part.index,
            off: start,
            len,
        });
        Ok(ticket)
    }

    /// Hand the open batch to a worker thread for compression.
    fn flush_batch(&mut self) -> Result<()> {
        let Some(ticket) = self.cur_ticket.take() else {
            return Ok(());
        };
        let raw = std::mem::take(&mut self.batch);
        let (tx, rx) = mpsc::channel();
        let algo = self.opts.algo.clone();
        let level = self.opts.zstd_level;
        self.pool.spawn(move || {
            let r = algo.encode_frame(Vec::new(), level, 0, |w| {
                w.write_all(&raw)?;
                Ok(())
            });
            let _ = tx.send(r);
        });
        self.queue.push_back(Pending {
            ticket,
            part: self.plan_part,
            rx,
        });
        while self.queue.len() > self.max_queue {
            self.write_oldest()?;
        }
        Ok(())
    }

    fn write_oldest(&mut self) -> Result<()> {
        let Some(p) = self.queue.pop_front() else {
            return Ok(());
        };
        let bytes = p.rx.recv().context("compression worker failed")??;
        self.ensure_part(p.part)?;
        let part = self.part.as_mut().expect("part is open");
        part.w
            .write_all(&bytes)
            .with_context(|| format!("Cannot write {}", part.tmp.display()))?;
        self.frames[p.ticket] = Some(Loc {
            part: part.index,
            off: part.pos,
            len: bytes.len() as u64,
        });
        part.pos += bytes.len() as u64;
        Ok(())
    }

    fn drain(&mut self) -> Result<()> {
        while !self.queue.is_empty() {
            self.write_oldest()?;
        }
        Ok(())
    }

    fn ensure_part(&mut self, idx: u32) -> Result<()> {
        if self.part.as_ref().is_some_and(|p| p.index == idx) {
            return Ok(());
        }
        self.close_part()?;
        debug_assert_eq!(idx as usize, self.parts_done.len());
        let name = format!("data.part{:03}{}", idx, self.opts.algo.extension());
        let dest = self.opts.out_dir.join(&name);
        let tmp = tmp_path(&dest);
        let file =
            File::create(&tmp).with_context(|| format!("Cannot create {}", tmp.display()))?;
        self.part = Some(PartFile {
            index: idx,
            w: BufWriter::with_capacity(1 << 20, file),
            pos: 0,
            name,
            tmp,
            dest,
        });
        Ok(())
    }

    fn close_part(&mut self) -> Result<()> {
        let Some(mut part) = self.part.take() else {
            return Ok(());
        };
        let algo = self.opts.algo.clone();
        let level = self.opts.zstd_level;
        let tmp = part.tmp.clone();

        let result = (|| -> Result<()> {
            // end-of-archive marker, as its own frame
            let mut cw = CountW {
                inner: &mut part.w,
                n: 0,
            };
            algo.encode_frame(&mut cw, level, 0, |w| {
                w.write_all(&[0u8; 1024])?;
                Ok(())
            })?;
            part.pos += cw.n;
            part.w.flush()?;
            Ok(())
        })();
        let result = result.and_then(|()| {
            let file = part.w.into_inner().map_err(|e| e.into_error())?;
            file.sync_all()?;
            Ok(())
        });
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("Failed to finish {}", tmp.display()));
        }

        self.parts_done.push(PartRef {
            path: part.name,
            compression: algo,
            size: part.pos,
        });
        self.finished.push((part.tmp, part.dest));
        Ok(())
    }

    /// Finish everything and move the parts into their final names.
    pub fn commit(mut self) -> Result<Committed> {
        self.flush_batch()?;
        self.drain()?;
        self.close_part()?;
        for (tmp, dest) in &self.finished {
            fs::rename(tmp, dest)
                .with_context(|| format!("Cannot move {} into place", dest.display()))?;
        }
        self.finished.clear();
        let frames = self
            .frames
            .iter()
            .map(|f| f.ok_or_else(|| anyhow::anyhow!("internal error: frame was never written")))
            .collect::<Result<Vec<_>>>()?;
        Ok(Committed {
            parts: std::mem::take(&mut self.parts_done),
            frames,
        })
    }
}

impl Drop for ArchiveSink {
    fn drop(&mut self) {
        // not committed: remove temp parts
        if let Some(part) = self.part.take() {
            let tmp = part.tmp.clone();
            drop(part);
            let _ = fs::remove_file(tmp);
        }
        for (tmp, _) in self.finished.drain(..) {
            let _ = fs::remove_file(tmp);
        }
    }
}

/// Archive every regular file of `idx` (except dedup aliases) from `root`, and fill
/// in `idx.header.parts` and each entry's `loc`.
pub fn write_archive(root: &Path, idx: &mut ArchivumIndex, opts: &WriteOpts) -> Result<()> {
    let total_bytes: u64 = idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File && e.dedup_of.is_none())
        .map(|e| e.size)
        .sum();

    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} Archiving  [{bar:40.cyan/blue}] {bytes}/{total_bytes}  ETA {eta}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let mut sink = ArchiveSink::new(opts)?;
    let mut tickets: Vec<(usize, usize)> = vec![];

    for i in 0..idx.entries.len() {
        let e = &idx.entries[i];
        if e.entry_type != EntryType::File || e.dedup_of.is_some() {
            continue;
        }
        let (path, size, mode, mtime, expected) = (
            e.path.clone(),
            e.size,
            e.unix_mode,
            e.mtime,
            e.sha256.clone(),
        );

        let full = root.join(&path);
        let file = File::open(&full).with_context(|| format!("Cannot open {}", full.display()))?;
        let mut reader = HashingReader::new(BufReader::with_capacity(1 << 18, file));
        let ticket = sink.append(&path, size, mode, mtime, &mut reader)?;
        let (hash, bytes) = reader.finalize();

        // The checksum pass ran earlier; if the file changed in between, the
        // archive would contain data the index does not describe.
        if bytes != size || expected.as_deref().is_some_and(|h| h != hash) {
            bail!(
                "{} changed while the archive was being written; re-run once it is stable",
                path.display()
            );
        }

        tickets.push((i, ticket));
        pb.inc(size);
    }

    let committed = sink.commit()?;
    for (i, t) in tickets {
        idx.entries[i].loc = Some(committed.frames[t]);
    }
    let n_parts = committed.parts.len();
    idx.header.parts = committed.parts;

    pb.finish_with_message(format!(
        "{}  ({} parts, {})",
        "archive written".green(),
        n_parts,
        human(total_bytes)
    ));

    Ok(())
}
