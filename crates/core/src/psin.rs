//! PSIN — the tagged-section metadata container (serialised PSO).
//!
//! ```text
//! 0x00 'PSIN'
//! 0x04 u32 flags
//! 0x08 u32 schema hash
//! 0x0C u32 field
//! 0x10 ...        data area
//! ....            tagged sections
//! ```
//!
//! Sections are `tag[4] + big-endian u32 size` (size includes the 8-byte
//! header), and the chain runs to the end of the file. The interesting sections
//! for reverse engineering are:
//!
//! * `LSCH` — struct schemas: a 32-byte header per struct, marker `AA` x8 at
//!   `+0x10`, field count in the high half of the u32 at `+0x18`, then 16-byte
//!   field records `name_hash, offset, type, ref_hash`;
//! * `LENM` — enum value names: 24-byte header per enum, marker `EE` x8 at
//!   `+0x08`, count at `+0x10`, then 16-byte `(value: i64, name_hash: u32)`;
//! * `LCNS`/`PSIN` body — instance table: 16-byte `(type_hash, size)` records up
//!   to the `pppppppp` sentinel, then payload blocks aligned to 16 bytes.

use std::collections::HashMap;

use crate::read::{Be, Le};

#[derive(Clone, Debug)]
pub struct Section<'a> {
    pub offset: usize,
    pub tag: [u8; 4],
    pub size: usize,
    pub body: &'a [u8],
}

impl<'a> Section<'a> {
    pub fn tag_str(&self) -> String {
        String::from_utf8_lossy(&self.tag).into_owned()
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name_hash: u32,
    pub offset: u32,
    pub type_raw: u32,
    pub ref_hash: u32,
}

#[derive(Clone, Debug)]
pub struct StructSchema {
    pub hash: u32,
    pub flags: u32,
    pub size: u32,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct EnumSchema {
    pub hash: u32,
    pub values: Vec<(i64, u32)>,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub type_hash: u32,
    pub size: u32,
    pub offset: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Psin<'a> {
    pub sections: Vec<Section<'a>>,
    pub structs: Vec<StructSchema>,
    pub enums: Vec<EnumSchema>,
    pub instances: Vec<Instance>,
    pub payload_len: usize,
}

/// Parse the tagged-section chain.
pub fn sections(data: &[u8]) -> Vec<Section<'_>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let known = |t: &[u8]| {
        matches!(
            t,
            b"PSIN" | b"PMAP" | b"PSCH" | b"PSIG" | b"STRF" | b"STRE" | b"CHKS" | b"LSCH"
                | b"LENM" | b"LSNS" | b"LGID" | b"LRSC" | b"LSTR"
        )
    };
    while at + 8 <= data.len() {
        let tag = &data[at..at + 4];
        if !known(tag) {
            at += 1;
            continue;
        }
        let Some(size) = Be(&data[at..]).u32(4).map(|v| v as usize) else {
            break;
        };
        if size < 8 || at + size > data.len() {
            at += 1;
            continue;
        }
        let tag4: [u8; 4] = tag.try_into().unwrap();
        out.push(Section {
            offset: at,
            tag: tag4,
            size,
            body: &data[at + 8..at + size],
        });
        at += size;
    }
    out
}

/// Parse LSCH struct schemas.
pub fn parse_lsch(lsch: &[u8]) -> Vec<StructSchema> {
    let h = Le(lsch);
    let mut at = 0usize;
    let mut heads = Vec::new();
    while at + 32 <= lsch.len() && &lsch[at + 16..at + 24] == &[0xAA; 8] {
        heads.push((
            h.u32(at).unwrap_or(0),
            h.u32(at + 8).unwrap_or(0),
            h.u32(at + 24).unwrap_or(0),
            (h.u32(at + 28).unwrap_or(0) >> 16) as usize,
        ));
        at += 32;
    }
    let mut out = Vec::with_capacity(heads.len());
    for (hash, flags, size, count) in heads {
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            if at + 16 > lsch.len() {
                break;
            }
            fields.push(Field {
                name_hash: h.u32(at).unwrap_or(0),
                offset: h.u32(at + 4).unwrap_or(0),
                type_raw: h.u32(at + 8).unwrap_or(0),
                ref_hash: h.u32(at + 12).unwrap_or(0),
            });
            at += 16;
        }
        out.push(StructSchema {
            hash,
            flags,
            size,
            fields,
        });
    }
    out
}

/// Parse LENM enum schemas.
pub fn parse_lenm(lenm: &[u8]) -> Vec<EnumSchema> {
    let h = Le(lenm);
    let mut at = 0usize;
    let mut out = Vec::new();
    while at + 24 <= lenm.len() && &lenm[at + 8..at + 16] == &[0xEE; 8] {
        let hash = h.u32(at).unwrap_or(0);
        let count = h.u32(at + 16).unwrap_or(0) as usize;
        at += 24;
        let mut values = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            if at + 16 > lenm.len() {
                break;
            }
            values.push((
                h.u64(at).unwrap_or(0) as i64,
                h.u32(at + 8).unwrap_or(0),
            ));
            at += 16;
        }
        out.push(EnumSchema { hash, values });
    }
    out
}

/// Parse the PSIN body's instance table and payload layout.
pub fn parse_instances(body: &[u8]) -> (Vec<Instance>, usize) {
    let h = Le(body);
    let mut at = 0usize;
    let mut raw = Vec::new();
    while at + 8 <= body.len() {
        if &body[at..at + 8] == b"pppppppp" {
            at += 8;
            break;
        }
        if at + 16 > body.len() {
            break;
        }
        raw.push((h.u32(at).unwrap_or(0), h.u32(at + 4).unwrap_or(0)));
        at += 16;
    }
    let payload = &body[at..];
    let mut cursor = 0usize;
    let mut out = Vec::with_capacity(raw.len());
    for (type_hash, size) in raw {
        out.push(Instance {
            type_hash,
            size,
            offset: cursor,
        });
        cursor = cursor.saturating_add((size as usize + 15) & !15);
    }
    (out, payload.len())
}

/// Full parse: sections plus schemas plus instance table.
pub fn parse(data: &[u8]) -> Psin<'_> {
    let secs = sections(data);
    let get = |tag: &[u8; 4]| secs.iter().find(|s| &s.tag == tag).map(|s| s.body);
    let structs = get(b"LSCH").map(parse_lsch).unwrap_or_default();
    let enums = get(b"LENM").map(parse_lenm).unwrap_or_default();
    let (instances, payload_len) = get(b"PSIN").map(parse_instances).unwrap_or_default();
    Psin {
        sections: secs,
        structs,
        enums,
        instances,
        payload_len,
    }
}

/// JOAAT (Jenkins one-at-a-time) hash used for every RAGE name field.
pub fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for &b in s.as_bytes() {
        h = h.wrapping_add((b as char).to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// Resolve field/enum hashes against a caller-supplied dictionary.
pub fn resolve<'a>(hashes: impl Iterator<Item = u32>, dict: &'a HashMap<u32, String>) -> Vec<(u32, Option<&'a String>)> {
    hashes.map(|h| (h, dict.get(&h))).collect()
}

/// Field type categories derived from `type_raw & 0xFF`.
pub fn type_kind(type_raw: u32) -> &'static str {
    match type_raw & 0xFF {
        0x01 => "bool",
        0x02 | 0x10 => "u8",
        0x11 | 0x12 => "u16",
        0x13..=0x16 => "u32/f32",
        0x17 | 0x18 => "u64/f64",
        0x20..=0x22 => "vec3",
        0x44 | 0x48 | 0x4A => "hash",
        0x52 | 0x5C => "array",
        0x60 => "enum",
        0x05 => "struct",
        _ => "raw",
    }
}
