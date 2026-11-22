//! Placement records from `levels.rpf`: entity instances, prop instances and
//! the interiors that carry them, stored as a queryable SQLite point cloud.

use flate2::read::ZlibDecoder;
use memmap2::Mmap;
use rayon::prelude::*;
use rusqlite::{params, Connection};
use sentinel_core::zlib::inflate_stream;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const VLO: u64 = 0x5000_0000;
const VHI: u64 = 0x5800_0000;
const PLO: u64 = 0x6000_0000;
const PHI: u64 = 0x6800_0000;

const H_ENTITY: u32 = 0xce50_1483;
const H_PROP_I: u32 = 0xea6b_3a96;
const H_MLO: u32 = 0x09cc_28be;
const H_GRASS_I: u32 = 0xed86_f522;
const H_GRASS_B: u32 = 0x7c47_5b5d;

const WINDOW: usize = 16 << 20;
const PROBE: usize = 0x1000;
const MAX_PAGE: usize = 64 << 20;
const MAX_VIRT: usize = 512 << 20;
const FLUSH_AT: usize = 100_000;
const MAX_LIMIT: usize = 40_000;

const BOUNDS: [f32; 4] = [-13000.0, -9000.0, 7000.0, 11000.0];

/// Path of the placement database under a data root.
pub fn db_path(data_root: &Path) -> PathBuf {
    data_root.join("sentinel").join("placement.sqlite")
}

#[derive(Default, Clone, Copy, serde::Serialize)]
pub struct BuildReport {
    pub resources: u64,
    pub entities: u64,
    pub entities_kept: u64,
    pub props: u64,
    pub props_kept: u64,
    pub interiors: u64,
    pub seconds: f64,
}

#[derive(Clone, serde::Serialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub arch: u32,
    pub flags: u32,
    pub layer: u8,
    pub interior: u8,
    pub res: u64,
}

#[derive(Clone, serde::Serialize)]
pub struct MapMeta {
    pub built_at: String,
    pub entities: u64,
    pub props: u64,
    pub entities_stored: u64,
    pub props_stored: u64,
    pub interior_points: u64,
    pub bounds: [f32; 4],
}

#[derive(Clone, serde::Serialize)]
pub struct Density {
    pub total: u64,
    pub bins: u64,
    pub max: u32,
    pub width: usize,
    pub height: usize,
    pub cells: Vec<u32>,
}

#[inline]
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7],
    ])
}
#[inline]
fn f32le(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
fn is_zlib(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0x78 && matches!(b[1], 0x01 | 0x5e | 0x9c | 0xda) && (b[2] & 0x06) != 0x06
}
#[inline]
fn vptr(q: u64) -> Option<usize> {
    (VLO..VHI).contains(&q).then(|| (q - VLO) as usize)
}
#[inline]
fn pptr(q: u64) -> Option<usize> {
    (PLO..PHI).contains(&q).then(|| (q - PLO) as usize)
}
#[inline]
fn align16(x: usize) -> usize {
    (x + 15) & !15
}

/// A resource root page: zero vtable word at +0x00 and `0DRPz` at +0x10.
#[inline]
fn looks_root(v: &[u8]) -> bool {
    v.len() >= 0x20 && u64le(v, 0) == 0 && &v[0x10..0x15] == b"0DRPz"
}

// ------------------------------------------------------------------ carve

#[derive(Clone, Copy)]
struct Blk {
    hash: u32,
    size: usize,
    ptr: u64,
}

#[allow(dead_code)]
struct Carved {
    v: Vec<u8>,
    phys: Vec<u8>,
    phys_kind: u8,
    vc: usize,
    pc: usize,
    chunks: usize,
    need: usize,
    diag: u8,
    blocks: Vec<Blk>,
}

impl Carved {
    fn empty(diag: u8) -> Self {
        Carved {
            v: Vec::new(),
            phys: Vec::new(),
            phys_kind: 0,
            vc: 0,
            pc: 0,
            chunks: 0,
            need: 0,
            diag,
            blocks: Vec::new(),
        }
    }
}

fn block_table_probe(v: &[u8]) -> Option<()> {
    if v.len() < 0x50 {
        return None;
    }
    let nbl = u16le(v, 0x4c) as usize;
    let it = vptr(u64le(v, 0x38))?;
    if nbl == 0 || nbl > 4096 || it + nbl * 16 > v.len() {
        return None;
    }
    Some(())
}

fn block_table(v: &[u8]) -> Option<Vec<Blk>> {
    if v.len() < 0x50 {
        return None;
    }
    let nbl = u16le(v, 0x4c) as usize;
    let it = vptr(u64le(v, 0x38))?;
    if nbl == 0 || nbl > 4096 || it + nbl * 16 > v.len() {
        return None;
    }
    let mut out = Vec::with_capacity(nbl);
    for i in 0..nbl {
        let o = it + i * 16;
        out.push(Blk {
            hash: u32le(v, o),
            size: u32le(v, o + 4) as usize,
            ptr: u64le(v, o + 8),
        });
    }
    Some(out)
}

/// Read one virtual chunk. Virtual chunks are always zlib. The reference walk
/// searches a 1 MB span in 16-byte steps from the end of the previous stream,
/// because padding between chunks is not always zero.
fn vchunk(mm: &[u8], at: usize, span: usize) -> Option<(Vec<u8>, usize)> {
    let mut p = at & !15;
    let end = at.saturating_add(span).min(mm.len().saturating_sub(4));
    while p < end {
        if is_zlib(&mm[p..]) {
            let h = inflate_stream(&mm[p..], MAX_PAGE)?;
            if h.data.is_empty() {
                return None;
            }
            // A chunk that is itself a resource root means the walk has run
            // past this resource; do not swallow the next resource.
            if looks_root(&h.data) {
                return None;
            }
            return Some((h.data, p + h.consumed));
        }
        p += 16;
    }
    None
}

fn carve(mm: &[u8], off: usize) -> Carved {
    let first = match inflate_stream(&mm[off..], MAX_PAGE) {
        Some(h) => h,
        None => return Carved::empty(9),
    };
    let mut v = first.data;
    if !looks_root(&v) {
        return Carved::empty(9);
    }
    let mut cur = off + first.consumed;
    let mut chunks = 1usize;

    let mut bmap = None;
    for _ in 0..64 {
        if let Some(b) = vptr(u64le(&v, 8)) {
            if b + 0x10 <= v.len() {
                bmap = Some(b);
                break;
            }
        }
        match vchunk(mm, cur, 1 << 20) {
            Some((d, next)) => {
                v.extend_from_slice(&d);
                cur = next;
                chunks += 1;
                if v.len() > MAX_VIRT {
                    break;
                }
            }
            None => break,
        }
    }
    let bmap = match bmap {
        Some(b) => b,
        None => return Carved::empty(1),
    };
    let vc = v[bmap + 8] as usize;
    let pc = v[bmap + 9] as usize;

    let mut short = false;
    while chunks < vc && chunks < 64 {
        match vchunk(mm, cur, 1 << 20) {
            Some((d, next)) => {
                v.extend_from_slice(&d);
                cur = next;
                chunks += 1;
                if v.len() > MAX_VIRT {
                    break;
                }
            }
            None => {
                short = true;
                break;
            }
        }
    }

    let mut extra = 0;
    while block_table_probe(&v).is_none() && extra < 8 {
        match vchunk(mm, cur, 1 << 20) {
            Some((d, next)) => {
                v.extend_from_slice(&d);
                cur = next;
                chunks += 1;
                extra += 1;
                if v.len() > MAX_VIRT {
                    break;
                }
            }
            None => break,
        }
    }

    let blocks = match block_table(&v) {
        Some(b) => b,
        None => {
            let mut c = Carved::empty(if short { 3 } else { 2 });
            c.vc = vc;
            c.pc = pc;
            c.chunks = chunks;
            c.v = v;
            return c;
        }
    };

    let mut need = 0usize;
    for b in &blocks {
        if let Some(o) = pptr(b.ptr) {
            need = need.max(o + b.size);
        }
    }

    let mut phys: Vec<u8> = Vec::new();
    let mut phys_kind = 0u8;
    // need == 0 means the block table declares no physical blocks: there is no
    // physical page to find, so phys stays empty and the diagnostic stays 0.
    if need > 0 && cur < mm.len() {
        let ps = align16(cur);
        let mut best: Option<(f64, Vec<u8>, u8)> = None;
        fn consider(best: &mut Option<(f64, Vec<u8>, u8)>, data: Vec<u8>, kind: u8, need: usize) {
            let n = data.len();
            if n < need || n > need + 0x400 {
                return;
            }
            let score = if n == need {
                1.0
            } else if n <= need + 0x10 {
                0.75
            } else {
                0.5
            };
            if data.len() >= 0x20 && looks_root(&data) {
                return;
            }
            if best.as_ref().map(|(s, _, _)| score > *s).unwrap_or(true) {
                *best = Some((score, data, kind));
            }
        }
        if ps + 4 <= mm.len() && is_zlib(&mm[ps..]) {
            if let Some(h) = inflate_stream(&mm[ps..], need + 0x400) {
                consider(&mut best, h.data, 1, need);
            }
        }
        if best.is_none() {
            let mut scan = ps;
            for _ in 0..64 {
                match find_zlib(mm, scan + 0x10, 4 << 20) {
                    Some(z) => match inflate_stream(&mm[z..], need + 0x400) {
                        Some(h) => {
                            scan = z + h.consumed.max(1);
                            consider(&mut best, h.data, 1, need);
                        }
                        None => scan = z + 0x10,
                    },
                    None => break,
                }
            }
        }
        if best.is_none() && !is_zlib(&mm[ps..]) && ps + need <= mm.len() {
            let cand = &mm[ps..ps + need];
            if !(cand.len() >= 0x20 && looks_root(cand)) {
                best = Some((1.0, cand.to_vec(), 2));
            }
        }
        if let Some((_, data, kind)) = best {
            phys = data;
            phys_kind = kind;
        }
    }

    let diag = if need > 0 && phys_kind == 0 {
        4
    } else if short {
        3
    } else {
        0
    };
    Carved {
        v,
        phys,
        phys_kind,
        vc,
        pc,
        chunks,
        need,
        diag,
        blocks,
    }
}

fn find_zlib(mm: &[u8], from: usize, span: usize) -> Option<usize> {
    let end = (from + span).min(mm.len().saturating_sub(4));
    let mut o = from & !0x0f;
    while o < end {
        if is_zlib(&mm[o..]) {
            return Some(o);
        }
        o += 0x10;
    }
    None
}

fn blk_slice<'a>(c: &'a Carved, ptr: u64, size: usize) -> Option<&'a [u8]> {
    if let Some(o) = vptr(ptr) {
        if o + size <= c.v.len() {
            return Some(&c.v[o..o + size]);
        }
    } else if let Some(o) = pptr(ptr) {
        if o + size <= c.phys.len() {
            return Some(&c.phys[o..o + size]);
        }
    }
    None
}

// ------------------------------------------------------------------ probe

/// Truncating inflate: returns at most `cap` bytes. Distinct from
/// `inflate_stream`, which rejects a stream that does not fit.
fn inflate_cap(b: &[u8], cap: usize) -> Vec<u8> {
    let mut z = ZlibDecoder::new(b);
    let mut out = Vec::new();
    let _ = (&mut z).take(cap as u64).read_to_end(&mut out);
    out
}

fn probe_root(mm: &[u8], off: usize) -> bool {
    off.saturating_add(4) <= mm.len()
        && is_zlib(&mm[off..])
        && looks_root(&inflate_cap(&mm[off..], 0x400))
}

#[inline]
fn in_bounds(x: f32, y: f32, z: f32) -> bool {
    x.is_finite()
        && y.is_finite()
        && z.is_finite()
        && (-13000.0..=7000.0).contains(&x)
        && (-9000.0..=11000.0).contains(&y)
        && (-50.0..=500.0).contains(&z)
}

#[derive(Default)]
struct Local {
    resources: u64,
    entities: u64,
    entities_kept: u64,
    props: u64,
    props_kept: u64,
    interiors: u64,
    interior_points: u64,
}

fn add(dst: &mut Local, src: &Local) {
    dst.resources += src.resources;
    dst.entities += src.entities;
    dst.entities_kept += src.entities_kept;
    dst.props += src.props;
    dst.props_kept += src.props_kept;
    dst.interiors += src.interiors;
    dst.interior_points += src.interior_points;
}

/// Entity records are 0x90 bytes: quaternion at +0x10, guid at +0x20,
/// position at +0x38/+0x3C/+0x40, archetype at +0x44, flags at +0x48.
/// Prop instances are 0x40 bytes: translation at +0x0C/+0x1C/+0x2C,
/// scale at +0x30, archetype at +0x34.
fn harvest(c: &Carved, res: u64, points: &mut Vec<Point>, acc: &mut Local) {
    let interior = c.blocks.iter().any(|b| b.hash == H_MLO) as u8;
    if interior != 0 {
        acc.interiors += 1;
    }
    for b in &c.blocks {
        match b.hash {
            H_ENTITY => {
                if let Some(d) = blk_slice(c, b.ptr, b.size) {
                    for r in d.chunks_exact(0x90) {
                        acc.entities += 1;
                        let x = f32le(r, 0x38);
                        let y = f32le(r, 0x3c);
                        let z = f32le(r, 0x40);
                        if in_bounds(x, y, z) {
                            points.push(Point {
                                x,
                                y,
                                z,
                                arch: u32le(r, 0x44),
                                flags: u32le(r, 0x48),
                                layer: 0,
                                interior,
                                res,
                            });
                            acc.entities_kept += 1;
                            acc.interior_points += interior as u64;
                        }
                    }
                }
            }
            H_PROP_I => {
                if let Some(d) = blk_slice(c, b.ptr, b.size) {
                    for (i, r) in d.chunks_exact(0x40).enumerate() {
                        acc.props += 1;
                        if i % 4 != 0 {
                            continue;
                        }
                        let x = f32le(r, 0x0c);
                        let y = f32le(r, 0x1c);
                        let z = f32le(r, 0x2c);
                        if in_bounds(x, y, z) {
                            points.push(Point {
                                x,
                                y,
                                z,
                                arch: u32le(r, 0x34),
                                flags: 0,
                                layer: 1,
                                interior,
                                res,
                            });
                            acc.props_kept += 1;
                            acc.interior_points += interior as u64;
                        }
                    }
                }
            }
            H_GRASS_I | H_GRASS_B => {}
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ build

const SCHEMA: &str = "
CREATE TABLE points(x REAL NOT NULL, y REAL NOT NULL, z REAL NOT NULL, arch INTEGER NOT NULL, flags INTEGER NOT NULL, layer INTEGER NOT NULL, interior INTEGER NOT NULL, res INTEGER NOT NULL);
CREATE INDEX ix_x ON points(x, y);
CREATE INDEX ix_layer_x ON points(layer, x, y);
CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);
";

fn remove_stale(path: &Path) -> Result<(), String> {
    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let p = PathBuf::from(name);
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("remove {}: {e}", p.display())),
        }
    }
    Ok(())
}

fn insert_points(conn: &mut Connection, points: &[Point]) -> Result<(), String> {
    if points.is_empty() {
        return Ok(());
    }
    let tx = conn
        .transaction()
        .map_err(|e| format!("begin insert transaction: {e}"))?;
    {
        let mut stmt = tx
            .prepare("INSERT INTO points(x,y,z,arch,flags,layer,interior,res) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
            .map_err(|e| format!("prepare insert: {e}"))?;
        for p in points {
            stmt.execute(params![
                p.x as f64,
                p.y as f64,
                p.z as f64,
                p.arch as i64,
                p.flags as i64,
                p.layer as i64,
                p.interior as i64,
                p.res as i64
            ])
            .map_err(|e| format!("insert point: {e}"))?;
        }
    }
    tx.commit().map_err(|e| format!("commit insert: {e}"))
}

type Shared = Mutex<(Connection, Local)>;

fn flush(shared: &Shared, points: &mut Vec<Point>, local: &mut Local) -> Result<(), String> {
    let mut g = shared.lock().map_err(|_| "database lock poisoned".to_string())?;
    insert_points(&mut g.0, points)?;
    add(&mut g.1, local);
    points.clear();
    *local = Local::default();
    Ok(())
}

fn put_meta(conn: &Connection, k: &str, v: &str) -> Result<(), String> {
    conn.execute("INSERT INTO meta(k,v) VALUES (?1,?2)", params![k, v])
        .map_err(|e| format!("write meta {k}: {e}"))?;
    Ok(())
}

fn unix_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Full scan of levels.rpf, writes a fresh SQLite database. Creates parent dirs.
/// progress is called with (resources_done, resources_total) every ~2000 windows;
/// the total is a projection from the windows completed so far.
pub fn build(
    levels_rpf: &Path,
    out: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<BuildReport, String> {
    let t0 = std::time::Instant::now();
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
    }
    remove_stale(out)?;

    let f = File::open(levels_rpf)
        .map_err(|e| format!("open {}: {e}", levels_rpf.display()))?;
    let mm = unsafe { Mmap::map(&f) }.map_err(|e| format!("mmap {}: {e}", levels_rpf.display()))?;

    let mut conn = Connection::open(out)
        .map_err(|e| format!("create {}: {e}", out.display()))?;
    conn.execute_batch(SCHEMA)
        .map_err(|e| format!("create schema: {e}"))?;

    let nwin = (mm.len() + WINDOW - 1) / WINDOW;
    let windows_done = AtomicUsize::new(0);
    let shared: Shared = Mutex::new((conn, Local::default()));

    (0..nwin)
        .into_par_iter()
        .try_for_each(|w| -> Result<(), String> {
            let start = w * WINDOW;
            let end = ((w + 1) * WINDOW).min(mm.len());
            let mut points: Vec<Point> = Vec::new();
            let mut local = Local::default();
            // Resource roots sit at 0x10 + k * 0x1000: the first page begins
            // after a 16-byte header run, so the probe grid is offset by 0x10.
            let mut i = start + 0x10;
            while i + 0x20 < end {
                if probe_root(&mm, i) {
                    let c = carve(&mm, i);
                    if c.diag < 8 {
                        local.resources += 1;
                        harvest(&c, i as u64, &mut points, &mut local);
                        i += PROBE;
                        if points.len() >= FLUSH_AT {
                            flush(&shared, &mut points, &mut local)?;
                        }
                        continue;
                    }
                }
                i += PROBE;
            }
            flush(&shared, &mut points, &mut local)?;

            let done = windows_done.fetch_add(1, Ordering::SeqCst) + 1;
            if done % 2000 == 0 || done == nwin {
                let g = shared.lock().map_err(|_| "database lock poisoned".to_string())?;
                let found = g.1.resources;
                let projected = found.saturating_mul(nwin as u64) / done.max(1) as u64;
                progress(found, projected.max(found));
            }
            Ok(())
        })?;

    let (conn, totals) = shared
        .into_inner()
        .map_err(|_| "database lock poisoned".to_string())?;
    let bounds = format!(
        "{},{},{},{}",
        BOUNDS[0], BOUNDS[1], BOUNDS[2], BOUNDS[3]
    );
    put_meta(&conn, "built_at", &unix_now())?;
    put_meta(&conn, "entities", &totals.entities.to_string())?;
    put_meta(&conn, "props", &totals.props.to_string())?;
    put_meta(&conn, "entities_stored", &totals.entities_kept.to_string())?;
    put_meta(&conn, "props_stored", &totals.props_kept.to_string())?;
    put_meta(&conn, "interior_points", &totals.interior_points.to_string())?;
    put_meta(&conn, "bounds", &bounds)?;
    conn.execute_batch("PRAGMA optimize;")
        .map_err(|e| format!("finalize database: {e}"))?;

    Ok(BuildReport {
        resources: totals.resources,
        entities: totals.entities,
        entities_kept: totals.entities_kept,
        props: totals.props,
        props_kept: totals.props_kept,
        interiors: totals.interiors,
        seconds: t0.elapsed().as_secs_f64(),
    })
}

/// Open a placement database for reading.
pub fn open(path: &Path) -> Result<Connection, String> {
    // Read-only: a build may still be writing, and a reader must never take a
    // write lock on a 300 MB index.
    Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open {}: {e}", path.display()))
}

fn meta_str(conn: &Connection, k: &str) -> Result<String, String> {
    conn.query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| {
        r.get::<_, String>(0)
    })
    .map_err(|e| format!("read meta {k}: {e}"))
}

fn meta_u64(conn: &Connection, k: &str) -> Result<u64, String> {
    meta_str(conn, k)?
        .parse::<u64>()
        .map_err(|e| format!("parse meta {k}: {e}"))
}

/// Metadata written by the last build.
pub fn meta(conn: &Connection) -> Result<MapMeta, String> {
    let raw = meta_str(conn, "bounds")?;
    let mut bounds = [0f32; 4];
    for (i, part) in raw.split(',').enumerate() {
        if i == bounds.len() {
            break;
        }
        bounds[i] = part
            .trim()
            .parse::<f32>()
            .map_err(|e| format!("parse meta bounds: {e}"))?;
    }
    Ok(MapMeta {
        built_at: meta_str(conn, "built_at")?,
        entities: meta_u64(conn, "entities")?,
        props: meta_u64(conn, "props")?,
        entities_stored: meta_u64(conn, "entities_stored")?,
        props_stored: meta_u64(conn, "props_stored")?,
        interior_points: meta_u64(conn, "interior_points")?,
        bounds,
    })
}

fn filter_sql(layer: u8, interior_only: bool) -> String {
    let mut sql = String::from(
        "x >= ?1 AND x <= ?2 AND y >= ?3 AND y <= ?4 AND layer = ?5",
    );
    if interior_only {
        sql.push_str(" AND interior = 1");
    }
    sql
}

/// layer: 0 = entities, 1 = props. bbox = (x0,y0,x1,y1). limit clamped to 40000.
pub fn query(
    conn: &Connection,
    bbox: [f32; 4],
    layer: u8,
    interior_only: bool,
    limit: usize,
) -> Result<Vec<Point>, String> {
    let limit = limit.min(MAX_LIMIT) as i64;
    let sql = format!(
        "SELECT x,y,z,arch,flags,layer,interior,res FROM points WHERE {} LIMIT ?6",
        filter_sql(layer, interior_only)
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare query: {e}"))?;
    let rows = stmt
        .query_map(
            params![
                bbox[0] as f64,
                bbox[2] as f64,
                bbox[1] as f64,
                bbox[3] as f64,
                layer as i64,
                limit
            ],
            |r| {
                Ok(Point {
                    x: r.get::<_, f64>(0)? as f32,
                    y: r.get::<_, f64>(1)? as f32,
                    z: r.get::<_, f64>(2)? as f32,
                    arch: r.get::<_, i64>(3)? as u32,
                    flags: r.get::<_, i64>(4)? as u32,
                    layer: r.get::<_, i64>(5)? as u8,
                    interior: r.get::<_, i64>(6)? as u8,
                    res: r.get::<_, i64>(7)? as u64,
                })
            },
        )
        .map_err(|e| format!("query points: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("read point: {e}"))?);
    }
    Ok(out)
}

/// Total rows in the same filter (for the "showing n of m" caption).
pub fn count(
    conn: &Connection,
    bbox: [f32; 4],
    layer: u8,
    interior_only: bool,
) -> Result<u64, String> {
    let sql = format!(
        "SELECT COUNT(*) FROM points WHERE {}",
        filter_sql(layer, interior_only)
    );
    conn.query_row(
        &sql,
        params![
            bbox[0] as f64,
            bbox[2] as f64,
            bbox[1] as f64,
            bbox[3] as f64,
            layer as i64
        ],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.max(0) as u64)
    .map_err(|e| format!("count points: {e}"))
}

/// Bin every row in the bbox into a `width` x `height` grid. One pass over the
/// rows, no LIMIT.
pub fn density(
    conn: &Connection,
    bbox: [f32; 4],
    layer: u8,
    interior_only: bool,
    width: usize,
    height: usize,
) -> Result<Density, String> {
    let width = width.clamp(16, 2048);
    let height = height.clamp(16, 2048);
    let sql = format!(
        "SELECT x,y FROM points WHERE {}",
        filter_sql(layer, interior_only)
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare density: {e}"))?;
    let mut rows = stmt
        .query(params![
            bbox[0] as f64,
            bbox[2] as f64,
            bbox[1] as f64,
            bbox[3] as f64,
            layer as i64
        ])
        .map_err(|e| format!("query density: {e}"))?;

    let mut cells = vec![0u32; width * height];
    let mut total = 0u64;
    let mut bins = 0u64;
    let mut max = 0u32;
    let sx = (bbox[2] - bbox[0]) as f64;
    let sy = (bbox[3] - bbox[1]) as f64;

    while let Some(row) = rows.next().map_err(|e| format!("read density row: {e}"))? {
        let x: f64 = row.get(0).map_err(|e| format!("read density x: {e}"))?;
        let y: f64 = row.get(1).map_err(|e| format!("read density y: {e}"))?;
        // Same bounds test as the SQL index prefilter in `query`.
        if x < bbox[0] as f64
            || x > bbox[2] as f64
            || y < bbox[1] as f64
            || y > bbox[3] as f64
        {
            continue;
        }
        let col = if sx > 0.0 {
            (((x - bbox[0] as f64) / sx) * width as f64) as usize
        } else {
            0
        };
        let row_at = if sy > 0.0 {
            (((y - bbox[1] as f64) / sy) * height as f64) as usize
        } else {
            0
        };
        let i = row_at.min(height - 1) * width + col.min(width - 1);
        if cells[i] == 0 {
            bins += 1;
        }
        cells[i] += 1;
        max = max.max(cells[i]);
        total += 1;
    }

    Ok(Density {
        total,
        bins,
        max,
        width,
        height,
        cells,
    })
}

/// Even stride over rowid so a sampled view covers the whole extent instead of
/// the first rows: step = max(1, total / limit).
pub fn query_stratified(
    conn: &Connection,
    bbox: [f32; 4],
    layer: u8,
    interior_only: bool,
    limit: usize,
) -> Result<Vec<Point>, String> {
    let limit = limit.max(1) as i64;
    let total = count(conn, bbox, layer, interior_only)?;
    let step = ((total / limit as u64).max(1)) as i64;
    let sql = format!(
        "SELECT x,y,z,arch,flags,layer,interior,res FROM points WHERE {} AND (rowid % ?6) = 0 LIMIT ?7",
        filter_sql(layer, interior_only)
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare stratified query: {e}"))?;
    let rows = stmt
        .query_map(
            params![
                bbox[0] as f64,
                bbox[2] as f64,
                bbox[1] as f64,
                bbox[3] as f64,
                layer as i64,
                step,
                limit
            ],
            |r| {
                Ok(Point {
                    x: r.get::<_, f64>(0)? as f32,
                    y: r.get::<_, f64>(1)? as f32,
                    z: r.get::<_, f64>(2)? as f32,
                    arch: r.get::<_, i64>(3)? as u32,
                    flags: r.get::<_, i64>(4)? as u32,
                    layer: r.get::<_, i64>(5)? as u8,
                    interior: r.get::<_, i64>(6)? as u8,
                    res: r.get::<_, i64>(7)? as u64,
                })
            },
        )
        .map_err(|e| format!("query stratified points: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("read point: {e}"))?);
    }
    Ok(out)
}

