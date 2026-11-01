# Archivum - Architecture

This document describes the internal structure of the Archivum codebase.

---

## Module Overview

```
src/
├── main.rs          CLI entry point - clap parsing, command dispatch
├── output.rs        OutputCtx - unified quiet/json/dry-run/log routing
├── config.rs        config.toml loading (missing keys fall back to defaults), setup wizard
├── fsutil.rs        Atomic writes, output-directory safety, relative paths
│
├── scan.rs          Directory traversal (WalkDir), symlink detection, pruned excludes
├── checksum.rs      Parallel SHA-256 (Rayon), hashing reader
├── compress.rs      Per-frame encode and seekable decode: none/gzip/zstd/bzip2/lz4
├── tar_writer.rs    Frame writer: batching, parallel compression, atomic parts
│
├── index.rs         ArchivumIndex v4: parts, frame locations, read/validate/write, Blake3 seal
├── sign.rs          Ed25519 keys and index signatures
├── restore.rs       Parallel frame restore, symlink-safe paths, extract/cat helpers
├── verify.rs        Parts, sizes, per-frame SHA-256, seal and signature
├── diff.rs          Archive vs live-source drift detection
│
├── search.rs        Glob + substring search over index entries
├── stats.rs         Compression ratio, extension stats, dedup savings
├── cat.rs           Stream single file to stdout
│
├── update.rs        Incremental archive (new/modified only, reuses older parts)
├── merge.rs         Multi-archive merge into one
├── prune.rs         Age + count based pruning, protects archives others depend on
├── repair.rs        Index reconstruction from orphaned tar parts
│
├── completions.rs   Shell completion generation (bash/zsh/fish)
└── utils.rs         Formatting helpers, timestamps, print_banner
```

---

## Data Flow

### Create

```
Source directory
      │
      ▼
scan::scan_directory()          ← WalkDir + glob excludes
      │
      ▼
index::ArchivumIndex::build()   ← assign tar parts (two-pass)
      │
      ▼
checksum::compute_checksums()   ← parallel SHA-256 via Rayon
      │
      ▼
tar_writer::write_archive()     ← stream source → tar → compress
      │
      ▼
index.write()                   ← index.arc.json
checksum::seal_index()          ← index.arc.json.b3 (Blake3)
```

### Restore

```
index.arc.json → read, check Blake3 seal (+ signature with --pubkey), validate paths
      │
      ▼
Group wanted files by frame (a dedup alias is served from its origin's frame)
      │
      ▼
Frames in parallel (one worker each):
  ├── Decompress just that frame
  ├── Iterate its tar members
  └── Write matching files, hashing while writing → compare to sha256
      │
      ▼
Symlinks, then directory permissions/mtime (deepest first)
```

### Verify

```
Seal + signature → every referenced part exists and has the recorded size
      │
      ▼
Frames in parallel: recompute SHA-256 of every member, report
corrupt / missing / unlisted members. Exit code is non-zero on any problem.
```

---

## Key Design Decisions

### Frames, batching and the writer pipeline

`tar_writer::ArchiveSink` plans parts on the fly (uncompressed size and file-count limits), batches small files into ~1 MiB frames and hands each batch to a rayon worker for compression. Finished frames are written strictly in plan order, so the output never depends on the thread count. Large files (4 MiB and up) are streamed as their own frame; for zstd these use the multithreaded encoder, whose output is identical for any worker count of 1 or more. Frames below the threshold always use the single-threaded encoder, which keeps the rule independent of the machine.

Parts are written as `*.tmp` and renamed only when everything (data, end-of-archive frame, fsync) succeeded. The index and its seal are written the same way.

### OutputCtx

Every command receives an `&OutputCtx` reference. All output is routed through it:

```rust
pub struct OutputCtx {
    pub json: bool,
    pub quiet: bool,
    pub dry_run: bool,
    log: Option<Arc<Mutex<File>>>,
}
```

- `out.println(s)` - prints unless quiet; always logs to file
- `out.eprintln(s)` - always prints to stderr; always logs
- `out.raw(s)` - always prints (for JSON / cat output)
- `out.dry(s)` - prints `[dry-run] …` unless quiet

This ensures `--quiet`, `--log-file`, and `--json` work consistently across every command.

### Blake3 Index Seal

After writing `index.arc.json`, the Blake3 hash of its content is written to `index.arc.json.b3`. Before any verify or restore operation, this seal is checked. Any modification of the index - even a single byte - is detected immediately.

### Deduplication

When `--dedup` is enabled:

1. As checksums are computed, a `HashMap<sha256, path>` is maintained
2. If a file's SHA-256 matches a previously seen file, its `dedup_of` field is set to that path
3. Deduped files are included in the index but **not** written to the tar
4. During restore, dedup files are restored by copying from the already-restored original

### Path Traversal Guard

During restore, every entry path is checked for `..` components:

```rust
if entry.path.components().any(|c| c == Component::ParentDir) {
    bail!("Path traversal attempt: {}", entry.path.display());
}
```

This prevents a malicious archive from writing outside the target directory.

---

## Adding a New Command

1. Create `src/my_command.rs`
2. Add the Apache 2.0 header
3. Implement `pub fn my_command(..., out: &OutputCtx) -> Result<()>`
4. Add `mod my_command;` to `main.rs`
5. Add the `Commands::MyCommand { ... }` variant to the enum
6. Wire up the dispatch arm in the `match cli.command` block
7. Update `README.md` and `docs/COMMANDS.md`
