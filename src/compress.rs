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
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgo {
    #[default]
    None,
    Gzip,
    Bzip2,
    Lz4,
    Zstd,
}

/// `0` means "auto" (all available cores). The multithreaded zstd path is always
/// used for large frames (at least one worker) because its output is
/// byte-identical for every worker count, which keeps archives reproducible
/// across machines.
pub fn effective_zstd_threads(requested: u32) -> u32 {
    if requested > 0 {
        requested.min(64)
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1)
            .min(64)
    }
}

pub fn algo_from_magic(magic: &[u8]) -> Option<CompressionAlgo> {
    if magic.starts_with(&[0x1f, 0x8b]) {
        Some(CompressionAlgo::Gzip)
    } else if magic.starts_with(b"BZh") {
        Some(CompressionAlgo::Bzip2)
    } else if magic.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some(CompressionAlgo::Zstd)
    } else if magic.starts_with(&[0x04, 0x22, 0x4d, 0x18]) {
        Some(CompressionAlgo::Lz4)
    } else {
        None
    }
}

fn read_magic(file: &mut File, start: u64) -> io::Result<([u8; 4], usize)> {
    let mut buf = [0u8; 4];
    let mut n = 0;
    while n < buf.len() {
        let r = file.read(&mut buf[n..])?;
        if r == 0 {
            break;
        }
        n += r;
    }
    file.seek(SeekFrom::Start(start))?;
    Ok((buf, n))
}

impl CompressionAlgo {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "none" | "raw" => Ok(Self::None),
            "gzip" | "gz" => Ok(Self::Gzip),
            "bzip2" | "bz2" => Ok(Self::Bzip2),
            "lz4" => Ok(Self::Lz4),
            "zstd" | "zst" => Ok(Self::Zstd),
            other => bail!(
                "Unknown compression: '{}'. Use: none, gzip, bzip2, lz4, zstd",
                other
            ),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Bzip2 => "bzip2",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::None => ".tar",
            Self::Gzip => ".tar.gz",
            Self::Bzip2 => ".tar.bz2",
            Self::Lz4 => ".tar.lz4",
            Self::Zstd => ".tar.zst",
        }
    }

    /// Write one complete, independently decodable frame (a gzip member, bzip2
    /// stream, lz4 frame or zstd frame; raw bytes for `none`) and hand the
    /// underlying writer back. `zstd_workers == 0` compresses on the calling thread.
    ///
    /// Concatenated frames are still one valid stream for the matching standard
    /// tool, which is what makes the parts both seekable and self-describing.
    pub fn encode_frame<W: Write>(
        &self,
        out: W,
        zstd_level: i32,
        zstd_workers: u32,
        body: impl FnOnce(&mut dyn Write) -> Result<()>,
    ) -> Result<W> {
        Ok(match self {
            Self::None => {
                let mut out = out;
                body(&mut out)?;
                out.flush()?;
                out
            }
            Self::Gzip => {
                let mut e = flate2::write::GzEncoder::new(out, flate2::Compression::default());
                body(&mut e)?;
                e.finish()?
            }
            Self::Bzip2 => {
                let mut e = bzip2::write::BzEncoder::new(out, bzip2::Compression::default());
                body(&mut e)?;
                e.finish()?
            }
            Self::Lz4 => {
                let info = lz4_flex::frame::FrameInfo::new().content_checksum(true);
                let mut e = lz4_flex::frame::FrameEncoder::with_frame_info(info, out);
                body(&mut e)?;
                e.finish().map_err(io::Error::other)?
            }
            Self::Zstd => {
                let mut e = zstd::Encoder::new(out, zstd_level.clamp(1, 22))?;
                // content checksum: lets verify/restore detect bit rot inside a frame
                e.include_checksum(true)?;
                if zstd_workers > 0 {
                    e.multithread(zstd_workers)?;
                }
                body(&mut e)?;
                e.finish()?
            }
        })
    }

    /// Decoded byte stream of `len` bytes starting at `offset` in `path`
    /// (one frame). `len == 0` means "the whole file", decoded as a sequence of
    /// frames. The format is checked against the file's magic bytes first.
    pub fn open_stream(&self, path: &Path, offset: u64, len: u64) -> Result<Box<dyn Read>> {
        let mut file =
            File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
        file.seek(SeekFrom::Start(offset))?;
        let (magic, n) = read_magic(&mut file, offset)?;
        let detected = algo_from_magic(&magic[..n]);
        match (self, &detected) {
            (Self::None, None) => {}
            (Self::None, Some(d)) => bail!(
                "{} looks {}-compressed but the index says it is uncompressed",
                path.display(),
                d.name()
            ),
            (want, Some(d)) if want == d => {}
            (want, other) => bail!(
                "{} at offset {} is not a valid {} stream (detected: {})",
                path.display(),
                offset,
                want.name(),
                other.as_ref().map(|d| d.name()).unwrap_or("unknown format")
            ),
        }

        let src: Box<dyn Read> = if len > 0 {
            Box::new(file.take(len))
        } else {
            Box::new(file)
        };
        Ok(match self {
            Self::None => Box::new(BufReader::new(src)),
            Self::Gzip => Box::new(flate2::read::MultiGzDecoder::new(src)),
            Self::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(src)),
            Self::Lz4 => Box::new(lz4_flex::frame::FrameDecoder::new(BufReader::new(src))),
            Self::Zstd => Box::new(zstd::Decoder::new(src)?),
        })
    }
}
