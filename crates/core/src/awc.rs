//! ADAT — the audio bank container (`.awc`).
//!
//! ```text
//! 0x00 'ADAT'
//! 0x04 u32 flags
//! 0x08 u32 stream count
//! 0x0C u32 header length
//! ....     optional tables selected by flags
//! ....     stream table: one u32 per stream
//! ....     8-byte chunk records, until header length
//! ```
//!
//! Chunk record layout:
//!
//! ```text
//! bits 63..56  type byte
//! bits 55..28  size in bytes
//! bits 27..0   offset relative to the bank
//! ```
//!
//! Types seen in the shipping banks: `0x55` Opus payload, `0x76`/`0xFA` format
//! record (`u32 sample_count`, `u16 sample_rate` at +8, codec byte at +0x13),
//! `0x77` seek table, `0x01` peak envelope.
//!
//! This module parses the container and the format records. Opus frame splitting
//! is implemented (2 KiB blocks, `u16` length prefix per frame); actual PCM
//! decoding needs an Opus backend and lives behind a trait so the core stays
//! dependency-free.

use crate::read::{is_zlib_head, Le};
use crate::rpf9::{Result, Rpf9Error};
use crate::source::Source;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub kind: u8,
    pub size: usize,
    pub offset: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct FormatRecord {
    pub samples: u32,
    pub sample_rate: u16,
    pub codec: u8,
    pub channels: u16,
}

#[derive(Clone, Debug)]
pub struct Awc {
    pub flags: u32,
    pub streams: u32,
    pub header_len: u32,
    pub chunks: Vec<Chunk>,
    pub format: Option<FormatRecord>,
    /// Bank size in bytes: the furthest chunk end.
    pub bank_size: u64,
}

impl Awc {
    pub fn parse(src: &dyn Source, off: u64) -> Result<Awc> {
        let head = src.read_at(off, 0x40)?;
        if head.len() < 0x10 || &head[..4] != b"ADAT" {
            return Err(Rpf9Error::NotRpf9);
        }
        let h = Le(&head);
        let flags = h.u32(4).unwrap_or(0);
        let streams = h.u32(8).unwrap_or(0);
        let header_len = h.u32(0xC).unwrap_or(0);
        if !(0x10..=4 << 20).contains(&header_len) || streams > 128 {
            return Err(Rpf9Error::BadHeader(format!(
                "ADAT header length {header_len}, streams {streams}"
            )));
        }
        let data = src.read_at(off, (header_len as usize).min(4 << 20))?;
        let d = Le(&data);
        let mut pos = 0x10usize;
        if flags & 0x0040_0000 != 0 {
            pos += 4;
        }
        if flags & 0x0001_0000 != 0 {
            pos += 2 * streams as usize;
        }
        if flags & 0x0020_0000 != 0 {
            pos += 4 * (streams as usize + 1);
        }
        pos += 4 * streams as usize;
        if pos >= data.len() {
            return Err(Rpf9Error::Truncated);
        }
        let mut chunks = Vec::new();
        while pos + 8 <= header_len as usize && pos + 8 <= data.len() {
            let v = d.u64(pos).unwrap_or(0);
            chunks.push(Chunk {
                kind: ((v >> 56) & 0xFF) as u8,
                size: ((v >> 28) & 0x0FFF_FFFF) as usize,
                offset: v & 0x0FFF_FFFF,
            });
            pos += 8;
        }
        let bank_size = chunks
            .iter()
            .map(|c| c.offset + c.size as u64)
            .max()
            .unwrap_or(0);
        if bank_size == 0 || off + bank_size > src.len() {
            return Err(Rpf9Error::BadHeader("ADAT bank size invalid".into()));
        }
        let mut format = None;
        for c in &chunks {
            if matches!(c.kind, 0x76 | 0xFA) && c.size >= 20 {
                let fmt = src.read_at(off + c.offset, c.size.min(64))?;
                if fmt.len() >= 20 {
                    format = Some(FormatRecord {
                        samples: Le(&fmt).u32(0).unwrap_or(0),
                        sample_rate: Le(&fmt).u16(8).unwrap_or(0),
                        codec: fmt[0x13],
                        channels: Le(&fmt).u16(0x12).unwrap_or(0),
                    });
                }
                break;
            }
        }
        Ok(Awc {
            flags,
            streams,
            header_len,
            chunks,
            format,
            bank_size,
        })
    }

    /// Opus payload chunk offsets (`0x55`).
    pub fn payload_chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.iter().filter(|c| c.kind == 0x55 && c.size > 0)
    }

    /// Split a RAGE Opus payload into packets.
    ///
    /// The payload is stored as 2 KiB blocks; inside each block packets are
    /// `u16` little-endian length prefixed.
    pub fn split_opus_packets<'a>(blob: &'a [u8]) -> Vec<&'a [u8]> {
        let mut packets = Vec::new();
        for block in blob.chunks(2048) {
            let mut p = 0usize;
            while p + 2 <= block.len() {
                let len = u16::from_le_bytes([block[p], block[p + 1]]) as usize;
                if len == 0 || p + 2 + len > block.len() {
                    break;
                }
                packets.push(&block[p + 2..p + 2 + len]);
                p += 2 + len;
            }
        }
        packets
    }

    /// Channel count from the Opus TOC byte of the first packet.
    pub fn channels(packets: &[&[u8]]) -> usize {
        packets
            .first()
            .map(|p| if p[0] & 0x04 != 0 { 2 } else { 1 })
            .unwrap_or(1)
    }

    /// True when the bank looks like a zlib stream (some banks are compressed).
    pub fn is_zlib(head: &[u8]) -> bool {
        is_zlib_head(head)
    }

    /// PCM decoding is delegated: return the packets so the caller can feed an
    /// Opus decoder without this crate taking an audio dependency.
    pub fn export_plan(&self) -> String {
        let fmt = self
            .format
            .map(|f| {
                format!(
                    "{} Hz, {} ch, codec 0x{:02x}, {} samples",
                    f.sample_rate, f.channels, f.codec, f.samples
                )
            })
            .unwrap_or_else(|| "no format record".into());
        format!(
            "{} streams, {} chunks, {} bytes, {}",
            self.streams,
            self.chunks.len(),
            self.bank_size,
            fmt
        )
    }
}
