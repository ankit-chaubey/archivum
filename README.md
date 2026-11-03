<div align="center">

```
  ▲ Archivum
```

**Deterministic · Split · Checksummed · Compressed · Verifiable**

*Archive anything. Restore faithfully. Verify with confidence.*

---

[![Crates.io](https://img.shields.io/crates/v/archivum?style=for-the-badge&color=00d4ff&logo=rust)](https://crates.io/crates/archivum)
[![Downloads](https://img.shields.io/crates/d/archivum?style=for-the-badge&color=00d4ff)](https://crates.io/crates/archivum)
[![CI](https://img.shields.io/github/actions/workflow/status/ankit-chaubey/Archivum/ci.yml?style=for-the-badge&label=CI&logo=github)](https://github.com/ankit-chaubey/Archivum/actions)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue?style=for-the-badge)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange?style=for-the-badge&logo=rust)](https://www.rust-lang.org)

</div>

---

## What is Archivum?

**Archivum** is a modern, fast, and trustworthy command-line archive system built in Rust. It goes far beyond `tar` and `zip` - every file is checksummed with SHA-256, the index is human-readable JSON with a Blake3 integrity seal and an optional Ed25519 signature, and the archive can be split into any size, compressed with 5 algorithms, searched, diffed, deduplicated, merged, updated incrementally, and repaired. Data is stored in independently compressed frames, so a single file can be read without decompressing the rest, and restore and verify run in parallel.

Designed for **long-term backups**, **forensic preservation**, **offline cold storage**, and **DevOps workflows** where correctness is non-negotiable.

---

## Feature Comparison

| Feature | tar | zip | rsync | **Archivum** |
|---------|:---:|:---:|:-----:|:------------:|
| Split into custom-size parts | ✗ | ✗ | ✗ | ✓ |
| Split by file count | ✗ | ✗ | ✗ | ✓ |
| Human-readable JSON index | ✗ | ✗ | ✗ | ✓ |
| Blake3-sealed index integrity | ✗ | ✗ | ✗ | ✓ |
| Ed25519-signed index (`--sign`, `--pubkey`) | ✗ | ✗ | ✗ | ✓ |
| Read one file without decompressing the archive | ✗ | ✓ | ✗ | ✓ |
| Parallel restore and verify | ✗ | ✗ | ✗ | ✓ |
| Mixed compression across incremental updates | ✗ | ✗ | ✗ | ✓ |
| Restore that refuses to write through symlinks | ✗ | ✗ | ✗ | ✓ |
| Per-file SHA-256 checksums | ✗ | CRC-32 | ✗ | ✓ |
| 5 compression algorithms | ✗ | deflate | ✗ | ✓ |
| Content deduplication | ✗ | ✗ | ✓ | ✓ |
| Incremental update | ✗ | ✗ | ✓ | ✓ |
| Drift detection (`diff`) | ✗ | ✗ | ✓ | ✓ |
| Archive merging | ✗ | ✗ | ✗ | ✓ |
| Smart pruning | ✗ | ✗ | ✗ | ✓ |
| Auto repair | ✗ | ✗ | ✗ | ✓ |
| File search (glob + substring) | ✗ | ✗ | ✗ | ✓ |
| Archive statistics | ✗ | ✗ | ✗ | ✓ |
| Stream single file to stdout | ✗ | ✗ | ✗ | ✓ |
| Shell completions | ✗ | ✗ | ✗ | ✓ |
| JSON output for scripting | ✗ | ✗ | ✗ | ✓ |
| Restore single file (`extract`) | ✗ | ✓ | ✗ | ✓ |
| Symlink preservation | varies | ✗ | ✓ | ✓ |
| Dry-run mode | ✗ | ✗ | ✓ | ✓ |
| Quiet + log-file mode | ✗ | ✗ | ✗ | ✓ |

---

## Installation

### From crates.io *(recommended)*

```bash
cargo install archivum
```

### From source

```bash
git clone https://github.com/ankit-chaubey/Archivum
cd Archivum
cargo build --release
# Binary: ./target/release/archivum
sudo cp target/release/archivum /usr/local/bin/
```

### Pre-built binaries

Download the latest binary from the [Releases page](https://github.com/ankit-chaubey/Archivum/releases):

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `archivum-linux-x86_64` |
| Linux aarch64 | `archivum-linux-aarch64` |
| macOS x86_64 | `archivum-macos-x86_64` |
| macOS Apple Silicon | `archivum-macos-aarch64` |
| Windows x86_64 | `archivum-windows-x86_64.exe` |

### Shell completions

```bash
# Bash
archivum completions bash >> ~/.bashrc

# Zsh
archivum completions zsh >> ~/.zshrc

# Fish
archivum completions fish > ~/.config/fish/completions/archivum.fish
```

---

## Quick Start

```bash
# Create an archive (zstd compression, 2 GB parts)
archivum create ./my-project ./backup --compress zstd --split-gb 2

# List contents
archivum list ./backup/index.arc.json

# Verify integrity
archivum verify ./backup/index.arc.json

# Restore
archivum restore ./backup/index.arc.json ./restored

# Diff - what changed since the archive was made?
archivum diff ./backup/index.arc.json ./my-project

# Search for files
archivum search ./backup/index.arc.json "*.rs"

# View stats
archivum stats ./backup/index.arc.json
```

---

## Commands

### `create` - Create an archive

```
archivum create <SOURCE> <OUTPUT> [OPTIONS]
```

| Option | Description | Default |
|--------|-------------|---------|
| `--compress <ALGO>` | `none` \| `gzip` \| `zstd` \| `bzip2` \| `lz4` | `zstd` (config) |
| `--zstd-level <N>` | Zstd compression level (1–22) | `3` |
| `--split-gb <GB>` | Max size of each part in GB | `4` |
| `--split-files <N>` | Max files per part | `0` (unlimited) |
| `--exclude <GLOB>` | Exclude pattern (repeatable) | - |
| `--dedup` | Skip duplicate files (SHA-256 based) | off |
| `--zstd-threads <N>` | zstd worker threads (0 = all cores; output identical for any count) | `0` |
| `--overwrite` | Replace an existing archive in the output directory | off |
| `--sign <KEY>` | Sign the index with a secret key from `archivum keygen` | - |
| `--notes <TEXT>` | Attach a note to the archive | - |
| `--threads <N>` | Checksum parallelism | `4` |
| `--dry-run` | Show what would happen without writing | off |
| `--quiet` | Suppress all output | off |
| `--log-file <PATH>` | Append output to a log file | - |

```bash
# Zstd compression, 1 GB parts, exclude build artifacts
archivum create ./my-project ./backup \
  --compress zstd --split-gb 1 \
  --exclude "target/**" --exclude "*.log"

# Deduplicate, annotate, dry-run first
archivum create ./photos ./backup --dedup --notes "Family photos 2026" --dry-run

# Split by file count (max 500 files per part)
archivum create ./documents ./backup --split-files 500

# Log output for automation
archivum create ./data ./backup --compress lz4 --quiet --log-file /var/log/archivum.log
```

**Output structure:**
```
backup/
├── index.arc.json           ← JSON index (human-readable)
├── index.arc.json.b3        ← Blake3 integrity seal
├── data.part000.tar         ← Part 0
├── data.part001.tar.zst     ← Part 1 (compressed)
└── ...
```

---

### `list` - List contents

```
archivum list <INDEX> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Show all file entries |
| `--filter <GLOB>` | Filter entries by glob |
| `--json` | Output as JSON |

```bash
archivum list ./backup/index.arc.json
archivum list ./backup/index.arc.json --verbose --filter "**/*.rs"
archivum list ./backup/index.arc.json --json | jq '.header'
```

---

### `restore` - Restore an archive

```
archivum restore <INDEX> <TARGET> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--filter <GLOB>` | Only restore matching files |
| `-f, --force` | Overwrite existing files |
| `--restore-permissions` | Restore Unix file permissions |
| `--threads <N>` | Parallel workers (frames are independent) |
| `--dry-run` | Show what would be restored |

SHA-256 is verified while each file is written, modification times are restored, and the exit code is non-zero if any file is missing or corrupt (the intact files are still written). Restore never follows symlinks, so an archive cannot write outside `<TARGET>`.

```bash
# Full restore
archivum restore ./backup/index.arc.json ./restored

# Restore only Rust source files
archivum restore ./backup/index.arc.json ./restored --filter "**/*.rs"

# Force overwrite
archivum restore ./backup/index.arc.json ./restored --force --restore-permissions
```

> **Efficiency**: The restore engine groups files by tar part so each part is read exactly once - O(n + m) instead of the naïve O(n × m).

---

### `verify` - Verify integrity

```
archivum verify <INDEX> [OPTIONS]
```

Checks:
1. Blake3 seal on the index file (detects corruption and accidental edits)
2. Ed25519 signature, if the index is signed (and required with `--pubkey`)
3. Every referenced part exists and has the recorded size
4. Every file is present in its frame and its SHA-256 matches the stored value

| Option | Description |
|--------|-------------|
| `-c, --continue-on-error` | Keep scanning after the first problem |
| `--threads <N>` | Parallel workers |

The exit code is non-zero whenever a problem is found.

```bash
archivum verify ./backup/index.arc.json
archivum verify ./backup/index.arc.json --continue-on-error --json
```

---

### `diff` - Detect drift

```
archivum diff <INDEX> <SOURCE> [OPTIONS]
```

Compare an archive against the live source directory. Detects **added**, **removed**, and **modified** files.

| Option | Description |
|--------|-------------|
| `--changed-only` | Suppress unchanged files |
| `--checksum` | Use SHA-256 comparison (not just mtime/size) |
| `--json` | Output as JSON |

```bash
archivum diff ./backup/index.arc.json ./my-project --changed-only

# Output:
#   ~ MODIFIED  src/main.rs    (12.4 KB → 13.1 KB)
#   + ADDED     src/new_mod.rs (2.1 KB)
#   - REMOVED   old_file.txt
#   Added: 1  Removed: 1  Modified: 1  Unchanged: 142
```

---

### `search` - Search files

```
archivum search <INDEX> <PATTERN>
```

Search by glob pattern (`*.rs`, `**/*.jpg`) or substring (`config`, `2026`).

```bash
archivum search ./backup/index.arc.json "*.toml"
archivum search ./backup/index.arc.json "config" --json
```

---

### `stats` - Archive statistics

```
archivum stats <INDEX>
```

Displays compression ratio, size breakdown by file extension, part distribution, and deduplication savings.

```bash
archivum stats ./backup/index.arc.json
archivum stats ./backup/index.arc.json --json
```

---

### `info` - File metadata

```
archivum info <INDEX> <FILE>
```

```bash
archivum info ./backup/index.arc.json src/main.rs
```

Output:
```
──────────────────────────────────────────────────
  File:    src/main.rs
  Type:    file
  Size:    13.1 KiB
  SHA-256: e3b0c442...
  Part:    part000
  Modified: 2026-02-26 12:00:00 UTC
  Mode:    0o644
──────────────────────────────────────────────────
```

---

### `extract` - Extract single file

```
archivum extract <INDEX> <FILE> [OPTIONS]
```

```bash
archivum extract ./backup/index.arc.json src/main.rs
archivum extract ./backup/index.arc.json docs/report.pdf --output ./recovered.pdf
```

Only the frame holding the file is decompressed. Use `--force` to overwrite an existing output file; the checksum is verified while writing.

---

### `cat` - Stream file to stdout

```
archivum cat <INDEX> <FILE>
```

Stream a single file's content directly to stdout - perfect for piping. Only the frame holding the file is decompressed, so this is fast even in very large archives, and the checksum is verified at the end.

```bash
archivum cat ./backup/index.arc.json config.toml
archivum cat ./backup/index.arc.json data.csv | wc -l
archivum cat ./backup/index.arc.json script.sh | bash
```

---

### `update` - Incremental update

```
archivum update <OLD_INDEX> <SOURCE> <OUTPUT> [OPTIONS]
```

Creates a new archive containing only files that are **new or modified** since the last archive. Unchanged files are referenced from the older archive's parts, not re-archived. Directories and symlinks always come from the fresh scan, and the new index lists the older parts, so chains of updates keep working. `--compress` may differ from the old archive (each part records its own compression); without it the compression of the newest part is used. The output directory must differ from the old archive's directory.

| Option | Description |
|--------|-------------|
| `--checksum` | Compare SHA-256 instead of only size and mtime |
| `--compress`, `--zstd-level`, `--zstd-threads`, `--split-gb`, `--split-files`, `--exclude`, `--threads` | As for `create` |
| `--dedup` | Deduplicate identical files within the new data |
| `--sign <KEY>` | Sign the new index |

```bash
archivum update ./backup/index.arc.json ./my-project ./backup-v2
archivum update ./backup/index.arc.json ./src ./backup-v2 --checksum
```

---

### `merge` - Merge archives

```
archivum merge <INDEX1> <INDEX2> ... <OUTPUT>
```

Combine multiple archives into a single archive. Each input is read with its own compression, every source part is streamed once, and directories, symlinks and dedup aliases are kept. When a path exists in several inputs, the archive with the newest creation time wins. Data is verified while it is copied, and the merge aborts on any problem. Options: `--compress`, `--zstd-level`, `--zstd-threads`, `--split-gb`, `--overwrite`, `--sign <KEY>`.

```bash
archivum merge ./backup-jan/index.arc.json ./backup-feb/index.arc.json ./merged \
  --compress zstd
```

---

### `prune` - Prune old archives

```
archivum prune <DIR> [OPTIONS]
```

Remove old archive directories, keeping the most recent N or those newer than a time threshold. An archive that a newer incremental archive still reads parts from is never deleted.

| Option | Description | Default |
|--------|-------------|---------|
| `--keep <N>` | Keep the N most recent archives | `3` |
| `--max-age <DAYS>` | Remove archives older than N days | `0` (off) |
| `--dry-run` | Show what would be removed | off |

```bash
archivum prune /backups --keep 5
archivum prune /backups --max-age 90 --dry-run
```

---

### `repair` - Repair a corrupted index

```
archivum repair <DIR>
```

Rebuilds a missing `index.arc.json` by scanning the tar parts on disk. The compression is auto-detected (or given with `--compression`), checksums are computed from the archive content, and an existing index is never overwritten. Directories, symlinks and dedup relations are not stored in the parts and cannot be recovered, and the rebuilt index has no frame offsets, so restoring from it decodes whole parts.

```bash
archivum repair ./backup                       # compression is auto-detected
archivum repair ./backup --compression zstd
```

---

### `keygen` - Signing keys

```
archivum keygen <NAME>
```

Writes `NAME.key` (secret, mode 0600) and `NAME.pub`. An existing key is never overwritten.

```bash
archivum keygen mykey
archivum create ./data ./backup --sign mykey.key
archivum --pubkey mykey.pub verify ./backup/index.arc.json
archivum --pubkey mykey.pub restore ./backup/index.arc.json ./out
```

With `--pubkey` (or `pubkey` under `[security]` in the config) every command refuses an index that is unsigned, signed by another key, or modified after signing. The index lists the SHA-256 of every file, so the signature protects the whole archive.

---

### `completions` - Shell completions

```
archivum completions <SHELL>
```

```bash
archivum completions bash
archivum completions zsh
archivum completions fish
```

---

## Configuration

Archivum reads a `config.toml` from:
- **Linux/macOS**: `~/.config/archivum/config.toml`
- **Windows**: `%APPDATA%\archivum\config.toml`

```bash
# Interactive setup wizard
archivum setup

# Show current configuration
archivum config
```

**Example `config.toml`:**

```toml
[defaults]
compress   = "zstd"
zstd_level = 9
split_gb   = 2.0
split_files = 0
threads    = 8
zstd_threads = 0      # 0 = all cores

[output]
quiet = false
json  = false

[create]
dedup   = false
notes   = ""
exclude = ["**/.DS_Store", "**/Thumbs.db", "**/__pycache__/**"]

[restore]
force               = false
restore_permissions = true

[update]
checksum_diff = false

[prune]
keep_last    = 5
max_age_days = 0

[security]
sign_key = ""        # secret key used to sign every index written
pubkey   = ""        # public key every index must be signed with when reading
```

Any key you leave out falls back to its default, so a config can be as short as one line.

The file lives in the platform config directory (`~/.config/archivum/config.toml` on Linux). Set `ARCHIVUM_CONFIG=/path/to/config.toml` to use another file, for example in containers or CI.

See [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) for full reference.

---

## Index Format

The `index.arc.json` is intentionally human-readable - inspect with any editor, `jq`, or `grep` without Archivum installed.

```json
{
  "header": {
    "version": 4,
    "created_at_unix": 1740567000,
    "created_at_human": "2026-02-26 12:30:00 UTC",
    "total_files": 142,
    "total_dirs": 18,
    "total_symlinks": 3,
    "total_size": 52428800,
    "zstd_level": 9,
    "notes": "Production backup - pre-deploy",
    "parts": [{ "path": "data.part000.tar.zst", "compression": "zstd", "size": 18304211 }]
  },
  "entries": [
    {
      "path": "src/main.rs",
      "entry_type": "file",
      "size": 13421,
      "mtime": 1740500000,
      "unix_mode": 33188,
      "sha256": "e3b0c44298fc1c149afbf4c8996fb924...",
      "loc": { "part": 0, "off": 1048576, "len": 220114 }
    }
  ]
}
```

The accompanying `.b3` file contains a Blake3 hash of the index - `verify` checks this automatically to detect corruption or accidental edits. The hash is unkeyed, so for protection against deliberate changes sign the index: `archivum keygen mykey`, `archivum create ... --sign mykey.key`, then `archivum --pubkey mykey.pub verify ...`. Files are stored in independently compressed frames, so `cat` and `extract` read only the frame of the file you ask for and restore/verify run in parallel.

See [`docs/INDEX_FORMAT.md`](docs/INDEX_FORMAT.md) for the full schema reference.

---

## Compression Algorithms

| Algorithm | Flag | Speed | Ratio | Best for |
|-----------|------|:-----:|:-----:|----------|
| None | `none` | ⚡⚡⚡ | 1× | Media, already-compressed files |
| LZ4 | `lz4` | ⚡⚡⚡ | ~1.5× | Real-time, fast storage |
| Gzip | `gzip` | ⚡⚡ | ~2–4× | Universal compatibility |
| Zstd | `zstd` | ⚡⚡ | ~3–7× | Best all-round choice |
| Bzip2 | `bzip2` | ⚡ | ~3–6× | High-ratio, space-critical |

---

## Global Flags

These flags work with every command:

| Flag | Description |
|------|-------------|
| `--quiet` | Suppress all stdout output |
| `--json` | Output machine-readable JSON |
| `--dry-run` | Simulate without writing anything |
| `--log-file <PATH>` | Append all output to a file |
| `--pubkey <FILE>` | Only accept indexes signed with this public key |

---

## Performance

- **Parallel checksums**: SHA-256 computed with a configurable Rayon thread pool (`--threads`)
- **Frames**: small files are batched into ~1 MiB frames and compressed in parallel (`--threads`), large files get their own frame (multithreaded for zstd with `--zstd-threads`); output is byte-identical for any thread count
- **Random access**: `cat` and `extract` decompress only the frame holding the file
- **Parallel restore and verify**: frames are independent, so workers share one part
- **Streaming**: large files are streamed through the compressor, memory stays flat (about 50 MB peak for a 1 GB file)
- **Deduplication**: Skips re-writing files with identical SHA-256 hashes
- **Incremental update**: Only archives new/modified files, O(diff) not O(total)

Module layout and design notes are in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Automation Flags

All commands support these flags for automation:

```bash
archivum --quiet create ./src ./backup --compress zstd
archivum --json  list ./backup/index.arc.json | jq '.entries | length'
archivum --log-file /var/log/arc.log verify ./backup/index.arc.json
archivum --dry-run restore ./backup/index.arc.json ./out
```

---

## Exclude Patterns

Uses [glob](https://en.wikipedia.org/wiki/Glob_(programming)) syntax via `globset`. A pattern that matches a directory skips everything below it, and a pattern without a `/` (such as `node_modules` or `.DS_Store`) matches at any depth:

```bash
archivum create ./src ./out \
  --exclude "target/**"          \  # Rust build dir
  --exclude "node_modules/**"    \  # Node.js packages
  --exclude "**/.DS_Store"       \  # macOS metadata
  --exclude "**/*.tmp"           \  # Temp files
  --exclude "**/__pycache__/**"     # Python cache
```

---

## Automation Examples

```bash
# Daily backup cron - quiet, logged
0 2 * * * archivum create /data /backups/$(date +%F) \
  --compress zstd --dedup --quiet --log-file /var/log/archivum.log

# Verify backup integrity weekly
0 6 * * 1 archivum verify /backups/$(ls -t /backups | head -1)/index.arc.json \
  --json > /var/log/archivum-verify.json

# Prune old backups (keep last 14)
archivum prune /backups --keep 14

# CI pre-deploy archive
archivum create ./dist ./releases/v${VERSION} --compress zstd \
  --notes "Release ${VERSION}" --quiet

# Pipe a config file from archive
archivum cat ./backup/index.arc.json etc/nginx.conf | diff - /etc/nginx/nginx.conf
```

---

## Contributing

Contributions are welcome! See [`CONTRIBUTING.md`](CONTRIBUTING.md) for guidelines.

```bash
git clone https://github.com/ankit-chaubey/Archivum
cd Archivum
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt
```

---

## Security

Archivum treats archives as untrusted input: paths in the index are validated, restore never follows symlinks out of the target directory, and every file is checked against its SHA-256 while it is extracted. For protection against someone who deliberately modifies an archive, sign the index (`archivum keygen`, `--sign`, `--pubkey`).

To report a vulnerability, see [`SECURITY.md`](SECURITY.md).

---

## License

Apache 2.0 - see [LICENSE](LICENSE)

```
Copyright 2026 Ankit Chaubey

Licensed under the Apache License, Version 2.0
```

---

<div align="center">

**▲ Archivum v0.4.0**

Made with ♥ by [Ankit Chaubey](https://github.com/ankit-chaubey) · [ankitchaubey.dev@gmail.com](mailto:ankitchaubey.dev@gmail.com)

[crates.io](https://crates.io/crates/archivum) · [docs](docs/) · [changelog](CHANGELOG.md) · [issues](https://github.com/ankit-chaubey/Archivum/issues)

</div>
