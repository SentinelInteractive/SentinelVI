use super::{bad, err, internal, ApiError, ApiResult};
use crate::state::App;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use sentinel_toc::{FullToc, SlotFilter, SlotSearch};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

async fn read(
    app: Arc<App>,
    query: impl FnOnce(FullToc) -> ApiResult + Send + 'static,
) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let toc = FullToc::open_if_present(&app.cfg.paths)
            .map_err(index_unreadable)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "The full TOC index is missing."))?;
        query(toc)
    })
    .await
    .map_err(internal)?
}

fn index_unreadable(error: String) -> ApiError {
    eprintln!("Full TOC index: {error}");
    internal("The full TOC index could not be read.")
}

pub async fn roots(State(app): State<Arc<App>>) -> ApiResult {
    read(app, |toc| {
        let rows = toc.roots().map_err(index_unreadable)?;
        Ok(Json(json!({ "rows": rows })))
    })
    .await
}

#[derive(Deserialize)]
pub struct HeadersQuery {
    root: String,
    after: Option<i64>,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    80
}

pub async fn headers(State(app): State<Arc<App>>, Query(q): Query<HeadersQuery>) -> ApiResult {
    read(app, move |toc| {
        let page = toc
            .headers(&q.root, q.after, q.limit)
            .map_err(index_unreadable)?;
        Ok(Json(json!(page)))
    })
    .await
}

#[derive(Deserialize)]
pub struct SlotsQuery {
    root: Option<String>,
    header_offset: Option<i64>,
    field: Option<String>,
    q: Option<String>,
    after: Option<i64>,
    #[serde(default = "default_limit")]
    limit: u32,
}

pub async fn slots(State(app): State<Arc<App>>, Query(q): Query<SlotsQuery>) -> ApiResult {
    if q.header_offset.is_some() && q.root.is_none() {
        return Err(bad("Choose a root before filtering by header."));
    }
    let value = q.q.as_deref().unwrap_or("").trim();
    if value.len() > 4096 {
        return Err(bad("The search value is too long."));
    }
    read(app, move |toc| {
        let search = match (q.field.as_deref(), q.q.as_deref().map(str::trim)) {
            (_, None | Some("")) => None,
            (field, Some(value)) => Some(parse_search(field.unwrap_or("path"), value)?),
        };
        let filter = SlotFilter {
            root: q.root.as_deref(),
            header_offset: q.header_offset,
            search,
        };
        let page = toc
            .slots(&filter, q.after, q.limit)
            .map_err(index_unreadable)?;
        Ok(Json(json!(page)))
    })
    .await
}

fn parse_search<'a>(field: &str, value: &'a str) -> Result<SlotSearch<'a>, ApiError> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    Ok(match field {
        "path" => SlotSearch::Path(value),
        "hash32" => SlotSearch::Hash32(
            u32::from_str_radix(digits, 16).map_err(|_| bad("Enter a 32-bit hexadecimal hash."))?,
        ),
        "hash64" => SlotSearch::Hash64(
            u64::from_str_radix(digits, 16).map_err(|_| bad("Enter a 64-bit hexadecimal hash."))?,
        ),
        "root" => SlotSearch::Root(value),
        "slot" => SlotSearch::Slot(
            value
                .parse::<u32>()
                .map(i64::from)
                .map_err(|_| bad("Enter a nonnegative decimal slot number."))?,
        ),
        _ => return Err(bad("Search by path, hash32, hash64, root, or slot.")),
    })
}

pub async fn slot(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult {
    read(app, move |toc| {
        let slot = toc
            .slot(id)
            .map_err(index_unreadable)?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "This slot is not in the TOC index."))?;
        Ok(Json(json!({ "slot": slot })))
    })
    .await
}
