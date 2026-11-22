//! zlib helpers shared by the RPF9 carver, the RAGE9 virtual-chunk assembler and
//! the RDS5/ETPF reader.
//!
//! All helpers report the exact number of input bytes consumed, because chunk
//! boundaries in every one of these formats are defined by "where the deflate
//! stream ended", not by a stored length.

use flate2::{Decompress, FlushDecompress, Status};

use crate::read::is_zlib_head;

/// Result of inflating one stream: bytes consumed from the input, and the
/// inflated payload.
pub struct Inflated {
    pub consumed: usize,
    pub data: Vec<u8>,
}

/// Inflate one complete zlib stream from the start of `src`.
///
/// Returns `None` when the stream is invalid, truncated, or grows past
/// `max_out`. A stream that fills `max_out` exactly is rejected so a
/// pathological input cannot allocate without bound.
pub fn inflate_stream(src: &[u8], max_out: usize) -> Option<Inflated> {
    let mut dec = Decompress::new(true);
    let mut data = Vec::with_capacity(64 * 1024);
    let mut buf = [0u8; 64 * 1024];
    let mut pos = 0usize;
    loop {
        let before_in = dec.total_in();
        let before_out = dec.total_out();
        let status = dec
            .decompress(&src[pos..], &mut buf, FlushDecompress::None)
            .ok()?;
        let used = (dec.total_in() - before_in) as usize;
        let made = (dec.total_out() - before_out) as usize;
        pos += used;
        if made > 0 {
            if data.len() + made > max_out {
                return None;
            }
            data.extend_from_slice(&buf[..made]);
        }
        match status {
            Status::StreamEnd => {
                return Some(Inflated {
                    consumed: pos,
                    data,
                })
            }
            _ => {
                if used == 0 && made == 0 {
                    return None;
                }
                if pos >= src.len() {
                    return None;
                }
            }
        }
    }
}

/// Inflate the first zlib stream found at or after `needle` in `src`.
pub fn inflate_after(src: &[u8], needle: usize, max_out: usize) -> Option<Inflated> {
    let mut at = needle;
    while at + 3 < src.len() {
        if is_zlib_head(&src[at..]) {
            if let Some(hit) = inflate_stream(&src[at..], max_out) {
                return Some(hit);
            }
        }
        at += 1;
    }
    None
}
