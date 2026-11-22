use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use memmap2::Mmap;
use rayon::prelude::*;
use rusqlite::{params, Connection};
use sentinel_core::read::{is_zlib_head, Le};
use sentinel_core::zlib::inflate_stream;

const GRID: usize = 0x1000;
const CHUNK: usize = 256;
const MAX_PAGE: usize = 16 << 20;
const MAX_PAGES: usize = 8;
const WIN: usize = 1 << 20;
const REC_OFF: usize = 0x740;
const REC_STRIDE: usize = 0x70;
const MAX_SLOTS: usize = 4096;
const NAME_HINT: usize = 0x0B18;
const MAX_NAME: usize = 512;

pub fn db_path(data_root: &Path) -> PathBuf {
    data_root.join("sentinel").join("anim.sqlite")
}

#[derive(Default, Clone, Copy, serde::Serialize)]
pub struct BuildReport {
    pub resources: u64,
    pub clip_resources: u64,
    pub records: u64,
    pub named: u64,
    pub seconds: f64,
}

#[derive(Clone, serde::Serialize)]
pub struct Clip {
    pub id: i64,
    pub name: Option<String>,
    pub frames: u32,
    pub duration: f32,
    pub fps: Option<f32>,
    pub keyframes: u32,
    pub res: u64,
}

struct RawClip {
    frames: u32,
    duration: f32,
    fps: Option<f32>,
    keyframes: u32,
}

struct ScanOut {
    clips: Vec<Clip>,
}

fn e(err: impl std::fmt::Display) -> String {
    err.to_string()
}

fn looks_root(v: &[u8]) -> bool {
    let le = Le(v);
    v.len() >= 0x20 && le.u64(0) == Some(0) && v.get(0x10..0x15) == Some(b"0DRPz".as_slice())
}

fn next_page(mm: &[u8], cur: usize) -> Option<(Vec<u8>, usize)> {
    let mut p = cur & !15usize;
    let end = cur.saturating_add(WIN).min(mm.len().saturating_sub(4));
    while p < end {
        if is_zlib_head(&mm[p..]) {
            if let Some(h) = inflate_stream(&mm[p..], MAX_PAGE) {
                if h.data.is_empty() || looks_root(&h.data) {
                    return None;
                }
                return Some((h.data, p + h.consumed));
            }
        }
        p += 16;
    }
    None
}

fn is_name_byte(b: u8) -> bool {
    (0x20..=0x7e).contains(&b)
}

fn collect_names(v: &[u8]) -> Vec<String> {
    let hay = &v[..v.len().min(1 << 20)];
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut i = 0usize;
    while i < hay.len() {
        let suffix: &[u8] = if hay[i..].starts_with(b".clip") {
            b".clip"
        } else if hay[i..].starts_with(b".anim") {
            b".anim"
        } else {
            i += 1;
            continue;
        };
        let end = i + suffix.len();
        let mut start = i;
        let floor = start.saturating_sub(MAX_NAME);
        while start > floor && is_name_byte(hay[start - 1]) {
            start -= 1;
        }
        if start < i {
            let mut raw = &hay[start..end];
            if let Some(&l) = raw.first() {
                if l >= 0x20 && (l as usize == raw.len() - 1 || l as usize == raw.len()) {
                    raw = &raw[1..];
                }
            }
            if let Ok(s) = std::str::from_utf8(raw) {
                found.push((start, s.to_owned()));
            }
        }
        i = end;
    }
    found.sort_by_key(|(p, _)| *p);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, s) in found {
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    out
}

fn derive_fps(frames: u32, duration: f32) -> Option<f32> {
    if !(duration > 0.0) {
        return None;
    }
    let raw = frames as f64 / duration as f64;
    let rounded = (raw * 100.0).round() / 100.0;
    (20.0..=140.0).contains(&rounded).then(|| rounded as f32)
}

/// Name pairing is positional and unverified: for one resource, collect the distinct
/// `.clip`/`.anim` name strings in the order they appear; if that count equals the
/// number of accepted records in the resource, pair by order. Otherwise store every
/// record with a NULL name. This is a rule for ordering, not a verified name-to-record
/// mapping.
fn pair(recs: Vec<RawClip>, names: &[String], res: u64) -> Vec<Clip> {
    let matched = !recs.is_empty() && names.len() == recs.len();
    recs.into_iter()
        .enumerate()
        .map(|(i, r)| Clip {
            id: 0,
            name: if matched {
                Some(names[i].clone())
            } else {
                None
            },
            frames: r.frames,
            duration: r.duration,
            fps: r.fps,
            keyframes: r.keyframes,
            res,
        })
        .collect()
}

fn scan_one(mm: &[u8], off: usize) -> Option<ScanOut> {
    let tail = mm.get(off..)?;
    if !is_zlib_head(tail) {
        return None;
    }
    let first = inflate_stream(tail, MAX_PAGE)?;
    if !looks_root(&first.data) {
        return None;
    }
    let mut v = first.data;
    let mut cur = off + first.consumed;
    let mut pages = 1usize;
    let need = (REC_OFF + REC_STRIDE * 2).max(NAME_HINT + 0x40);
    while v.len() < need && pages < MAX_PAGES {
        match next_page(mm, cur) {
            Some((d, next)) => {
                v.extend_from_slice(&d);
                cur = next;
                pages += 1;
            }
            None => break,
        }
    }
    let end = v.len().min(REC_OFF + MAX_SLOTS * REC_STRIDE);
    let mut recs = Vec::new();
    let mut o = REC_OFF;
    while o + REC_STRIDE <= end && recs.len() < MAX_SLOTS {
        let le = Le(&v[o..o + REC_STRIDE]);
        let frames = le.u16(6).unwrap_or(0) as u32;
        let duration = le.f32(8).unwrap_or(0.0);
        if (0.05..=600.0).contains(&duration) && (2..=20000).contains(&frames) {
            recs.push(RawClip {
                frames,
                duration,
                fps: derive_fps(frames, duration),
                keyframes: le.u32(0).unwrap_or(0),
            });
        }
        o += REC_STRIDE;
    }
    Some(ScanOut {
        clips: pair(recs, &collect_names(&v), off as u64),
    })
}

pub fn build(
    anim_rpf: &Path,
    out: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<BuildReport, String> {
    let t0 = std::time::Instant::now();
    let file = File::open(anim_rpf).map_err(e)?;
    let mm = unsafe { Mmap::map(&file) }.map_err(e)?;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(e)?;
    }
    let mut conn = Connection::open(out).map_err(e)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS clips(id INTEGER PRIMARY KEY, name TEXT, frames INTEGER NOT NULL, duration REAL NOT NULL, fps REAL, keyframes INTEGER NOT NULL, res INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS ix_name ON clips(name);
         CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);",
    )
    .map_err(e)?;
    conn.execute("DELETE FROM clips", []).map_err(e)?;

    let ngrid = (mm.len() + GRID - 1) / GRID;
    let mut clips: Vec<Clip> = Vec::new();
    let mut resources = 0u64;
    let mut clip_resources = 0u64;
    let mut c = 0usize;
    while c < ngrid {
        let end = (c + CHUNK).min(ngrid);
        let part: Vec<ScanOut> = (c..end)
            .into_par_iter()
            .filter_map(|i| scan_one(&mm, i * GRID + 0x10))
            .collect();
        for p in part {
            resources += 1;
            if !p.clips.is_empty() {
                clip_resources += 1;
            }
            clips.extend(p.clips);
        }
        c = end;
        (progress)(c as u64, ngrid as u64);
    }

    let mut named = 0u64;
    for (i, clip) in clips.iter_mut().enumerate() {
        clip.id = (i + 1) as i64;
        if clip.name.is_some() {
            named += 1;
        }
        let _ = clip.duration;
    }
    let records = named;
    let seconds = t0.elapsed().as_secs_f64();
    let tx = conn.transaction().map_err(e)?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO clips(id,name,frames,duration,fps,keyframes,res)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(e)?;
        for clip in clips.iter().filter(|c| c.name.is_some()) {
            stmt.execute(params![
                clip.id,
                clip.name,
                clip.frames as i64,
                clip.duration as f64,
                clip.fps.map(|v| v as f64),
                clip.keyframes as i64,
                clip.res as i64,
            ])
            .map_err(e)?;
        }
    }
    let built_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string();
    for (k, v) in [
        ("built_at", built_at),
        ("records", records.to_string()),
        ("named", named.to_string()),
        ("resources", resources.to_string()),
        ("clip_resources", clip_resources.to_string()),
    ] {
        tx.execute(
            "INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            params![k, v],
        )
        .map_err(e)?;
    }
    tx.commit().map_err(e)?;
    (progress)(ngrid as u64, ngrid as u64);

    Ok(BuildReport {
        resources,
        clip_resources,
        records,
        named,
        seconds,
    })
}

pub fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(e)?;
    conn.execute_batch("PRAGMA busy_timeout=5000;").map_err(e)?;
    Ok(conn)
}

pub fn list(
    conn: &Connection,
    q: Option<&str>,
    offset: u32,
    limit: u32,
) -> Result<(Vec<Clip>, u64), String> {
    let pat = q.map(|s| format!("%{s}%"));
    let total = match pat.as_deref() {
        Some(p) => conn
            .query_row("SELECT COUNT(*) FROM clips WHERE name LIKE ?1", [p], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(e)?,
        None => conn
            .query_row("SELECT COUNT(*) FROM clips", [], |r| r.get::<_, i64>(0))
            .map_err(e)?,
    } as u64;

    let sql = match pat.as_deref() {
        Some(_) => "SELECT id,name,frames,duration,fps,keyframes,res FROM clips \
                    WHERE name LIKE ?1 ORDER BY id LIMIT ?2 OFFSET ?3",
        None => "SELECT id,name,frames,duration,fps,keyframes,res FROM clips \
                 ORDER BY id LIMIT ?1 OFFSET ?2",
    };
    let mut stmt = conn.prepare(sql).map_err(e)?;
    let mut rows = match pat.as_deref() {
        Some(p) => stmt
            .query(params![p, limit as i64, offset as i64])
            .map_err(e)?,
        None => stmt.query(params![limit as i64, offset as i64]).map_err(e)?,
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(e)? {
        out.push(Clip {
            id: row.get(0).map_err(e)?,
            name: row.get(1).map_err(e)?,
            frames: row.get::<_, i64>(2).map_err(e)? as u32,
            duration: row.get::<_, f64>(3).map_err(e)? as f32,
            fps: row.get::<_, Option<f64>>(4).map_err(e)?.map(|v| v as f32),
            keyframes: row.get::<_, i64>(5).map_err(e)? as u32,
            res: row.get::<_, i64>(6).map_err(e)? as u64,
        });
    }
    Ok((out, total))
}

