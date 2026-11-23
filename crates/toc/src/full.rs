use std::io::ErrorKind;
use std::path::Path;

use rusqlite::{params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension, Row};
use serde::Serialize;

use crate::{map_err, Paths, Res};

const SLOT_COLUMNS: &str = "rowid, root, archive_offset, slot, data_offset_absolute, \
    data_offset_relative, stored_bytes, logical_bytes, kind, hash32, hash64, \
    path, name_origin, signature_status, header_offset";
const MAX_PAGE: u32 = 200;

pub struct FullToc {
    conn: Connection,
}

#[derive(Serialize)]
pub struct Root {
    pub root: String,
    pub slot_count: i64,
    pub header_count: i64,
}

#[derive(Serialize)]
pub struct Header {
    pub id: i64,
    pub root: String,
    pub archive_offset: i64,
    pub header_offset: i64,
    pub slot_count: i64,
    pub signature_status: String,
}

#[derive(Serialize)]
pub struct Slot {
    pub id: i64,
    pub root: String,
    pub archive_offset: i64,
    pub slot: i64,
    pub data_offset_absolute: i64,
    pub data_offset_relative: i64,
    pub stored_bytes: i64,
    pub logical_bytes: i64,
    pub kind: String,
    pub hash32: Option<String>,
    pub hash64: Option<String>,
    pub path: Option<String>,
    pub name_origin: String,
    pub signature_status: String,
    pub header_offset: i64,
    pub display_name: String,
}

#[derive(Serialize)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub next: Option<i64>,
}

pub enum SlotSearch<'a> {
    Path(&'a str),
    Hash32(u32),
    Hash64(u64),
    Root(&'a str),
    Slot(i64),
}

#[derive(Default)]
pub struct SlotFilter<'a> {
    pub root: Option<&'a str>,
    pub header_offset: Option<i64>,
    pub search: Option<SlotSearch<'a>>,
}

impl FullToc {
    pub fn open(path: &Path) -> Res<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(map_err)?;
        // SQLite can defer file validation until the first statement.
        conn.prepare("SELECT root, entries, headers FROM roots LIMIT 0")
            .map_err(map_err)?;
        Ok(Self { conn })
    }

    pub fn open_if_present(paths: &Paths) -> Res<Option<Self>> {
        let path = paths.full_toc_db();
        match std::fs::symlink_metadata(&path) {
            Ok(_) => Self::open(&path).map(Some),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    pub fn roots(&self) -> Res<Vec<Root>> {
        self.query(
            "SELECT root, entries, headers FROM roots ORDER BY root",
            [],
            |r| {
                Ok(Root {
                    root: r.get(0)?,
                    slot_count: r.get(1)?,
                    header_count: r.get(2)?,
                })
            },
        )
    }

    pub fn headers(&self, root: &str, after: Option<i64>, limit: u32) -> Res<Page<Header>> {
        let limit = limit.clamp(1, MAX_PAGE);
        let rows = self.query(
            "SELECT h.root, h.header_offset, h.signature_status, \
                (SELECT count(*) FROM slots s WHERE s.root = h.root \
                 AND s.archive_offset = h.header_offset) \
             FROM headers h WHERE h.root = ? AND h.header_offset > ? \
             ORDER BY h.header_offset LIMIT ?",
            rusqlite::params![root, after.unwrap_or(-1), limit + 1],
            |r| {
                let offset = r.get(1)?;
                Ok(Header {
                    id: offset,
                    root: r.get(0)?,
                    archive_offset: offset,
                    header_offset: offset,
                    signature_status: r.get(2)?,
                    slot_count: r.get(3)?,
                })
            },
        )?;
        Ok(page(rows, limit, |header| header.header_offset))
    }

    pub fn slots(
        &self,
        filter: &SlotFilter<'_>,
        after: Option<i64>,
        limit: u32,
    ) -> Res<Page<Slot>> {
        let limit = limit.clamp(1, MAX_PAGE);
        let mut sql = format!("SELECT {SLOT_COLUMNS} FROM slots WHERE rowid > ?");
        let mut args = vec![Value::Integer(after.unwrap_or(0))];
        let mut predicate = |clause: &str, value: Value| {
            sql.push_str(" AND ");
            sql.push_str(clause);
            args.push(value);
        };
        if let Some(root) = filter.root {
            predicate("root = ?", root.to_owned().into());
        }
        if let Some(offset) = filter.header_offset {
            // The schema equates these offsets; (root, archive_offset) is indexed.
            predicate("archive_offset = ?", offset.into());
        }
        match filter.search {
            Some(SlotSearch::Path(path)) => predicate(
                "path IS NOT NULL AND instr(lower(path), lower(?)) > 0 AND name_origin = 'DictionaryMatch'",
                path.to_owned().into(),
            ),
            Some(SlotSearch::Hash32(hash)) => predicate("hash32 = ?", i64::from(hash).into()),
            Some(SlotSearch::Hash64(hash)) => predicate("hash64 = ?", (hash as i64).into()),
            Some(SlotSearch::Root(root)) => predicate("root = ?", root.to_owned().into()),
            Some(SlotSearch::Slot(slot)) => predicate("slot = ?", slot.into()),
            None => {}
        }
        sql.push_str(" ORDER BY rowid LIMIT ?");
        args.push(i64::from(limit + 1).into());
        let rows = self.query(&sql, params_from_iter(args), slot_row)?;
        Ok(page(rows, limit, |slot| slot.id))
    }

    pub fn slot(&self, id: i64) -> Res<Option<Slot>> {
        self.conn
            .query_row(
                &format!("SELECT {SLOT_COLUMNS} FROM slots WHERE rowid = ?"),
                [id],
                slot_row,
            )
            .optional()
            .map_err(map_err)
    }

    fn query<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        row: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Res<Vec<T>> {
        let mut stmt = self.conn.prepare(sql).map_err(map_err)?;
        let rows = stmt.query_map(params, row).map_err(map_err)?;
        rows.collect::<rusqlite::Result<_>>().map_err(map_err)
    }
}

fn page<T>(mut rows: Vec<T>, limit: u32, cursor: impl FnOnce(&T) -> i64) -> Page<T> {
    let next = (rows.len() > limit as usize).then(|| {
        rows.pop();
        cursor(&rows[limit as usize - 1])
    });
    Page { rows, next }
}

fn slot_row(r: &Row<'_>) -> rusqlite::Result<Slot> {
    let slot = r.get(3)?;
    let name_origin: String = r.get(12)?;
    let path: Option<String> = if name_origin == "DictionaryMatch" {
        r.get(11)?
    } else {
        None
    };
    let hash64 = r
        .get::<_, Option<i64>>(10)?
        .map(|n| format!("{:016x}", n as u64));
    let display_name = path
        .clone()
        .or_else(|| hash64.as_ref().map(|hash| format!("Hash {hash}")))
        .unwrap_or_else(|| format!("Slot {slot}"));
    Ok(Slot {
        id: r.get(0)?,
        root: r.get(1)?,
        archive_offset: r.get(2)?,
        slot,
        data_offset_absolute: r.get(4)?,
        data_offset_relative: r.get(5)?,
        stored_bytes: r.get(6)?,
        logical_bytes: r.get(7)?,
        kind: r.get(8)?,
        hash32: r.get::<_, Option<u32>>(9)?.map(|n| format!("{n:08x}")),
        hash64,
        path,
        name_origin,
        signature_status: r.get(13)?,
        header_offset: r.get(14)?,
        display_name,
    })
}
