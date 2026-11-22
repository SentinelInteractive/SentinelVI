//! PNG (RGBA8) and PPM writers. No image crate, no external codec: a PNG with
//! per-row filter type 0 only needs zlib, which the crate already links.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use flate2::write::ZlibEncoder;
use flate2::Compression;

const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0usize;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn write_chunk(w: &mut impl Write, kind: &[u8], body: &[u8]) -> io::Result<()> {
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    let mut head = Vec::with_capacity(4 + body.len());
    head.extend_from_slice(kind);
    head.extend_from_slice(body);
    w.write_all(&head)?;
    w.write_all(&crc32(&head).to_be_bytes())
}

/// Write straight RGBA8 (row-major, top-down) as PNG.
pub fn write_png_rgba(path: &Path, w: usize, h: usize, rgba: &[u8]) -> io::Result<()> {
    let mut f = BufWriter::new(File::create(path)?);
    f.write_all(&png_rgba(w, h, rgba))?;
    f.flush()
}

/// Encode straight RGBA8 (row-major, top-down) as PNG bytes.
pub fn png_rgba(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), w * h * 4, "rgba buffer does not match dimensions");
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0u8); // filter: none
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(&raw).expect("zlib to memory");
    let stream = enc.finish().expect("zlib to memory");

    let mut f: Vec<u8> = Vec::with_capacity(stream.len() + 64);
    f.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        .expect("write to memory");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, no filter, no interlace
    write_chunk(&mut f, b"IHDR", &ihdr).expect("write to memory");
    write_chunk(&mut f, b"IDAT", &stream).expect("write to memory");
    write_chunk(&mut f, b"IEND", &[]).expect("write to memory");
    f
}

/// Widen a single-channel buffer to RGBA8.
pub fn gray_to_rgba(gray: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(gray.len() * 4);
    for &v in gray {
        out.extend_from_slice(&[v, v, v, 255]);
    }
    out
}
