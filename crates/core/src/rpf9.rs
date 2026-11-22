//! RPF9 container: header, header census, zlib payload carver and the RAGE9
//! "virtual resource" page chain used by drawables, navmeshes and animations.
//!
//! # Header
//!
//! ```text
//! 0x00 u32 magic 'RPF9' (0x52504639, stored as the bytes 39 46 50 52)
//! 0x04 u32 header_len   payload starts here; both record tables are inside
//! 0x08 u32 split        end of record table 1
//! 0x0C u32 trailer_len  bytes at the end of the file
//! 0x10 u64 archive_id
//! 0x18 u32 0x47A00      constant
//! 0x1C u32 0x150        constant
//! 0x20 u32 version      high u16 = format revision, low u16 = build revision
//! 0x24 u32 count        declared resource (or resource + directory) count
//! 0x28 u32 key          build key; not used for payload decryption
//! 0x2C u32 flags        0x90F01 or 0x90F02
//! 0x30 u32 0x10         constant
//! 0x34 12 bytes         zero
//! 0x40 ...              record table 1 (enciphered in retail archives)
//! ```
//!
//! The two record tables are enciphered; the payload is not. Everything this
//! crate does therefore works from the plaintext fields plus a payload scan.

use std::borrow::Cow;
use std::fmt;

use crate::cache::ChunkCache;
use crate::read::{is_zlib_head, Le};
use crate::source::Source;
use crate::zlib::inflate_stream;

pub const MAGIC_RPF9: u32 = 0x5250_4639;
pub const MAGIC_RPF8: u32 = 0x5250_4638;
pub const MAGIC_RPF7: u32 = 0x5250_4637;
pub const MAGIC_BYTES: &[u8] = b"9FPR";

/// Constant 32-byte record-table prefix present at `0x40` in every RPF9 seen.
pub const TABLE_PREFIX: [u8; 32] = [
    0x5f, 0x08, 0xda, 0xab, 0xec, 0xbd, 0xd4, 0x02, 0x58, 0xc3, 0x51, 0x68, 0x07, 0x02, 0x4c,
    0x24, 0xfb, 0x9f, 0x75, 0x67, 0x55, 0xef, 0x4b, 0xe4, 0x58, 0x78, 0x5b, 0xfe, 0xd8, 0xaa,
    0x69, 0x89,
];

/// Virtual address window used by RAGE9 in-page pointers.
pub const VIRT_BASE: u64 = 0x5000_0000;
pub const VIRT_END: u64 = 0x5800_0000;
/// Physical (GPU buffer) window used by RAGE9 meshlet geometry.
pub const PHYS_BASE: u64 = 0x6000_0000;
pub const PHYS_END: u64 = 0x7000_0000;

#[derive(Debug)]
pub enum Rpf9Error {
    Io(std::io::Error),
    NotRpf9,
    Truncated,
    BadHeader(String),
    NoZlibStream(u64),
    Decompress(String),
    NoGeometry,
}

impl fmt::Display for Rpf9Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rpf9Error::Io(e) => write!(f, "io: {e}"),
            Rpf9Error::NotRpf9 => write!(f, "not an RPF9 archive (magic mismatch)"),
            Rpf9Error::Truncated => write!(f, "truncated input"),
            Rpf9Error::BadHeader(m) => write!(f, "bad header: {m}"),
            Rpf9Error::NoZlibStream(o) => write!(f, "no zlib stream at 0x{o:x}"),
            Rpf9Error::Decompress(m) => write!(f, "decompress: {m}"),
            Rpf9Error::NoGeometry => write!(f, "no meshlet geometry in resource"),
        }
    }
}

impl std::error::Error for Rpf9Error {}

impl From<std::io::Error> for Rpf9Error {
    fn from(e: std::io::Error) -> Self {
        Rpf9Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Rpf9Error>;

/// Parsed RPF9 header. `offset` is the file position of the magic, so nested
/// archives inside a container archive parse identically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub offset: u64,
    pub header_len: u32,
    pub split: u32,
    pub trailer_len: u32,
    pub archive_id: u64,
    pub version: u32,
    pub count: u32,
    pub key: u32,
    pub flags: u32,
    pub table1_bytes: u32,
    pub table2_bytes: u32,
    pub has_standard_prefix: bool,
}

impl Header {
    /// Parse at `off` in `data`. `data` must start at the magic.
    pub fn parse_at(data: &[u8], off: u64) -> Option<Header> {
        if data.len() < 0x40 || Le(data).u32(0)? != MAGIC_RPF9 {
            return None;
        }
        let h = Le(data);
        let header_len = h.u32(4)?;
        let split = h.u32(8)?;
        let trailer_len = h.u32(0xC)?;
        let count = h.u32(0x24)?;
        // Structural sanity: keeps random 9FPR bytes from being reported.
        if header_len < 0x40 || header_len > 0x10_0000 {
            return None;
        }
        if split < 0x40 || split > header_len {
            return None;
        }
        if count > 8_000_000 {
            return None;
        }
        let flags = h.u32(0x2C)?;
        if flags & 0xFFFF_0000 != 0x0009_0000 {
            return None;
        }
        Some(Header {
            offset: off,
            header_len,
            split,
            trailer_len,
            archive_id: h.u64(0x10)?,
            version: h.u32(0x20)?,
            count,
            key: h.u32(0x28)?,
            flags,
            table1_bytes: split - 0x40,
            table2_bytes: header_len - split,
            has_standard_prefix: data.len() >= 0x60 && data[0x40..0x60] == TABLE_PREFIX,
        })
    }

    pub fn version_pair(&self) -> (u16, u16) {
        ((self.version >> 16) as u16, self.version as u16)
    }

    pub fn table1_records(&self) -> u32 {
        self.table1_bytes / 16
    }

    pub fn table2_records(&self) -> u32 {
        self.table2_bytes / 16
    }

    /// Payload region, as `[start, end)` file offsets.
    pub fn payload_range(&self, file_len: u64) -> (u64, u64) {
        let start = self.offset + self.header_len as u64;
        let end = file_len.saturating_sub(self.trailer_len as u64).max(start);
        (start, end)
    }
}

/// One header found by [`scan_headers`].
#[derive(Clone, Debug)]
pub struct ScanHit {
    pub offset: u64,
    pub header: Header,
}

/// Find every RPF9 header in `src`, in file order.
///
/// Searches for the 4-byte marker with an 8-byte carry between blocks, then
/// validates candidates with [`Header::parse_at`]. Cheap enough to run over a
/// 76 GB archive: it is bounded by I/O, not by parsing.
pub fn scan_headers(src: &dyn Source, block: usize) -> Result<Vec<ScanHit>> {
    let block = block.max(1 << 20);
    let len = src.len();
    let mut hits = Vec::new();
    let mut base = 0u64;
    while base < len {
        let want = block.min((len - base) as usize);
        let data = src.read_at(base, want + 8)?;
        let mut i = 0usize;
        while i + 4 <= want {
            if &data[i..i + 4] == MAGIC_BYTES {
                if let Some(header) = Header::parse_at(&data[i..], base + i as u64) {
                    hits.push(ScanHit {
                        offset: base + i as u64,
                        header,
                    });
                    i += 0x1000;
                    continue;
                }
            }
            i += 1;
        }
        base += want as u64;
    }
    Ok(hits)
}

/// Payload class assigned by the carver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Psin,
    Drpz,
    Rbf0,
    Awc,
    Bink2,
    Rsc9,
    Tpfd,
    Xml,
    Text,
    Bin,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Psin => "psin",
            Kind::Drpz => "0drpz",
            Kind::Rbf0 => "rbf0",
            Kind::Awc => "awc",
            Kind::Bink2 => "bink2",
            Kind::Rsc9 => "rsc9",
            Kind::Tpfd => "ttpf",
            Kind::Xml => "xml",
            Kind::Text => "text",
            Kind::Bin => "bin",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Kind::Psin => ".psin",
            Kind::Drpz => ".0drpz",
            Kind::Rbf0 => ".rbf0",
            Kind::Awc => ".awc",
            Kind::Bink2 => ".bk2",
            Kind::Rsc9 => ".rsc9",
            Kind::Tpfd => ".ttpf",
            Kind::Xml => ".xml",
            Kind::Text => ".txt",
            Kind::Bin => ".bin",
        }
    }

    fn from_magic(data: &[u8]) -> Option<Kind> {
        let m = [
            (b"PSIN".as_slice(), Kind::Psin),
            (b"0DRPz".as_slice(), Kind::Drpz),
            (b"RBF0".as_slice(), Kind::Rbf0),
            (b"\xfb\xff\x01\x00".as_slice(), Kind::Rbf0),
            (b"ADAT".as_slice(), Kind::Awc),
            (b"KB2".as_slice(), Kind::Bink2),
            (b"RSC9".as_slice(), Kind::Rsc9),
            (b"RDS5".as_slice(), Kind::Tpfd),
        ];
        m.iter()
            .find(|(magic, _)| data.len() >= magic.len() && &data[..magic.len()] == *magic)
            .map(|(_, k)| *k)
    }

    /// Fallback classification for untyped zlib output.
    pub fn classify(data: &[u8]) -> Kind {
        if let Some(k) = Kind::from_magic(data) {
            return k;
        }
        let mut head = &data[..data.len().min(64)];
        if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
            head = &head[3..];
        }
        if head.starts_with(b"<?xml") || (head[..head.len().min(8)].contains(&b'<') && head.contains(&b'>')) {
            return Kind::Xml;
        }
        if std::str::from_utf8(data).is_err() {
            return Kind::Bin;
        }
        if data[..data.len().min(256)]
            .iter()
            .all(|&c| (32..127).contains(&c) || matches!(c, 9 | 10 | 13))
        {
            return Kind::Text;
        }
        Kind::Bin
    }
}

/// A carved payload stream.
pub struct Carved {
    pub offset: u64,
    pub comp_size: usize,
    pub raw_size: usize,
    pub kind: Kind,
    pub fingerprint: u64,
    /// A complete XML document: starts with `<`, ends with `>`.
    pub complete_xml: bool,
    /// Present when the carver was asked to retain the payload.
    pub data: Option<Vec<u8>>,
}

impl Carved {
    pub fn is_complete_xml(&self) -> bool {
        self.complete_xml
    }
}

/// True when `data` looks like a finished XML document rather than a 64 KiB
/// continuation page.
///
/// Mirrors the byte-level test: optional UTF-8 BOM, then the first non-blank
/// byte is `<` and the last non-blank byte is `>`.
pub fn looks_complete_xml(data: &[u8]) -> bool {
    let s = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    let blank = |b: &u8| matches!(*b, b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C);
    match (
        s.iter().position(|b| !blank(b)),
        s.iter().rposition(|b| !blank(b)),
    ) {
        (Some(a), Some(b)) if a <= b => s[a] == b'<' && s[b] == b'>',
        _ => false,
    }
}

/// Carver options.
#[derive(Clone, Copy, Debug)]
pub struct CarveOptions {
    /// Scan every N bytes (1 = every byte, 16 = payload alignment assumption).
    pub step: usize,
    /// Retain inflated payloads up to this many bytes.
    pub keep_below: usize,
    /// Abort inflating a single stream beyond this many bytes.
    pub max_stream: usize,
    /// Run candidate inflation on a rayon pool.
    pub parallel: bool,
}

impl Default for CarveOptions {
    fn default() -> Self {
        Self {
            step: 1,
            keep_below: 0,
            max_stream: 512 << 20,
            parallel: true,
        }
    }
}

/// 64-bit content fingerprint used to drop duplicate streams.
///
/// RPF9 payloads appear twice for a reason we have not identified (some
/// archives carry a copy of a stream in a different table), so de-duplication
/// is required for a stable inventory. FNV-1a plus the length is enough for the
/// tens of thousands of streams in a single archive.
#[inline]
pub fn fingerprint(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h ^ (data.len() as u64).rotate_left(32)
}

/// Carve every zlib stream in `src`.
///
/// This mirrors the payload carver used by the Python tooling: walk candidate
/// `78 xx` heads, inflate from each, classify the output, drop duplicates by
/// content. The stream census for `common.rpf` is the regression fixture
/// (`954` streams, `213` complete XML documents).
pub fn carve(src: &dyn Source, opts: &CarveOptions) -> Result<Vec<Carved>> {
    let start = match peek_header(src)? {
        Some(hit) => hit.header.header_len as u64,
        None => 0,
    };
    let len = src.len();

    // Candidate offsets per 32 MiB block, 8-byte carry so a head split across a
    // block boundary is still found.
    const BLOCK: usize = 32 << 20;
    const CARRY: usize = 8;
    let mut candidates: Vec<u64> = Vec::new();
    let mut base = start;
    let mut carry: Vec<u8> = Vec::new();
    while base < len {
        let want = BLOCK.min((len - base) as usize);
        let mut data = Vec::with_capacity(want + carry.len());
        data.extend_from_slice(&carry);
        data.extend_from_slice(&src.read_at(base, want)?);
        let chunk_start = base - carry.len() as u64;
        let mut i = 0usize;
        while i + 2 < data.len() {
            if data[i] == 0x78 && matches!(data[i + 1], 0x01 | 0x5e | 0x9c | 0xda) {
                let off = chunk_start + i as u64;
                if off >= start && (opts.step <= 1 || off % opts.step as u64 == 0) {
                    candidates.push(off);
                }
            }
            i += 1;
        }
        carry = data[data.len().saturating_sub(CARRY)..].to_vec();
        base += want as u64;
    }
    candidates.sort_unstable();
    candidates.dedup();

    let inflate_one = |off: u64| -> Option<Carved> {
        // Start small: false positives (a `78 9c` inside compressed data) die on
        // a few hundred bytes of input. Grow only for real, large streams.
        let mut window = 256usize << 10;
        loop {
            let data = src.window(off, window).ok()?;
            if data.is_empty() {
                return None;
            }
            if let Some(hit) = inflate_stream(&data, opts.max_stream) {
                if hit.data.is_empty() {
                    return None;
                }
                let kind = Kind::classify(&hit.data);
                let fingerprint = fingerprint(&hit.data);
                let complete_xml = kind == Kind::Xml && looks_complete_xml(&hit.data);
                let keep = opts.keep_below > 0 && hit.data.len() <= opts.keep_below;
                return Some(Carved {
                    offset: off,
                    comp_size: hit.consumed,
                    raw_size: hit.data.len(),
                    kind,
                    fingerprint,
                    complete_xml,
                    data: if keep { Some(hit.data) } else { None },
                });
            }
            if window >= (64 << 20) || window as u64 >= src.len().saturating_sub(off) {
                return None;
            }
            window *= 8;
        }
    };

    let mut out: Vec<Carved> = if opts.parallel {
        use rayon::prelude::*;
        candidates
            .par_iter()
            .filter_map(|&off| inflate_one(off))
            .collect()
    } else {
        candidates.iter().filter_map(|&off| inflate_one(off)).collect()
    };

    out.sort_by_key(|c| c.offset);
    let mut seen = std::collections::HashSet::with_capacity(out.len());
    out.retain(|c| seen.insert(c.fingerprint));
    Ok(out)
}

/// Read a header at offset 0 (top-level archive) if the file starts with RPF9.
pub fn peek_header(src: &dyn Source) -> Result<Option<ScanHit>> {
    let head = src.read_at(0, 0x100)?;
    Ok(Header::parse_at(&head, 0).map(|header| ScanHit { offset: 0, header }))
}

/// RAGE9 resource: the concatenated virtual page plus the physical GPU buffer.
pub struct VirtualResource {
    pub virt: Vec<u8>,
    pub phys: Vec<u8>,
    pub virtual_pages: usize,
    pub physical_pages: usize,
}

#[inline]
pub fn vptr(q: u64) -> Option<usize> {
    (VIRT_BASE..VIRT_END).contains(&q).then(|| (q - VIRT_BASE) as usize)
}

#[inline]
pub fn pptr(q: u64) -> Option<usize> {
    (PHYS_BASE..PHYS_END).contains(&q).then(|| (q - PHYS_BASE) as usize)
}

/// Inflate a stream starting at `off` by reading growing windows from `src`.
///
/// `None` means "not a complete stream inside the read budget", which is the
/// expected answer for the ~10% of resource pages that are not zlib.
pub fn inflate_at(src: &dyn Source, off: u64, max_out: usize) -> Option<crate::zlib::Inflated> {
    let mut window = 1usize << 20;
    loop {
        let avail = src.len().saturating_sub(off);
        if avail == 0 {
            return None;
        }
        let want = window.min(avail as usize);
        let data = src.read_at(off, want).ok()?;
        if let Some(hit) = inflate_stream(&data, max_out) {
            return Some(hit);
        }
        if want as u64 >= avail || window >= (256 << 20) {
            return None;
        }
        window *= 4;
    }
}

/// Find the next zlib head at or after `from` within `span` bytes.
fn next_zlib_in(src: &dyn Source, from: u64, span: usize) -> Option<u64> {
    let data = src.read_at(from, span).ok()?;
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if is_zlib_head(&data[i..]) {
            return Some(from + i as u64);
        }
        i += 1;
    }
    None
}

/// Assemble a RAGE9 virtual resource from its page chain.
///
/// Layout: the first zlib page starts at `off`. Its first 8 bytes are zero, and
/// the block map pointer at `+8` (`0x50xxxxxx`) gives the page-table offset.
/// The page table stores `[virtual_page_count, physical_page_count]` at `+8/+9`.
/// Physical (GPU buffer) pages follow the virtual pages in the stream.
pub fn read_virtual_resource(
    src: &dyn Source,
    off: u64,
    max_virt: usize,
    max_phys: usize,
) -> Result<VirtualResource> {
    let first = inflate_at(src, off, max_virt).ok_or(Rpf9Error::NoZlibStream(off))?;
    let mut virt = first.data;
    let mut next = off + first.consumed as u64;
    if virt.len() < 0x80 {
        return Err(Rpf9Error::Truncated);
    }
    let h = Le(&virt);
    // A page whose bytes at 0x60 look like the 0x1601 geometry tag is a
    // mid-resource geometry page, never a resource root.
    if h.u64(0x60) == Some(0x1601) {
        return Err(Rpf9Error::BadHeader(
            "mid-resource geometry page, not a resource root".into(),
        ));
    }
    if h.u64(0) != Some(0) {
        return Err(Rpf9Error::BadHeader("vtable word is not zero".into()));
    }
    let bmap_off = h
        .u64(8)
        .and_then(vptr)
        .ok_or_else(|| Rpf9Error::BadHeader("missing block map pointer".into()))?;
    if bmap_off > 4 << 20 {
        return Err(Rpf9Error::BadHeader("block map out of range".into()));
    }

    let mut pages = 1usize;
    while virt.len() <= bmap_off + 10 && pages < 32 {
        append_page(src, &mut virt, &mut next, max_virt)?;
        pages += 1;
    }
    let v_cnt;
    let p_cnt;
    {
        let h = Le(&virt);
        v_cnt = *virt.get(bmap_off + 8).ok_or(Rpf9Error::Truncated)? as usize;
        p_cnt = *virt.get(bmap_off + 9).ok_or(Rpf9Error::Truncated)? as usize;
        let _ = h;
    }
    // 22,628 placement resources declare zero physical pages and hold 97% of the
    // entity records, so zero is valid here.
    if !(1..=32).contains(&v_cnt) || p_cnt > 64 {
        return Err(Rpf9Error::BadHeader(format!(
            "block map counts v={v_cnt} p={p_cnt}"
        )));
    }
    while pages < v_cnt {
        append_page(src, &mut virt, &mut next, max_virt)?;
        pages += 1;
    }
    let mut phys = Vec::new();
    for _ in 0..p_cnt {
        let zoff = match next_zlib_in(src, next, 1 << 20) {
            Some(z) => z,
            None => break,
        };
        let hit = match inflate_at(src, zoff, max_phys) {
            Some(h) => h,
            None => break,
        };
        phys.extend_from_slice(&hit.data);
        next = zoff + hit.consumed as u64;
    }
    Ok(VirtualResource {
        virt,
        phys,
        virtual_pages: v_cnt,
        physical_pages: p_cnt,
    })
}

/// Append the next zlib page of a resource to `virt` and advance `next`.
fn append_page(
    src: &dyn Source,
    virt: &mut Vec<u8>,
    next: &mut u64,
    max_out: usize,
) -> Result<()> {
    let zoff = next_zlib_in(src, *next, 1 << 20).ok_or(Rpf9Error::NoZlibStream(*next))?;
    let hit = inflate_at(src, zoff, max_out).ok_or(Rpf9Error::NoZlibStream(zoff))?;
    virt.extend_from_slice(&hit.data);
    *next = zoff + hit.consumed as u64;
    Ok(())
}

/// Same as [`read_virtual_resource`] but with a chunk cache in front of the page
/// reads, for viewers that revisit the same resources.
pub fn read_virtual_cached(
    src: &dyn Source,
    cache: &ChunkCache,
    off: u64,
) -> Result<std::sync::Arc<VirtualResource>> {
    if let Some(hit) = cache.get_virtual(off) {
        return Ok(hit);
    }
    let res = std::sync::Arc::new(read_virtual_resource(src, off, 64 << 20, 64 << 20)?);
    cache.put_virtual(off, res.clone());
    Ok(res)
}

/// Convenience: read `len` bytes at `off` and return the borrowed slice when the
/// source is mapped.
pub fn slice<'a>(src: &'a dyn Source, off: u64, len: usize) -> Result<Cow<'a, [u8]>> {
    Ok(src.window(off, len)?)
}
