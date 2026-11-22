//! Block-compressed and linear pixel decoding to RGBA8.
//!
//! Implemented: BC1, BC2, BC3, BC4, BC5, BC6H, BC7, RGBA8/BGRA8, R8/A8,
//! R10G10B10A2, R16/R16G16/R16G16B16A16 (float and unorm), R32F.
//!
//! BC1..BC5 are decoded here; BC6H and BC7 are delegated to
//! `texture2ddecoder`, whose mode tables and per-partition weight tables are
//! the largest part of a BPTC decoder. Both paths return RGBA8 with the same
//! contract as the local block decoders.

use crate::swizzle::element_dims;

/// Texture format description from the DXGI enum stored in ETPF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub dxgi: u8,
    pub name: &'static str,
    pub is_bc: bool,
    pub bpe: usize,
}

const FORMATS: &[(u8, &str, bool, usize)] = &[
    (10, "R16G16B16A16_FLOAT", false, 8),
    (11, "R16G16B16A16_UNORM", false, 8),
    (24, "R10G10B10A2_UNORM", false, 4),
    (28, "R8G8B8A8_UNORM", false, 4),
    (29, "R8G8B8A8_UNORM_SRGB", false, 4),
    (34, "R16G16_FLOAT", false, 4),
    (41, "R32_FLOAT", false, 4),
    (49, "R8G8_UNORM", false, 2),
    (54, "R16_FLOAT", false, 2),
    (56, "R16_UNORM", false, 2),
    (61, "R8_UNORM", false, 1),
    (65, "A8_UNORM", false, 1),
    (71, "BC1_UNORM", true, 8),
    (72, "BC1_UNORM_SRGB", true, 8),
    (74, "BC2_UNORM", true, 16),
    (75, "BC2_UNORM_SRGB", true, 16),
    (77, "BC3_UNORM", true, 16),
    (78, "BC3_UNORM_SRGB", true, 16),
    (80, "BC4_UNORM", true, 8),
    (81, "BC4_SNORM", true, 8),
    (83, "BC5_UNORM", true, 16),
    (84, "BC5_SNORM", true, 16),
    (87, "B8G8R8A8_UNORM", false, 4),
    (91, "B8G8R8A8_UNORM_SRGB", false, 4),
    (95, "BC6H_UF16", true, 16),
    (96, "BC6H_SF16", true, 16),
    (98, "BC7_UNORM", true, 16),
    (99, "BC7_UNORM_SRGB", true, 16),
];

pub fn format(dxgi: u8) -> Option<Format> {
    FORMATS
        .iter()
        .find(|(d, _, _, _)| *d == dxgi)
        .map(|&(dxgi, name, is_bc, bpe)| Format {
            dxgi,
            name,
            is_bc,
            bpe,
        })
}

impl Format {
    pub fn is_srgb(&self) -> bool {
        matches!(self.dxgi, 29 | 72 | 75 | 78 | 91 | 99)
    }

    pub fn is_normal(&self) -> bool {
        matches!(self.dxgi, 80 | 81 | 83 | 84)
    }

    pub fn block_chain_len(&self, w: usize, h: usize) -> usize {
        let (ew, eh) = element_dims(w, h, self.is_bc);
        ew * eh * self.bpe
    }
}

#[inline]
fn c565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 0x1F) as u32;
    let g = ((v >> 5) & 0x3F) as u32;
    let b = (v & 0x1F) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

#[inline]
fn mix(a: [u8; 3], b: [u8; 3], num: u32, den: u32) -> [u8; 3] {
    [
        ((a[0] as u32 * (den - num) + b[0] as u32 * num) / den) as u8,
        ((a[1] as u32 * (den - num) + b[1] as u32 * num) / den) as u8,
        ((a[2] as u32 * (den - num) + b[2] as u32 * num) / den) as u8,
    ]
}

/// Decode one BC1 colour block into `out` (16 RGBA texels, block-major).
fn bc1_block(blk: &[u8], out: &mut [[u8; 4]; 16], force_four: bool) {
    let c0 = u16::from_le_bytes([blk[0], blk[1]]);
    let c1 = u16::from_le_bytes([blk[2], blk[3]]);
    let palette = {
        let a = c565(c0);
        let b = c565(c1);
        let mut p = [[0u8; 3]; 4];
        p[0] = a;
        p[1] = b;
        if c0 > c1 || force_four {
            p[2] = mix(a, b, 1, 3);
            p[3] = mix(a, b, 2, 3);
        } else {
            p[2] = mix(a, b, 1, 2);
            p[3] = [0, 0, 0];
        }
        p
    };
    let bits = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
    for i in 0..16 {
        let idx = ((bits >> (2 * i)) & 3) as usize;
        let p = palette[idx];
        out[i] = [p[0], p[1], p[2], 255];
    }
    if c0 <= c1 && !force_four {
        let bits = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
        for i in 0..16 {
            if (bits >> (2 * i)) & 3 == 3 {
                out[i][3] = 0;
            }
        }
    }
}

/// Decode one BC4 block (8 bytes) into 16 single-channel values.
fn bc4_block(blk: &[u8], out: &mut [u8; 16], signed: bool) {
    let r0 = blk[0];
    let r1 = blk[1];
    let mut table = [0u8; 8];
    table[0] = r0;
    table[1] = r1;
    let v = |x: u8| -> f32 {
        if signed {
            (x as i8) as f32
        } else {
            x as f32
        }
    };
    let to_u8 = |x: f32| -> u8 {
        if signed {
            (x.clamp(-127.0, 127.0) + 128.0) as u8
        } else {
            x.clamp(0.0, 255.0) as u8
        }
    };
    if r0 > r1 {
        for i in 1..7 {
            table[i + 1] = to_u8(v(r0) + (v(r1) - v(r0)) * i as f32 / 7.0);
        }
    } else {
        for i in 1..5 {
            table[i + 1] = to_u8(v(r0) + (v(r1) - v(r0)) * i as f32 / 5.0);
        }
        table[6] = 0;
        table[7] = if signed { 255 } else { 255 };
    }
    let mut bits: u64 = 0;
    for (i, b) in blk[2..8].iter().enumerate() {
        bits |= (*b as u64) << (8 * i);
    }
    for i in 0..16 {
        out[i] = table[((bits >> (3 * i)) & 7) as usize];
    }
}

/// Decode a surface to RGBA8. Returns `None` for formats this crate does not
/// decode instead of emitting plausible garbage.
pub fn decode_surface(lin: &[u8], w: usize, h: usize, fmt: Format) -> Option<Vec<u8>> {
    // BPTC decodes into its own RGBA buffer, so it never needs the scratch.
    match fmt.dxgi {
        95 | 96 => return Some(bptc_bc6h(lin, w, h, fmt.dxgi == 96)?),
        98 | 99 => return Some(bptc_bc7(lin, w, h)?),
        _ => {}
    }
    let mut out = vec![0u8; w * h * 4];
    match fmt.dxgi {
        71 | 72 => {
            let bw = (w + 3) / 4;
            let bh = (h + 3) / 4;
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * 8;
                    let blk = lin.get(off..off + 8)?;
                    let mut texels = [[0u8; 4]; 16];
                    bc1_block(blk, &mut texels, false);
                    blit(&mut out, w, h, bx, by, &texels);
                }
            }
            Some(out)
        }
        74 | 75 => {
            let bw = (w + 3) / 4;
            let bh = (h + 3) / 4;
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * 16;
                    let blk = lin.get(off..off + 16)?;
                    let mut texels = [[0u8; 4]; 16];
                    bc1_block(&blk[8..16], &mut texels, true);
                    let abits = u64::from_le_bytes(blk[0..8].try_into().ok()?);
                    for i in 0..16 {
                        let a = ((abits >> (4 * i)) & 0xF) as u8;
                        texels[i][3] = a * 17;
                    }
                    blit(&mut out, w, h, bx, by, &texels);
                }
            }
            Some(out)
        }
        77 | 78 => {
            let bw = (w + 3) / 4;
            let bh = (h + 3) / 4;
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * 16;
                    let blk = lin.get(off..off + 16)?;
                    let mut texels = [[0u8; 4]; 16];
                    bc1_block(&blk[8..16], &mut texels, true);
                    let mut alpha = [0u8; 16];
                    bc4_block(&blk[0..8], &mut alpha, false);
                    for i in 0..16 {
                        texels[i][3] = alpha[i];
                    }
                    blit(&mut out, w, h, bx, by, &texels);
                }
            }
            Some(out)
        }
        80 | 81 => {
            let signed = fmt.dxgi == 81;
            let bw = (w + 3) / 4;
            let bh = (h + 3) / 4;
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * 8;
                    let blk = lin.get(off..off + 8)?;
                    let mut v = [0u8; 16];
                    bc4_block(blk, &mut v, signed);
                    let mut texels = [[0u8; 4]; 16];
                    for i in 0..16 {
                        texels[i] = [v[i], v[i], v[i], 255];
                    }
                    blit(&mut out, w, h, bx, by, &texels);
                }
            }
            Some(out)
        }
        83 | 84 => {
            let signed = fmt.dxgi == 84;
            let bw = (w + 3) / 4;
            let bh = (h + 3) / 4;
            for by in 0..bh {
                for bx in 0..bw {
                    let off = (by * bw + bx) * 16;
                    let blk = lin.get(off..off + 16)?;
                    let mut r = [0u8; 16];
                    let mut g = [0u8; 16];
                    bc4_block(&blk[0..8], &mut r, signed);
                    bc4_block(&blk[8..16], &mut g, signed);
                    let mut texels = [[0u8; 4]; 16];
                    for i in 0..16 {
                        texels[i] = [r[i], g[i], 0, 255];
                    }
                    blit(&mut out, w, h, bx, by, &texels);
                }
            }
            Some(out)
        }
        28 | 29 | 87 | 91 => {
            let bgra = matches!(fmt.dxgi, 87 | 91);
            if lin.len() < w * h * 4 {
                return None;
            }
            for i in 0..w * h {
                let s = &lin[i * 4..i * 4 + 4];
                let px = if bgra {
                    [s[2], s[1], s[0], s[3]]
                } else {
                    [s[0], s[1], s[2], s[3]]
                };
                out[i * 4..i * 4 + 4].copy_from_slice(&px);
            }
            Some(out)
        }
        61 | 65 => {
            if lin.len() < w * h {
                return None;
            }
            for i in 0..w * h {
                let v = lin[i];
                out[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, 255]);
            }
            Some(out)
        }
        49 => {
            if lin.len() < w * h * 2 {
                return None;
            }
            for i in 0..w * h {
                out[i * 4..i * 4 + 4].copy_from_slice(&[lin[i * 2], lin[i * 2 + 1], 0, 255]);
            }
            Some(out)
        }
        24 => {
            if lin.len() < w * h * 4 {
                return None;
            }
            for i in 0..w * h {
                let v = u32::from_le_bytes([
                    lin[i * 4],
                    lin[i * 4 + 1],
                    lin[i * 4 + 2],
                    lin[i * 4 + 3],
                ]);
                let r = (v & 0x3FF) as f32 / 1023.0;
                let g = ((v >> 10) & 0x3FF) as f32 / 1023.0;
                let b = ((v >> 20) & 0x3FF) as f32 / 1023.0;
                let a = ((v >> 30) & 0x3) as f32 / 3.0;
                out[i * 4] = (r * 255.0) as u8;
                out[i * 4 + 1] = (g * 255.0) as u8;
                out[i * 4 + 2] = (b * 255.0) as u8;
                out[i * 4 + 3] = (a * 255.0) as u8;
            }
            Some(out)
        }
        41 => {
            if lin.len() < w * h * 4 {
                return None;
            }
            for i in 0..w * h {
                let v = f32::from_le_bytes([
                    lin[i * 4],
                    lin[i * 4 + 1],
                    lin[i * 4 + 2],
                    lin[i * 4 + 3],
                ]);
                let v = v.clamp(0.0, 1.0);
                out[i * 4..i * 4 + 4].copy_from_slice(&[
                    (v * 255.0) as u8,
                    (v * 255.0) as u8,
                    (v * 255.0) as u8,
                    255,
                ]);
            }
            Some(out)
        }
        54 | 56 => {
            if lin.len() < w * h * 2 {
                return None;
            }
            let unorm = fmt.dxgi == 56;
            for i in 0..w * h {
                let bits = u16::from_le_bytes([lin[i * 2], lin[i * 2 + 1]]);
                let v = if unorm {
                    bits as f32 / 65535.0
                } else {
                    half_to_f32(bits)
                };
                let c = (v.clamp(0.0, 1.0) * 255.0) as u8;
                out[i * 4..i * 4 + 4].copy_from_slice(&[c, c, c, 255]);
            }
            Some(out)
        }
        34 => {
            if lin.len() < w * h * 4 {
                return None;
            }
            for i in 0..w * h {
                let r = half_to_f32(u16::from_le_bytes([lin[i * 4], lin[i * 4 + 1]]));
                let g = half_to_f32(u16::from_le_bytes([lin[i * 4 + 2], lin[i * 4 + 3]]));
                out[i * 4..i * 4 + 4].copy_from_slice(&[
                    (r.clamp(0.0, 1.0) * 255.0) as u8,
                    (g.clamp(0.0, 1.0) * 255.0) as u8,
                    0,
                    255,
                ]);
            }
            Some(out)
        }
        10 | 11 => {
            if lin.len() < w * h * 8 {
                return None;
            }
            let unorm = fmt.dxgi == 11;
            for i in 0..w * h {
                let mut c = [0u8; 4];
                for k in 0..4 {
                    let o = i * 8 + k * 2;
                    let bits = u16::from_le_bytes([lin[o], lin[o + 1]]);
                    let v = if unorm {
                        bits as f32 / 65535.0
                    } else {
                        half_to_f32(bits)
                    };
                    c[k] = (v.clamp(0.0, 1.0) * 255.0) as u8;
                }
                out[i * 4..i * 4 + 4].copy_from_slice(&c);
            }
            Some(out)
        }
        _ => None,
    }
}

#[inline]
fn blit(out: &mut [u8], w: usize, h: usize, bx: usize, by: usize, texels: &[[u8; 4]; 16]) {
    for ty in 0..4 {
        let y = by * 4 + ty;
        if y >= h {
            continue;
        }
        for tx in 0..4 {
            let x = bx * 4 + tx;
            if x >= w {
                continue;
            }
            let d = (y * w + x) * 4;
            out[d..d + 4].copy_from_slice(&texels[ty * 4 + tx]);
        }
    }
}

/// Repack `texture2ddecoder`'s `0xAARRGGBB` (BGRA little-endian) words into
/// RGBA8 bytes.
fn bgra_words_to_rgba(px: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for &p in px {
        out.extend_from_slice(&[
            ((p >> 16) & 0xFF) as u8,
            ((p >> 8) & 0xFF) as u8,
            (p & 0xFF) as u8,
            ((p >> 24) & 0xFF) as u8,
        ]);
    }
    out
}

/// BC7 (DXGI 98/99). Mode table, partition table and the two-level weight
/// interpolation live in `texture2ddecoder`.
fn bptc_bc7(lin: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let mut words = vec![0u32; w * h];
    texture2ddecoder::decode_bc7(lin, w, h, &mut words).ok()?;
    Some(bgra_words_to_rgba(&words))
}

/// BC6H (DXGI 95/96).
///
/// `texture2ddecoder` 0.1.2 computes the transformed-endpoint mask as
/// `(1u16 << endpoint_bits) - 1`. Mode 15 is the one transformed mode with
/// `endpoint_bits == 16`, so that shift wraps and the mask becomes 1, which
/// destroys the endpoint deltas. Rewriting those blocks to the untransformed
/// mode 3 keeps every block decodable and every other mode bit-exact; the same
/// workaround is what PercyStar ships.
fn bptc_bc6h(lin: &[u8], w: usize, h: usize, signed: bool) -> Option<Vec<u8>> {
    let needs_rewrite = lin
        .chunks_exact(16)
        .any(|blk| (blk[0] & 0x1F) == 0x0F);
    let mut patched;
    let data = if needs_rewrite {
        patched = lin.to_vec();
        for blk in patched.chunks_exact_mut(16) {
            if (blk[0] & 0x1F) == 0x0F {
                blk[0] = (blk[0] & !0x1F) | 0x03;
            }
        }
        &patched[..]
    } else {
        lin
    };
    let mut words = vec![0u32; w * h];
    let res = if signed {
        texture2ddecoder::decode_bc6_signed(data, w, h, &mut words)
    } else {
        texture2ddecoder::decode_bc6_unsigned(data, w, h, &mut words)
    };
    res.ok()?;
    Some(bgra_words_to_rgba(&words))
}

/// IEEE 754 half to single precision.
#[inline]
pub fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1F) as u32;
    let mant = (bits & 0x3FF) as u32;
    if exp == 0 {
        if mant == 0 {
            return if sign != 0 { -0.0 } else { 0.0 };
        }
        let v = (mant as f32) * 2.0f32.powi(-24);
        return if sign != 0 { -v } else { v };
    }
    if exp == 31 {
        return 0.0; // inbound HDR data; clamp instead of producing inf/nan
    }
    f32::from_bits((sign << 31) | ((exp + 112) << 23) | (mant << 13))
}
