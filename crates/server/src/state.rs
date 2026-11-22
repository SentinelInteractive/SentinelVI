//! Shared state for the local server: paths, job queue, parsed texture headers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sentinel_toc::Paths;

#[derive(Clone)]
pub struct Config {
    pub paths: Paths,
    pub web: PathBuf,
    pub port: u16,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Job {
    pub id: u64,
    pub kind: String,
    pub label: String,
    pub state: String,
    pub started: u64,
    pub finished: Option<u64>,
    pub output: Option<String>,
    pub message: Option<String>,
    pub progress: Option<(u64, u64)>,
}

#[derive(Clone)]
pub struct RasterMeta {
    pub total: u64,
    pub bins: u64,
    pub max: u32,
    pub width: usize,
    pub height: usize,
}

pub struct RasterCacheMeta {
    pub entries: HashMap<String, RasterMeta>,
    pub order: Vec<String>,
}

pub struct App {
    pub cfg: Config,
    pub jobs: Mutex<Vec<Job>>,
    next_id: AtomicU64,
    /// One index build at a time: they all read tens of gigabytes from disk.
    pub busy: AtomicBool,
    /// Parsed `.ttpf` headers, keyed by path relative to the game directory.
    pub ttpfs: Mutex<HashMap<String, Arc<sentinel_core::tpfd::Rds5>>>,
    /// Last eight density rasters, keyed by `bbox|layer|interior|w|h`.
    pub map_rasters: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    pub map_raster_meta: Mutex<RasterCacheMeta>,
    /// Decoded world heightmap: PNG bytes plus its world bounds.
    pub terrain: Mutex<Option<Arc<(Vec<u8>, [f32; 4])>>>,
}

const RASTER_CACHE_MAX: usize = 8;

impl App {
    pub fn new(cfg: Config) -> Arc<App> {
        Arc::new(App {
            cfg,
            jobs: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
            busy: AtomicBool::new(false),
            ttpfs: Mutex::new(HashMap::new()),
            map_rasters: Mutex::new(HashMap::new()),
            map_raster_meta: Mutex::new(RasterCacheMeta {
                entries: HashMap::new(),
                order: Vec::new(),
            }),
            terrain: Mutex::new(None),
        })
    }

    pub fn raster_cached(&self, key: &str) -> Option<(Arc<Vec<u8>>, RasterMeta)> {
        let rasters = self.map_rasters.lock().ok()?;
        let mut meta = self.map_raster_meta.lock().ok()?;
        let png = rasters.get(key)?.clone();
        let stats = meta.entries.get(key)?.clone();
        if let Some(i) = meta.order.iter().position(|k| k == key) {
            let k = meta.order.remove(i);
            meta.order.push(k);
        }
        Some((png, stats))
    }

    pub fn raster_insert(&self, key: String, png: Vec<u8>, stats: RasterMeta) {
        let (Ok(mut rasters), Ok(mut meta)) =
            (self.map_rasters.lock(), self.map_raster_meta.lock())
        else {
            return;
        };
        if let Some(i) = meta.order.iter().position(|k| k == &key) {
            meta.order.remove(i);
        }
        if !rasters.contains_key(&key) && rasters.len() >= RASTER_CACHE_MAX {
            if let Some(old) = meta.order.first().cloned() {
                rasters.remove(&old);
                meta.entries.remove(&old);
                meta.order.remove(0);
            }
        }
        rasters.insert(key.clone(), Arc::new(png));
        meta.entries.insert(key.clone(), stats);
        meta.order.push(key);
    }

    pub fn jobs_snapshot(&self) -> Vec<Job> {
        let mut v = self.jobs.lock().unwrap().clone();
        v.sort_by_key(|j| std::cmp::Reverse(j.id));
        v
    }

    pub fn push_job(&self, kind: &str, label: &str) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let job = Job {
            id,
            kind: kind.to_string(),
            label: label.to_string(),
            state: "queued".into(),
            started: now(),
            finished: None,
            output: None,
            message: None,
            progress: None,
        };
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(job);
        if jobs.len() > 200 {
            jobs.remove(0);
        }
        id
    }

    pub fn update_job<F: FnOnce(&mut Job)>(&self, id: u64, f: F) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
            f(j);
        }
    }

    pub fn finish_job(&self, id: u64, state: &str, output: Option<String>, message: Option<String>) {
        self.update_job(id, |j| {
            j.state = state.into();
            j.finished = Some(now());
            j.output = output;
            j.message = message;
            j.progress = None;
        });
    }
}

/// Seconds since the Unix epoch. The UI formats these locally; no time zone
/// assumption is made here.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Texture database locations, relative to the game directory. The `.ttpf`
/// files sit under `data/ttdb` in the base install and in several packs.
pub fn ttpfd_files(game: &std::path::Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![game.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        seen += 1;
        if seen > 4000 {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                // Do not descend into the archive payload directories twice:
                // pack/*/data mirrors data/, and depth stays shallow.
                if p.components().count() < game.components().count() + 6 {
                    stack.push(p);
                }
            } else if p.extension().map(|e| e.eq_ignore_ascii_case("ttpf")) == Some(true) {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if let Ok(rel) = p.strip_prefix(game) {
                    out.push((rel.to_string_lossy().replace('\\', "/"), size));
                }
            }
        }
    }
    out.sort();
    out
}
