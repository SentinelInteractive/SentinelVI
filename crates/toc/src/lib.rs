//! Data access for a SentinelVI installation.
//!
//! Everything here reads sidecar indexes that are derived from the installed
//! game data. Nothing in this crate knows about HTTP or about the UI.

use std::path::{Path, PathBuf};

/// Archive names as they appear in `vi_toc.db` and on disk.
pub const ARCHIVES: &[&str] = &[
    "anim_rpf",
    "appdata0_rpf",
    "audio_rpf",
    "common_rpf",
    "data_rpf",
    "levels_rpf",
    "movies_rpf",
    "navmeshes_rpf",
    "packs_rpf",
    "script_rpf",
    "shaders_rpf",
    "textures_rpf",
];

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
    pub game: PathBuf,
}

impl Paths {
    /// Resolve the data root from `SENTINEL_DATA`, then `SENTINEL_GAME`, then
    /// `D:\GTA6`. The game directory is the first child of the root that holds
    /// `levels.rpf`, falling back to the root itself.
    pub fn resolve() -> Result<Paths, String> {
        let root = std::env::var_os("SENTINEL_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"D:\GTA6"));
        if !root.is_dir() {
            return Err(format!(
                "data path {} does not exist; set SENTINEL_DATA",
                root.display()
            ));
        }
        if let Some(g) = std::env::var_os("SENTINEL_GAME") {
            let game = PathBuf::from(g);
            if game.join("levels.rpf").is_file() {
                return Ok(Paths { root, game });
            }
            return Err(format!(
                "SENTINEL_GAME={} does not contain levels.rpf",
                game.display()
            ));
        }
        let mut game = root.clone();
        if let Ok(rd) = std::fs::read_dir(&root) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() && p.join("levels.rpf").is_file() {
                    game = p;
                    break;
                }
            }
        }
        if !game.join("levels.rpf").is_file() {
            return Err(format!(
                "no levels.rpf under {}; set SENTINEL_GAME to the game directory",
                root.display()
            ));
        }
        Ok(Paths { root, game })
    }

    pub fn toc_db(&self) -> PathBuf {
        self.root.join("vi_toc.db")
    }
    pub fn bin_store(&self) -> PathBuf {
        self.root.join("bin_store")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("sentinel")
    }
    pub fn cache(&self, name: &str) -> PathBuf {
        self.cache_dir().join(name)
    }
    pub fn archive(&self, name: &str) -> PathBuf {
        // The index stores archive names as `levels_rpf`; the files are
        // `levels.rpf`.
        let base = name.strip_suffix("_rpf").unwrap_or(name);
        self.game.join(format!("{base}.rpf"))
    }

    /// `levels.rpf` is 76 GB; it only exists when the full game install is present.
    pub fn has_archive(&self, name: &str) -> bool {
        self.archive(name).is_file()
    }
}

/// One carved payload stream.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Stream {
    pub id: i64,
    pub archive: String,
    pub pack: String,
    pub stream_offset: i64,
    pub comp_size: i64,
    pub uncomp_size: i64,
    pub payload_kind: String,
    pub magic: String,
    pub nested_rpf: bool,
    pub folder: Option<String>,
    pub filename: Option<String>,
    pub extension: Option<String>,
    pub name: Option<String>,
    pub confidence: String,
    pub source: String,
    pub candidates: i64,
    pub sha1: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Stats {
    pub streams: i64,
    pub named: i64,
    pub with_extension: i64,
    pub folders: i64,
    pub unique_names: i64,
    pub nested: i64,
    pub packs: i64,
    pub pool: i64,
    pub by_confidence: Vec<(String, i64)>,
    pub by_archive: Vec<(String, i64)>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Pack {
    pub archive: String,
    pub pack: String,
    pub file: String,
    pub offset: i64,
    pub entries: i64,
    pub version: i64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Archive {
    pub archive: String,
    pub packs: i64,
    pub streams: i64,
    pub named: i64,
}

pub struct Toc {
    conn: rusqlite::Connection,
}

type Res<T> = Result<T, String>;

fn map_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const STREAM_COLS: &str = "id, archive, pack, stream_offset, comp_size, uncomp_size, payload_kind, \
     magic, nested_rpf, folder, filename, extension, name, confidence, source, candidates, sha1";

impl Toc {
    pub fn open(db: &Path) -> Res<Toc> {
        let conn = rusqlite::Connection::open_with_flags(
            db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(map_err)?;
        Ok(Toc { conn })
    }

    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Stream> {
        Ok(Stream {
            id: r.get(0)?,
            archive: r.get(1)?,
            pack: r.get(2)?,
            stream_offset: r.get(3)?,
            comp_size: r.get(4)?,
            uncomp_size: r.get(5)?,
            payload_kind: r.get(6)?,
            magic: r.get(7)?,
            nested_rpf: r.get::<_, i64>(8)? != 0,
            folder: r.get(9)?,
            filename: r.get(10)?,
            extension: r.get(11)?,
            name: r.get(12)?,
            confidence: r.get(13)?,
            source: r.get(14)?,
            candidates: r.get(15)?,
            sha1: r.get(16)?,
        })
    }

    pub fn stats(&self) -> Res<Stats> {
        let one = |sql: &str| -> i64 {
            self.conn
                .query_row(sql, [], |r| r.get::<_, i64>(0))
                .unwrap_or(0)
        };
        let mut by_confidence: Vec<(String, i64)> = Vec::new();
        {
            let mut st = self
                .conn
                .prepare("SELECT confidence, count(*) FROM streams GROUP BY 1 ORDER BY 2 DESC")
                .map_err(map_err)?;
            let rows = st
                .query_map([], |r| {
                    Ok::<_, rusqlite::Error>((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(map_err)?;
            by_confidence = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)?;
        }
        let mut by_archive: Vec<(String, i64)> = Vec::new();
        {
            let mut st = self
                .conn
                .prepare("SELECT archive, count(*) FROM streams GROUP BY 1 ORDER BY 2 DESC")
                .map_err(map_err)?;
            let rows = st
                .query_map([], |r| {
                    Ok::<_, rusqlite::Error>((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(map_err)?;
            by_archive = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)?;
        }
        Ok(Stats {
            streams: one("SELECT count(*) FROM streams"),
            named: one("SELECT count(*) FROM streams WHERE name IS NOT NULL"),
            with_extension: one("SELECT count(*) FROM streams WHERE extension IS NOT NULL"),
            folders: one("SELECT count(DISTINCT folder) FROM streams WHERE folder IS NOT NULL"),
            unique_names: one("SELECT count(DISTINCT name) FROM streams WHERE name IS NOT NULL"),
            nested: one("SELECT count(*) FROM streams WHERE nested_rpf=1"),
            packs: one("SELECT count(*) FROM packs"),
            pool: one("SELECT count(*) FROM pool"),
            by_confidence,
            by_archive,
        })
    }

    pub fn archives(&self) -> Res<Vec<Archive>> {
        use std::collections::HashMap;
        let mut map: HashMap<String, Archive> = HashMap::new();
        {
            let mut st = self
                .conn
                .prepare("SELECT archive, count(*) FROM packs GROUP BY archive")
                .map_err(map_err)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map_err(map_err)?;
            for row in rows {
                let (a, n) = row.map_err(map_err)?;
                map.insert(
                    a.clone(),
                    Archive {
                        archive: a,
                        packs: n,
                        streams: 0,
                        named: 0,
                    },
                );
            }
        }
        {
            let mut st = self
                .conn
                .prepare(
                    "SELECT archive, count(*), sum(CASE WHEN name IS NULL THEN 0 ELSE 1 END)
                     FROM streams GROUP BY archive",
                )
                .map_err(map_err)?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    ))
                })
                .map_err(map_err)?;
            for row in rows {
                let (a, streams, named) = row.map_err(map_err)?;
                let entry = map.entry(a.clone()).or_insert(Archive {
                    archive: a,
                    packs: 0,
                    streams: 0,
                    named: 0,
                });
                entry.streams = streams;
                entry.named = named;
            }
        }
        let mut out: Vec<Archive> = map.into_values().collect();
        out.sort_by(|a, b| b.streams.cmp(&a.streams));
        Ok(out)
    }

    pub fn packs(&self, archive: &str, limit: u32) -> Res<Vec<Pack>> {
        let mut st = self
            .conn
            .prepare(
                "SELECT archive, pack, file, offset, entries, version FROM packs
                 WHERE archive = ?1 ORDER BY offset LIMIT ?2",
            )
            .map_err(map_err)?;
        let rows = st
            .query_map(rusqlite::params![archive, limit], |r| {
                Ok(Pack {
                    archive: r.get(0)?,
                    pack: r.get(1)?,
                    file: r.get(2)?,
                    offset: r.get(3)?,
                    entries: r.get(4)?,
                    version: r.get(5)?,
                })
            })
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    }

    pub fn streams_in_pack(&self, pack: &str, offset: u32, limit: u32) -> Res<(Vec<Stream>, i64)> {
        let total: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM streams WHERE pack = ?1",
                [pack],
                |r| r.get(0),
            )
            .map_err(map_err)?;
        let sql = format!(
            "SELECT {STREAM_COLS} FROM streams WHERE pack = ?1 ORDER BY stream_offset LIMIT ?2 OFFSET ?3"
        );
        let mut st = self.conn.prepare(&sql).map_err(map_err)?;
        let rows = st
            .query_map(rusqlite::params![pack, limit, offset], |r| Toc::row(r))
            .map_err(map_err)?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)?;
        Ok((out, total))
    }

    /// Prefix matches first, then substring matches. Confidence breaks ties, so
    /// an exact name never sorts below a weak guess.
    pub fn search(&self, q: &str, limit: u32) -> Res<(Vec<Stream>, i64)> {
        let raw = q.trim();
        if raw.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let cleaned = raw.replace(['%', '_'], "");
        let like_prefix = format!("{}%", cleaned);
        let like_any = format!("%{}%", cleaned);
        let total: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM streams WHERE name LIKE ?1",
                [&like_any],
                |r| r.get(0),
            )
            .map_err(map_err)?;
        let sql = format!(
            "SELECT {STREAM_COLS}, length(name) AS l FROM streams
             WHERE name LIKE ?2
             ORDER BY CASE confidence WHEN 'exact' THEN 0 WHEN 'path_prefix' THEN 1
                                     WHEN 'identifier' THEN 2 ELSE 3 END,
                      length(name), name LIMIT ?1"
        );
        let mut seen = Vec::new();
        {
            let mut st = self.conn.prepare(&sql).map_err(map_err)?;
            let rows = st
                .query_map(rusqlite::params![limit, like_prefix], |r| Toc::row(r))
                .map_err(map_err)?;
            for row in rows {
                seen.push(row.map_err(map_err)?);
            }
        }
        if (seen.len() as u32) < limit {
            let mut st = self.conn.prepare(&sql).map_err(map_err)?;
            let rows = st
                .query_map(rusqlite::params![limit, like_any], |r| Toc::row(r))
                .map_err(map_err)?;
            for row in rows {
                let s = row.map_err(map_err)?;
                if !seen.iter().any(|x| x.id == s.id) {
                    seen.push(s);
                    if seen.len() as u32 >= limit {
                        break;
                    }
                }
            }
        }
        Ok((seen, total))
    }

    pub fn stream(&self, id: i64) -> Res<Option<Stream>> {
        let sql = format!("SELECT {STREAM_COLS} FROM streams WHERE id = ?1");
        let mut st = self.conn.prepare(&sql).map_err(map_err)?;
        let mut rows = st.query_map([id], |r| Toc::row(r)).map_err(map_err)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(map_err)?)),
            None => Ok(None),
        }
    }

/// Clip names carried by the animation archive. The index names a clip when a
/// payload holds a `.clip` string or path prefix; 13,199 streams carry one.
pub fn clip_names(&self, q: Option<&str>, offset: u32, limit: u32) -> Res<(Vec<Stream>, i64)> {
    let like = match q {
        Some(text) if !text.is_empty() => format!("%{}%", text.replace(['%', '_'], "")),
        _ => "%.clip".to_string(),
    };
    let total: i64 = self
        .conn
        .query_row(
            "SELECT count(*) FROM streams
             WHERE archive = 'anim_rpf' AND name LIKE ?1 AND name LIKE '%.clip'",
            [&like],
            |r| r.get(0),
        )
        .map_err(map_err)?;
    let sql = format!(
        "SELECT {STREAM_COLS} FROM streams
         WHERE archive = 'anim_rpf' AND name LIKE ?1 AND name LIKE '%.clip'
         ORDER BY name LIMIT ?2 OFFSET ?3"
    );
    let mut st = self.conn.prepare(&sql).map_err(map_err)?;
    let rows = st
        .query_map(rusqlite::params![like, limit, offset], |r| Toc::row(r))
        .map_err(map_err)?;
    let out = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)?;
    Ok((out, total))
}

    pub fn folder_of(&self, folder: &str) -> Res<Vec<Stream>> {        let sql = format!(
            "SELECT {STREAM_COLS} FROM streams WHERE folder = ?1 ORDER BY name LIMIT 200"
        );
        let mut st = self.conn.prepare(&sql).map_err(map_err)?;
        let rows = st
            .query_map([folder], |r| Toc::row(r))
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    }
}

/// Resolve and decompress one carved payload from the bin store.
pub struct BinStore {
    conn: rusqlite::Connection,
    dir: PathBuf,
}

impl BinStore {
    pub fn open(dir: &Path) -> Res<BinStore> {
        let db = dir.join("index.sqlite");
        let conn = rusqlite::Connection::open_with_flags(
            &db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("{}: {e}", db.display()))?;
        Ok(BinStore {
            conn,
            dir: dir.to_path_buf(),
        })
    }

    /// `(spool file, offset, compressed length)` for one archive stream offset.
    pub fn locate(&self, archive: &str, stream_offset: i64) -> Res<(String, i64, i64)> {
        self.conn
            .query_row(
                "SELECT spool_file, spool_offset, payload_len FROM streams
                 WHERE archive = ?1 AND stream_offset = ?2",
                rusqlite::params![archive, stream_offset],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| format!("stream not in bin store: {e}"))
    }

    pub fn payload(&self, archive: &str, stream_offset: i64) -> Res<Vec<u8>> {
        use std::io::Read;
        let (spool, off, len) = self.locate(archive, stream_offset)?;
        let path = self.dir.join(&spool);
        let mut f = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        use std::io::Seek;
        f.seek(std::io::SeekFrom::Start(off as u64))
            .map_err(map_err)?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).map_err(map_err)?;
        let mut dec = zstd::stream::read::Decoder::new(&buf[..]).map_err(map_err)?;
        let mut out = Vec::new();
        dec.read_to_end(&mut out).map_err(map_err)?;
        Ok(out)
    }
}

/// File name used when a stream is exported.
pub fn export_name(s: &Stream) -> String {
    let base = s
        .name
        .clone()
        .or_else(|| s.filename.clone())
        .unwrap_or_else(|| format!("stream_{}", s.id));
    let mut clean: String = base
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    clean = clean.trim_matches('_').to_string();
    if clean.is_empty() {
        clean = format!("stream_{}", s.id);
    }
    match &s.extension {
        Some(ext) if !ext.is_empty() && !clean.ends_with(&format!(".{ext}")) => {
            format!("{clean}.{ext}")
        }
        _ => clean,
    }
}

/// `sentinel info` needs a compact summary without opening the archive tree.
#[derive(serde::Serialize)]
pub struct Info {
    pub product: &'static str,
    pub version: &'static str,
    pub data_root: String,
    pub game_dir: String,
    pub archives: Vec<(String, String, bool)>,
}

pub fn info(paths: &Paths) -> Info {
    let archives = ARCHIVES
        .iter()
        .map(|a| {
            let p = paths.archive(a);
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            (
                a.to_string(),
                crate::human_size(size as i64),
                p.is_file(),
            )
        })
        .collect();
    Info {
        product: "SentinelVI",
        version: env!("CARGO_PKG_VERSION"),
        data_root: paths.root.display().to_string(),
        game_dir: paths.game.display().to_string(),
        archives,
    }
}

/// Sizes are shown as exact byte counts in the data views, and humanised only
/// where a column heading is already in bytes.
pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes.max(0) as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}
