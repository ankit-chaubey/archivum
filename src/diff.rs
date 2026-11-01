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
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::checksum::hash_file;
use crate::index::ArchivumIndex;
use crate::output::OutputCtx;
use crate::scan::{EntryType, scan_directory_with};
use crate::utils::human;

pub fn diff(
    index_path: &Path,
    source: &Path,
    changed_only: bool,
    use_checksum: bool,
    exclude: &[String],
    out: &OutputCtx,
) -> Result<()> {
    let idx = ArchivumIndex::read(index_path)?;

    out.println(&format!(
        "{} {} vs {}",
        "Diff:".cyan().bold(),
        index_path.display().to_string().yellow(),
        source.display().to_string().yellow()
    ));
    if use_checksum {
        out.println(&format!(
            "  {}",
            "Using SHA-256 checksum comparison".dimmed()
        ));
    }
    out.println("");

    let archived: HashMap<&Path, &crate::index::IndexEntry> = idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File)
        .map(|e| (e.path.as_path(), e))
        .collect();

    let scan = scan_directory_with(source, exclude, &[])?;
    for w in &scan.warnings {
        out.eprintln(&format!("  {} {}", "warning:".yellow(), w));
    }
    let current = scan.entries;
    let current_map: HashMap<&Path, &crate::scan::ScanEntry> = current
        .iter()
        .filter(|e| e.entry_type == EntryType::File)
        .map(|e| (e.relative_path.as_path(), e))
        .collect();

    let mut added: Vec<(PathBuf, u64)> = vec![];
    let mut removed: Vec<PathBuf> = vec![];
    let mut modified: Vec<(PathBuf, String)> = vec![];
    let mut unchanged = 0usize;

    for (&path, se) in &current_map {
        if let Some(ae) = archived.get(path) {
            let size_changed = se.size != ae.size;
            let mtime_changed = se.mtime != ae.mtime;
            let stored = ae.sha256.as_deref().filter(|h| !h.is_empty());

            if size_changed {
                modified.push((
                    path.to_path_buf(),
                    format!("size {} -> {}", human(ae.size), human(se.size)),
                ));
            } else if use_checksum && stored.is_some() {
                // a touched file with identical content is not a change
                match hash_file(&source.join(path)) {
                    Ok(actual) if Some(actual.as_str()) == stored => unchanged += 1,
                    Ok(actual) => modified.push((
                        path.to_path_buf(),
                        format!(
                            "checksum mismatch ({}... vs {}...)",
                            stored.and_then(|s| s.get(..8)).unwrap_or(""),
                            &actual[..8]
                        ),
                    )),
                    Err(e) => modified.push((path.to_path_buf(), format!("unreadable: {e}"))),
                }
            } else if mtime_changed {
                modified.push((path.to_path_buf(), "mtime changed".to_string()));
            } else {
                unchanged += 1;
            }
        } else {
            added.push((path.to_path_buf(), se.size));
        }
    }

    for &path in archived.keys() {
        if !current_map.contains_key(path) {
            removed.push(path.to_path_buf());
        }
    }

    if out.json {
        let result = serde_json::json!({
            "added":    added.iter().map(|(p, s)| serde_json::json!({"path": p, "size": s})).collect::<Vec<_>>(),
            "removed":  removed.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "modified": modified.iter().map(|(p, r)| serde_json::json!({"path": p, "reason": r})).collect::<Vec<_>>(),
            "unchanged": unchanged
        });
        out.raw(&serde_json::to_string_pretty(&result).unwrap());
        out.raw("\n");
        return Ok(());
    }

    if !changed_only {
        out.println(&format!(
            "  {} {}",
            "Unchanged:".dimmed(),
            unchanged.to_string().dimmed()
        ));
    }

    added.sort();
    removed.sort();
    modified.sort();
    for (path, size) in &added {
        out.println(&format!(
            "  {} {} ({})",
            "+ ADDED".green().bold(),
            path.display(),
            human(*size).green()
        ));
    }
    for path in &removed {
        out.println(&format!(
            "  {} {}",
            "- REMOVED".red().bold(),
            path.display()
        ));
    }
    for (path, reason) in &modified {
        out.println(&format!(
            "  {} {} - {}",
            "~ MODIFIED".yellow().bold(),
            path.display(),
            reason.dimmed()
        ));
    }

    out.println("");
    out.println(&"-".repeat(60).dimmed().to_string());
    out.println(&format!(
        "  Added: {}  Removed: {}  Modified: {}  Unchanged: {}",
        added.len().to_string().green(),
        removed.len().to_string().red(),
        modified.len().to_string().yellow(),
        unchanged.to_string().dimmed()
    ));
    out.println(&"-".repeat(60).dimmed().to_string());

    Ok(())
}
