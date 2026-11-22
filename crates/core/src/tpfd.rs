//! RDS5 / ETPF — the streaming texture database (`.ttpf`).
//!
//! ```text
//! 0x00 'RDS5'
//! 0x04 u32 chunk-table entries
//! 0x08 u32 flags a
//! 0x0C u32 flags b
//! 0x10 u32 header pages      -> ETPF block starts at pages * 0x100
//! 0x18 (entries * 8)         chunk table
//! .... zero padding to the next 0x100
//! ETPF descriptor block
//! .... chunk payloads
//! ```
//!
//! Each 8-byte table entry stores the **end** of one chunk plus the codec of the
//! *next* chunk:
//!
//! ```text
//! next codec  = (lo >> 26) & 0xF      0 RAW, 1..7 BCPack BC1..BC7, 8 ZLIB
//! file end    = (lo & 0x03FFFFFF) * 0x100
//! virtual end = (hi & 0x000FFFFF) * 0x4000 + ((lo >> 30) & 3) * 0x1000
//! ```
//!
//! Chunk `i` spans `[end(i-1), end(i))`, with chunk 0 starting at the 0x100
//! aligned end of the table. Chunk 0 is the ETPF block, not zlib.
//!
//! ETPF block:
//!
//! ```text
//! 0x00 'ETPF'
//! 0x04 u32 A   constant 634 in every complete database
//! 0x08 u32 B   descriptor record count
//! 0x0C u32 C   still unexplained (0x4290 for the graphics database)
//! 0x10 (B * 16) descriptors, see `Desc`
//! ```
//!
//! Descriptor:
//!
//! ```text
//! u0 bits  0..13  width - 1
//! u0 bits 14..27  height - 1
//! u1 bits  0..2   format byte (0x1c for BC7, 0x1d for BC3, ...)
//! u1 bits  3..9   DXGI_FORMAT
//! u1 bits 24..31  mip level count (0 = single surface)
//! u2 bits  0..19  virtual page index of the surface (page * 0x1000)
//! u3              nonzero on 5 records; unclassified
//! ```

use std::collections::HashMap;

use crate::bc::{decode_surface, format, Format};
use crate::read::{self, is_zlib_head, Le};
use crate::rpf9::{Result, Rpf9Error};
use crate::source::Source;
use crate::swizzle::{self, geom_4kb, geom_64kb, padded_size};
use crate::zlib::inflate_stream;

/// Codec identifiers used by the chunk table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Raw,
    BcPack(u8),
    Zlib,
}

impl Codec {
    fn from_bits(b: u8) -> Codec {
        match b {
            0 => Codec::Raw,
            1..=7 => Codec::BcPack(b),
            8 => Codec::Zlib,
            _ => Codec::Raw,
        }
    }

    pub fn name(self) -> String {
        match self {
            Codec::Raw => "RAW".into(),
            Codec::Zlib => "ZLIB".into(),
            Codec::BcPack(n) => format!("BCPack BC{n}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    pub file_start: u64,
    pub file_end: u64,
    pub virt_start: u64,
    pub virt_len: u64,
    pub codec: Codec,
}

#[derive(Clone, Copy, Debug)]
pub struct Desc {
    pub index: usize,
    pub width: usize,
    pub height: usize,
    pub format_byte: u8,
    pub dxgi: u8,
    pub mips: u8,
    /// Virtual byte offset of the surface (`page * 0x1000`).
    pub virt_offset: u64,
    /// Bytes between this surface and the next distinct surface page.
    pub alloc_hint: u64,
}

/// Parsed `.ttpf` database. Cheap to keep resident: the descriptor list and the
/// chunk table only.
#[derive(Clone, Debug)]
pub struct Rds5 {
    pub file_len: u64,
    pub entries: u32,
    pub flags: [u32; 2],
    pub header_pages: u32,
    pub etpf: EtpfHeader,
    pub descs: Vec<Desc>,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EtpfHeader {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl Rds5 {
    pub fn parse(src: &dyn Source) -> Result<Rds5> {
        let len = src.len();
        let head = src.read_at(0, 0x18)?;
        if head.len() < 0x18 || &head[..4] != b"RDS5" {
            return Err(Rpf9Error::NotRpf9);
        }
        let h = Le(&head);
        let entries = h.u32(4).unwrap_or(0);
        if entries == 0 || entries > 4_000_000 {
            return Err(Rpf9Error::BadHeader(format!("chunk entries {entries}")));
        }
        let flags = [h.u32(8).unwrap_or(0), h.u32(0xC).unwrap_or(0)];
        let header_pages = h.u32(0x10).unwrap_or(0);
        let etpf_off = header_pages as u64 * 0x100;
        let table = src.read_at(0x18, entries as usize * 8)?;
        let t = Le(&table);
        let mut chunks = Vec::with_capacity(entries as usize);
        let mut prev_end = etpf_off;
        let mut vstart = 0u64;
        let mut codec = Codec::Raw;
        for i in 0..entries as usize {
            let lo = t.u32(i * 8).unwrap_or(0);
            let hi = t.u32(i * 8 + 4).unwrap_or(0);
            let end = (lo & 0x03FF_FFFF) as u64 * 0x100;
            let next_codec = Codec::from_bits(((lo >> 26) & 0xF) as u8);
            let vend = (hi & 0x000F_FFFF) as u64 * 0x4000 + ((lo >> 30) as u64) * 0x1000;
            if end > prev_end && vend > vstart {
                chunks.push(Chunk {
                    file_start: prev_end,
                    file_end: end,
                    virt_start: vstart,
                    virt_len: vend - vstart,
                    codec,
                });
                vstart = vend;
            }
            prev_end = end;
            codec = next_codec;
        }

        // ETPF block sits at the header-page boundary.
        let ehdr = src.read_at(etpf_off, 0x10)?;
        if ehdr.len() < 0x10 || &ehdr[..4] != b"ETPF" {
            return Err(Rpf9Error::BadHeader(format!(
                "missing ETPF header at 0x{etpf_off:x}"
            )));
        }
        let eh = Le(&ehdr);
        let etpf = EtpfHeader {
            a: eh.u32(4).unwrap_or(0),
            b: eh.u32(8).unwrap_or(0),
            c: eh.u32(0xC).unwrap_or(0),
        };
        let count = (etpf.b as usize).min(1_000_000);
        let recs = src.read_at(etpf_off + 0x10, count * 16)?;
        let r = Le(&recs);
        let mut descs = Vec::with_capacity(count);
        for i in 0..count {
            let u0 = match r.u32(i * 16) {
                Some(v) => v,
                None => break,
            };
            let u1 = r.u32(i * 16 + 4).unwrap_or(0);
            let u2 = r.u32(i * 16 + 8).unwrap_or(0);
            let width = ((u0 & 0x3FFF) + 1) as usize;
            let height = (((u0 >> 14) & 0x3FFF) + 1) as usize;
            descs.push(Desc {
                index: i,
                width,
                height,
                format_byte: (u1 & 0xFF) as u8,
                dxgi: ((u1 >> 3) & 0x7F) as u8,
                mips: (u1 >> 24) as u8,
                virt_offset: (u2 & 0xFFFFF) as u64 * 0x1000,
                alloc_hint: 0,
            });
        }
        // Allocation hint: distance to the next distinct surface page.
        let mut pages: Vec<u64> = descs.iter().map(|d| d.virt_offset).collect();
        pages.sort_unstable();
        pages.dedup();
        let mut next: HashMap<u64, u64> = HashMap::with_capacity(pages.len());
        for w in pages.windows(2) {
            next.insert(w[0], w[1]);
        }
        for d in &mut descs {
            d.alloc_hint = next
                .get(&d.virt_offset)
                .map(|n| n - d.virt_offset)
                .unwrap_or(0);
        }

        Ok(Rds5 {
            file_len: len,
            entries,
            flags,
            header_pages,
            etpf,
            descs,
            chunks,
        })
    }

    pub fn codec_census(&self) -> [usize; 10] {
        let mut counts = [0usize; 10];
        for c in &self.chunks {
            let idx = match c.codec {
                Codec::Raw => 0,
                Codec::Zlib => 8,
                Codec::BcPack(n) => n as usize,
            };
            counts[idx.min(9)] += 1;
        }
        counts
    }

    /// Total virtual size (last chunk's virtual end).
    pub fn virtual_size(&self) -> u64 {
        self.chunks
            .last()
            .map(|c| c.virt_start + c.virt_len)
            .unwrap_or(0)
    }

    /// Read `len` bytes of the virtual stream at `off`, inflating only chunks
    /// that overlap the request.
    pub fn read_virtual(&self, src: &dyn Source, off: u64, len: usize) -> Result<Vec<u8>> {
        let end = off + len as u64;
        let mut out = vec![0u8; len];
        let first = self
            .chunks
            .partition_point(|c| c.virt_start + c.virt_len <= off);
        let mut covered = 0usize;
        for chunk in &self.chunks[first..] {
            if chunk.virt_start >= end {
                break;
            }
            let raw = src.read_at(chunk.file_start, (chunk.file_end - chunk.file_start) as usize)?;
            let data = match chunk.codec {
                Codec::Zlib if is_zlib_head(&raw) => {
                    match inflate_stream(&raw, (chunk.virt_len as usize).max(1 << 16) * 2) {
                        Some(hit) => hit.data,
                        None => continue,
                    }
                }
                _ if raw.len() as u64 == chunk.virt_len => raw,
                Codec::BcPack(n) => {
                    return Err(Rpf9Error::Decompress(format!(
                        "chunk uses Xbox BCPack BC{n}; ASIC-only, not decodable here"
                    )))
                }
                _ => continue,
            };
            let s0 = chunk.virt_start.max(off);
            let s1 = (chunk.virt_start + data.len() as u64).min(end);
            if s1 > s0 {
                out[(s0 - off) as usize..(s1 - off) as usize]
                    .copy_from_slice(&data[(s0 - chunk.virt_start) as usize..(s1 - chunk.virt_start) as usize]);
                covered += (s1 - s0) as usize;
            }
        }
        if covered == 0 {
            return Err(Rpf9Error::Decompress(
                "no decodable chunk covers the request (BCPack?)".into(),
            ));
        }
        Ok(out)
    }

    pub fn desc(&self, idx: usize) -> Option<&Desc> {
        self.descs.get(idx)
    }

    /// Virtual offset of mip 0 for `d`, given the surface's padded byte length.
    ///
    /// A mipmapped surface stores its chain smallest-first, so mip 0 sits at the
    /// *tail* of the allocation rather than at the surface pointer. The tail is
    /// recovered from the distance to the next surface page; when that distance
    /// is implausible, or the tail reads back as a uniform block (the usual sign
    /// of a wrong guess), the surface pointer itself is used.
    pub fn mip0_offset(&self, src: &dyn Source, d: &Desc, padded: usize) -> u64 {
        if d.mips == 0 {
            return d.virt_offset;
        }
        let alloc = d.alloc_hint;
        let plausible = alloc > padded as u64 && alloc <= (padded as u64) * 2 + 65_536;
        if !plausible {
            return d.virt_offset;
        }
        let tail = d.virt_offset + alloc - padded as u64;
        match self.read_virtual(src, tail, padded) {
            Ok(t) => {
                let uniform = t.iter().take(256).all(|&b| b == t[0]);
                if uniform {
                    d.virt_offset
                } else {
                    tail
                }
            }
            Err(_) => d.virt_offset,
        }
    }

    /// Decode one descriptor to RGBA8.
    ///
    /// Mip policy: a descriptor with `mips > 0` stores its chain smallest-first
    /// inside the allocation, so mip 0 sits at the tail. The tail position is
    /// recomputed from the distance to the next surface page; when that distance
    /// is implausible the surface pointer itself is used.
    pub fn decode(&self, src: &dyn Source, idx: usize) -> Result<DecodedTexture> {
        let d = self.desc(idx).ok_or(Rpf9Error::Truncated)?;
        let fmt = format(d.dxgi).ok_or_else(|| {
            Rpf9Error::BadHeader(format!(
                "unsupported DXGI {} (format byte 0x{:02x})",
                d.dxgi, d.format_byte
            ))
        })?;
        let mip0_lin = fmt.block_chain_len(d.width, d.height);
        // Mipmapped BC surfaces use 4 KB tiles at every level; single-surface or
        // uncompressed surfaces above 64 KB use the 64 KB streaming tiles.
        let use_64k = (d.mips == 0 || !fmt.is_bc) && mip0_lin > 65_536;
        let (geom, layout64) = if use_64k {
            (geom_64kb(fmt.bpe), true)
        } else {
            (geom_4kb(fmt.bpe), false)
        };
        let padded = padded_size(d.width, d.height, fmt.is_bc, geom);

        let surface_off = self.mip0_offset(src, d, padded);
        let raw = self.read_virtual(src, surface_off, padded)?;
        let lin = swizzle::deswizzle(&raw, d.width, d.height, fmt.is_bc, geom, layout64);
        let rgba = decode_surface(&lin, d.width, d.height, fmt).ok_or_else(|| {
            Rpf9Error::BadHeader(format!(
                "{} (DXGI {}) is not decoded by this build",
                fmt.name, fmt.dxgi
            ))
        })?;
        Ok(DecodedTexture {
            name: format!(
                "tex_{:05}_{}x{}_{}",
                d.index, d.width, d.height, fmt.name
            ),
            width: d.width,
            height: d.height,
            dxgi: d.dxgi,
            format_name: fmt.name,
            mips: d.mips,
            surface_offset: surface_off,
            used_64kb_tiles: use_64k,
            packed_len: lin.len(),
            linear: lin,
            rgba,
        })
    }

    /// Export the surface as a DDS with a DX10 header, for tools that can take
    /// the raw swizzled-away blocks but not the block decode.
    pub fn dds(&self, src: &dyn Source, idx: usize) -> Result<Vec<u8>> {
        let d = self.desc(idx).ok_or(Rpf9Error::Truncated)?;
        let fmt = format(d.dxgi)
            .ok_or_else(|| Rpf9Error::BadHeader(format!("unsupported DXGI {}", d.dxgi)))?;
        let mip0_lin = fmt.block_chain_len(d.width, d.height);
        let use_64k = (d.mips == 0 || !fmt.is_bc) && mip0_lin > 65_536;
        let geom = if use_64k { geom_64kb(fmt.bpe) } else { geom_4kb(fmt.bpe) };
        let padded = padded_size(d.width, d.height, fmt.is_bc, geom);
        // Same mip-0 placement rule as `decode()`: exporting from the surface
        // pointer on a mipmapped descriptor writes the smallest mip's bytes
        // under a mip-0 header.
        let surface_off = self.mip0_offset(src, d, padded);
        let raw = self.read_virtual(src, surface_off, padded)?;
        let lin = swizzle::deswizzle(&raw, d.width, d.height, fmt.is_bc, geom, use_64k);
        Ok(build_dds_dx10(d.width as u32, d.height as u32, d.dxgi, &lin))
    }
}

#[derive(Clone, Debug)]
pub struct DecodedTexture {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub dxgi: u8,
    pub format_name: &'static str,
    pub mips: u8,
    pub surface_offset: u64,
    pub used_64kb_tiles: bool,
    /// Tightly packed element data after deswizzle (blocks for BC formats).
    pub packed_len: usize,
    pub linear: Vec<u8>,
    pub rgba: Vec<u8>,
}

fn build_dds_dx10(w: u32, h: u32, dxgi: u8, data: &[u8]) -> Vec<u8> {
    // 4 magic + 124 header + 20 DX10 header = 148 bytes before the payload.
    let mut out = Vec::with_capacity(148 + data.len());
    out.extend_from_slice(b"DDS ");
    out.extend_from_slice(&124u32.to_le_bytes()); // header size
    let flags: u32 = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000; // caps|height|width|pixel format|linear size
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // depth
    out.extend_from_slice(&1u32.to_le_bytes()); // mip count
    out.extend_from_slice(&[0u8; 44]); // reserved1[11]
    out.extend_from_slice(&32u32.to_le_bytes()); // pixel format size
    out.extend_from_slice(&4u32.to_le_bytes()); // DDPF_FOURCC
    out.extend_from_slice(b"DX10");
    out.extend_from_slice(&[0u8; 20]); // bit count + R/G/B/A masks
    // dwCaps, dwCaps2..4, dwReserved2. The DX10 struct has to start at 0x80,
    // so these 16 bytes cannot be skipped.
    out.extend_from_slice(&0x1000u32.to_le_bytes()); // DDSCAPS_TEXTURE
    out.extend_from_slice(&[0u8; 16]); // caps2, caps3, caps4, reserved2
    out.extend_from_slice(&(dxgi as u32).to_le_bytes()); // DXGI_FORMAT
    out.extend_from_slice(&3u32.to_le_bytes()); // TEXTURE2D
    out.extend_from_slice(&0u32.to_le_bytes()); // misc
    out.extend_from_slice(&1u32.to_le_bytes()); // array size
    out.extend_from_slice(&0u32.to_le_bytes()); // misc2
    out.extend_from_slice(data);
    out
}

/// Read the ETPF descriptor count without materialising the chunk table.
pub fn probe_header(src: &dyn Source) -> Result<(u32, EtpfHeader)> {
    let head = src.read_at(0, 0x18)?;
    if head.len() < 0x18 || &head[..4] != b"RDS5" {
        return Err(Rpf9Error::NotRpf9);
    }
    let h = Le(&head);
    let pages = h.u32(0x10).unwrap_or(0);
    let ehdr = src.read_at(pages as u64 * 0x100, 0x10)?;
    if ehdr.len() < 0x10 || &ehdr[..4] != b"ETPF" {
        return Err(Rpf9Error::BadHeader("missing ETPF".into()));
    }
    let eh = read::Le(&ehdr);
    Ok((
        h.u32(4).unwrap_or(0),
        EtpfHeader {
            a: eh.u32(4).unwrap_or(0),
            b: eh.u32(8).unwrap_or(0),
            c: eh.u32(0xC).unwrap_or(0),
        },
    ))
}

/// DXGI helpers re-exported for callers that only need names/block sizes.
pub fn format_by_dxgi(dxgi: u8) -> Option<Format> {
    format(dxgi)
}
