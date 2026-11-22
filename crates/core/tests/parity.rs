//! Property and fixture tests.
//!
//! Fixture tests run only when the environment points at real game data, so
//! `cargo test` works on any machine:
//!
//! ```text
//! set SRPF9_COMMON=C:\path\to\common.rpf
//! set SRPF9_GRAPHICS=C:\path\to\ttdb_graphics.ttpf
//! set SRPF9_LEVELS=C:\path\to\levels.rpf
//! ```
//!
//! The asserted numbers are the ones recorded in the research report, so a
//! regression shows up as a failed assertion rather than a silently different
//! inventory.

use std::collections::HashSet;
use std::sync::Arc;

use sentinel_rpf9_core::bc;
use sentinel_rpf9_core::mhiu::Mhiu;
use sentinel_rpf9_core::psin;
use sentinel_rpf9_core::rpf9::{self, CarveOptions, Header, Kind};
use sentinel_rpf9_core::source::{open, BytesSource, Source};
use sentinel_rpf9_core::swizzle::{self, geom_4kb, geom_64kb};
use sentinel_rpf9_core::tpfd::Rds5;

fn env_src(var: &str) -> Option<Arc<dyn Source>> {
    let path = std::env::var(var).ok()?;
    if !std::path::Path::new(&path).exists() {
        return None;
    }
    open(path, 8 << 30).ok().map(Arc::from)
}

#[test]
fn xml_completeness_survives_bom_and_trailing_space() {
    assert!(rpf9::looks_complete_xml(b"<root><a/></root>"));
    assert!(rpf9::looks_complete_xml(b"\xEF\xBB\xBF<root/>\r\n"));
    assert!(!rpf9::looks_complete_xml(b"<root><a"));
    assert!(!rpf9::looks_complete_xml(b"\x00\x00<root"));
    assert!(!rpf9::looks_complete_xml(b""));
}

#[test]
fn header_roundtrip_on_synthetic_archive() {
    let mut buf = vec![0u8; 0x200];
    buf[..4].copy_from_slice(b"9FPR");
    buf[4..8].copy_from_slice(&0x180u32.to_le_bytes()); // header_len
    buf[8..12].copy_from_slice(&0x100u32.to_le_bytes()); // split
    buf[0x0C..0x10].copy_from_slice(&0x40u32.to_le_bytes()); // trailer
    buf[0x10..0x18].copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&0x0001_0006u32.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&7u32.to_le_bytes());
    buf[0x28..0x2C].copy_from_slice(&0xDA72_573Eu32.to_le_bytes());
    buf[0x2C..0x30].copy_from_slice(&0x0009_0F02u32.to_le_bytes());
    buf[0x40..0x60].copy_from_slice(&rpf9::TABLE_PREFIX);
    let h = Header::parse_at(&buf, 0).expect("valid header");
    assert_eq!(h.header_len, 0x180);
    assert_eq!(h.split, 0x100);
    assert_eq!(h.version_pair(), (1, 6));
    assert_eq!(h.table1_records(), 12);
    assert!(h.has_standard_prefix);
    assert_eq!(h.payload_range(0x1000), (0x180, 0xFC0));
    // A header with a non-RPF9 flag word must be rejected.
    buf[0x2C..0x30].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    assert!(Header::parse_at(&buf, 0).is_none());
}

#[test]
fn tile_permutations_are_bijections() {
    for bpe in [1usize, 2, 4, 8, 16] {
        for (geom, layout64) in [(geom_4kb(bpe), false), (geom_64kb(bpe), true)] {
            let count = geom.tile_w * geom.tile_h;
            let mut seen = HashSet::with_capacity(count);
            for ly in 0..geom.tile_h {
                for lx in 0..geom.tile_w {
                    let m = if layout64 {
                        swizzle::tile_index_64kb(bpe, lx, ly)
                    } else {
                        swizzle::tile_index_4kb(bpe, lx, ly)
                    };
                    assert!(m < count, "bpe {bpe} index {m} >= {count}");
                    assert!(seen.insert(m), "bpe {bpe} duplicate tile index {m}");
                }
            }
            assert_eq!(seen.len(), count);
        }
    }
}

#[test]
fn deswizzle_is_lossless_for_dense_surfaces() {
    // A 64x64 RGBA surface smeared with a position dependent pattern; after a
    // swizzle round trip the linear layout must come back byte identical.
    let (w, h, bpe) = (64usize, 64usize, 4usize);
    let geom = geom_4kb(bpe);
    let padded = swizzle::padded_size(w, h, false, geom);
    let mut src = vec![0u8; padded];
    for y in 0..h {
        for x in 0..w {
            for b in 0..bpe {
                src[(y * w + x) * bpe + b] = ((x * 7 + y * 13 + b * 29) & 0xFF) as u8;
            }
        }
    }
    // Swizzle: place each element at its tile slot.
    let mut swz = vec![0u8; padded];
    let tiles_x = w / geom.tile_w;
    for y in 0..h {
        for x in 0..w {
            let m = swizzle::tile_index_4kb(bpe, x % geom.tile_w, y % geom.tile_h);
            let tile = (y / geom.tile_h) * tiles_x + x / geom.tile_w;
            let d = ((tile << geom.shift) | m) * bpe;
            let s = (y * w + x) * bpe;
            swz[d..d + bpe].copy_from_slice(&src[s..s + bpe]);
        }
    }
    let back = swizzle::deswizzle(&swz, w, h, false, geom, false);
    assert_eq!(back, src);
}

#[test]
fn bc1_solid_block_decodes_to_one_colour() {
    // c0 == c1 == red, all indices 0 -> every texel red, opaque.
    let mut blk = [0u8; 8];
    blk[0..2].copy_from_slice(&0xF800u16.to_le_bytes());
    blk[2..4].copy_from_slice(&0xF800u16.to_le_bytes());
    let fmt = bc::format(71).unwrap();
    let rgba = bc::decode_surface(&blk, 4, 4, fmt).expect("BC1 decodes");
    assert_eq!(rgba.len(), 4 * 4 * 4);
    for px in rgba.chunks_exact(4) {
        assert_eq!(px, [255, 0, 0, 255]);
    }
}

#[test]
fn bc7_and_bc6h_blocks_decode() {
    // BC7 mode 0 is the single-bit prefix "1", so one bit set is a valid block;
    // all-zero endpoints and indices give opaque black.
    let bc7 = bc::format(98).unwrap();
    let mut blk = [0u8; 16];
    blk[0] = 0x01;
    let rgba = bc::decode_surface(&blk, 4, 4, bc7).expect("BC7 decodes");
    assert_eq!(rgba.len(), 4 * 4 * 4);
    for px in rgba.chunks_exact(4) {
        assert_eq!(&px[..3], &[0, 0, 0]);
        assert_eq!(px[3], 255);
    }

    // BC6H: mode 0, all-zero payload. Only the RGBA8 shape and opacity are
    // pinned; HDR endpoint values are the decoder's business.
    for dxgi in [95u8, 96] {
        let fmt = bc::format(dxgi).unwrap();
        let rgba = bc::decode_surface(&[0u8; 16], 4, 4, fmt).expect("BC6H decodes");
        assert_eq!(rgba.len(), 4 * 4 * 4);
        assert!(rgba.chunks_exact(4).all(|px| px[3] == 255));
    }

    // A BC6H mode-15 block (low five bits 0x0F) must not trap: it is the mode
    // whose endpoint mask overflows inside texture2ddecoder.
    let mut blk = [0u8; 16];
    blk[0] = 0x0F;
    let fmt = bc::format(95).unwrap();
    let rgba = bc::decode_surface(&blk, 4, 4, fmt).expect("BC6H mode 15 decodes");
    assert_eq!(rgba.len(), 4 * 4 * 4);
}

#[test]
fn dds_export_takes_mip0_from_the_allocation_tail() {
    // Synthetic database where the surface allocation is twice the padded size,
    // so mip 0 lives at the tail: virt 0x2000 instead of the descriptor's 0x1000.
    let mut buf = vec![0xAAu8; 0x3200];
    buf[..4].copy_from_slice(b"RDS5");
    buf[4..8].copy_from_slice(&2u32.to_le_bytes()); // entries
    buf[0x10..0x14].copy_from_slice(&1u32.to_le_bytes()); // header pages
    // entry 0: end 0x200, next codec RAW, no virtual end
    buf[0x18..0x1C].copy_from_slice(&0x0000_0002u32.to_le_bytes());
    buf[0x1C..0x20].copy_from_slice(&0u32.to_le_bytes());
    // entry 1: end 0x3200, next codec RAW, virtual end 0x3000
    let lo1: u32 = (3 << 30) | 0x32;
    buf[0x20..0x24].copy_from_slice(&lo1.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&0u32.to_le_bytes());
    // ETPF: two descriptors, both 4x4 BC1, so the first one's allocation hint is
    // the distance to the second surface page (0x2000).
    buf[0x100..0x104].copy_from_slice(b"ETPF");
    buf[0x104..0x108].copy_from_slice(&634u32.to_le_bytes());
    buf[0x108..0x10C].copy_from_slice(&2u32.to_le_bytes());
    buf[0x10C..0x110].copy_from_slice(&32u32.to_le_bytes());
    for (i, page) in [1u32, 3].iter().enumerate() {
        let u0: u32 = 3 | (3 << 14);
        let u1: u32 = 71 << 3 | (1 << 24); // dxgi 71, one mip
        let o = 0x110 + i * 16;
        buf[o..o + 4].copy_from_slice(&u0.to_le_bytes());
        buf[o + 4..o + 8].copy_from_slice(&u1.to_le_bytes());
        buf[o + 8..o + 12].copy_from_slice(&page.to_le_bytes());
    }
    // Mark the tail's first block distinctly from the 0xAA "head".
    for (k, b) in buf[0x2200..0x2208].iter_mut().enumerate() {
        *b = 1 + k as u8;
    }
    let src = BytesSource::new("synthetic.ttpf", buf);
    let r = Rds5::parse(&src).expect("parses");
    assert_eq!(r.descs.len(), 2);
    assert_eq!(r.descs[0].alloc_hint, 0x2000);

    let dds = r.dds(&src, 0).expect("dds export");
    assert_eq!(dds.len(), 148 + 8, "4x4 BC1 is one 8-byte block after the 148-byte header");
    assert_eq!(&dds[84..88], b"DX10");
    assert_eq!(
        &dds[148..],
        &[1, 2, 3, 4, 5, 6, 7, 8],
        "mip 0 must come from the allocation tail, not the surface pointer"
    );
}

#[test]
fn mhiu_rows_and_sentinel() {
    let mut buf = vec![0u8; 44 + 4 * 8 + 3];
    buf[..4].copy_from_slice(b"MHIU");
    buf[6..8].copy_from_slice(&4u16.to_le_bytes());
    buf[8..10].copy_from_slice(&4u16.to_le_bytes());
    buf[0x0C..0x10].copy_from_slice(&(-100.0f32).to_le_bytes());
    buf[0x10..0x14].copy_from_slice(&(-200.0f32).to_le_bytes());
    buf[0x14..0x18].copy_from_slice(&(0.0f32).to_le_bytes());
    buf[0x18..0x1C].copy_from_slice(&(100.0f32).to_le_bytes());
    buf[0x1C..0x20].copy_from_slice(&(200.0f32).to_le_bytes());
    buf[0x20..0x24].copy_from_slice(&(255.0f32).to_le_bytes());
    // row 1: start 1, count 2, offset 0
    let r1 = 44 + 8;
    buf[r1..r1 + 2].copy_from_slice(&1u16.to_le_bytes());
    buf[r1 + 2..r1 + 4].copy_from_slice(&2u16.to_le_bytes());
    buf[r1 + 4..r1 + 8].copy_from_slice(&(-1i32).to_le_bytes());
    buf[44 + 32..44 + 34].copy_from_slice(&[60, 250]);
    let m = Mhiu::parse(&buf).expect("parses");
    assert_eq!((m.width, m.height), (4, 4));
    assert_eq!(m.heights[0], 23, "missing rows keep the sea sentinel");
    assert_eq!(m.heights[4 + 1], 60);
    assert_eq!(m.rows_present, 1);
    let z = m.elevation_m(1, 1);
    assert!((0.0..=255.0).contains(&z));
}

#[test]
fn psin_sections_and_schema_chain() {
    // PSIN chunk (80 bytes) followed by LSCH with one struct of two fields.
    let mut psin_body = vec![0u8; 72];
    psin_body[..4].copy_from_slice(&0x1122_3344u32.to_le_bytes());
    psin_body[4..8].copy_from_slice(&32u32.to_le_bytes());
    for (i, rec) in [(0x1122_3344u32, 16u32), (0x5566_7788u32, 16u32)].iter().enumerate() {
        let o = i * 16;
        psin_body[o..o + 4].copy_from_slice(&rec.0.to_le_bytes());
        psin_body[o + 4..o + 8].copy_from_slice(&rec.1.to_le_bytes());
    }
    psin_body[32..40].copy_from_slice(b"pppppppp");
    let mut data = Vec::new();
    data.extend_from_slice(b"PSIN");
    data.extend_from_slice(&((8 + psin_body.len()) as u32).to_be_bytes());
    data.extend_from_slice(&psin_body);

    let mut lsch = Vec::new();
    lsch.extend_from_slice(&0x1122_3344u32.to_le_bytes());
    lsch.extend_from_slice(&0u32.to_le_bytes());
    lsch.extend_from_slice(&0u32.to_le_bytes());
    lsch.extend_from_slice(&0u32.to_le_bytes());
    lsch.extend_from_slice(&[0xAA; 8]);
    lsch.extend_from_slice(&16u32.to_le_bytes());
    lsch.extend_from_slice(&(2u32 << 16).to_le_bytes());
    lsch.extend_from_slice(&psin::joaat("Count").to_le_bytes());
    lsch.extend_from_slice(&0u32.to_le_bytes());
    lsch.extend_from_slice(&0x15u32.to_le_bytes());
    lsch.extend_from_slice(&0u32.to_le_bytes());
    lsch.extend_from_slice(&psin::joaat("Enabled").to_le_bytes());
    lsch.extend_from_slice(&4u32.to_le_bytes());
    lsch.extend_from_slice(&0x01u32.to_le_bytes());
    lsch.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(b"LSCH");
    data.extend_from_slice(&((8 + lsch.len()) as u32).to_be_bytes());
    data.extend_from_slice(&lsch);

    let p = psin::parse(&data);
    assert_eq!(p.sections.len(), 2);
    assert_eq!(p.sections[0].tag_str(), "PSIN");
    assert_eq!(p.structs.len(), 1);
    assert_eq!(p.structs[0].fields.len(), 2);
    assert_eq!(p.structs[0].fields[1].offset, 4);
    assert_eq!(psin::type_kind(0x01), "bool");
    assert_eq!(p.instances.len(), 2);
    assert_eq!(p.instances[1].offset, 16);
}

#[test]
fn rds5_chunk_table_uses_previous_entry_end() {
    // Synthetic database: one 0x100 page header, two chunks, ETPF at 0x100.
    let mut buf = vec![0u8; 0x400];
    buf[..4].copy_from_slice(b"RDS5");
    buf[4..8].copy_from_slice(&2u32.to_le_bytes()); // entries
    buf[0x10..0x14].copy_from_slice(&1u32.to_le_bytes()); // header pages
    // entry 0: end 0x200, codec 8 (zlib) for the next chunk, virtual end 0x1000
    let lo0: u32 = (8 << 26) | (1 << 30) | (0x200 >> 8);
    let hi0: u32 = 0; // vend 0
    buf[0x18..0x1C].copy_from_slice(&lo0.to_le_bytes());
    buf[0x1C..0x20].copy_from_slice(&hi0.to_le_bytes());
    // entry 1: end 0x300, codec 0, virtual end 0x2000
    let lo1: u32 = (3 << 30) | (0x300 >> 8);
    let hi1: u32 = 0x2000 / 0x4000;
    buf[0x20..0x24].copy_from_slice(&lo1.to_le_bytes());
    buf[0x24..0x28].copy_from_slice(&hi1.to_le_bytes());
    // ETPF header with one descriptor
    buf[0x100..0x104].copy_from_slice(b"ETPF");
    buf[0x104..0x108].copy_from_slice(&634u32.to_le_bytes());
    buf[0x108..0x10C].copy_from_slice(&1u32.to_le_bytes());
    buf[0x10C..0x110].copy_from_slice(&32u32.to_le_bytes());
    // descriptor: 4x4 RGBA, dxgi 28, no mips, page 1
    let u0: u32 = 3 | (3 << 14);
    let u1: u32 = 28 << 3;
    let u2: u32 = 1;
    buf[0x110..0x114].copy_from_slice(&u0.to_le_bytes());
    buf[0x114..0x118].copy_from_slice(&u1.to_le_bytes());
    buf[0x118..0x11C].copy_from_slice(&u2.to_le_bytes());
    let src = BytesSource::new("synthetic.ttpf", buf);
    let r = Rds5::parse(&src).expect("parses");
    assert_eq!(r.entries, 2);
    assert_eq!(r.etpf.a, 634);
    assert_eq!(r.descs.len(), 1);
    assert_eq!((r.descs[0].width, r.descs[0].height), (4, 4));
    assert_eq!(r.descs[0].dxgi, 28);
    assert_eq!(r.descs[0].virt_offset, 0x1000);
    assert_eq!(r.chunks.len(), 2);
    // chunk 0 starts at the ETPF page and ends at entry 0's end
    assert_eq!(r.chunks[0].file_start, 0x100);
    assert_eq!(r.chunks[0].file_end, 0x200);
    assert_eq!(r.chunks[1].file_start, 0x200);
    assert_eq!(r.chunks[1].virt_start, 0x1000);
    assert_eq!(r.chunks[1].virt_len, 0x2000);
}

#[test]
fn carve_classifies_payload_magics() {
    let payloads: &[(&[u8], Kind)] = &[
        (b"PSIN\x00\x00\x00\x00 padded", Kind::Psin),
        (b"ADAT\x00\x00\x00\x00 padded", Kind::Awc),
        (b"<?xml version=\"1.0\"?><root/>", Kind::Xml),
        (b"plain ascii text payload", Kind::Text),
    ];
    for (payload, expect) in payloads {
        assert_eq!(Kind::classify(payload), *expect, "payload {:?}", String::from_utf8_lossy(payload));
    }
}

#[test]
fn common_rpf_fixture() {
    let Some(src) = env_src("SRPF9_COMMON") else {
        return;
    };
    let h = rpf9::peek_header(&*src).expect("reads").expect("header");
    assert_eq!(h.header.header_len, 0x9580);
    assert_eq!(h.header.split, 0x8520);
    assert_eq!(h.header.trailer_len, 0x3A10);
    assert_eq!(h.header.version, 0x0001_0006);
    assert_eq!(h.header.count, 418);
    assert_eq!(h.header.key, 0xDA72_573E);
    assert_eq!(h.header.flags, 0x0009_0F02);
    assert!(h.header.has_standard_prefix);

    let streams = rpf9::carve(&*src, &CarveOptions::default()).expect("carves");
    assert_eq!(streams.len(), 954);
    assert_eq!(
        streams.iter().filter(|s| s.kind == Kind::Xml).count(),
        334
    );
    assert_eq!(streams.iter().filter(|s| s.is_complete_xml()).count(), 213);
    assert_eq!(streams.iter().filter(|s| s.kind == Kind::Bin).count(), 106);

    let map = sentinel_rpf9_core::mhiu::find_in_archive(&*src, 0x40).expect("mhiu");
    assert_eq!((map.width, map.height), (2048, 2048));
    assert!(map.bmin[2] < map.bmax[2]);
}

#[test]
fn graphics_ttpf_fixture() {
    let Some(src) = env_src("SRPF9_GRAPHICS") else {
        return;
    };
    let r = Rds5::parse(&*src).expect("parses");
    assert_eq!(r.entries, 1550);
    assert_eq!((r.etpf.a, r.etpf.b, r.etpf.c), (634, 912, 17040));
    assert_eq!(r.descs.len(), 912);
    assert_eq!(r.chunks.len(), 1549);
    assert_eq!(r.virtual_size(), 125_677_568);
    assert_eq!(r.header_pages, 49);
}

#[test]
fn levels_drawable_fixture() {
    let Some(src) = env_src("SRPF9_LEVELS") else {
        return;
    };
    // bk_02_road_str_slod dictionary: 8 drawables, 317 meshlet geometries.
    let m = sentinel_rpf9_core::mesh::decode(&*src, 8_679_358_480, "slod").expect("decodes");
    assert_eq!(m.parts.len(), 8);
    assert_eq!(m.geom_count, 317);
    let tower = m
        .parts
        .iter()
        .find(|p| p.name == "bk_04_slod.#dr")
        .expect("bk_04_slod part");
    let (lo, hi) = m.part_bounds(tower);
    let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    assert!((ext[0] - 224.2).abs() < 2.0, "x extent {ext:?}");
    assert!((ext[1] - 197.1).abs() < 2.0, "y extent {ext:?}");
    assert!((ext[2] - 207.7).abs() < 2.0, "z extent {ext:?}");
}
