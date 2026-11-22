//! Tiled-texture deswizzle for the Xbox Series (RDNA2 "SW_4KB_D") and the
//! streaming texture database ("64 KB") tile layouts.
//!
//! A surface is stored as a grid of pixel tiles, and inside a tile the elements
//! are permuted by a fixed bit rotation of the local coordinates. Both layouts
//! share the same structure; they differ in tile size, which is a function of
//! the element size:
//!
//! | element | 4 KB tile | 64 KB tile | tile shift |
//! |--------:|----------:|-----------:|-----------:|
//! | 16 B    |   16x16   |    64x64   | 12         |
//! |  8 B    |   32x16   |   128x64   | 13         |
//! |  4 B    |   32x32   |  128x128   | 14         |
//! |  2 B    |   64x32   |   256x128  | 15         |
//! |  1 B    |   64x64   |   256x256  | 16         |
//!
//! Element counts are block counts for the BC formats (`ceil(dim/4)`), so a
//! 16-byte BC7 block behaves like a 16-byte pixel.

/// Element size in bytes, tile dimensions, and the tile shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileGeom {
    pub tile_w: usize,
    pub tile_h: usize,
    pub shift: u8,
    pub bpe: usize,
}

/// 4 KB tile geometry for an element size.
///
/// A 4 KB tile holds `4096 / bpe` elements, so the tile shift is
/// `12 - log2(bpe)` and the tile is square-ish: 16 bytes -> 16x16, 1 byte ->
/// 64x64.
pub fn geom_4kb(bpe: usize) -> TileGeom {
    let (tile_w, tile_h, shift) = match bpe {
        16 => (16, 16, 8u8),
        8 => (32, 16, 9),
        4 => (32, 32, 10),
        2 => (64, 32, 11),
        _ => (64, 64, 12),
    };
    TileGeom { tile_w, tile_h, shift, bpe }
}

/// 64 KB tile geometry for an element size (`16 - log2(bpe)` elements per tile).
pub fn geom_64kb(bpe: usize) -> TileGeom {
    let (tile_w, tile_h, shift) = match bpe {
        16 => (64, 64, 12u8),
        8 => (128, 64, 13),
        4 => (128, 128, 14),
        2 => (256, 128, 15),
        _ => (256, 256, 16),
    };
    TileGeom { tile_w, tile_h, shift, bpe }
}

/// Linear element count for a surface of `w` x `h` pixels (`is_bc` = block
/// compressed, so dimensions are divided by four before counting elements).
pub fn element_dims(w: usize, h: usize, is_bc: bool) -> (usize, usize) {
    let ew = if is_bc { (w + 3) / 4 } else { w }.max(1);
    let eh = if is_bc { (h + 3) / 4 } else { h }.max(1);
    (ew, eh)
}

/// Padded byte size when the surface is stored with `geom` tiles.
pub fn padded_size(w: usize, h: usize, is_bc: bool, geom: TileGeom) -> usize {
    let (ew, eh) = element_dims(w, h, is_bc);
    let pw = (ew + geom.tile_w - 1) / geom.tile_w * geom.tile_w;
    let ph = (eh + geom.tile_h - 1) / geom.tile_h * geom.tile_h;
    pw * ph * geom.bpe
}

/// Element index inside a tile for the 4 KB layout.
///
/// The permutation is a fixed rotation of the local coordinate bits; the order
/// differs per element size because the tile aspect ratio differs.
#[inline]
#[allow(unused_variables)]
pub fn tile_index_4kb(bpe: usize, lx: usize, ly: usize) -> usize {
    let (x0, y0, x1, y1, x2, y2, x3, y3, x4, y4) = (
        lx & 1,
        ly & 1,
        (lx >> 1) & 1,
        (ly >> 1) & 1,
        (lx >> 2) & 1,
        (ly >> 2) & 1,
        (lx >> 3) & 1,
        (ly >> 3) & 1,
        (lx >> 4) & 1,
        (ly >> 4) & 1,
    );
    match bpe {
        16 => (ly & 3) | ((lx & 3) << 2) | (y2 << 4) | (x2 << 5) | (y3 << 6) | (x3 << 7),
        8 => (lx & 1) | ((ly & 3) << 1) | (((lx >> 1) & 3) << 3) | (y2 << 5) | (x3 << 6) | (y3 << 7) | (x4 << 8),
        4 => (lx & 3) | ((ly & 3) << 2) | (x2 << 4) | (y2 << 5) | (y3 << 6) | (x3 << 7) | (y4 << 8) | (x4 << 9),
        2 => (lx & 7) | ((ly & 7) << 3) | (x3 << 6) | (y3 << 7) | (x4 << 8) | (y4 << 9) | ((lx >> 5) & 1) << 10,
        _ => {
            (ly & 7)
                | (x1 << 3)
                | (x0 << 4)
                | (x2 << 5)
                | (y3 << 6)
                | (x3 << 7)
                | (x4 << 8)
                | (y4 << 9)
                | ((lx >> 5) & 1) << 10
                | ((ly >> 5) & 1) << 11
        }
    }
}

/// Element index inside a tile for the 64 KB streaming layout.
#[inline]
pub fn tile_index_64kb(bpe: usize, lx: usize, ly: usize) -> usize {
    let x0 = lx & 1;
    let y0 = ly & 1;
    let x1 = (lx >> 1) & 1;
    let y1 = (ly >> 1) & 1;
    let y2 = (ly >> 2) & 1;
    let x2 = (lx >> 2) & 1;
    let y3 = (ly >> 3) & 1;
    let x3 = (lx >> 3) & 1;
    let y4 = (ly >> 4) & 1;
    let x4 = (lx >> 4) & 1;
    let y5 = (ly >> 5) & 1;
    let x5 = (lx >> 5) & 1;
    let x6 = (lx >> 6) & 1;
    let y6 = (ly >> 6) & 1;
    let y7 = (ly >> 7) & 1;
    let x7 = (lx >> 7) & 1;
    match bpe {
        16 => x0 | (y0 << 1) | (x1 << 2) | (y1 << 3) | (y2 << 4) | (x2 << 5) | (y3 << 6) | (x3 << 7) | (y4 << 8) | (x4 << 9) | (y5 << 10) | (x5 << 11),
        8 => x0 | (y0 << 1) | (((lx >> 1) & 3) << 2) | (y1 << 4) | (y2 << 5) | (x3 << 6) | (y3 << 7) | (x4 << 8) | (y4 << 9) | (x5 << 10) | (y5 << 11) | (x6 << 12),
        4 => (lx & 3) | ((ly & 3) << 2) | (x2 << 4) | (y2 << 5) | (y3 << 6) | (x3 << 7) | (y4 << 8) | (x4 << 9) | (y5 << 10) | (x5 << 11) | (y6 << 12) | (x6 << 13),
        2 => (lx & 7) | ((ly & 7) << 3) | (x3 << 6) | (y3 << 7) | (x4 << 8) | (y4 << 9) | (x5 << 10) | (y5 << 11) | (x6 << 12) | (y6 << 13) | (x7 << 14),
        _ => (ly & 7) | (x1 << 3) | (x0 << 4) | (x2 << 5) | (y3 << 6) | (x3 << 7) | (x4 << 8) | (y4 << 9) | (x5 << 10) | (y5 << 11) | (x6 << 12) | (y6 << 13) | (x7 << 14) | (y7 << 15),
    }
}

/// Deswizzle one surface into tightly packed element order.
///
/// `raw` may be shorter than the padded size (truncated archives); missing
/// elements read as zero.
pub fn deswizzle(
    raw: &[u8],
    w: usize,
    h: usize,
    is_bc: bool,
    geom: TileGeom,
    layout64: bool,
) -> Vec<u8> {
    let (ew, eh) = element_dims(w, h, is_bc);
    let needed = padded_size(w, h, is_bc, geom);
    let mut src_owned = Vec::new();
    let src: &[u8] = if raw.len() >= needed {
        raw
    } else {
        src_owned.resize(needed, 0);
        let n = raw.len().min(needed);
        src_owned[..n].copy_from_slice(&raw[..n]);
        &src_owned
    };
    let mut out = vec![0u8; ew * eh * geom.bpe];
    let tiles_x = ((ew + geom.tile_w - 1) / geom.tile_w).max(1);
    let bpe = geom.bpe;
    for y in 0..eh {
        let ty = y / geom.tile_h;
        let ly = y % geom.tile_h;
        for x in 0..ew {
            let tx = x / geom.tile_w;
            let lx = x % geom.tile_w;
            let m = if layout64 {
                tile_index_64kb(bpe, lx, ly)
            } else {
                tile_index_4kb(bpe, lx, ly)
            };
            let tile = ty * tiles_x + tx;
            let s = ((tile << geom.shift) | m) * bpe;
            let d = (y * ew + x) * bpe;
            out[d..d + bpe].copy_from_slice(&src[s..s + bpe]);
        }
    }
    out
}
