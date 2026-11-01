# Archivum - Index Format Reference (v4)

The `index.arc.json` file is the heart of every Archivum archive. It is plain JSON holding all metadata, checksums and the location of every file's data, so an archive can be listed, searched, verified and restored from it alone. It is readable with any editor or `jq`.

Format v4 is a clean break from v1-v3. This build refuses older indexes with a clear message; restore those archives with the release that wrote them.

---

## Files

```
<output>/
├── index.arc.json         the index
├── index.arc.json.b3      Blake3 seal (hex): detects corruption and accidental edits
├── index.arc.json.sig     optional Ed25519 signature (see `archivum keygen`, `--sign`, `--pubkey`)
├── data.part000.tar.zst   data parts
└── data.part001.tar.zst
```

The `.b3` hash is unkeyed, so it does not stop someone who deliberately rewrites both files. The signature does: with `--pubkey KEY` every command refuses an index that is unsigned, signed by another key, or modified after signing. Because the index holds the SHA-256 of every file, a valid signature protects all of the archive's content.

---

## Data layout: parts and frames

A **part** is a sequence of independently compressed **frames** (a zstd frame, gzip member, bzip2 stream or lz4 frame; raw bytes for `none`). A frame holds either one large file (4 MiB and up) or a batch of small files (about 1 MiB of tar). Every file in the index records the frame that holds it.

Consequences:

- Reading one file decompresses only its own frame (`cat`, `extract`).
- Frames are independent, so restore and verify run in parallel, even inside one part.
- Damage stays inside a frame: a flipped bit costs the files of that frame, not the rest of the part.
- Small frames are compressed on a worker pool while the output order stays fixed, so the bytes do not depend on the thread count.
- Concatenating the frames of a part gives an ordinary tar stream. `zstd -dc data.part000.tar.zst | tar tf -` works (and `gzip`, `bzip2`, `lz4` likewise), because each part ends with a frame holding the tar end-of-archive blocks.

---

## Schema

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
    "parts": [
      { "path": "data.part000.tar.zst", "compression": "zstd", "size": 18304211 },
      { "path": "../2026-02-01/data.part000.tar.gz", "compression": "gzip", "size": 40211876 }
    ]
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
    },
    { "path": "src/copy.rs", "entry_type": "file", "size": 13421, "mtime": 1740500000,
      "unix_mode": 33188, "sha256": "e3b0c4...", "dedup_of": "src/main.rs" },
    { "path": "link", "entry_type": "symlink", "size": 0, "mtime": 1740500000,
      "unix_mode": 41471, "symlink_target": "src/main.rs" },
    { "path": "src", "entry_type": "directory", "size": 0, "mtime": 1740500000, "unix_mode": 16877 }
  ]
}
```

### Header

| Field | Type | Description |
|-------|------|-------------|
| `version` | int | Index format version (`4`) |
| `created_at_unix` / `created_at_human` | int / string | Creation time |
| `total_files`, `total_dirs`, `total_symlinks` | int | Counts |
| `total_size` | int | Sum of all file sizes in bytes |
| `zstd_level` | int | Level used for new zstd frames (informational) |
| `notes` | string | Free text |
| `parts` | array | Every part this archive reads, see below |

### `parts[]`

| Field | Description |
|-------|-------------|
| `path` | Path relative to the index file's directory. May point into another directory (`../older/data.part000.tar.zst`): incremental archives list the parts of the archives they build on |
| `compression` | `none`, `gzip`, `bzip2`, `lz4` or `zstd`. Every part has its own, so an update can switch algorithm |
| `size` | Compressed size in bytes, so `verify` spots a truncated part without reading it |

New parts of an archive come first, followed by the parts of the archive it was updated from.

### Entries

| Field | Required | Description |
|-------|----------|-------------|
| `path` | yes | Relative path with `/` separators. Absolute paths and `..` are rejected on read |
| `entry_type` | yes | `file`, `directory` or `symlink` |
| `size` | yes | Size in bytes (0 for directories and symlinks) |
| `mtime`, `unix_mode` | no | Restored by `restore` (mode with `--restore-permissions`) |
| `sha256` | files | Checked while restoring, extracting and verifying |
| `loc` | stored files | `{part, off, len}`: index into `parts`, byte offset of the frame, compressed length. `len: 0` means the frame boundaries are unknown (index rebuilt by `repair`): the whole part is decoded to find the file |
| `dedup_of` | aliases | Path of the entry that holds the data; the alias has no `loc` |
| `symlink_target` | symlinks | Link target |

Entries are sorted by path. The same tree always produces the same index apart from `created_at_*`.

---

## Inspecting with jq

```bash
jq '.header' index.arc.json
jq -r '.entries[].path' index.arc.json
jq -r '.entries[] | select(.size > 10485760) | "\(.size)\t\(.path)"' index.arc.json | sort -rn
jq -r '.entries[] | select(.dedup_of != null) | "\(.path) -> \(.dedup_of)"' index.arc.json
jq '[.entries[].loc | select(. != null) | .off] | unique | length' index.arc.json   # frame count
```

---

## Version history

| Version | Changes |
|---------|---------|
| v4 | Frames and per-file `loc`; per-part compression and size in `parts`; optional Ed25519 signature; paths validated on read. Replaces `tar_part`, `tar_base`, `part_bases`, `total_parts` and the single `compression` field |
| v3 | `notes`, `dedup_of`, `zstd_level`, `part_bases`, Blake3 seal |
| v2 | `created_at_human`, `total_symlinks`, `total_parts`, `compression`, `sha256`, `symlink_target` |
| v1 | Basic `path`, `size`, `mtime`, `tar_part` |
