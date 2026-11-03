# Changelog

All notable changes to Archivum are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.4.0] - 2026-11-01

A correctness, safety and performance release. It includes a **breaking index
format change (v4)**: archives written by 0.3.0 and earlier are not readable by this
release (a clear error says so); restore them with the version that wrote them.

Several commands in 0.3.0 could silently produce archives that restored wrong or
incomplete data. All of those are fixed, and every fix has a regression test in
`tests/integration.rs`.

### Breaking

- **Index format v4** (see `docs/INDEX_FORMAT.md`). The header lists `parts[]` (path,
  compression, compressed size) and every stored file carries a `loc {part, off, len}`.
  This replaces `tar_part`, `tar_base`, `part_bases`, `total_parts` and the single
  `compression` field. Absent optional fields are no longer written.
- Index versions other than 4 are rejected.
- `verify` exits non-zero whenever it finds a problem, also with `--continue-on-error`
  (which now only means "keep scanning after the first problem").
- Tar headers are written from the index (fixed uid/gid), so the same tree always
  produces the same bytes.
- `create` and `merge` refuse to write over an existing archive unless `--overwrite`.
- `repair` auto-detects the compression, computes checksums from the archive content
  and refuses to overwrite an existing index.

### Added

- **Frames.** Parts are sequences of independently compressed frames for every
  algorithm: small files are batched to about 1 MiB, large files get their own. `cat`
  and `extract` decompress only the frame of the requested file (one file from a
  218 MB archive in 8 ms instead of a full scan), restore and verify run in parallel
  across frames, and damage is confined to one frame. Concatenated frames are still an
  ordinary tar stream for `gzip`, `bzip2`, `zstd` or `lz4` plus `tar`.
- **Parallel compression** of small frames on a worker pool for all algorithms
  (`--threads`). Output is byte-identical for any thread count.
- **zstd multithreading** for large frames: `--zstd-threads N` and `zstd_threads` in the
  config (0 = all cores). Output is identical for any worker count of 1 or more.
- zstd and lz4 frames carry content checksums.
- **Per-part compression.** `update` may switch algorithm and chains can mix gzip, zstd
  and so on. Without `--compress`, `update` uses the compression of the newest part.
- **Ed25519 signing.** `archivum keygen NAME`, `--sign KEY` on `create`, `update` and
  `merge`, and a global `--pubkey FILE` (or `[security]` in the config) that makes every
  command refuse unsigned, foreign-signed or modified indexes. `verify` reports the
  signature.
- `restore` verifies SHA-256 while extracting, restores modification times, extracts
  frames in parallel (`--threads`) and reports every missing or corrupt file.
  `extract` and `cat` verify what they stream. `extract --force`, `verify --threads`,
  `diff --exclude`.
- `verify` checks that every part exists and has the recorded size.
- `stats` shows the compression of the archive.
- `ARCHIVUM_CONFIG` environment variable to point at another config file.
- Integration test suite (`tests/integration.rs`).

### Fixed (data loss and silent corruption)

- `restore` and `verify` ignored which older archive a part belonged to, so incremental
  archives restored stale or missing files while `verify` printed PASS.
- `merge` read source parts with the output compression and swallowed every error: a
  merge with a different `--compress` produced an empty archive with a full index. It
  now reads each source with its own settings, streams every part once, keeps dedup
  aliases, directories and symlinks, verifies data while copying and aborts on any
  problem. For a path present in several inputs, the newest archive wins.
- `update` into the old archive's directory overwrote the parts it depends on; new
  directories and symlinks were never recorded; chained updates kept stale references;
  a dedup alias whose origin changed restored the wrong content.
- `prune` could delete archives that newer incremental archives still read from.
- `verify` passed when a tar member was missing or when nothing had been checked.
- `restore` skipped dedup aliases when the origin already existed or was filtered out,
  ignored members missing from the data, restored symlinks despite `--filter`, failed
  on read-only directories and exited 0 after partial failure.
- Compressor and flush errors (for example disk full) were ignored when a part was
  closed. Parts are now finished explicitly and written under a `.tmp` name, and the
  index and its seal are written atomically.
- A file changing between the checksum pass and archiving is detected and aborts the run.
- Unreadable files abort checksumming instead of being archived without a hash;
  unreadable directories during the scan are reported instead of skipped silently.
- `--exclude node_modules` prunes the whole directory, and patterns without a `/`
  (such as `.DS_Store`) match at any depth.
- The output directory is never archived into itself.
- Paths in the index are stored with `/` separators on every platform. Previously an index written on Windows contained backslashes, which restored as a single oddly named file on Linux and macOS.
- `diff` honours the configured excludes, and with `--checksum` a touched but identical
  file is not reported as modified.
- A config file that omits keys no longer fails to load (missing keys fall back to
  defaults), and parse errors show their cause. The shipped `config.toml.example`
  failed to load before.

### Security

- `restore` creates directories one component at a time and refuses to follow symlinks,
  so an archive cannot write outside the target directory.
- Index entries with absolute or `..` paths, dangling dedup references, bad part
  references or an unsupported format version are rejected when the index is read.
- The `.b3` seal is documented as corruption detection, not tamper protection (the hash
  is unkeyed); use signing for that.

### Removed

- The unused `src/tar.rs`.

---

## [0.3.0] - 2026-06-09

### Security

- Patched RUSTSEC-2026-0067: `unpack_in` symlink chmod exploit (tar 0.4.44 and below)
- Patched RUSTSEC-2026-0068: PAX size header incorrectly ignored (tar 0.4.44 and below)
- Bumped `tar` dependency to 0.4.46

### Fixed

- Clippy warning `unnecessary_sort_by` in `stats.rs`

---

## [0.2.0] - 2026-02-26

### Added

- **`search` command** - Find files by glob (`*.rs`) or substring (`config`)
- **`stats` command** - Compression ratio, extension breakdown, dedup savings, per-part sizes
- **`cat` command** - Stream any archived file directly to stdout
- **`update` command** - Incremental archive: only re-archives new/modified files
- **`merge` command** - Combine multiple archives into one unified archive
- **`prune` command** - Remove old archives by count (`--keep N`) or age (`--max-age DAYS`)
- **`repair` command** - Rebuild a corrupted/missing `index.arc.json` from tar parts
- **`completions` command** - Generate shell completions (bash / zsh / fish)
- **`setup` command** - Interactive `config.toml` setup wizard
- **`config` command** - Display current effective configuration
- **5 compression algorithms**: `none`, `gzip`, `zstd`, `bzip2`, `lz4`
- **Content deduplication** (`--dedup`) - SHA-256 based, skips identical files
- **Archive notes** (`--notes`) - Attach a human-readable note to any archive
- **Split by file count** (`--split-files N`) - complement to `--split-gb`
- **Blake3 index integrity seal** - `index.arc.json.b3` tamper detection
- **`--quiet` flag** - Suppress all stdout output
- **`--json` flag** - Machine-readable JSON output for every command
- **`--dry-run` flag** - Safe simulation mode: no files written
- **`--log-file` flag** - Append structured output to a file
- **Configuration file** (`~/.config/archivum/config.toml`) with all defaults
- **Index v3** - Adds `notes`, `dedup_of`, `zstd_level`, multi-base part refs, Blake3 seal
- **Parallel checksums** via Rayon thread pool

### Fixed

- Search: glob patterns like `sub` incorrectly matched only exact strings, not substrings
- Quiet mode: `--quiet` flag was ignored in `list` and banner output
- Log file: log file was empty because `list` used raw `println!` bypassing `OutputCtx`
- Create: nonexistent source directory produced success exit code instead of error

### Improved

- Restore engine: O(n + m) grouping by tar part (was O(n × m))
- All commands respect `--quiet`, `--json`, `--log-file` consistently
- Richer colored terminal output throughout

### Security

- Path traversal guard in restore (`..` components rejected)
- Blake3 seal on index file - tampering is detected before restore/verify

---

## [0.1.0] - 2024

### Added

- **SHA-256 checksums** for every archived file
- **Multi-algorithm compression**: `none`, `gzip`, `zstd`
- **Parallel checksum computation** with `--threads`
- **`verify` command** - part existence + checksum verification
- **`diff` command** - detect added/removed/modified files vs source
- **`info` command** - full metadata for a single file
- **`extract` command** - restore a single file without full restore
- **Symlink preservation**
- **Glob-based exclude patterns** (`--exclude "*.log"`)
- **Filter patterns** in `list` and `restore`
- **Rich CLI** with clap, colored output, progress bars
- **GitHub Actions**: CI (Linux/macOS/Windows), publish workflow, security audit
- **Index v2** - adds `created_at_human`, `total_symlinks`, `total_parts`, `compression`, `sha256`, `symlink_target`
- **Two-pass tar writing**
- **Efficient restore** grouped by tar part
- `--force` flag for restore
- `--restore-permissions` flag
- `--verbose` flag for `list`

### Fixed

- Panic on missing CLI arguments
- Inefficient restore re-opening tar per file
- Root directory included as empty entry in scan
- No cross-platform timestamp support

---

[0.3.0]: https://github.com/ankit-chaubey/Archivum/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/ankit-chaubey/Archivum/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/ankit-chaubey/Archivum/releases/tag/v0.1.0
