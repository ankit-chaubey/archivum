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
use globset::{Glob, GlobSet, GlobSetBuilder};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use crate::checksum::HashingReader;
use crate::index::{ArchivumIndex, IndexEntry, Loc};
use crate::output::OutputCtx;
use crate::scan::EntryType;
use crate::utils::human;

/// Walk `rel_dir` below `base`, creating missing directories one component at a
/// time. A symlink (or anything that is not a directory) anywhere on the way is
/// refused, so an archive can never make us write outside `base`.
fn walk_dirs(base: &Path, rel_dir: &Path) -> Result<PathBuf> {
    let mut cur = base.to_path_buf();
    for comp in rel_dir.components() {
        match comp {
            Component::Normal(c) => cur.push(c),
            Component::CurDir => continue,
            _ => bail!("Unsafe path component in {}", rel_dir.display()),
        }
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                bail!("Refusing to write through symlink: {}", cur.display())
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => bail!("{} exists and is not a directory", cur.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&cur) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    // another worker created it first: it must still be a real directory
                    match fs::symlink_metadata(&cur) {
                        Ok(m) if m.is_dir() => {}
                        _ => bail!("{} is not a directory", cur.display()),
                    }
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("Cannot create {}", cur.display()));
                }
            },
            Err(e) => return Err(e).with_context(|| format!("Cannot stat {}", cur.display())),
        }
    }
    Ok(cur)
}

/// Absolute destination for `rel`, with all parent directories created safely.
fn prepare_dest(base: &Path, rel: &Path) -> Result<PathBuf> {
    let parent = walk_dirs(base, rel.parent().unwrap_or_else(|| Path::new("")))?;
    let name = rel
        .file_name()
        .with_context(|| format!("Invalid path {}", rel.display()))?;
    Ok(parent.join(name))
}

/// Decide whether `path` may be written. Existing files are only replaced with
/// `force`, and only after being unlinked (so we never write through a symlink).
fn claim_dest(path: &Path, force: bool) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            if !force {
                return Ok(false);
            }
            if m.is_dir() {
                bail!("{} is a directory", path.display());
            }
            fs::remove_file(path).with_context(|| format!("Cannot replace {}", path.display()))?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e).with_context(|| format!("Cannot stat {}", path.display())),
    }
}

fn set_mtime(f: &File, mtime: Option<u64>) {
    if let Some(m) = mtime {
        let _ = f.set_modified(UNIX_EPOCH + Duration::from_secs(m));
    }
}

#[cfg(unix)]
fn apply_permissions(path: &Path, entry: &IndexEntry) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = entry.unix_mode {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777));
    }
}

fn finish_file(path: &Path, entry: &IndexEntry, permissions: bool) {
    if let Ok(f) = OpenOptions::new().write(true).open(path) {
        set_mtime(&f, entry.mtime);
    }
    #[cfg(unix)]
    if permissions {
        apply_permissions(path, entry);
    }
    #[cfg(not(unix))]
    let _ = permissions;
}

/// One tar member and every destination that needs its content
/// (the original plus any dedup aliases that were selected).
struct Want<'a> {
    src: &'a IndexEntry,
    dests: Vec<&'a IndexEntry>,
}

struct Shared<'a> {
    base: &'a Path,
    force: bool,
    permissions: bool,
    out: &'a OutputCtx,
    pb: &'a ProgressBar,
    errors: &'a Mutex<Vec<String>>,
    restored: &'a AtomicU64,
    skipped: &'a AtomicU64,
}

impl Shared<'_> {
    fn fail(&self, msg: String) {
        self.errors.lock().unwrap().push(msg);
    }
}

fn extract_member(item: &mut dyn Read, want: &Want, sh: &Shared) -> Result<()> {
    let mut writable: Vec<(PathBuf, &IndexEntry)> = vec![];
    for d in &want.dests {
        let path = prepare_dest(sh.base, &d.path)?;
        if claim_dest(&path, sh.force)? {
            writable.push((path, d));
        } else {
            sh.skipped.fetch_add(1, Ordering::Relaxed);
            sh.pb.suspend(|| {
                sh.out.println(&format!(
                    "  {} {}",
                    "skip (exists):".dimmed(),
                    path.display()
                ))
            });
        }
    }
    let Some((primary, _)) = writable.first() else {
        return Ok(());
    };

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(primary)
        .with_context(|| format!("Cannot write {}", primary.display()))?;

    let mut reader = HashingReader::new(item);
    let copied = io::copy(&mut reader, &mut file);
    let (hash, bytes) = reader.finalize();
    if let Err(e) = copied {
        drop(file);
        let _ = fs::remove_file(primary);
        return Err(e).context("read/write error while extracting");
    }
    drop(file);

    for (path, _) in writable.iter().skip(1) {
        fs::copy(primary, path).with_context(|| format!("Cannot write {}", path.display()))?;
    }
    for (path, d) in &writable {
        finish_file(path, d, sh.permissions);
    }
    sh.restored
        .fetch_add(writable.len() as u64, Ordering::Relaxed);

    if bytes != want.src.size {
        bail!(
            "size mismatch (index says {} bytes, archive has {})",
            want.src.size,
            bytes
        );
    }
    if let Some(expected) = &want.src.sha256 {
        if *expected != hash {
            bail!(
                "CORRUPT: checksum mismatch (expected {}..., got {}...)",
                expected.get(..12).unwrap_or(expected),
                &hash[..12]
            );
        }
    }
    Ok(())
}

fn restore_frame(
    idx: &ArchivumIndex,
    index_dir: &Path,
    loc: Loc,
    mut wants: HashMap<PathBuf, Want>,
    sh: &Shared,
) {
    let where_ = idx.part_path(index_dir, loc.part);

    let fail_all = |wants: HashMap<PathBuf, Want>, why: String| {
        for (member, w) in wants {
            sh.fail(format!("{}: {}", member.display(), why));
            sh.pb.inc(w.src.size);
        }
    };

    let reader = match idx.open_frame(index_dir, loc) {
        Ok(r) => r,
        Err(e) => {
            fail_all(wants, format!("cannot open data: {:#}", e));
            return;
        }
    };
    let mut archive = tar::Archive::new(reader);
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            fail_all(wants, format!("cannot read {}: {}", where_.display(), e));
            return;
        }
    };

    let mut read_error = None;
    for item in entries {
        let mut item = match item {
            Ok(i) => i,
            Err(e) => {
                read_error = Some(e.to_string());
                break;
            }
        };
        let member = match item.path() {
            Ok(p) => p.into_owned(),
            Err(_) => continue,
        };
        let Some(want) = wants.remove(&member) else {
            continue;
        };
        if let Err(e) = extract_member(&mut item, &want, sh) {
            sh.fail(format!("{}: {:#}", want.src.path.display(), e));
        }
        sh.pb.inc(want.src.size);
    }

    let why = match read_error {
        Some(e) => format!("data in {} is damaged: {}", where_.display(), e),
        None => format!("not found in {}", where_.display()),
    };
    fail_all(wants, why);
}

fn create_symlink(
    base: &Path,
    entry: &IndexEntry,
    link_target: &Path,
    force: bool,
    out: &OutputCtx,
) -> Result<()> {
    let link_path = prepare_dest(base, &entry.path)?;
    match fs::symlink_metadata(&link_path) {
        Ok(m) => {
            if !force {
                out.println(&format!(
                    "  {} {}",
                    "skip (exists):".dimmed(),
                    link_path.display()
                ));
                return Ok(());
            }
            if m.is_dir() {
                bail!("{} is a directory", link_path.display());
            }
            fs::remove_file(&link_path)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("Cannot stat {}", link_path.display())),
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(link_target, &link_path)
        .with_context(|| format!("Cannot create symlink {}", link_path.display()))?;
    #[cfg(not(unix))]
    {
        let _ = (&link_path, link_target);
        out.println("  symlinks skipped on non-Unix");
    }
    Ok(())
}

pub fn restore(
    index_path: &Path,
    target: &Path,
    filter: Option<&str>,
    force: bool,
    restore_permissions: bool,
    threads: usize,
    out: &OutputCtx,
) -> Result<()> {
    let idx = ArchivumIndex::read(index_path)
        .with_context(|| format!("Cannot read index: {}", index_path.display()))?;
    let index_dir = index_path.parent().unwrap_or(Path::new("."));

    let globset = build_filter(filter)?;
    let map = idx.path_map();

    out.println(&format!(
        "{} {} -> {}",
        "Restoring:".cyan().bold(),
        index_path.display().to_string().yellow(),
        target.display().to_string().yellow()
    ));
    out.println("");

    let base: PathBuf = if out.dry_run {
        out.dry(&format!("would create directory: {}", target.display()));
        target.to_path_buf()
    } else {
        fs::create_dir_all(target)
            .with_context(|| format!("Cannot create target dir {}", target.display()))?;
        target.canonicalize()?
    };

    let errors: Mutex<Vec<String>> = Mutex::new(vec![]);
    let restored = AtomicU64::new(0);
    let skipped = AtomicU64::new(0);

    // pass 1: directories (permissions are applied last)
    let dirs: Vec<&IndexEntry> = idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::Directory && matches_filter(&globset, &e.path))
        .collect();
    for d in &dirs {
        if out.dry_run {
            out.dry(&format!("mkdir {}", base.join(&d.path).display()));
        } else if let Err(e) = walk_dirs(&base, &d.path) {
            errors
                .lock()
                .unwrap()
                .push(format!("{}: {:#}", d.path.display(), e));
        }
    }

    // pass 2: regular files grouped by the tar part that really holds the data.
    // Dedup aliases are served from their origin's member, so they work even when
    // the origin is filtered out or already exists on disk.
    let mut groups: BTreeMap<Loc, HashMap<PathBuf, Want>> = BTreeMap::new();
    for e in idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::File && matches_filter(&globset, &e.path))
    {
        let src = idx.origin_of(&map, e);
        let Some(loc) = src.loc else {
            errors.lock().unwrap().push(format!(
                "{}: no data location in the index",
                e.path.display()
            ));
            continue;
        };
        groups
            .entry(loc)
            .or_default()
            .entry(src.path.clone())
            .or_insert_with(|| Want { src, dests: vec![] })
            .dests
            .push(e);
    }

    let total_files: u64 = groups
        .values()
        .flat_map(|g| g.values())
        .map(|w| w.dests.len() as u64)
        .sum();
    let total_bytes: u64 = groups
        .values()
        .flat_map(|g| g.values())
        .map(|w| w.src.size)
        .sum();

    let pb = ProgressBar::new(total_bytes);
    pb.set_style(
        ProgressStyle::with_template(
            "  {spinner:.cyan} Restoring  [{bar:40.cyan/blue}] {bytes}/{total_bytes}  ETA {eta}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    if out.dry_run {
        for w in groups.values().flat_map(|g| g.values()) {
            for d in &w.dests {
                out.dry(&format!(
                    "restore {} ({})",
                    base.join(&d.path).display(),
                    human(d.size)
                ));
            }
        }
    } else {
        let sh = Shared {
            base: &base,
            force,
            permissions: restore_permissions,
            out,
            pb: &pb,
            errors: &errors,
            restored: &restored,
            skipped: &skipped,
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build thread pool: {}", e))?;
        let jobs: Vec<(Loc, HashMap<PathBuf, Want>)> = groups.into_iter().collect();
        pool.install(|| {
            jobs.into_par_iter()
                .for_each(|(loc, wants)| restore_frame(&idx, index_dir, loc, wants, &sh));
        });
    }

    pb.finish_with_message(format!(
        "{}  ({} files, {})",
        "restore finished".green(),
        total_files,
        human(total_bytes)
    ));

    // pass 3: symlinks (after files, so nothing can be written through them)
    for e in idx
        .entries
        .iter()
        .filter(|e| e.entry_type == EntryType::Symlink && matches_filter(&globset, &e.path))
    {
        let Some(link_target) = &e.symlink_target else {
            continue;
        };
        if out.dry_run {
            out.dry(&format!(
                "symlink {} -> {}",
                base.join(&e.path).display(),
                link_target.display()
            ));
            continue;
        }
        if let Err(err) = create_symlink(&base, e, link_target, force, out) {
            errors
                .lock()
                .unwrap()
                .push(format!("{}: {:#}", e.path.display(), err));
        }
    }

    // pass 4: directory mtime / permissions, deepest first, once all files exist
    if !out.dry_run {
        let mut ordered = dirs.clone();
        ordered.sort_by_key(|d| std::cmp::Reverse(d.path.components().count()));
        for d in ordered {
            let path = base.join(&d.path);
            if let Ok(f) = File::open(&path) {
                set_mtime(&f, d.mtime);
            }
            #[cfg(unix)]
            if restore_permissions {
                apply_permissions(&path, d);
            }
        }
    }

    out.println("");
    out.println(&format!(
        "  {} {}  ({} files written, {} skipped)",
        "Restored to:".cyan().bold(),
        target.display().to_string().yellow(),
        restored.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed)
    ));

    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        for e in errors.iter().take(50) {
            out.eprintln(&format!("  {} {}", "ERROR".red().bold(), e));
        }
        if errors.len() > 50 {
            out.eprintln(&format!("  ... and {} more", errors.len() - 50));
        }
        bail!("restore finished with {} error(s)", errors.len());
    }

    Ok(())
}

/// Locate `file` (following dedup aliases) and hand a reader for its tar member to `f`.
pub fn with_member<T>(
    idx: &ArchivumIndex,
    index_dir: &Path,
    file: &Path,
    f: impl FnOnce(&mut dyn Read, &IndexEntry) -> Result<T>,
) -> Result<T> {
    let map = idx.path_map();
    let entry = map
        .get(file)
        .copied()
        .with_context(|| format!("File not found in archive: {}", file.display()))?;

    if entry.entry_type != EntryType::File {
        bail!("Entry is not a regular file: {}", file.display());
    }

    let origin = idx.origin_of(&map, entry);
    let loc = origin
        .loc
        .with_context(|| format!("No data location for {}", file.display()))?;
    let reader = idx.open_frame(index_dir, loc)?;
    let mut archive = tar::Archive::new(reader);

    for item in archive.entries()? {
        let mut item = item?;
        if item.path()?.as_ref() == origin.path.as_path() {
            return f(&mut item, origin);
        }
    }

    bail!(
        "File not found inside its archive frame: {}",
        file.display()
    );
}

/// Compare what was streamed with what the index recorded.
pub fn check_stream(origin: &IndexEntry, hash: &str, bytes: u64) -> Result<()> {
    if bytes != origin.size {
        bail!(
            "{}: size mismatch (index says {} bytes, archive has {})",
            origin.path.display(),
            origin.size,
            bytes
        );
    }
    if let Some(expected) = &origin.sha256 {
        if expected != hash {
            bail!(
                "{}: CORRUPT, checksum mismatch (expected {}..., got {}...)",
                origin.path.display(),
                expected.get(..12).unwrap_or(expected),
                &hash[..12]
            );
        }
    }
    Ok(())
}

pub fn extract_single(
    idx: &ArchivumIndex,
    index_dir: &Path,
    file: &Path,
    output: Option<&Path>,
    force: bool,
    out: &OutputCtx,
) -> Result<()> {
    let out_path = match output {
        Some(p) => p.to_path_buf(),
        None => file
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| file.to_path_buf()),
    };

    if out.dry_run {
        out.dry(&format!(
            "extract {} to {}",
            file.display(),
            out_path.display()
        ));
        return Ok(());
    }

    if !force && fs::symlink_metadata(&out_path).is_ok() {
        bail!(
            "{} already exists (use --force to overwrite)",
            out_path.display()
        );
    }
    if let Some(p) = out_path.parent() {
        if !p.as_os_str().is_empty() {
            fs::create_dir_all(p)?;
        }
    }
    if force {
        if let Ok(m) = fs::symlink_metadata(&out_path) {
            if !m.is_dir() {
                fs::remove_file(&out_path)?;
            }
        }
    }

    with_member(idx, index_dir, file, |reader, origin| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out_path)
            .with_context(|| format!("Cannot write {}", out_path.display()))?;
        let mut hr = HashingReader::new(reader);
        io::copy(&mut hr, &mut f)?;
        let (hash, bytes) = hr.finalize();
        check_stream(origin, &hash, bytes)
    })?;

    out.println(&format!(
        "{} {}",
        "Extracted:".green().bold(),
        out_path.display().to_string().yellow()
    ));
    Ok(())
}

fn build_filter(pattern: Option<&str>) -> Result<Option<GlobSet>> {
    match pattern {
        None => Ok(None),
        Some(p) => {
            let mut b = GlobSetBuilder::new();
            b.add(Glob::new(p)?);
            Ok(Some(b.build()?))
        }
    }
}

fn matches_filter(gs: &Option<GlobSet>, path: &Path) -> bool {
    match gs {
        None => true,
        Some(g) => g.is_match(path),
    }
}
