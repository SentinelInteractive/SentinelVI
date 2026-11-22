//! Local API and static host for SentinelVI.
//!
//! Every number this server returns comes from an index derived from the game
//! data on disk. When an index has not been built the endpoint answers with
//! `{"exists": false}` and the UI says so; it never fills the gap.

pub mod audio;
pub mod ogg;
pub mod state;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use sentinel_core::source::MmapSource;
use sentinel_core::tpfd::Rds5;
use state::{App, Config, Job, RasterMeta};

type ApiError = (StatusCode, String);
type ApiResult = Result<Json<Value>, ApiError>;

fn err<E: std::fmt::Display>(code: StatusCode, e: E) -> ApiError {
    (code, e.to_string())
}

fn bad<E: std::fmt::Display>(e: E) -> ApiError {
    err(StatusCode::BAD_REQUEST, e)
}

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    err(StatusCode::INTERNAL_SERVER_ERROR, e)
}

pub fn router(app: Arc<App>) -> Router {
    let warm = app.clone();
    std::thread::spawn(move || prewarm_density(&warm));    let api = Router::new()
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/library/archives", get(archives))
        .route("/library/packs", get(packs))
        .route("/library/streams", get(streams))
        .route("/search", get(search))
        .route("/asset/{id}", get(asset))
        .route("/asset/{id}/payload", get(payload))
        .route("/asset/{id}/hex", get(payload_hex))
        .route("/export/{id}", post(export))
        .route("/map/meta", get(map_meta))
        .route("/map/points", get(map_points))
        .route("/map/density", get(map_density))
        .route("/map/terrain", get(map_terrain))
        .route("/textures/files", get(texture_files))
        .route("/textures/entries", get(texture_entries))
        .route("/textures/preview", get(texture_preview))
        .route("/audio/banks", get(audio_banks))
        .route("/audio/stream/{offset}", get(audio_stream))
        .route("/anim/clips", get(anim_clips))
        .route("/jobs", get(jobs))
        .route("/index/{kind}", post(index))
        .with_state(app.clone());

    let web = app.cfg.web.clone();
    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .fallback(move |uri: axum::http::Uri| {
            let web = web.clone();
            async move { serve_static(&web, uri.path()).await }
        })
}

fn mime_of(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

const INDEX_HTML: &str = "index.html";

/// Static files from `web/`. Any unknown path that is not a file falls back to
/// the shell, because every screen is a client route.
async fn serve_static(web: &std::path::Path, uri_path: &str) -> Response {
    let rel = uri_path.trim_start_matches('/');
    if rel.split('/').any(|seg| seg == "..") {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    }
    let mut path = if rel.is_empty() {
        web.join(INDEX_HTML)
    } else {
        web.join(rel)
    };
    if !path.is_file() {
        path = web.join(INDEX_HTML);
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, mime_of(&path)),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Answered before any database is opened, so a client can tell a slow index
/// from a dead process.
async fn health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    }))
}

/// Builds the whole-world density raster once at startup so the first map load
/// does not wait on a two-second scan.
fn prewarm_density(app: &App) {
    let path = app.cfg.paths.cache("placement.sqlite");
    if !path.is_file() {
        return;
    }
    let world = [-13000.0f32, -9000.0, 7000.0, 11000.0];
    let key = format!("-13000.0,-9000.0,7000.0,11000.0|0|0|1024|1024");
    if app.raster_cached(&key).is_some() {
        return;
    }
    let Ok(conn) = sentinel_placement::open(&path) else {
        return;
    };
    let Ok(d) = sentinel_placement::density(&conn, world, 0, false, 1024, 1024) else {
        return;
    };
    let stats = state::RasterMeta {
        total: d.total,
        bins: d.bins,
        max: d.max,
        width: d.width,
        height: d.height,
    };
    let png = sentinel_core::img::png_rgba(d.width, d.height, &raster_rgba(&d));
    app.raster_insert(key, png, stats);
}

fn toc(app: &App) -> Result<sentinel_toc::Toc, ApiError> {    let db = app.cfg.paths.toc_db();
    sentinel_toc::Toc::open(&db).map_err(|e| internal(format!("{}: {e}", db.display())))
}

async fn info(State(app): State<Arc<App>>) -> ApiResult {
    let t = toc(&app)?;
    let stats = t.stats().map_err(internal)?;
    let paths = &app.cfg.paths;
    let placement = paths.cache("placement.sqlite");
    let anim = paths.cache("anim.sqlite");
    let audio = paths.cache("audio.sqlite");
    let cache_state = |p: &PathBuf, kind: &str| -> Value {
        if !p.is_file() {
            return json!({"exists": false, "kind": kind});
        }
        json!({
            "exists": true,
            "kind": kind,
            "bytes": std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            "modified": std::fs::metadata(p).ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        })
    };
    Ok(Json(json!({
        "product": "SentinelVI",
        "version": env!("CARGO_PKG_VERSION"),
        "data_root": paths.root.display().to_string(),
        "game_dir": paths.game.display().to_string(),
        "counts": {
            "streams": stats.streams,
            "named": stats.named,
            "packs": stats.packs,
            "pool": stats.pool,
            "folders": stats.folders,
            "archives": stats.by_archive.len(),
        },
        "caches": {
            "placement": cache_state(&placement, "placement"),
            "anim": cache_state(&anim, "anim"),
            "audio": cache_state(&audio, "audio"),
        },
    })))
}

async fn archives(State(app): State<Arc<App>>) -> ApiResult {
    let t = toc(&app)?;
    let rows = t.archives().map_err(internal)?;
    Ok(Json(json!({ "rows": rows })))
}

#[derive(serde::Deserialize)]
struct PacksQuery {
    archive: String,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    500
}

async fn packs(State(app): State<Arc<App>>, Query(q): Query<PacksQuery>) -> ApiResult {
    let t = toc(&app)?;
    let rows = t.packs(&q.archive, q.limit.min(5000)).map_err(internal)?;
    Ok(Json(json!({ "rows": rows })))
}

#[derive(serde::Deserialize)]
struct StreamsQuery {
    pack: String,
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_rows")]
    limit: u32,
}

fn default_rows_usize() -> usize {
    200
}

fn default_rows() -> u32 {
    200
}

async fn streams(State(app): State<Arc<App>>, Query(q): Query<StreamsQuery>) -> ApiResult {
    let t = toc(&app)?;
    let (rows, total) = t
        .streams_in_pack(&q.pack, q.offset, q.limit.min(2000))
        .map_err(internal)?;
    Ok(Json(json!({ "total": total, "offset": q.offset, "rows": rows })))
}

#[derive(serde::Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_search")]
    limit: u32,
}

fn default_search() -> u32 {
    60
}

async fn search(State(app): State<Arc<App>>, Query(q): Query<SearchQuery>) -> ApiResult {
    let t = toc(&app)?;
    let (rows, total) = t.search(&q.q, q.limit.min(400)).map_err(internal)?;
    Ok(Json(json!({ "q": q.q, "total": total, "rows": rows })))
}

async fn asset(State(app): State<Arc<App>>, AxumPath(id): AxumPath<i64>) -> ApiResult {
    let t = toc(&app)?;
    match t.stream(id).map_err(internal)? {
        Some(s) => {
            let name = sentinel_toc::export_name(&s);
            Ok(Json(json!({ "stream": s, "export_name": name })))
        }
        None => Err(err(StatusCode::NOT_FOUND, format!("no stream {id}"))),
    }
}

async fn payload(State(app): State<Arc<App>>, AxumPath(id): AxumPath<i64>) -> Result<Response, ApiError> {
    let t = toc(&app)?;
    let s = t
        .stream(id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("no stream {id}")))?;
    let store = sentinel_toc::BinStore::open(&app.cfg.paths.bin_store()).map_err(internal)?;
    let bytes = store
        .payload(&s.archive, s.stream_offset)
        .map_err(|e| err(StatusCode::CONFLICT, e))?;
    let name = sentinel_toc::export_name(&s);
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{name}\""),
            ),
        ],
        bytes,
    )
        .into_response())
}

#[derive(serde::Deserialize)]
struct HexQuery {
    #[serde(default = "default_hex_bytes")]
    bytes: usize,
}

fn default_hex_bytes() -> usize {
    512
}

/// First bytes of a decoded payload as hex rows. The UI shows them as a
/// preview, so a 16 MB stream does not get pulled into the browser.
async fn payload_hex(
    State(app): State<Arc<App>>,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<HexQuery>,
) -> ApiResult {
    let t = toc(&app)?;
    let s = t
        .stream(id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("no stream {id}")))?;
    let store = sentinel_toc::BinStore::open(&app.cfg.paths.bin_store()).map_err(internal)?;
    let bytes = store
        .payload(&s.archive, s.stream_offset)
        .map_err(|e| err(StatusCode::CONFLICT, e))?;
    let take = q.bytes.min(2048).min(bytes.len());
    let mut rows = Vec::new();
    for (i, chunk) in bytes[..take].chunks(16).enumerate() {
        let mut hex = String::with_capacity(chunk.len() * 3);
        for b in chunk {
            hex.push_str(&format!("{b:02x} "));
        }
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' })
            .collect();
        rows.push(json!({ "offset": i * 16, "hex": hex.trim_end(), "ascii": ascii }));
    }
    Ok(Json(json!({
        "id": id,
        "total": bytes.len(),
        "shown": take,
        "rows": rows,
    })))
}

async fn export(State(app): State<Arc<App>>, AxumPath(id): AxumPath<i64>) -> ApiResult {
    let t = toc(&app)?;
    let s = t
        .stream(id)
        .map_err(internal)?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("no stream {id}")))?;
    let name = sentinel_toc::export_name(&s);
    let job = app.push_job("export", &format!("Export {name}"));
    let app2 = app.clone();
    std::thread::spawn(move || {
        app2.update_job(job, |j| j.state = "running".into());
        let out_dir = app2.cfg.paths.cache_dir().join("exports");
        let result = (|| -> Result<PathBuf, String> {
            std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
            let store = sentinel_toc::BinStore::open(&app2.cfg.paths.bin_store())?;
            let bytes = store.payload(&s.archive, s.stream_offset)?;
            let path = out_dir.join(&name);
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        match result {
            Ok(p) => app2.finish_job(job, "done", Some(p.display().to_string()), None),
            Err(e) => app2.finish_job(job, "failed", None, Some(e)),
        }
    });
    Ok(Json(json!({ "job": job })))
}

fn placement_path(app: &App) -> PathBuf {
    app.cfg.paths.cache("placement.sqlite")
}

async fn map_meta(State(app): State<Arc<App>>) -> ApiResult {
    let path = placement_path(&app);
    if !path.is_file() {
        return Ok(Json(json!({"exists": false})));
    }
    let conn = sentinel_placement::open(&path).map_err(internal)?;
    let meta = sentinel_placement::meta(&conn)
        .map_err(|e| internal(format!("placement index: {e}")))?;
    Ok(Json(json!({"exists": true, "meta": meta})))
}

#[derive(serde::Deserialize)]
struct MapQuery {
    #[serde(default = "default_bbox")]
    bbox: String,
    #[serde(default)]
    layer: u8,
    #[serde(default)]
    interior: u8,
    #[serde(default = "default_points")]
    limit: usize,
    #[serde(default = "default_stride")]
    stride: usize,
    #[serde(default)]
    mode: String,
}

fn default_bbox() -> String {
    "-13000.0,-9000.0,7000.0,11000.0".into()
}

fn default_points() -> usize {
    20000
}

fn default_stride() -> usize {
    1
}

fn parse_bbox(raw: &str) -> Result<[f32; 4], ApiError> {
    let nums: Vec<f32> = raw
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    if nums.len() != 4 {
        return Err(bad("bbox must be x0,y0,x1,y1"));
    }
    Ok([nums[0], nums[1], nums[2], nums[3]])
}

async fn map_points(State(app): State<Arc<App>>, Query(q): Query<MapQuery>) -> ApiResult {
    let sample = q.mode == "sample" || q.stride > 1;
    let mode = if sample { "sample" } else { "exact" };
    let path = placement_path(&app);
    if !path.is_file() {
        return Ok(Json(json!({"exists": false, "points": [], "total": 0, "mode": mode})));
    }
    let bbox = parse_bbox(&q.bbox)?;
    let conn = sentinel_placement::open(&path).map_err(internal)?;
    let total = sentinel_placement::count(&conn, bbox, q.layer, q.interior != 0)
        .map_err(|e| internal(format!("placement index: {e}")))?;
    let limit = q.limit.min(40000);
    let rows = if sample {
        sentinel_placement::query_stratified(&conn, bbox, q.layer, q.interior != 0, limit)
    } else {
        sentinel_placement::query(&conn, bbox, q.layer, q.interior != 0, limit)
    }
    .map_err(|e| internal(format!("placement index: {e}")))?;
    let points: Vec<[f32; 2]> = rows.iter().map(|p| [p.x, p.y]).collect();
    Ok(Json(json!({
        "exists": true,
        "total": total,
        "shown": points.len(),
        "bbox": bbox,
        "mode": mode,
        "points": points,
    })))
}

#[derive(serde::Deserialize)]
struct DensityQuery {
    #[serde(default = "default_bbox")]
    bbox: String,
    #[serde(default)]
    layer: u8,
    #[serde(default)]
    interior: u8,
    #[serde(default = "default_raster")]
    w: usize,
    #[serde(default = "default_raster")]
    h: usize,
}

fn default_raster() -> usize {
    1024
}

fn raster_rgba(d: &sentinel_placement::Density) -> Vec<u8> {
    let mut rgba = vec![0u8; d.width * d.height * 4];
    if d.max == 0 {
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        return rgba;
    }
    let denom = (1.0 + d.max as f64).ln().max(1e-6);
    for (i, &cell) in d.cells.iter().enumerate() {
        let v = if cell == 0 {
            0u8
        } else {
            // Square root of the log ratio lifts single-record cells off the
            // floor without flattening the dense cores.
            let t = (1.0 + cell as f64).ln() / denom;
            ((255.0 * t.sqrt()).round() as u8).clamp(52, 255)
        };
        let o = i * 4;
        rgba[o] = v;
        rgba[o + 1] = v;
        rgba[o + 2] = v;
        rgba[o + 3] = 255;
    }
    rgba
}

fn density_response(png: &[u8], stats: &RasterMeta) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    let mut set = |name: &'static str, value: String| {
        if let Ok(v) = HeaderValue::from_str(&value) {
            headers.insert(header::HeaderName::from_static(name), v);
        }
    };
    set("x-sentinel-total", stats.total.to_string());
    set("x-sentinel-bins", stats.bins.to_string());
    set("x-sentinel-max", stats.max.to_string());
    set("x-sentinel-size", format!("{}x{}", stats.width, stats.height));
    (headers, png.to_vec()).into_response()
}

async fn map_density(
    State(app): State<Arc<App>>,
    Query(q): Query<DensityQuery>,
) -> Result<Response, ApiError> {
    let bbox = parse_bbox(&q.bbox)?;
    let layer = q.layer;
    let interior = q.interior != 0;
    let w = q.w;
    let h = q.h;
    let key = format!("{}|{}|{}|{}|{}", q.bbox, q.layer, q.interior, w, h);

    if let Some((png, stats)) = app.raster_cached(&key) {
        return Ok(density_response(&png, &stats));
    }

    let path = placement_path(&app);
    if !path.is_file() {
        return Err(err(StatusCode::NOT_FOUND, "placement index not built"));
    }

    let (png, stats) = tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, RasterMeta), String> {
        let conn = sentinel_placement::open(&path).map_err(|e| format!("placement index: {e}"))?;
        let d = sentinel_placement::density(&conn, bbox, layer, interior, w, h)?;
        let rgba = raster_rgba(&d);
        let png = sentinel_core::img::png_rgba(d.width, d.height, &rgba);
        let stats = RasterMeta {
            total: d.total,
            bins: d.bins,
            max: d.max,
            width: d.width,
            height: d.height,
        };
        Ok((png, stats))
    })
    .await
    .map_err(internal)?
    .map_err(|e| internal(format!("placement index: {e}")))?;

    app.raster_insert(key, png.clone(), stats.clone());
    Ok(density_response(&png, &stats))
}

/// World heightmap from `common.rpf`, decoded with the core MHIU reader. The
/// raster is its own georeference: the MHIU header carries the world bounds.
async fn map_terrain(State(app): State<Arc<App>>) -> Result<Response, ApiError> {
    let cached = app.terrain.lock().unwrap().clone();
    let (png, bounds) = match cached {
        Some(hit) => (hit.0.clone(), hit.1),
        None => {
            let path = app.cfg.paths.archive("common");
            let cache_dir = app.cfg.paths.cache_dir();
            let built = tokio::task::spawn_blocking(move || decode_terrain(&path, &cache_dir))
                .await
                .map_err(internal)?
                .map_err(|e| err(StatusCode::NOT_FOUND, e))?;
            let arc = Arc::new(built);
            *app.terrain.lock().unwrap() = Some(arc.clone());
            (arc.0.clone(), arc.1)
        }
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=3600"));
    let bounds_text = format!("{},{},{},{}", bounds[0], bounds[1], bounds[2], bounds[3]);
    if let Ok(v) = HeaderValue::from_str(&bounds_text) {
        headers.insert(header::HeaderName::from_static("x-sentinel-bounds"), v);
    }
    Ok((headers, png).into_response())
}

/// One heightmap decode: `mhiu::find_in_archive` locates the page chain in the
/// archive, the hillshade is downsampled to 1024 px and flattened into the
/// dim grey band the map draws under the placement density.
fn decode_terrain(archive: &std::path::Path, cache_dir: &std::path::Path) -> Result<(Vec<u8>, [f32; 4]), String> {
    let src = MmapSource::open(archive, 64 << 20).map_err(|e| format!("{}: {e}", archive.display()))?;
    let map = sentinel_core::mhiu::find_in_archive(src.as_ref(), 0x40)
        .map_err(|e| format!("heightmap decode: {e}"))?;
    let shade = map.hillshade();
    let out = 1024usize;
    let step = (map.width / out).max(1);
    let w = (map.width / step).min(out);
    let h = (map.height / step).min(out);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src_i = (y * step) * map.width + x * step;
            let v = shade.get(src_i).copied().unwrap_or(23) as u32;
            let grey = 20 + (v * 44 / 255) as u8;
            let o = (y * w + x) * 4;
            rgba[o] = grey;
            rgba[o + 1] = grey;
            rgba[o + 2] = grey;
            rgba[o + 3] = 255;
        }
    }
    let bounds = [map.bmin[0], map.bmin[1], map.bmax[0], map.bmax[1]];
    let png = sentinel_core::img::png_rgba(w, h, &rgba);
    let _ = std::fs::create_dir_all(cache_dir);
    let _ = std::fs::write(cache_dir.join("terrain.png"), &png);
    let meta = serde_json::json!({
        "bounds": bounds,
        "width": w,
        "height": h,
        "source": "common.rpf",
    });
    let _ = std::fs::write(cache_dir.join("terrain.json"), meta.to_string());
    Ok((png, bounds))
}

fn parse_ttpf(app: &App, rel: &str) -> Result<Arc<Rds5>, ApiError> {
    if let Some(hit) = app.ttpfs.lock().unwrap().get(rel) {
        return Ok(hit.clone());
    }
    let path = app.cfg.paths.game.join(rel);
    let src = MmapSource::open(&path, 3 << 30)
        .map_err(|e| internal(format!("{}: {e}", path.display())))?;
    let rds = Rds5::parse(src.as_ref())
        .map_err(|e| internal(format!("{}: {e}", path.display())))?;
    let arc = Arc::new(rds);
    app.ttpfs
        .lock()
        .unwrap()
        .insert(rel.to_string(), arc.clone());
    Ok(arc)
}

async fn texture_files(State(app): State<Arc<App>>) -> ApiResult {
    let files = state::ttpfd_files(&app.cfg.paths.game);
    let mut rows = Vec::new();
    for (rel, size) in files {
        match parse_ttpf(&app, &rel) {
            Ok(rds) => rows.push(json!({
                "path": rel,
                "bytes": size,
                "textures": rds.descs.len(),
                "entries": rds.entries,
            })),
            Err((_, msg)) => rows.push(json!({
                "path": rel,
                "bytes": size,
                "textures": 0,
                "error": msg,
            })),
        }
    }
    Ok(Json(json!({ "rows": rows })))
}

#[derive(serde::Deserialize)]
struct EntryQuery {
    file: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_rows_usize")]
    limit: usize,
}

async fn texture_entries(State(app): State<Arc<App>>, Query(q): Query<EntryQuery>) -> ApiResult {
    let rds = parse_ttpf(&app, &q.file)?;
    let total = rds.descs.len();
    let limit = q.limit.min(400);
    let rows: Vec<Value> = (q.offset..(q.offset + limit).min(total))
        .filter_map(|i| rds.desc(i).map(|d| {
            json!({
                "index": d.index,
                "width": d.width,
                "height": d.height,
                "mips": d.mips,
                "dxgi": d.dxgi,
                "format": sentinel_core::bc::format(d.dxgi).map(|f| f.name).unwrap_or("unknown"),
            })
        }))
        .collect();
    Ok(Json(json!({ "file": q.file, "total": total, "offset": q.offset, "rows": rows })))
}

#[derive(serde::Deserialize)]
struct PreviewQuery {
    file: String,
    index: usize,
}

async fn texture_preview(State(app): State<Arc<App>>, Query(q): Query<PreviewQuery>) -> Result<Response, ApiError> {
    let rds = parse_ttpf(&app, &q.file)?;
    let d = rds
        .desc(q.index)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, format!("no texture {}", q.index)))?;
    if d.width > 2048 || d.height > 2048 {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "surface is {} x {}; preview is limited to 2048 px",
                d.width, d.height
            ),
        ));
    }
    let path = app.cfg.paths.game.join(&q.file);
    let src = MmapSource::open(&path, 3 << 30)
        .map_err(|e| internal(format!("{}: {e}", path.display())))?;
    let tex = rds
        .decode(src.as_ref(), q.index)
        .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    let png = sentinel_core::img::png_rgba(tex.width, tex.height, &tex.rgba);
    Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
}

#[derive(serde::Deserialize)]
struct PageQuery {
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_rows")]
    limit: u32,
}

async fn audio_banks(State(app): State<Arc<App>>, Query(q): Query<PageQuery>) -> ApiResult {
    let path = app.cfg.paths.cache("audio.sqlite");
    if !path.is_file() {
        return Ok(Json(json!({"exists": false, "rows": [], "total": 0})));
    }
    let conn = audio::open(&path).map_err(internal)?;
    let (rows, total) = audio::list(&conn, q.offset, q.limit.min(500)).map_err(internal)?;
    Ok(Json(json!({"exists": true, "total": total, "offset": q.offset, "rows": rows})))
}

async fn audio_stream(
    State(app): State<Arc<App>>,
    AxumPath(offset): AxumPath<u64>,
) -> Result<Response, ApiError> {
    let archive = app.cfg.paths.archive("audio");
    let (bytes, secs) = tokio::task::spawn_blocking(move || audio::ogg_for_bank(&archive, offset))
        .await
        .map_err(internal)?
        .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, axum::http::HeaderValue::from_static("audio/ogg"));
    headers.insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    if let Ok(v) = axum::http::HeaderValue::from_str(&format!("{secs:.2}")) {
        headers.insert(axum::http::HeaderName::from_static("x-sentinel-duration"), v);
    }
    Ok((headers, bytes).into_response())
}

#[derive(serde::Deserialize)]
struct ClipQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: u32,
    #[serde(default = "default_rows")]
    limit: u32,
}

async fn anim_clips(State(app): State<Arc<App>>, Query(q): Query<ClipQuery>) -> ApiResult {
    let path = app.cfg.paths.cache("anim.sqlite");
    let needle = if q.q.trim().is_empty() { None } else { Some(q.q.trim()) };
    if path.is_file() {
        let conn = sentinel_anim::open(&path).map_err(internal)?;
        let (rows, total) = sentinel_anim::list(&conn, needle, q.offset, q.limit.min(500))
            .map_err(|e| internal(format!("animation index: {e}")))?;
        if total > 0 {
            return Ok(Json(json!({
                "exists": true, "source": "records", "total": total, "offset": q.offset, "rows": rows
            })));
        }
    }
    // No clip record in the index carries a name, so the screen falls back to
    // the clip names the directory index already holds.
    let t = toc(&app)?;
    let (streams, total) = t
        .clip_names(needle, q.offset, q.limit.min(500))
        .map_err(internal)?;
    let rows: Vec<Value> = streams
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.name,
                "frames": null,
                "duration": null,
                "fps": null,
                "keyframes": null,
                "bytes": s.uncomp_size,
                "res": s.stream_offset,
            })
        })
        .collect();
    Ok(Json(json!({
        "exists": true, "source": "names", "total": total, "offset": q.offset, "rows": rows
    })))
}

async fn jobs(State(app): State<Arc<App>>) -> ApiResult {
    let rows: Vec<Job> = app.jobs_snapshot();
    Ok(Json(json!({ "rows": rows, "busy": app.busy.load(Ordering::Relaxed) })))
}

async fn index(
    State(app): State<Arc<App>>,
    AxumPath(kind): AxumPath<String>,
) -> ApiResult {
    let (out, label) = match kind.as_str() {
        "placement" => (
            app.cfg.paths.cache("placement.sqlite"),
            "Build placement index",
        ),
        "anim" => (app.cfg.paths.cache("anim.sqlite"), "Build animation index"),
        "audio" => (app.cfg.paths.cache("audio.sqlite"), "Build audio bank index"),
        other => return Err(bad(format!("unknown index {other}"))),
    };
    if app
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(err(StatusCode::CONFLICT, "an index build is already running"));
    }
    let job = app.push_job("index", label);
    let app2 = app.clone();
    std::thread::spawn(move || {
        app2.update_job(job, |j| j.state = "running".into());
        let progress = {
            let a = app2.clone();
            move |done: u64, total: u64| {
                a.update_job(job, |j| j.progress = Some((done, total)));
            }
        };
        let game = app2.cfg.paths.game.clone();
        let result = match kind.as_str() {
            "placement" => {
                std::fs::create_dir_all(out.parent().unwrap_or(&game)).ok();
                sentinel_placement::build(&game.join("levels.rpf"), &out, &progress)
                    .map(|r| {
                        format!(
                            "{} resources, {} entities stored, {} props stored",
                            r.resources, r.entities_kept, r.props_kept
                        )
                    })
            }
            "anim" => {
                std::fs::create_dir_all(out.parent().unwrap_or(&game)).ok();
                sentinel_anim::build(&game.join("anim.rpf"), &out, &progress).map(|r| {
                    format!("{} clip records, {} named", r.records, r.named)
                })
            }
            "audio" => {
                std::fs::create_dir_all(out.parent().unwrap_or(&game)).ok();
                audio::build(&game.join("audio.rpf"), &out, &progress)
                    .map(|n| format!("{n} banks"))
            }
            _ => Err("unknown index".to_string()),
        };
        match result {
            Ok(msg) => app2.finish_job(job, "done", Some(out.display().to_string()), Some(msg)),
            Err(e) => app2.finish_job(job, "failed", None, Some(e)),
        }
        app2.busy.store(false, Ordering::SeqCst);
    });
    Ok(Json(json!({ "job": job })))
}


