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

//! End-to-end tests: every bug fixed in 0.4.0 and every v4 feature has a regression test here.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Env {
    root: TempDir,
}

impl Env {
    fn new() -> Self {
        Env {
            root: tempfile::tempdir().unwrap(),
        }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }
    fn arc(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_archivum"))
            .args(args)
            .env("HOME", self.p("home"))
            .env("XDG_CONFIG_HOME", self.p("home/.config"))
            .env("APPDATA", self.p("home/appdata"))
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let o = self.arc(args);
        assert!(
            o.status.success(),
            "archivum {:?} failed:\n{}\n{}",
            args,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
    fn fails(&self, args: &[&str]) -> String {
        let o = self.arc(args);
        assert!(
            !o.status.success(),
            "archivum {:?} should have failed",
            args
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    }
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn write(base: &Path, rel: &str, content: &str) {
    let p = base.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn read(base: &Path, rel: &str) -> String {
    fs::read_to_string(base.join(rel)).unwrap()
}

/// All regular files below `base` as (relative path, content).
fn snapshot(base: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(root, &p, out);
            } else if ft.is_file() {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    let mut v = vec![];
    walk(base, base, &mut v);
    v.sort();
    v
}

fn sample_tree(base: &Path) {
    write(base, "a.txt", "alpha");
    write(base, "dir/b.txt", "bravo bravo bravo");
    write(base, "dir/sub/c.bin", &"c".repeat(100_000));
    write(base, "empty.txt", "");
    let long = format!(
        "{}/{}.txt",
        "long_directory_name_".repeat(6),
        "f".repeat(30)
    );
    write(base, &long, "long path content");
}

#[test]
fn roundtrip_every_algorithm() {
    for algo in ["none", "gzip", "bzip2", "lz4", "zstd"] {
        let e = Env::new();
        let src = e.p("src");
        sample_tree(&src);
        let out = e.p("out");
        e.ok(&["create", s(&src), s(&out), "--compress", algo]);
        let idx = out.join("index.arc.json");
        e.ok(&["verify", s(&idx)]);
        let r = e.p("restored");
        e.ok(&["restore", s(&idx), s(&r)]);
        assert_eq!(snapshot(&src), snapshot(&r), "algo {algo}");
    }
}

#[test]
fn zstd_output_is_identical_for_any_thread_count() {
    let e = Env::new();
    let src = e.p("src");
    sample_tree(&src);
    e.ok(&["create", s(&src), s(&e.p("o1")), "--zstd-threads", "1"]);
    e.ok(&["create", s(&src), s(&e.p("o4")), "--zstd-threads", "4"]);
    assert_eq!(
        fs::read(e.p("o1/data.part000.tar.zst")).unwrap(),
        fs::read(e.p("o4/data.part000.tar.zst")).unwrap()
    );
}

#[test]
fn same_tree_gives_same_bytes() {
    let e = Env::new();
    let src = e.p("src");
    sample_tree(&src);
    e.ok(&["create", s(&src), s(&e.p("o1")), "--compress", "none"]);
    e.ok(&["create", s(&src), s(&e.p("o2")), "--compress", "none"]);
    assert_eq!(
        fs::read(e.p("o1/data.part000.tar")).unwrap(),
        fs::read(e.p("o2/data.part000.tar")).unwrap()
    );
}

#[test]
fn exclude_prunes_whole_directories_and_matches_at_any_depth() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "keep.txt", "k");
    write(&src, "node_modules/pkg/x.js", "x");
    write(&src, "sub/.DS_Store", "junk");
    write(&src, "sub/ok.txt", "ok");
    e.ok(&[
        "create",
        s(&src),
        s(&e.p("out")),
        "--exclude",
        "node_modules",
        "--exclude",
        ".DS_Store",
    ]);
    let list = e.ok(&["list", "-v", s(&e.p("out/index.arc.json"))]);
    assert!(list.contains("keep.txt") && list.contains("sub/ok.txt"));
    assert!(
        !list.contains("x.js"),
        "children of excluded dir leaked: {list}"
    );
    assert!(!list.contains(".DS_Store"));
}

#[test]
fn create_refuses_existing_archive_and_never_archives_itself() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "a");
    let out = src.join("backup");
    e.ok(&["create", s(&src), s(&out)]);
    e.fails(&["create", s(&src), s(&out)]);
    e.ok(&["create", s(&src), s(&out), "--overwrite"]);
    let list = e.ok(&["list", "-v", s(&out.join("index.arc.json"))]);
    assert!(
        !list.contains("data.part"),
        "archive contains itself: {list}"
    );
}

#[test]
fn dedup_aliases_restore_even_when_origin_exists_or_is_filtered() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "same content");
    write(&src, "b.txt", "same content");
    write(&src, "c.txt", "other");
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--dedup"]);
    let idx = out.join("index.arc.json");

    // origin already present on disk
    let r1 = e.p("r1");
    write(&r1, "a.txt", "old");
    e.ok(&["restore", s(&idx), s(&r1)]);
    assert_eq!(read(&r1, "b.txt"), "same content");
    assert_eq!(read(&r1, "a.txt"), "old");

    // filter selects only the alias
    let r2 = e.p("r2");
    e.ok(&["restore", s(&idx), s(&r2), "--filter", "b.txt"]);
    assert_eq!(read(&r2, "b.txt"), "same content");
    assert!(!r2.join("a.txt").exists());

    // cat / extract follow aliases
    let o = e.arc(&["cat", s(&idx), "b.txt"]);
    assert_eq!(o.stdout, b"same content");
}

#[test]
fn incremental_chain_restores_and_verifies() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "same");
    write(&src, "b.txt", "same");
    write(&src, "c.txt", "other");
    write(&src, "keep/k.txt", "keep");
    let o1 = e.p("o1");
    e.ok(&["create", s(&src), s(&o1), "--dedup"]);

    // a.txt (the dedup origin of b.txt) changes, a new dir appears
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&src, "a.txt", "CHANGED");
    fs::create_dir_all(src.join("newdir/inner")).unwrap();
    let o2 = e.p("o2");
    e.ok(&["update", s(&o1.join("index.arc.json")), s(&src), s(&o2)]);

    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&src, "c.txt", "third version");
    fs::remove_file(src.join("keep/k.txt")).unwrap();
    let o3 = e.p("o3");
    e.ok(&["update", s(&o2.join("index.arc.json")), s(&src), s(&o3)]);

    let idx3 = o3.join("index.arc.json");
    e.ok(&["verify", s(&idx3)]);
    let r = e.p("restored");
    e.ok(&["restore", s(&idx3), s(&r)]);
    assert_eq!(read(&r, "a.txt"), "CHANGED");
    assert_eq!(read(&r, "b.txt"), "same");
    assert_eq!(read(&r, "c.txt"), "third version");
    assert!(!r.join("keep/k.txt").exists());
    assert!(
        r.join("newdir/inner").is_dir(),
        "new directories must be recorded"
    );
}

#[test]
fn update_rejects_unsafe_targets() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "a");
    let o1 = e.p("o1");
    e.ok(&["create", s(&src), s(&o1), "--compress", "gzip"]);
    let idx = o1.join("index.arc.json");
    // same directory as the old archive would overwrite the parts it depends on
    e.fails(&["update", s(&idx), s(&src), s(&o1)]);
    // the output may not be the source either
    e.fails(&["update", s(&idx), s(&src), s(&src)]);
}

#[test]
fn update_chain_can_change_compression() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "first");
    write(&src, "b.txt", "stays");
    e.ok(&["create", s(&src), s(&e.p("o1")), "--compress", "gzip"]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&src, "a.txt", "second");
    e.ok(&[
        "update",
        s(&e.p("o1/index.arc.json")),
        s(&src),
        s(&e.p("o2")),
        "--compress",
        "zstd",
    ]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&src, "c.txt", "third");
    // no flag: continues with the compression of the newest part (zstd)
    e.ok(&[
        "update",
        s(&e.p("o2/index.arc.json")),
        s(&src),
        s(&e.p("o3")),
    ]);
    let idx = e.p("o3/index.arc.json");
    assert!(e.p("o3/data.part000.tar.zst").exists());
    let info = e.ok(&["stats", s(&idx)]);
    assert!(info.contains("gzip") && info.contains("zstd"), "{info}");
    e.ok(&["verify", s(&idx)]);
    e.ok(&["restore", s(&idx), s(&e.p("r"))]);
    assert_eq!(snapshot(&src), snapshot(&e.p("r")));
}

#[test]
fn merge_keeps_everything_and_handles_mixed_compression() {
    let e = Env::new();
    let a = e.p("a");
    write(&a, "x.txt", "dup");
    write(&a, "y.txt", "dup");
    write(&a, "emptydir/.keep", "k");
    fs::remove_file(a.join("emptydir/.keep")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("x.txt", a.join("lnk")).unwrap();
    let b = e.p("b");
    write(&b, "z.txt", "zzz");

    e.ok(&[
        "create",
        s(&a),
        s(&e.p("oa")),
        "--dedup",
        "--compress",
        "zstd",
    ]);
    e.ok(&["create", s(&b), s(&e.p("ob")), "--compress", "gzip"]);

    let merged = e.p("m");
    e.ok(&[
        "merge",
        s(&e.p("oa/index.arc.json")),
        s(&e.p("ob/index.arc.json")),
        "--output",
        s(&merged),
        "--compress",
        "lz4",
    ]);
    let idx = merged.join("index.arc.json");
    e.ok(&["verify", s(&idx)]);
    let r = e.p("r");
    e.ok(&["restore", s(&idx), s(&r)]);
    assert_eq!(read(&r, "x.txt"), "dup");
    assert_eq!(read(&r, "y.txt"), "dup");
    assert_eq!(read(&r, "z.txt"), "zzz");
    assert!(r.join("emptydir").is_dir());
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(r.join("lnk")).unwrap(),
        PathBuf::from("x.txt")
    );
}

#[test]
fn merge_newest_archive_wins_duplicate_paths() {
    let e = Env::new();
    let a = e.p("a");
    write(&a, "f.txt", "old");
    e.ok(&["create", s(&a), s(&e.p("oa"))]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let b = e.p("b");
    write(&b, "f.txt", "new");
    e.ok(&["create", s(&b), s(&e.p("ob"))]);
    // argument order must not matter
    e.ok(&[
        "merge",
        s(&e.p("ob/index.arc.json")),
        s(&e.p("oa/index.arc.json")),
        "--output",
        s(&e.p("m")),
    ]);
    e.ok(&["restore", s(&e.p("m/index.arc.json")), s(&e.p("r"))]);
    assert_eq!(read(&e.p("r"), "f.txt"), "new");
}

#[test]
fn prune_never_deletes_archives_that_newer_ones_depend_on() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "one");
    write(&src, "b.txt", "two");
    let base = e.p("backups");
    e.ok(&["create", s(&src), s(&base.join("a1"))]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&src, "a.txt", "ONE");
    e.ok(&[
        "update",
        s(&base.join("a1/index.arc.json")),
        s(&src),
        s(&base.join("a2")),
    ]);
    e.ok(&["prune", s(&base), "--keep", "1", "--max-age", "0"]);
    assert!(
        base.join("a1/index.arc.json").exists(),
        "base archive was pruned"
    );
    e.ok(&["restore", s(&base.join("a2/index.arc.json")), s(&e.p("r"))]);
    assert_eq!(read(&e.p("r"), "b.txt"), "two");
}

/// Rewrite an uncompressed tar part keeping only the members `keep` accepts.
fn rewrite_tar(path: &Path, keep: impl Fn(&str) -> bool) {
    let data = fs::read(path).unwrap();
    let mut ar = tar::Archive::new(&data[..]);
    let mut members = vec![];
    for item in ar.entries().unwrap() {
        let mut item = item.unwrap();
        let name = item.path().unwrap().to_string_lossy().into_owned();
        let mut buf = vec![];
        std::io::Read::read_to_end(&mut item, &mut buf).unwrap();
        members.push((name, buf));
    }
    let mut b = tar::Builder::new(vec![]);
    for (name, buf) in members.iter().filter(|(n, _)| keep(n)) {
        let mut h = tar::Header::new_gnu();
        h.set_size(buf.len() as u64);
        h.set_mode(0o644);
        b.append_data(&mut h, name, &buf[..]).unwrap();
    }
    fs::write(path, b.into_inner().unwrap()).unwrap();
}

#[test]
fn verify_and_restore_notice_missing_members() {
    let e = Env::new();
    let src = e.p("src");
    for i in 1..=3 {
        write(&src, &format!("f{i}.txt"), &format!("file{i}"));
    }
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--compress", "none"]);
    rewrite_tar(&out.join("data.part000.tar"), |n| n != "f3.txt");
    let idx = out.join("index.arc.json");
    let v = e.fails(&["verify", s(&idx)]);
    assert!(v.contains("f3.txt"), "{v}");
    e.fails(&["restore", s(&idx), s(&e.p("r"))]);
}

#[test]
fn verify_detects_corrupted_content() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "AAAAAAAAAAAAAAAA");
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--compress", "none"]);
    let part = out.join("data.part000.tar");
    let mut data = fs::read(&part).unwrap();
    let pos = data
        .windows(16)
        .position(|w| w == b"AAAAAAAAAAAAAAAA")
        .unwrap();
    data[pos] = b'B';
    fs::write(&part, data).unwrap();
    let idx = out.join("index.arc.json");
    e.fails(&["verify", s(&idx)]);
    e.fails(&["restore", s(&idx), s(&e.p("r"))]);
    e.fails(&["cat", s(&idx), "a.txt"]);
}

#[test]
fn tampered_index_is_rejected() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "a");
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out)]);
    let idx = out.join("index.arc.json");
    let text = fs::read_to_string(&idx).unwrap().replace("a.txt", "b.txt");
    fs::write(&idx, text).unwrap();
    e.fails(&["list", s(&idx)]);
}

#[cfg(unix)]
#[test]
fn restore_never_writes_through_symlinks() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "d/f.txt", "hi");
    let outside = e.p("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, src.join("evil")).unwrap();
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--compress", "none"]);

    // craft an index + tar member that tries to write evil/sub/pwned.txt
    let idx_path = out.join("index.arc.json");
    let mut j: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&idx_path).unwrap()).unwrap();
    let mut file_entry = j["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"] == "d/f.txt")
        .unwrap()
        .clone();
    file_entry["path"] = "evil/sub/pwned.txt".into();
    j["entries"].as_array_mut().unwrap().push(file_entry);
    fs::write(&idx_path, serde_json::to_string_pretty(&j).unwrap()).unwrap();
    fs::remove_file(out.join("index.arc.json.b3")).unwrap();

    let part = out.join("data.part000.tar");
    let data = fs::read(&part).unwrap();
    let mut b = tar::Builder::new(vec![]);
    for item in tar::Archive::new(&data[..]).entries().unwrap() {
        let mut item = item.unwrap();
        let name = item.path().unwrap().to_string_lossy().into_owned();
        let mut buf = vec![];
        std::io::Read::read_to_end(&mut item, &mut buf).unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_size(buf.len() as u64);
        h.set_mode(0o644);
        b.append_data(&mut h, &name, &buf[..]).unwrap();
        if name == "d/f.txt" {
            let mut h2 = tar::Header::new_gnu();
            h2.set_size(buf.len() as u64);
            h2.set_mode(0o644);
            b.append_data(&mut h2, "evil/sub/pwned.txt", &buf[..])
                .unwrap();
        }
    }
    fs::write(&part, b.into_inner().unwrap()).unwrap();

    // the target already contains a symlink pointing out of it
    let target = e.p("r");
    fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&outside, target.join("evil")).unwrap();
    e.fails(&["restore", s(&idx_path), s(&target)]);
    assert!(!outside.join("sub").exists(), "wrote outside the target");

    // on a fresh target the same archive stays inside it
    let fresh = e.p("fresh");
    let _ = e.arc(&["restore", s(&idx_path), s(&fresh)]);
    assert!(!outside.join("sub").exists(), "wrote outside the target");
}

#[cfg(unix)]
#[test]
fn restore_filter_applies_to_symlinks_and_readonly_dirs_work() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    let src = e.p("src");
    write(&src, "ro/f.txt", "x");
    write(&src, "other/g.txt", "g");
    std::os::unix::fs::symlink("ro/f.txt", src.join("link")).unwrap();
    fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o555)).unwrap();
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out)]);
    fs::set_permissions(src.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();

    let r = e.p("r");
    e.ok(&[
        "restore",
        s(&out.join("index.arc.json")),
        s(&r),
        "--restore-permissions",
    ]);
    assert_eq!(read(&r, "ro/f.txt"), "x");
    let mode = fs::metadata(r.join("ro")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o555);
    fs::set_permissions(r.join("ro"), fs::Permissions::from_mode(0o755)).unwrap();

    let r2 = e.p("r2");
    e.ok(&[
        "restore",
        s(&out.join("index.arc.json")),
        s(&r2),
        "--filter",
        "other/*",
    ]);
    assert!(r2.join("other/g.txt").exists());
    assert!(
        fs::symlink_metadata(r2.join("link")).is_err(),
        "filter ignored for symlink"
    );
}

#[test]
fn repair_rebuilds_a_usable_index() {
    let e = Env::new();
    let src = e.p("src");
    sample_tree(&src);
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--split-files", "2"]);
    fs::remove_file(out.join("index.arc.json")).unwrap();
    fs::remove_file(out.join("index.arc.json.b3")).unwrap();
    e.ok(&["repair", s(&out)]);
    let idx = out.join("index.arc.json");
    e.ok(&["verify", s(&idx)]);
    e.ok(&["restore", s(&idx), s(&e.p("r"))]);
    assert_eq!(snapshot(&src), snapshot(&e.p("r")));
    // an existing index is never overwritten
    e.fails(&["repair", s(&out)]);
}

#[test]
fn split_files_creates_multiple_parts_that_restore_in_parallel() {
    let e = Env::new();
    let src = e.p("src");
    for i in 0..20 {
        write(&src, &format!("f{i:02}.txt"), &format!("content {i}"));
    }
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--split-files", "3"]);
    assert!(out.join("data.part006.tar.zst").exists());
    let idx = out.join("index.arc.json");
    e.ok(&["verify", s(&idx), "--threads", "4"]);
    e.ok(&["restore", s(&idx), s(&e.p("r")), "--threads", "4"]);
    assert_eq!(snapshot(&src), snapshot(&e.p("r")));
}

#[test]
fn diff_uses_excludes_and_checksums() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "aaa");
    write(&src, "skip.log", "log");
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--exclude", "*.log"]);
    let idx = out.join("index.arc.json");
    let d = e.ok(&[
        "diff",
        s(&idx),
        s(&src),
        "--exclude",
        "*.log",
        "--changed-only",
    ]);
    assert!(!d.contains("skip.log"), "{d}");
    // touching a file without changing it is not a change when --checksum is used
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(&src, "a.txt", "aaa");
    let d = e.ok(&["diff", s(&idx), s(&src), "--exclude", "*.log", "--checksum"]);
    assert!(d.contains("Modified: 0"), "{d}");
}

fn random_bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect()
}

#[test]
fn large_files_get_their_own_frames_and_roundtrip() {
    for algo in ["zstd", "gzip", "lz4", "none"] {
        let e = Env::new();
        let src = e.p("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("big1.bin"), random_bytes(5 << 20, 1)).unwrap();
        fs::write(src.join("big2.bin"), random_bytes(6 << 20, 2)).unwrap();
        write(&src, "small.txt", "tiny");
        let out = e.p("out");
        e.ok(&["create", s(&src), s(&out), "--compress", algo]);
        let idx = out.join("index.arc.json");
        e.ok(&["verify", s(&idx)]);
        e.ok(&["restore", s(&idx), s(&e.p("r"))]);
        assert_eq!(snapshot(&src), snapshot(&e.p("r")), "algo {algo}");
        // cat reads just the frame of the requested file
        let o = e.arc(&["cat", s(&idx), "big2.bin"]);
        assert_eq!(o.stdout, random_bytes(6 << 20, 2), "algo {algo}");
    }
}

#[test]
fn damage_stays_inside_one_frame() {
    let e = Env::new();
    let src = e.p("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("big1.bin"), random_bytes(5 << 20, 1)).unwrap();
    fs::write(src.join("big2.bin"), random_bytes(5 << 20, 2)).unwrap();
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out), "--compress", "none"]);
    let part = out.join("data.part000.tar");
    let mut data = fs::read(&part).unwrap();
    let pos = data.len() - (3 << 20); // inside big2.bin
    data[pos] ^= 0xff;
    fs::write(&part, data).unwrap();

    let idx = out.join("index.arc.json");
    let v = e.fails(&["verify", s(&idx), "--continue-on-error"]);
    assert!(v.contains("big2.bin") && !v.contains("big1.bin ("), "{v}");
    // the intact file is still restored even though the restore as a whole fails
    let r = e.p("r");
    e.fails(&["restore", s(&idx), s(&r)]);
    assert_eq!(
        fs::read(r.join("big1.bin")).unwrap(),
        random_bytes(5 << 20, 1)
    );
    // and cat on the intact file is fine
    assert!(e.arc(&["cat", s(&idx), "big1.bin"]).status.success());
}

#[test]
fn parts_are_standard_streams() {
    use std::io::Read;
    for algo in ["none", "gzip", "zstd"] {
        let e = Env::new();
        let src = e.p("src");
        sample_tree(&src);
        let out = e.p("out");
        e.ok(&["create", s(&src), s(&out), "--compress", algo]);
        let ext = match algo {
            "none" => "tar",
            "gzip" => "tar.gz",
            _ => "tar.zst",
        };
        let raw = fs::read(out.join(format!("data.part000.{ext}"))).unwrap();
        let mut tarball = vec![];
        match algo {
            "none" => tarball = raw,
            "gzip" => {
                flate2::read::MultiGzDecoder::new(&raw[..])
                    .read_to_end(&mut tarball)
                    .unwrap();
            }
            _ => tarball = zstd::decode_all(&raw[..]).unwrap(),
        }
        let mut names: Vec<String> = tar::Archive::new(&tarball[..])
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut expected: Vec<String> = snapshot(&src).into_iter().map(|(p, _)| p).collect();
        expected.sort();
        assert_eq!(names, expected, "algo {algo}");
    }
}

#[test]
fn output_does_not_depend_on_worker_count() {
    for algo in ["zstd", "gzip"] {
        let e = Env::new();
        let src = e.p("src");
        sample_tree(&src);
        for i in 0..200 {
            write(&src, &format!("many/f{i:03}.txt"), &"x".repeat(i * 37));
        }
        e.ok(&[
            "create",
            s(&src),
            s(&e.p("a")),
            "--compress",
            algo,
            "--threads",
            "1",
        ]);
        e.ok(&[
            "create",
            s(&src),
            s(&e.p("b")),
            "--compress",
            algo,
            "--threads",
            "8",
        ]);
        let ext = if algo == "zstd" { "tar.zst" } else { "tar.gz" };
        assert_eq!(
            fs::read(e.p(&format!("a/data.part000.{ext}"))).unwrap(),
            fs::read(e.p(&format!("b/data.part000.{ext}"))).unwrap(),
            "algo {algo}"
        );
        assert_eq!(
            fs::read(e.p("a/index.arc.json")).unwrap().len(),
            fs::read(e.p("b/index.arc.json")).unwrap().len()
        );
    }
}

#[test]
fn signed_indexes_are_enforced_with_pubkey() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "a");
    let key = e.p("mykey");
    e.ok(&["keygen", s(&key)]);
    let (sec, public) = (e.p("mykey.key"), e.p("mykey.pub"));
    assert!(sec.exists() && public.exists());
    e.fails(&["keygen", s(&key)]); // never overwrite a key

    let signed = e.p("signed");
    e.ok(&["create", s(&src), s(&signed), "--sign", s(&sec)]);
    let unsigned = e.p("unsigned");
    e.ok(&["create", s(&src), s(&unsigned)]);
    let sidx = signed.join("index.arc.json");
    let uidx = unsigned.join("index.arc.json");

    e.ok(&["--pubkey", s(&public), "verify", s(&sidx)]);
    e.ok(&["--pubkey", s(&public), "restore", s(&sidx), s(&e.p("r"))]);
    // unsigned archives are refused when a key is required
    e.fails(&["--pubkey", s(&public), "verify", s(&uidx)]);
    // so is a signature from a different key
    e.ok(&["keygen", s(&e.p("other"))]);
    e.fails(&["--pubkey", s(&e.p("other.pub")), "verify", s(&sidx)]);

    // rewriting the index (and its blake3 seal) breaks the signature
    let text = fs::read_to_string(&sidx).unwrap().replace("a.txt", "z.txt");
    fs::write(&sidx, &text).unwrap();
    fs::write(
        signed.join("index.arc.json.b3"),
        blake3::hash(text.as_bytes()).to_hex().as_str(),
    )
    .unwrap();
    e.ok(&["list", s(&sidx)]); // seal matches, so it is readable without a key
    e.fails(&["--pubkey", s(&public), "list", s(&sidx)]);

    // update and merge can sign too
    let src2 = e.p("src2");
    write(&src2, "b.txt", "b");
    let m = e.p("m");
    e.ok(&["create", s(&src2), s(&e.p("o2")), "--sign", s(&sec)]);
    e.ok(&["create", s(&src), s(&e.p("o1")), "--sign", s(&sec)]);
    e.ok(&[
        "merge",
        s(&e.p("o1/index.arc.json")),
        s(&e.p("o2/index.arc.json")),
        "--output",
        s(&m),
        "--sign",
        s(&sec),
    ]);
    e.ok(&[
        "--pubkey",
        s(&public),
        "verify",
        s(&m.join("index.arc.json")),
    ]);
}

#[test]
fn old_index_versions_are_rejected_clearly() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "a");
    let out = e.p("out");
    e.ok(&["create", s(&src), s(&out)]);
    let idx = out.join("index.arc.json");
    let text = fs::read_to_string(&idx)
        .unwrap()
        .replace("\"version\": 4", "\"version\": 3");
    fs::write(&idx, text).unwrap();
    fs::remove_file(out.join("index.arc.json.b3")).unwrap();
    let msg = e.fails(&["list", s(&idx)]);
    assert!(msg.contains("v3") && msg.contains("v4"), "{msg}");
}

#[test]
fn partial_and_example_configs_are_accepted() {
    let e = Env::new();
    let src = e.p("src");
    write(&src, "a.txt", "a");
    let cfg_dir = e.p("home/.config/archivum");
    fs::create_dir_all(&cfg_dir).unwrap();
    for text in [
        include_str!("../config.toml.example").to_string(),
        "[defaults]\ncompress = \"gzip\"\n".to_string(),
        String::new(),
    ] {
        fs::write(cfg_dir.join("config.toml"), &text).unwrap();
        let o = e.arc(&["create", s(&src), s(&e.p("o")), "--overwrite"]);
        assert!(o.status.success());
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!err.contains("Could not parse config"), "{err}");
    }
    // the partial config really applied
    fs::write(
        cfg_dir.join("config.toml"),
        "[defaults]\ncompress = \"gzip\"\n",
    )
    .unwrap();
    e.ok(&["create", s(&src), s(&e.p("o2"))]);
    assert!(e.p("o2/data.part000.tar.gz").exists());
}
