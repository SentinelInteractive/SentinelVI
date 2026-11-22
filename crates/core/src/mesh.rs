//! RAGE9 drawables: drawable dictionaries, LOD0 geometry enumeration and
//! meshlet (virtual geometry) decoding.
//!
//! A drawable stores geometry as meshlet batches:
//!
//! * the **meshlet buffer** (physical `0x60xxxxxx` page) holds one header per
//!   meshlet (`u16 vertex_base`, `u16 tri_base`), a 32-bit packed triangle list,
//!   and a `u16` vertex remap table;
//! * the **vertex buffer** holds eight 32-bit stream descriptors, then the
//!   packed vertex stream. Stream 0 is 8 bytes per vertex: two 24-bit coordinates
//!   and one 16-bit coordinate, dequantised by a per-batch origin/scale and
//!   reordered by a 3-way axis permutation. A stream tagged `0x28` holds 4-byte
//!   UVs in 1/8192 units.
//!
//! Triangle words pack three 7-bit indices relative to the meshlet's vertex
//! base; each index addresses the remap table, which addresses the vertex
//! stream. Vertex bases are not monotonic across meshlets, which is why the
//! remap table is mandatory.
//!
//! Everything here was derived from the bytes; the offsets and tag values are
//! format facts, not borrowed code.

use std::collections::HashSet;
use std::path::Path;

use crate::read::Le;
use crate::rpf9::{pptr, vptr, Result, Rpf9Error};
use crate::source::Source;

/// One meshlet geometry: physical vertex-buffer and meshlet-buffer spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GeomRef {
    pub vb_off: usize,
    pub vb_bytes: usize,
    pub mb_off: usize,
    pub mb_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub geoms: usize,
    pub tris: std::ops::Range<usize>,
    pub verts: std::ops::Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub name: String,
    pub verts: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tris: Vec<[u32; 3]>,
    pub tri_geom: Vec<u16>,
    pub parts: Vec<Part>,
    pub geom_count: usize,
    pub meshlet_count: usize,
    pub bone_count: usize,
    pub is_vehicle: bool,
    pub ext: &'static str,
}

impl Mesh {
    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &self.verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        (lo, hi)
    }

    pub fn part_bounds(&self, part: &Part) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for t in &self.tris[part.tris.clone()] {
            for &i in t {
                if let Some(v) = self.verts.get(i as usize) {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                }
            }
        }
        (lo, hi)
    }

    /// Wavefront OBJ with one `o` group per drawable and per-triangle material
    /// ids preserved as `usemtl geom_<n>`.
    pub fn write_obj(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(f, "# sentinel-rpf9-core {}", crate::VERSION)?;
        writeln!(f, "# {} verts {} tris {} geoms", self.verts.len(), self.tris.len(), self.geom_count)?;
        for v in &self.verts {
            writeln!(f, "v {:.5} {:.5} {:.5}", v[0], v[1], v[2])?;
        }
        for uv in &self.uvs {
            writeln!(f, "vt {:.5} {:.5}", uv[0], uv[1])?;
        }
        if self.parts.is_empty() {
            writeln!(f, "o {}", self.name)?;
            write_tris(&mut f, self, &self.tris, 0)?;
        } else {
            for part in &self.parts {
                writeln!(f, "o {}", sanitize(&part.name))?;
                write_tris(&mut f, self, &self.tris[part.tris.clone()], part.tris.start)?;
            }
        }
        Ok(())
    }
}

fn write_tris(
    f: &mut impl std::io::Write,
    mesh: &Mesh,
    tris: &[[u32; 3]],
    base: usize,
) -> std::io::Result<()> {
    let mut last_geom = u16::MAX;
    for (i, t) in tris.iter().enumerate() {
        let g = mesh.tri_geom.get(base + i).copied().unwrap_or(0);
        if g != last_geom {
            writeln!(f, "usemtl geom_{g}")?;
            last_geom = g;
        }
        let has_uv = mesh.uvs.len() > *t.iter().max().unwrap_or(&0) as usize;
        if has_uv {
            writeln!(
                f,
                "f {0}/{0} {1}/{1} {2}/{2}",
                t[0] + 1,
                t[1] + 1,
                t[2] + 1
            )?;
        } else {
            writeln!(f, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1)?;
        }
    }
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' })
        .collect()
}

/// Resolve a meshlet link pointer to `(physical offset, byte length)`.
#[inline]
pub fn resolve_meshlet_buffer(v: &[u8], link: u64) -> Option<(usize, usize)> {
    let lo = vptr(link)?;
    let h = Le(v);
    let mo = vptr(h.u64(lo)?)?;
    if mo + 0x20 > v.len() {
        return None;
    }
    let mb_off = pptr(h.u64(mo + 0x18)?)?;
    let words = h.u32(mo + 8)? as usize;
    Some((mb_off, words * 4))
}

/// True when the resource contains at least one complete meshlet geometry.
pub fn has_meshlets(v: &[u8]) -> bool {
    let h = Le(v);
    let mut off = 0x28usize;
    while off + 0x10 <= v.len() {
        if h.u64(off - 8) == Some(0x1601)
            && h.u64(off).and_then(pptr).is_some()
            && h.u64(off - 0x28)
                .and_then(|link| resolve_meshlet_buffer(v, link))
                .is_some()
        {
            return true;
        }
        off += 8;
    }
    false
}

/// LOD0 geometries of the drawable that starts at `base`.
pub fn lod0_geoms(v: &[u8], base: usize) -> Vec<GeomRef> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if base + 0x80 > v.len() {
        return out;
    }
    let h = Le(v);
    let Some(lod0) = h.u64(base + 0x50).and_then(vptr) else {
        return out;
    };
    if lod0 + 0x10 > v.len() {
        return out;
    }
    let Some(models) = h.u64(lod0).and_then(vptr) else {
        return out;
    };
    let model_cnt = match h.u16(lod0 + 8) {
        Some(c) => c as usize,
        None => return out,
    };
    if model_cnt == 0 || model_cnt > 128 || models + model_cnt * 8 > v.len() {
        return out;
    }
    for mi in 0..model_cnt {
        let Some(mp) = h.u64(models + mi * 8).and_then(vptr) else {
            continue;
        };
        if mp + 0x18 > v.len() {
            continue;
        }
        let Some(garr) = h.u64(mp + 8).and_then(vptr) else {
            continue;
        };
        let gcnt = match h.u16(mp + 0x10) {
            Some(c) => c as usize,
            None => continue,
        };
        if gcnt == 0 || gcnt > 256 || garr + gcnt * 8 > v.len() {
            continue;
        }
        for gi in 0..gcnt {
            let Some(gp) = h.u64(garr + gi * 8).and_then(vptr) else {
                continue;
            };
            if gp + 0x70 > v.len() || h.u64(gp + 0x60) != Some(0x1601) {
                continue;
            }
            let Some(vb_off) = h.u64(gp + 0x68).and_then(pptr) else {
                continue;
            };
            let Some((mb_off, mb_bytes)) = h
                .u64(gp + 0x40)
                .and_then(|link| resolve_meshlet_buffer(v, link))
            else {
                continue;
            };
            let vb_bytes = (h.u32(gp + 0x58).unwrap_or(0) as usize) * 4;
            if seen.insert((vb_off, mb_off)) {
                out.push(GeomRef {
                    vb_off,
                    vb_bytes,
                    mb_off,
                    mb_bytes,
                });
            }
        }
    }
    out
}

/// Fallback enumeration: every `0x1601` geometry tag in the page.
pub fn collect_all_geoms(v: &[u8]) -> Vec<GeomRef> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let h = Le(v);
    let mut off = 0x28usize;
    while off + 0x10 <= v.len() {
        if h.u64(off - 8) == Some(0x1601) {
            if let (Some(vb_off), Some((mb_off, mb_bytes))) = (
                h.u64(off).and_then(pptr),
                h.u64(off - 0x28)
                    .and_then(|link| resolve_meshlet_buffer(v, link)),
            ) {
                let vb_bytes = (h.u32(off - 16).unwrap_or(0) as usize) * 4;
                if seen.insert((vb_off, mb_off)) {
                    out.push(GeomRef {
                        vb_off,
                        vb_bytes,
                        mb_off,
                        mb_bytes,
                    });
                }
            }
        }
        off += 8;
    }
    out
}

/// Drawable name: the `foo.#dr` / `.#ft` / `.#dd` string, else the name pointer
/// at drawable `+0xa8`.
pub fn model_name(v: &[u8]) -> Option<(String, &'static str)> {
    let mut i = 1usize;
    while i + 2 < v.len() {
        if v[i] == b'#' && v[i - 1] == b'.' {
            let ext = match (v[i + 1], v[i + 2]) {
                (b'd', b'r') => ".#dr",
                (b'f', b't') => ".#ft",
                (b'd', b'd') => ".#dd",
                _ => {
                    i += 1;
                    continue;
                }
            };
            let dot = i - 1;
            let mut start = dot;
            while start > 0 {
                let c = v[start - 1];
                if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
                    start -= 1;
                } else {
                    break;
                }
            }
            if (2..=96).contains(&(dot - start)) {
                let stem = String::from_utf8_lossy(&v[start..dot]);
                return Some((format!("{stem}{ext}"), ext));
            }
        }
        i += 1;
    }
    if v.len() >= 0xb0 {
        let h = Le(v);
        if let Some(no) = h.u64(0xa8).and_then(vptr) {
            if let Some(rel) = v.get(no..).and_then(|s| s.iter().position(|&b| b == 0)) {
                if (2..=96).contains(&rel) {
                    let s = &v[no..no + rel];
                    let ok = s
                        .iter()
                        .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'#'));
                    if ok {
                        let name = String::from_utf8_lossy(s).trim_end_matches('.').to_string();
                        if name.len() >= 2 {
                            let ext = if name.ends_with(".#ft") { ".#ft" } else { ".#dr" };
                            let full = if name.contains(".#") { name } else { format!("{name}.#dr") };
                            return Some((full, ext));
                        }
                    }
                }
            }
        }
    }
    None
}

/// Fragment bucket parts that must not be reported as vehicles.
const PART_SUFFIXES: &[&str] = &[
    "_dmg_", "_livery", "_roof_", "_roofext_", "_spoiler", "_exhaust", "_bumf_", "_bumr_",
    "_skirt_", "_bonnet", "_wing_", "_door_", "_mirror", "_lights_", "_intakes_", "_platemount",
    "_towbar", "_speakermount", "_deliverybag", "_antennaset", "_radar_", "_horn", "_outboard",
    "_inboard", "_mast_", "_mount", "_turret", "_weap_", "_wheeliebar", "_induction_",
    "_whiplight", "_rack_", "_sign_mesh", "_armour_plate", "_fairing_", "_rescuehoist",
    "_footboards", "_callsign", "_shorad", "_badge", "_trim", "_overlay",
];

const VEHICLE_MARKERS: &[&str] = &[
    "wheel_lf",
    "door_dside_f",
    "chassis",
    "bodyshell",
    "cockpit",
    "rudder",
    "rotor_main",
];

pub fn is_vehicle(v: &[u8], name: &str) -> bool {
    let lo = name.to_ascii_lowercase();
    if !lo.ends_with(".#ft") || v.len() < 28_000 {
        return false;
    }
    if PART_SUFFIXES.iter().any(|p| lo.contains(p)) {
        return false;
    }
    VEHICLE_MARKERS
        .iter()
        .any(|needle| v.windows(needle.len()).any(|w| w == needle.as_bytes()))
}

/// `(name, drawable offset)` for every entry of a drawable dictionary root.
pub fn dictionary_entries(v: &[u8]) -> Vec<(String, usize)> {
    if v.len() < 0x40 {
        return Vec::new();
    }
    let h = Le(v);
    let (Some(hashes), Some(arr)) = (h.u64(0x20).and_then(vptr), h.u64(0x30).and_then(vptr)) else {
        return Vec::new();
    };
    let cnt = match h.u16(0x38) {
        Some(c) => c as usize,
        None => return Vec::new(),
    };
    if !(2..=4096).contains(&cnt)
        || h.u16(0x28).map(|c| c as usize) != Some(cnt)
        || hashes + cnt * 4 > v.len()
        || arr + cnt * 8 > v.len()
    {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(cnt);
    for i in 0..cnt {
        let Some(d) = h.u64(arr + i * 8).and_then(vptr) else {
            return Vec::new();
        };
        if d + 0xb0 > v.len() || h.u64(d + 0x50).and_then(vptr).is_none() {
            return Vec::new();
        }
        let name = h
            .u64(d + 0xa8)
            .and_then(vptr)
            .and_then(|n| {
                let len = v.get(n..)?.iter().take(128).position(|&b| b == 0)?;
                let s = &v[n..n + len];
                (len >= 2 && s.iter().all(|b| b.is_ascii_graphic()))
                    .then(|| String::from_utf8_lossy(s).into_owned())
            })
            .unwrap_or_else(|| format!("0x{:08x}", h.u32(hashes + i * 4).unwrap_or(0)));
        out.push((name, d));
    }
    out
}

/// Skeleton bone count for the drawable (drawable `+0x18` -> skeleton `+0x6c`).
pub fn bone_count(v: &[u8]) -> usize {
    let h = Le(v);
    let mut bases = vec![0usize];
    if v.len() >= 0x20 {
        if let Some(fo) = h.u64(0x18).and_then(vptr) {
            if fo + 0x20 <= v.len() {
                bases.push(fo);
            }
        }
    }
    for base in bases {
        if base + 0x20 > v.len() {
            continue;
        }
        let Some(so) = h.u64(base + 0x18).and_then(vptr) else {
            continue;
        };
        if so + 0x70 <= v.len() {
            let cnt = h.u16(so + 0x6C).unwrap_or(0) as usize;
            if (1..=1024).contains(&cnt) && h.u64(so + 0x28).and_then(vptr).is_some() {
                return cnt;
            }
        }
    }
    0
}

#[derive(Default)]
struct MeshBuild {
    verts: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    tris: Vec<[u32; 3]>,
    tri_geom: Vec<u16>,
    meshlets: usize,
}

/// Decode one meshlet geometry into the build buffer.
fn append_geometry(p: &[u8], g: GeomRef, geom_id: u16, b: &mut MeshBuild) {
    if g.mb_off + 0x10 > p.len() || g.vb_off + 0x80 > p.len() {
        return;
    }
    let h = Le(p);
    let num_meshlets = (h.u32(g.mb_off).unwrap_or(0) & 0xFFFF) as usize;
    let tot_tris = h.u32(g.mb_off + 4).unwrap_or(0) as usize;
    let tot_vrefs = h.u32(g.mb_off + 8).unwrap_or(0) as usize;
    if num_meshlets == 0 || tot_tris == 0 || tot_vrefs == 0 {
        return;
    }
    let tri_base = g.mb_off + 0x0C + num_meshlets * 16;
    let remap_base = tri_base + tot_tris * 4;
    if remap_base + tot_vrefs * 2 > p.len() {
        return;
    }

    let mut hdrs = [0u32; 8];
    for (k, slot) in hdrs.iter_mut().enumerate() {
        *slot = h.u32(g.vb_off + k * 4).unwrap_or(0);
    }
    let stream0_off = ((hdrs[0] >> 8) << 1) as usize;
    let mut stream0_end = g.vb_bytes;
    let mut uv_stream_off: Option<usize> = None;
    for &s in &hdrs[1..] {
        if s == 0 {
            continue;
        }
        let so = ((s >> 8) << 1) as usize;
        let tag = s & 0xFF;
        if so > stream0_off && so < stream0_end {
            stream0_end = so;
        }
        if tag == 0x28 && uv_stream_off.is_none() {
            uv_stream_off = Some(so);
        }
    }
    let vbase = g.vb_off + stream0_off;
    if vbase >= p.len() {
        return;
    }
    let max_vb_verts = (stream0_end.saturating_sub(stream0_off) / 8)
        .min((p.len() - vbase) / 8);
    if max_vb_verts == 0 {
        return;
    }

    let ox = h.f32(g.vb_off + 0x60).unwrap_or(0.0);
    let oy = h.f32(g.vb_off + 0x64).unwrap_or(0.0);
    let oz = h.f32(g.vb_off + 0x68).unwrap_or(0.0);
    let sx = h.f32(g.vb_off + 0x70).unwrap_or(1.0);
    let sy = h.f32(g.vb_off + 0x74).unwrap_or(1.0);
    let sz = h.f32(g.vb_off + 0x78).unwrap_or(1.0);
    let slot_axis: [usize; 3] = match h.u32(g.vb_off + 0x6C).unwrap_or(2) {
        0 => [2, 1, 0],
        1 => [2, 0, 1],
        _ => [0, 1, 2],
    };

    let base_v = b.verts.len() as u32;
    let mut max_used = 0usize;
    let start_tris = b.tris.len();

    for mi in 0..num_meshlets {
        let m = g.mb_off + 0x0C + mi * 16;
        let v_off = h.u16(m + 4).unwrap_or(0) as usize;
        let t_off = h.u16(m + 6).unwrap_or(0) as usize;
        let t_end = if mi + 1 < num_meshlets {
            h.u16(g.mb_off + 0x0C + (mi + 1) * 16 + 6).unwrap_or(0) as usize
        } else {
            tot_tris
        };
        for ti in t_off..t_end.min(tot_tris) {
            let w = match h.u32(tri_base + ti * 4) {
                Some(w) => w,
                None => break,
            };
            let r0 = v_off + (w & 0x7F) as usize;
            let r1 = v_off + ((w >> 7) & 0x7F) as usize;
            let r2 = v_off + ((w >> 14) & 0x7F) as usize;
            if r0 >= tot_vrefs || r1 >= tot_vrefs || r2 >= tot_vrefs {
                continue;
            }
            let i0 = h.u16(remap_base + r0 * 2).unwrap_or(0xFFFF) as usize;
            let i1 = h.u16(remap_base + r1 * 2).unwrap_or(0xFFFF) as usize;
            let i2 = h.u16(remap_base + r2 * 2).unwrap_or(0xFFFF) as usize;
            if i0 >= max_vb_verts || i1 >= max_vb_verts || i2 >= max_vb_verts {
                continue;
            }
            if i0 == i1 || i1 == i2 || i0 == i2 {
                continue;
            }
            b.tris
                .push([base_v + i0 as u32, base_v + i1 as u32, base_v + i2 as u32]);
            max_used = max_used.max(i0).max(i1).max(i2);
        }
    }
    let added = b.tris.len() - start_tris;
    if added == 0 {
        return;
    }
    let v_cnt = (max_used + 1).min(max_vb_verts);
    b.verts.reserve(v_cnt);
    b.uvs.reserve(v_cnt);
    for i in 0..v_cnt {
        let o = vbase + i * 8;
        // 8 bytes: 24-bit x, 24-bit y, 16-bit z, interleaved as y.lo, x.0, x.1,
        // x.2, y.1, y.2, z.0, z.1.
        let ix = (p[o + 1] as u32) | ((p[o + 2] as u32) << 8) | ((p[o + 3] as u32) << 16);
        let iy = (p[o] as u32) | ((p[o + 4] as u32) << 8) | ((p[o + 5] as u32) << 16);
        let iz = (p[o + 6] as u32) | ((p[o + 7] as u32) << 8);
        let slots = [ox + ix as f32 * sx, oy + iy as f32 * sy, oz + iz as f32 * sz];
        let mut pos = [0.0f32; 3];
        for (slot, &axis) in slot_axis.iter().enumerate() {
            pos[axis] = slots[slot];
        }
        b.verts.push(pos);
        let uv = uv_stream_off
            .map(|uvo| g.vb_off + uvo + i * 4)
            .filter(|&u| u + 4 <= p.len())
            .map(|u| {
                [
                    h.u16(u).unwrap_or(0) as f32 / 8192.0,
                    h.u16(u + 2).unwrap_or(0) as f32 / 8192.0,
                ]
            })
            .unwrap_or([0.0, 0.0]);
        b.uvs.push(uv);
    }
    b.tri_geom.resize(b.tri_geom.len() + added, geom_id);
    b.meshlets += num_meshlets;
}

/// Decode a drawable (or drawable dictionary) resource at `file_off`.
pub fn decode(src: &dyn Source, file_off: u64, fallback: &str) -> Result<Mesh> {
    let res = crate::rpf9::read_virtual_resource(src, file_off, 64 << 20, 64 << 20)?;
    let v = &res.virt;
    let p = &res.phys;
    if p.len() < 0x90 {
        return Err(Rpf9Error::BadHeader("physical buffer missing".into()));
    }
    let (name, ext) = model_name(v).unwrap_or_else(|| (fallback.to_string(), ".#dr"));
    let dict = dictionary_entries(v);
    let groups: Vec<(String, Option<usize>, Vec<GeomRef>)> = if !dict.is_empty() {
        dict.iter()
            .map(|(n, base)| (n.clone(), Some(*base), lod0_geoms(v, *base)))
            .filter(|(_, _, g)| !g.is_empty())
            .collect()
    } else {
        let mut bases = vec![0usize];
        let h = Le(v);
        if v.len() >= 0x20 {
            if let Some(fo) = h.u64(0x18).and_then(vptr) {
                if fo + 0x80 <= v.len() {
                    bases.push(fo);
                }
            }
        }
        match bases
            .into_iter()
            .map(|b| (b, lod0_geoms(v, b)))
            .find(|(_, g)| !g.is_empty())
        {
            Some((b, lod0)) => vec![(name.clone(), Some(b), lod0)],
            None => vec![(name.clone(), None, collect_all_geoms(v))],
        }
    };
    let geom_count: usize = groups.iter().map(|(_, _, g)| g.len()).sum();
    if geom_count == 0 {
        return Err(Rpf9Error::NoGeometry);
    }

    let mut b = MeshBuild::default();
    let mut parts = Vec::with_capacity(groups.len());
    let mut gi = 0u16;
    for (pname, _base, geoms) in &groups {
        let t0 = b.tris.len();
        let v0 = b.verts.len();
        for g in geoms {
            append_geometry(p, *g, gi, &mut b);
            gi = gi.wrapping_add(1);
        }
        if b.tris.len() > t0 {
            parts.push(Part {
                name: pname.clone(),
                geoms: geoms.len(),
                tris: t0..b.tris.len(),
                verts: v0..b.verts.len(),
            });
        }
    }
    if b.verts.is_empty() {
        return Err(Rpf9Error::BadHeader("zero vertices decoded".into()));
    }
    Ok(Mesh {
        name: name.clone(),
        verts: b.verts,
        uvs: b.uvs,
        tris: b.tris,
        tri_geom: b.tri_geom,
        parts,
        geom_count,
        meshlet_count: b.meshlets,
        bone_count: bone_count(v),
        is_vehicle: is_vehicle(v, &name),
        ext,
    })
}
