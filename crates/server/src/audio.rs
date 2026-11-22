use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use sentinel_core::awc::Awc;
use sentinel_core::{MmapSource, Source};

use crate::ogg::OpusStream;

#[derive(Clone, serde::Serialize)]
pub struct Bank {
    pub offset: u64,
    pub size: u64,
    pub streams: u32,
    pub chunks: u32,
    pub samples: u32,
    pub rate: u16,
    pub codec: u8,
    pub channels: u16,
}

pub fn db_path(data_root: &Path) -> PathBuf {
    data_root.join("sentinel").join("audio.sqlite")
}

const SCAN_BLOCK: u64 = 8 * 1024 * 1024;
const INSERT_BATCH: usize = 10_000;

fn open_source(path: &Path) -> Result<Box<dyn Source>, String> {
    MmapSource::open(path, u64::MAX).map_err(|e| format!("mmap {}: {e}", path.display()))
}

/// Read the archive in 8 MiB windows, keeping a 3-byte lookahead so a signature
/// straddling two windows is still found. Hits are deduplicated by offset.
fn scan_adat(src: &dyn Source) -> Result<Vec<u64>, String> {
    let len = src.len();
    let mut hits = Vec::new();
    let mut pos = 0u64;
    let mut last: Option<u64> = None;

    while pos < len {
        let step = (len - pos).min(SCAN_BLOCK);
        let take = (len - pos).min(SCAN_BLOCK + 3) as usize;
        let view = src
            .window(pos, take)
            .map_err(|e| format!("read archive at {pos}: {e}"))?;
        let buf: &[u8] = &view;
        if buf.len() >= 4 {
            for i in 0..=buf.len() - 4 {
                if &buf[i..i + 4] == b"ADAT" {
                    let off = pos + i as u64;
                    if last.map_or(true, |prev| off > prev) {
                        hits.push(off);
                        last = Some(off);
                    }
                }
            }
        }
        pos += step;
    }
    Ok(hits)
}

fn channels_of(src: &dyn Source, awc: &Awc, off: u64) -> u16 {
    if let Some(fmt) = awc.format {
        if fmt.channels != 0 {
            return fmt.channels;
        }
    }
    let Some(chunk) = awc.payload_chunks().next() else {
        return 0;
    };
    let Ok(head) = src.read_at(off + chunk.offset, 64) else {
        return 0;
    };
    let packets = Awc::split_opus_packets(&head);
    if packets.is_empty() {
        0
    } else {
        Awc::channels(&packets) as u16
    }
}

fn validate(src: &dyn Source, off: u64) -> Option<Bank> {
    let awc = Awc::parse(src, off).ok()?;
    let (samples, rate, codec) = match awc.format {
        Some(f) => (f.samples, f.sample_rate, f.codec),
        None => (0, 0, 0),
    };
    Some(Bank {
        offset: off,
        size: awc.bank_size,
        streams: awc.streams,
        chunks: awc.chunks.len() as u32,
        samples,
        rate,
        codec,
        channels: channels_of(src, &awc, off),
    })
}

fn insert_batch(tx: &rusqlite::Transaction<'_>, rows: &[Bank], base_id: i64) -> Result<(), String> {
    let mut stmt = tx
        .prepare(
            "INSERT INTO banks(id, off, size, streams, chunks, samples, rate, codec, channels) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .map_err(|e| format!("prepare bank insert: {e}"))?;
    for (i, bank) in rows.iter().enumerate() {
        stmt.execute(rusqlite::params![
            base_id + i as i64 + 1,
            bank.offset as i64,
            bank.size as i64,
            bank.streams as i64,
            bank.chunks as i64,
            bank.samples as i64,
            bank.rate as i64,
            bank.codec as i64,
            bank.channels as i64,
        ])
        .map_err(|e| format!("insert bank at offset {}: {e}", bank.offset))?;
    }
    Ok(())
}

fn built_at() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs().to_string(),
        Err(e) => format!("-{}", e.duration().as_secs()),
    }
}

/// Scan audio.rpf for ADAT signatures, validate each one with `Awc::parse`, and
/// write the surviving banks to the SQLite catalog. `progress` receives
/// (validated signatures, total signatures).
pub fn build(
    audio_rpf: &Path,
    out: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<u32, String> {
    let source = open_source(audio_rpf)?;
    let src: &dyn Source = source.as_ref();
    let hits = scan_adat(src)?;
    let total = hits.len() as u64;

    let mut conn = Connection::open(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS banks; \
         DROP TABLE IF EXISTS meta; \
         CREATE TABLE banks(id INTEGER PRIMARY KEY, off INTEGER NOT NULL, size INTEGER NOT NULL, \
         streams INTEGER NOT NULL, chunks INTEGER NOT NULL, samples INTEGER NOT NULL, \
         rate INTEGER NOT NULL, codec INTEGER NOT NULL, channels INTEGER NOT NULL); \
         CREATE TABLE meta(k TEXT PRIMARY KEY, v TEXT NOT NULL);",
    )
    .map_err(|e| format!("create audio schema in {}: {e}", out.display()))?;

    let mut rows = Vec::with_capacity(INSERT_BATCH);
    let mut written = 0u32;
    {
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin audio transaction: {e}"))?;

        for (i, &off) in hits.iter().enumerate() {
            if let Some(bank) = validate(src, off) {
                rows.push(bank);
            }
            progress(i as u64 + 1, total);
            if rows.len() == INSERT_BATCH {
                insert_batch(&tx, &rows, written as i64)?;
                written += rows.len() as u32;
                rows.clear();
            }
        }
        if !rows.is_empty() {
            insert_batch(&tx, &rows, written as i64)?;
            written += rows.len() as u32;
        }

        tx.execute(
            "INSERT INTO meta(k, v) VALUES ('built_at', ?1)",
            rusqlite::params![built_at()],
        )
        .map_err(|e| format!("write built_at: {e}"))?;
        tx.execute(
            "INSERT INTO meta(k, v) VALUES ('banks', ?1)",
            rusqlite::params![written.to_string()],
        )
        .map_err(|e| format!("write banks meta: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit audio catalog: {e}"))?;
    }
    progress(total, total);
    Ok(written)
}

pub fn open(path: &Path) -> Result<Connection, String> {
    Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))
}

pub fn list(conn: &Connection, offset: u32, limit: u32) -> Result<(Vec<Bank>, u64), String> {
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM banks", [], |row| row.get(0))
        .map_err(|e| format!("count banks: {e}"))?;
    if limit == 0 {
        return Ok((Vec::new(), total as u64));
    }

    let mut stmt = conn
        .prepare(
            "SELECT off, size, streams, chunks, samples, rate, codec, channels \
             FROM banks ORDER BY off LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| format!("prepare bank list: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64, offset as i64], |row| {
            Ok(Bank {
                offset: row.get::<_, i64>(0)? as u64,
                size: row.get::<_, i64>(1)? as u64,
                streams: row.get::<_, i64>(2)? as u32,
                chunks: row.get::<_, i64>(3)? as u32,
                samples: row.get::<_, i64>(4)? as u32,
                rate: row.get::<_, i64>(5)? as u16,
                codec: row.get::<_, i64>(6)? as u8,
                channels: row.get::<_, i64>(7)? as u16,
            })
        })
        .map_err(|e| format!("query banks: {e}"))?;

    let mut banks = Vec::new();
    for row in rows {
        banks.push(row.map_err(|e| format!("read bank row: {e}"))?);
    }
    Ok((banks, total as u64))
}

/// Build a playable Ogg Opus file for one bank. With no format record, the
/// duration is estimated at one 20 ms frame per packet and the rate at 48 kHz.
/// RAGE banks carry no pre-skip value, so OpusHead uses zero and all granules
/// are counted from the first decoded sample.
pub fn ogg_for_bank(audio_rpf: &Path, bank_offset: u64) -> Result<(Vec<u8>, f32), String> {
    let source = open_source(audio_rpf)?;
    let src: &dyn Source = source.as_ref();
    let awc = Awc::parse(src, bank_offset).map_err(|e| format!("bank at {bank_offset}: {e}"))?;

    let mut blobs = Vec::new();
    for chunk in awc.payload_chunks() {
        let at = bank_offset + chunk.offset;
        let blob = src
            .window(at, chunk.size)
            .map_err(|e| format!("read payload at {at}: {e}"))?;
        if !blob.is_empty() {
            blobs.push(blob);
        }
    }
    let packets: Vec<&[u8]> = blobs
        .iter()
        .flat_map(|blob| Awc::split_opus_packets(&blob[..]))
        .collect();
    if packets.is_empty() {
        return Err(format!("bank at {bank_offset} has no Opus payload packets"));
    }

    let format = awc.format;
    let rate = match format {
        Some(f) if f.sample_rate != 0 => f.sample_rate as u32,
        _ => 48_000,
    };
    let duration = match format {
        Some(f) if f.samples != 0 && f.sample_rate != 0 => f.samples as f32 / f.sample_rate as f32,
        _ => packets.len() as f32 * 0.020,
    };
    let channels = match format {
        Some(f) if f.channels != 0 => f.channels as u8,
        _ => Awc::channels(&packets) as u8,
    };

    let stream = OpusStream {
        channels,
        sample_rate: rate,
        pre_skip: 0,
    };
    Ok((crate::ogg::mux_opus(&stream, &packets, duration), duration))
}
