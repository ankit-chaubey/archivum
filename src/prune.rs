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
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fsutil::INDEX_NAME;
use crate::index::ArchivumIndex;
use crate::output::OutputCtx;
use crate::utils::now;

struct ArchiveInfo {
    dir: PathBuf,
    canon: PathBuf,
    created_at: u64,
    /// Other archive directories whose parts this archive still reads.
    depends_on: HashSet<PathBuf>,
}

pub fn prune(base_dir: &Path, keep_last: usize, max_age_days: u64, out: &OutputCtx) -> Result<()> {
    out.println(&format!(
        "{} {} (keep={}, max_age={}d)",
        "Pruning archives in:".cyan().bold(),
        base_dir.display().to_string().yellow(),
        keep_last,
        max_age_days
    ));
    out.println("");

    if !base_dir.is_dir() {
        anyhow::bail!("Not a directory: {}", base_dir.display());
    }

    let mut archives: Vec<ArchiveInfo> = vec![];

    for entry in fs::read_dir(base_dir)? {
        let entry = entry?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let index_path = dir.join(INDEX_NAME);
        if !index_path.exists() {
            continue;
        }
        match ArchivumIndex::read(&index_path) {
            Ok(idx) => {
                let canon = dir.canonicalize()?;
                let depends_on = idx
                    .header
                    .parts
                    .iter()
                    .filter_map(|p| dir.join(&p.path).parent()?.canonicalize().ok())
                    .filter(|p| *p != canon)
                    .collect();
                archives.push(ArchiveInfo {
                    dir,
                    canon,
                    created_at: idx.header.created_at_unix,
                    depends_on,
                });
            }
            Err(e) => {
                out.println(&format!(
                    "  {} {} - {}",
                    "skip (unreadable):".dimmed(),
                    index_path.display(),
                    e
                ));
            }
        }
    }

    archives.sort_by_key(|a| a.created_at);

    out.println(&format!("  Found {} archive(s)", archives.len()));

    if archives.len() <= keep_last {
        out.println(&format!(
            "  {} Nothing to prune - count ({}) <= keep_last ({})",
            "OK".green().bold(),
            archives.len(),
            keep_last
        ));
        return Ok(());
    }

    let now_secs = now();
    let max_age_secs = max_age_days * 86400;

    let candidates = &archives[..archives.len() - keep_last];
    let mut to_delete: Vec<&ArchiveInfo> = vec![];

    for arch in candidates {
        let age_secs = now_secs.saturating_sub(arch.created_at);
        let too_old = max_age_days > 0 && age_secs >= max_age_secs;

        if too_old || max_age_days == 0 {
            to_delete.push(arch);
        }
    }

    // An incremental archive reads parts from the archives it was built on.
    // Never delete an archive that a surviving archive still depends on.
    loop {
        let delete_set: HashSet<&Path> = to_delete.iter().map(|a| a.canon.as_path()).collect();
        let needed: HashSet<&Path> = archives
            .iter()
            .filter(|a| !delete_set.contains(a.canon.as_path()))
            .flat_map(|a| a.depends_on.iter().map(|p| p.as_path()))
            .collect();
        let before = to_delete.len();
        let mut protected = vec![];
        to_delete.retain(|a| {
            if needed.contains(a.canon.as_path()) {
                protected.push(a.dir.clone());
                false
            } else {
                true
            }
        });
        for p in protected {
            out.println(&format!(
                "  {} {} (newer archives still depend on its parts)",
                "keep:".yellow().bold(),
                p.display()
            ));
        }
        if to_delete.len() == before {
            break;
        }
    }

    if to_delete.is_empty() {
        out.println(&"  Nothing qualified for deletion.".dimmed().to_string());
        return Ok(());
    }

    out.println(&format!(
        "  {} archive(s) to delete:",
        to_delete.len().to_string().red()
    ));

    for arch in &to_delete {
        let age_days = now_secs.saturating_sub(arch.created_at) / 86400;
        out.println(&format!(
            "    {} (age: {} days)",
            arch.dir.display().to_string().red(),
            age_days
        ));

        if out.dry_run {
            out.dry(&format!("would delete: {}", arch.dir.display()));
        } else {
            delete_archive(&arch.dir, out)?;
        }
    }

    if !out.dry_run {
        out.println(&format!(
            "\n  {} Pruned {} archive(s)",
            "Done.".green().bold(),
            to_delete.len()
        ));
    }

    Ok(())
}

fn delete_archive(dir: &Path, out: &OutputCtx) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        let is_archive_file = name.starts_with("data.part")
            || name == "index.arc.json"
            || name == "index.arc.json.b3"
            || name == "index.arc.json.sig"
            || name == "index.arc.json.tmp"
            || name == "index.arc.json.b3.tmp";

        if is_archive_file {
            fs::remove_file(&path)
                .map_err(|e| anyhow::anyhow!("Cannot delete {}: {}", path.display(), e))?;
        }
    }

    if fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir(dir)?;
        out.println(&format!("  {} {}", "Deleted:".red().bold(), dir.display()));
    } else {
        out.println(&format!(
            "  {} {} (directory not empty - only archive files removed)",
            "Cleaned:".yellow(),
            dir.display()
        ));
    }

    Ok(())
}
