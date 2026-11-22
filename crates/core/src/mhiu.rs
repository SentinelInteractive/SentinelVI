//! MHIU — the world elevation raster stored as one or more zlib pages inside an
//! RPF9 archive.
//!
//! ```text
//! 0x00  'MHIU'
//! 0x06  u16 width             (2048 in the shipping map)
//! 0x08  u16 height            (2048)
//! 0x0C  f32 bmin.x  0x10 bmin.y  0x14 bmin.z
//! 0x18  f32 bmax.x  0x1C bmax.y  0x20 bmax.z
//! 0x2C  h * 8 byte row table: u16 start_x, u16 count, i32 pixel_offset
//! ....  u8 payload; a row copies `count` bytes from payload[start_x + offset]
//! ```
//!
//! Rows that are absent from the payload keep the sentinel value 23, which is
//! the sea level band. The raster is page-stitched: each zlib page inflates to
//! exactly 64 KiB except the last one.

use std::path::Path;

use flate2::{Decompress, FlushDecompress, Status};

use crate::read::{is_zlib_head, Le};
use crate::rpf9::{Result, Rpf9Error};
use crate::source::Source;

/// Sea-level / missing-row sentinel used by the encoder.
pub const SENTINEL: u8 = 23;

#[derive(Clone, Debug)]
pub struct Mhiu {
    pub width: usize,
    pub height: usize,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub heights: Vec<u8>,
    /// Rows present in the payload (diagnostics).
    pub rows_present: usize,
}

impl Mhiu {
    pub fn parse(raw: &[u8]) -> Option<Mhiu> {
        if raw.len() < 44 || &raw[..4] != b"MHIU" {
            return None;
        }
        let h = Le(raw);
        let width = h.u16(6)? as usize;
        let height = h.u16(8)? as usize;
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return None;
        }
        if raw.len() < 44 + height * 8 {
            return None;
        }
        let bmin = [h.f32(0x0C)?, h.f32(0x10)?, h.f32(0x14)?];
        let bmax = [h.f32(0x18)?, h.f32(0x1C)?, h.f32(0x20)?];
        let rows = &raw[44..44 + height * 8];
        let pix = &raw[44 + height * 8..];
        let mut heights = vec![SENTINEL; width * height];
        let mut rows_present = 0usize;
        for y in 0..height {
            let r = &rows[y * 8..y * 8 + 8];
            let sx = u16::from_le_bytes([r[0], r[1]]) as usize;
            let cnt = u16::from_le_bytes([r[2], r[3]]) as usize;
            let off = i32::from_le_bytes([r[4], r[5], r[6], r[7]]) as isize;
            if cnt == 0 || sx + cnt > width {
                continue;
            }
            let src = off + sx as isize;
            if src < 0 || (src as usize) + cnt > pix.len() {
                continue;
            }
            heights[y * width + sx..y * width + sx + cnt]
                .copy_from_slice(&pix[src as usize..src as usize + cnt]);
            rows_present += 1;
        }
        Some(Mhiu {
            width,
            height,
            bmin,
            bmax,
            heights,
            rows_present,
        })
    }

    /// Metres above the raster origin for one texel.
    pub fn elevation_m(&self, px: usize, py: usize) -> f32 {
        let v = self
            .heights
            .get(py * self.width + px)
            .copied()
            .unwrap_or(SENTINEL) as f32;
        self.bmin[2] + (v / 255.0) * (self.bmax[2] - self.bmin[2])
    }

    /// World position of a texel: x/y interpolate across the bounds, y is
    /// flipped because raster rows run north to south.
    pub fn world_xy(&self, px: usize, py: usize) -> (f32, f32) {
        let u = px as f32 / self.width as f32;
        let v = 1.0 - py as f32 / self.height as f32;
        (
            self.bmin[0] + u * (self.bmax[0] - self.bmin[0]),
            self.bmin[1] + v * (self.bmax[1] - self.bmin[1]),
        )
    }

    pub fn stats(&self) -> MhiuStats {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut land = 0usize;
        for y in 0..self.height {
            for x in 0..self.width {
                let z = self.elevation_m(x, y);
                lo = lo.min(z);
                hi = hi.max(z);
                if self.heights[y * self.width + x] > 26 {
                    land += 1;
                }
            }
        }
        MhiuStats {
            min_z: lo,
            max_z: hi,
            land_texels: land,
            texels: self.width * self.height,
        }
    }

    /// Colour ramp: sea, beach, lowland, highland, snow.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 4);
        for &v in &self.heights {
            let (r, g, b) = ramp(v);
            out.extend_from_slice(&[r, g, b, 255]);
        }
        out
    }

    /// Hillshaded relief, 8-bit grey, single channel.
    pub fn hillshade(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.width * self.height];
        for y in 0..self.height {
            let y1 = (y + 1).min(self.height - 1);
            for x in 0..self.width {
                let x1 = (x + 1).min(self.width - 1);
                let v = self.heights[y * self.width + x] as f32;
                let vx = self.heights[y * self.width + x1] as f32;
                let vy = self.heights[y1 * self.width + x] as f32;
                let shade = (0.85 + ((v - vx) + (v - vy)) * 0.04).clamp(0.45, 1.25);
                out[y * self.width + x] = (v * shade).clamp(0.0, 255.0) as u8;
            }
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MhiuStats {
    pub min_z: f32,
    pub max_z: f32,
    pub land_texels: usize,
    pub texels: usize,
}

fn ramp(v: u8) -> (u8, u8, u8) {
    if v <= SENTINEL {
        (16, 44, 92)
    } else if v <= 30 {
        let t = (v - SENTINEL) * 14;
        (45 + t, 115 + t, 72)
    } else if v <= 85 {
        let t = ((v - 30) as u16 * 120 / 55) as u8;
        (80 + t, 145 + t / 2, 70)
    } else {
        let t = ((v - 85) as u16 * 100 / 170) as u8;
        (155 + t, 145 + t, 130 + t)
    }
}

/// Inflate at most `cap` bytes; returns `(consumed, output)`.
fn inflate_prefix(src: &[u8], cap: usize) -> Option<(usize, Vec<u8>)> {
    let mut dec = Decompress::new(true);
    let mut out = Vec::with_capacity(cap.min(1 << 16));
    let mut buf = vec![0u8; cap.min(1 << 16)];
    let mut pos = 0usize;
    loop {
        let bi = dec.total_in();
        let bo = dec.total_out();
        let status = dec.decompress(&src[pos..], &mut buf, FlushDecompress::None).ok()?;
        let used = (dec.total_in() - bi) as usize;
        let made = (dec.total_out() - bo) as usize;
        pos += used;
        if made > 0 {
            out.extend_from_slice(&buf[..made]);
            if out.len() >= cap {
                out.truncate(cap);
                return Some((pos, out));
            }
        }
        match status {
            Status::StreamEnd => return Some((pos, out)),
            _ => {
                if (used == 0 && made == 0) || pos >= src.len() {
                    return None;
                }
            }
        }
    }
}

/// Stitch the MHIU pages that start inside `src`, beginning at `start`.
///
/// Page boundaries are implicit: each page inflates to exactly 64 KiB and the
/// next page begins at the next 16-byte aligned offset after the previous
/// stream ended.
pub fn stitch_pages(src: &dyn Source, start: u64, max_bytes: usize) -> Result<Vec<u8>> {
    let len = src.len();
    let mut cur = start;
    let mut out: Vec<u8> = Vec::with_capacity(2 << 20);
    loop {
        if cur >= len {
            break;
        }
        let want = (256 << 10).min((len - cur) as usize);
        let data = src.read_at(cur, want)?;
        if !is_zlib_head(&data) {
            break;
        }
        let Some((consumed, page)) = inflate_prefix(&data, 1 << 16) else {
            break;
        };
        if consumed == 0 || page.is_empty() {
            break;
        }
        out.extend_from_slice(&page);
        if out.len() > max_bytes {
            return Err(Rpf9Error::BadHeader("MHIU stream exceeds size cap".into()));
        }
        cur = (cur + consumed as u64 + 15) & !15;
        if page.len() != (1 << 16) {
            break;
        }
    }
    Ok(out)
}

/// Find and decode the first MHIU in an archive.
///
/// Scans 16-byte aligned offsets from `from` for a zlib head whose first page
/// spells `MHIU`, then stitches the remaining pages.
pub fn find_in_archive(src: &dyn Source, from: u64) -> Result<Mhiu> {
    let len = src.len();
    let mut at = from.max(0x40);
    // Walk the file in 256 KiB windows; the head test and the prefix inflate are
    // both cheap, and a hit stops the scan immediately.
    while at + 3 < len {
        let want = (256 << 10).min((len - at) as usize);
        let data = src.read_at(at, want)?;
        let mut i = 0usize;
        while i + 3 < data.len() {
            if is_zlib_head(&data[i..]) && at + i as u64 >= from {
                let abs = at + i as u64;
                if abs % 16 == 0 {
                    if let Some((_, page)) = inflate_prefix(&data[i..], 44) {
                        if page.len() >= 4 && &page[..4] == b"MHIU" {
                            let stitched = stitch_pages(src, abs, 8 << 20)?;
                            return Mhiu::parse(&stitched)
                                .ok_or_else(|| Rpf9Error::BadHeader("MHIU parse failed".into()));
                        }
                    }
                }
                i += 1;
                continue;
            }
            i += 1;
        }
        at += want.saturating_sub(3) as u64;
        if want == 0 {
            break;
        }
    }
    Err(Rpf9Error::BadHeader("no MHIU heightmap found".into()))
}

/// Convenience wrapper for a loose `.mhiu` file.
pub fn from_file(path: &Path) -> Result<Mhiu> {
    let data = std::fs::read(path)?;
    Mhiu::parse(&data).ok_or_else(|| Rpf9Error::BadHeader("MHIU parse failed".into()))
}
