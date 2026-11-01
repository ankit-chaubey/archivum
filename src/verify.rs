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
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::checksum::HashingReader;
use crate::index::{ArchivumIndex, IndexEntry, Loc, SealStatus};
use crate::output::OutputCtx;
use crate::scan::EntryType;
use crate::sign::SigStatus;

#[derive(Default)]
struct FrameResult {
    ok: usize,
    /// (path, detail)
    corrupt: Vec<(PathBuf, String)>,
    missing: Vec<PathBuf>,
    unchecked: usize,
    extra: usize,
    error: Option<String>,
}

fn check_frame(
    idx: &ArchivumIndex,
    index_dir: &Path,
    loc: Loc,
    entries: &[&IndexEntry],
    pb: &ProgressBar,
    abort: &AtomicBool,
    stop_on_error: bool,
) -> FrameResult {
    let mut res = FrameResult::default();
    let mut want: HashMap<&Path, &IndexEntry> =
        entries.iter().map(|e| (e.path.as_path(), *e)).collect();
    let total: u64 = entries.iter().map(|e| e.size).sum();

    let reader = match idx.open_frame(index_dir, loc) {
        Ok(r) => r,
        Err(e) => {
            res.error = Some(format!("{:#}", e));
            res.missing = want.keys().map(|p| p.to_path_buf()).collect();
            pb.inc(total);
            return res;
        }
    };
    let mut archive = tar::Archive::new(reader);
    let members = match archive.entries() {
        Ok(m) => m,
        Err(e) => {
            res.error = Some(e.to_string());
            res.missing = want.keys().map(|p| p.to_path_buf()).collect();
            pb.inc(total);
            return res;
        }
    };

    for item in members {
        if stop_on_error && abort.load(Ordering::Relaxed) {
            return res;
        }
        let mut item = match item {
            Ok(i) => i,
            Err(e) => {
                res.error = Some(e.to_string());
                break;
            }
        };
        let path = match item.path() {
            Ok(p) => p.into_owned(),
            Err(_) => continue,
        };
        let Some(entry) = want.remove(path.as_path()) else {
            res.extra += 1;
            continue;
        };

        let mut hr = HashingReader::new(&mut item);
        let read = std::io::copy(&mut hr, &mut std::io::sink());
        let (hash, bytes) = hr.finalize();
        pb.inc(entry.size);

        if let Err(e) = read {
            res.error = Some(e.to_string());
            res.corrupt.push((path, format!("read error: {}", e)));
            break;
        }
        if bytes != entry.size {
            res.corrupt
                .push((path, format!("size {} (index says {})", bytes, entry.size)));
        } else {
            match &entry.sha256 {
                Some(exp) if *exp == hash => res.ok += 1,
                Some(exp) => res.corrupt.push((
                    path,
                    format!(
                        "expected {}... got {}...",
                        exp.get(..12).unwrap_or(exp),
                        &hash[..12]
                    ),
                )),
                None => res.unchecked += 1,
            }
        }
        if stop_on_error && !res.corrupt.is_empty() {
            abort.store(true, Ordering::Relaxed);
        }
    }

    // anything still wanted is not in the data (truncated or damaged frame)
    for (path, e) in want {
        res.missing.push(path.to_path_buf());
        pb.inc(e.size);
    }
    res
}

pub fn verify(
    index_path: &Path,
    continue_on_error: bool,
    threads: usize,
    out: &OutputCtx,
) -> Result<()> {
    let idx = ArchivumIndex::read(index_path)
        .with_context(|| format!("Cannot read index: {}", index_path.display()))?;
    let index_dir = index_path.parent().unwrap_or(Path::new("."));

    out.println(&format!(
        "{} {}",
        "Verifying archive:".cyan().bold(),
        index_path.display().to_string().yellow()
    ));
    out.println("");

    let seal = ArchivumIndex::seal_status(index_path)?;
    if seal == SealStatus::Missing {
        out.println(&format!(
            "  {} index seal (.b3) is missing; the index itself is not integrity-checked",
            "WARN".yellow().bold()
        ));
    }
    let sig = ArchivumIndex::signature_status(index_path)?;
    match &sig {
        SigStatus::Valid { signer, trusted } => out.println(&format!(
            "  {}  index signed by {}{}",
            "OK".green(),
            signer,
            if *trusted {
                ""
            } else {
                " (not the --pubkey key)"
            }
        )),
        SigStatus::Invalid(why) => out.println(&format!(
            "  {} index signature invalid: {}",
            "FAIL".red().bold(),
            why
        )),
        SigStatus::Absent => {}
    }

    // every file that owns data, grouped by the frame that holds it
    let mut groups: BTreeMap<Loc, Vec<&IndexEntry>> = BTreeMap::new();
    for e in idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File && e.dedup_of.is_none())
    {
        if let Some(loc) = e.loc {
            groups.entry(loc).or_default().push(e);
        }
    }

    // every referenced part must exist and have the recorded size
    let mut used_parts: Vec<u32> = groups.keys().map(|l| l.part).collect();
    used_parts.dedup();
    let mut bad_parts: Vec<u32> = vec![];
    for part in used_parts {
        let info = &idx.header.parts[part as usize];
        let path = idx.part_path(index_dir, part);
        match std::fs::metadata(&path) {
            Ok(m) if info.size == 0 || m.len() == info.size => out.println(&format!(
                "  {}  {}",
                "OK".green(),
                path.display().to_string().dimmed()
            )),
            Ok(m) => {
                out.println(&format!(
                    "  {} {} (size {} instead of {}: truncated or modified)",
                    "BAD".red().bold(),
                    path.display(),
                    m.len(),
                    info.size
                ));
                bad_parts.push(part);
            }
            Err(_) => {
                out.println(&format!("  {} {}", "MISSING".red().bold(), path.display()));
                bad_parts.push(part);
            }
        }
    }
    let parts_missing = bad_parts.len();

    let total_bytes: u64 = groups.values().flatten().map(|e| e.size).sum();
    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} Verifying  [{bar:40.cyan/blue}] {bytes}/{total_bytes}  ETA {eta}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let abort = AtomicBool::new(false);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;
    let jobs: Vec<(&Loc, &Vec<&IndexEntry>)> = groups.iter().collect();
    let results: Vec<FrameResult> = pool.install(|| {
        jobs.par_iter()
            .map(|(loc, entries)| {
                check_frame(
                    &idx,
                    index_dir,
                    **loc,
                    entries,
                    &pb,
                    &abort,
                    !continue_on_error,
                )
            })
            .collect()
    });
    pb.finish_with_message("verification done");

    let mut ok = 0usize;
    let mut unchecked = 0usize;
    let mut extra = 0usize;
    let mut corrupt: Vec<(PathBuf, String)> = vec![];
    let mut missing: Vec<PathBuf> = vec![];
    let mut frame_errors: Vec<String> = vec![];
    for r in results {
        ok += r.ok;
        unchecked += r.unchecked;
        extra += r.extra;
        corrupt.extend(r.corrupt);
        missing.extend(r.missing);
        if let Some(e) = r.error {
            frame_errors.push(e);
        }
    }

    for (p, detail) in &corrupt {
        out.eprintln(&format!(
            "  {} {} ({})",
            "CORRUPT".red().bold(),
            p.display(),
            detail
        ));
    }
    for p in missing.iter().take(50) {
        out.eprintln(&format!(
            "  {} {} (not found in its frame)",
            "MISSING".red().bold(),
            p.display()
        ));
    }
    if missing.len() > 50 {
        out.eprintln(&format!("  ... and {} more missing", missing.len() - 50));
    }
    for e in frame_errors.iter().take(20) {
        out.eprintln(&format!("  {} {}", "DATA ERROR".red().bold(), e));
    }

    let problems = corrupt.len()
        + missing.len()
        + frame_errors.len()
        + parts_missing
        + usize::from(seal == SealStatus::Invalid)
        + usize::from(matches!(sig, SigStatus::Invalid(_)));
    let pass = problems == 0;

    if out.json {
        let result = serde_json::json!({
            "status": if pass { "PASS" } else { "FAIL" },
            "ok": ok,
            "corrupt": corrupt.len(),
            "missing": missing.len(),
            "unchecked": unchecked,
            "extra_members": extra,
            "parts_bad": parts_missing,
            "frame_errors": frame_errors.len(),
            "all_parts_present": parts_missing == 0,
            "index_seal": match seal {
                SealStatus::Valid => "valid",
                SealStatus::Missing => "missing",
                SealStatus::Invalid => "invalid",
            },
            "index_signature": match &sig {
                SigStatus::Absent => "absent".to_string(),
                SigStatus::Valid { signer, .. } => format!("valid:{signer}"),
                SigStatus::Invalid(_) => "invalid".to_string(),
            },
        });
        out.raw(&serde_json::to_string_pretty(&result).unwrap());
        out.raw("\n");
    } else {
        out.println("");
        out.println(&"-".repeat(50).dimmed().to_string());
        let status_str = if pass {
            "PASS".green().bold().to_string()
        } else {
            "FAIL".red().bold().to_string()
        };
        let red_if = |n: usize| {
            if n > 0 {
                n.to_string().red().to_string()
            } else {
                n.to_string().green().to_string()
            }
        };
        out.println(&format!(
            "  {}  OK: {}  CORRUPT: {}  MISSING: {}",
            status_str,
            ok.to_string().green(),
            red_if(corrupt.len()),
            red_if(missing.len() + parts_missing)
        ));
        if unchecked > 0 {
            out.println(&format!(
                "  {} {} file(s) have no stored checksum (read OK, content not compared)",
                "note:".yellow(),
                unchecked
            ));
        }
        if extra > 0 {
            out.println(&format!(
                "  {} {} tar member(s) are not listed in the index",
                "note:".yellow(),
                extra
            ));
        }
        out.println(&"-".repeat(50).dimmed().to_string());
    }

    if !pass {
        anyhow::bail!("archive failed verification ({} problem(s))", problems);
    }
    Ok(())
}
