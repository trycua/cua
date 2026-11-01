//! The HTTP route a host serves its exported snapshot files on:
//!
//! `GET /v1/snapshots/blobs/<sha256>` with `Authorization: Bearer <grant>`
//! returns that file when it belongs to the manifest the grant is for, the
//! file has not changed since it was hashed, and the caller is allowed
//! (the embedding server's peer rule). `Range: bytes=<start>-[<end>]` is
//! honoured (206), so a copy that stopped resumes where it stopped.
//! `HEAD` answers the same without the body.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Request};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http::{HeaderValue, StatusCode, header};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use crate::hash::Stamp;
use crate::store::Store;
use crate::{HTTP_BASE, valid_sha};

/// The cua home whose snapshots are served (read on every request: the
/// host's policy may change).
pub type HomeFn = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;
/// Whether the request may be served at all (a direct host's peer rule):
/// `Err(why)` refuses it with 403.
pub type AllowFn = Arc<dyn Fn(&Request) -> Result<(), String> + Send + Sync>;

/// The route, mounted at [`HTTP_BASE`].
pub fn router(home: HomeFn, allow: AllowFn) -> Router {
    let path = format!("{HTTP_BASE}/blobs/{{sha}}");
    Router::new().route(
        &path,
        get(move |UrlPath(sha): UrlPath<String>, req: Request| {
            let home = home.clone();
            let allow = allow.clone();
            async move { blob(home, allow, sha, req).await }
        }),
    )
}

fn refuse(code: StatusCode, why: impl Into<String>) -> Response {
    (code, why.into()).into_response()
}

/// `bytes=<start>-[<end>]` (one range) within `len`: `(start, end_excl)`.
fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let r = h.trim().strip_prefix("bytes=")?;
    if r.contains(',') {
        return None;
    }
    let (a, b) = r.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    let end = match b.trim() {
        "" => len,
        e => e.parse::<u64>().ok()?.saturating_add(1).min(len),
    };
    (start <= end && start <= len).then_some((start, end))
}

async fn blob(home: HomeFn, allow: AllowFn, sha: String, req: Request) -> Response {
    if let Err(why) = allow(&req) {
        return refuse(StatusCode::FORBIDDEN, why);
    }
    if !valid_sha(&sha) {
        return refuse(StatusCode::BAD_REQUEST, "not a sha256");
    }
    let Some(grant) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let (scheme, rest) = v.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
        })
    else {
        return refuse(StatusCode::UNAUTHORIZED, "a snapshot grant is required");
    };
    let Some(home) = home() else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "this host serves no snapshots",
        );
    };
    let store = Store::of_home(&home);
    let key = match store.grant_key() {
        Ok(k) => k,
        Err(e) => return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let digest = match crate::grant::verify(&key, grant, crate::grant::now()) {
        Ok(d) => d,
        Err(e) => return refuse(StatusCode::FORBIDDEN, e.to_string()),
    };
    let index = match store.export_index(&digest) {
        Ok(i) => i,
        Err(e) => return refuse(StatusCode::NOT_FOUND, e.to_string()),
    };
    let Some(file) = index.files.get(&sha) else {
        return refuse(StatusCode::FORBIDDEN, "the grant is not for this file");
    };
    match Stamp::of(&file.path) {
        Ok(s) if s == file.stamp => {}
        Ok(_) => {
            return refuse(
                StatusCode::CONFLICT,
                "the file changed since the snapshot was exported; export it again",
            );
        }
        Err(e) => return refuse(StatusCode::NOT_FOUND, e.to_string()),
    }
    let len = file.stamp.size;
    let range = req
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|h| parse_range(h, len));
    let (start, end, partial) = match range {
        None => (0, len, false),
        Some(Some((s, e))) => (s, e, true),
        Some(None) => {
            let mut r = refuse(StatusCode::RANGE_NOT_SATISFIABLE, "bad range");
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{len}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return r;
        }
    };
    let mut builder = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_LENGTH, end - start)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header("x-cua-sha256", sha.as_str());
    if partial {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{len}", end.saturating_sub(1)),
        );
    }
    if req.method() == http::Method::HEAD {
        return builder
            .body(Body::empty())
            .unwrap_or_else(|e| refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    let mut f = match tokio::fs::File::open(&file.path).await {
        Ok(f) => f,
        Err(e) => return refuse(StatusCode::NOT_FOUND, e.to_string()),
    };
    if let Err(e) = f.seek(std::io::SeekFrom::Start(start)).await {
        return refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        f.take(end - start),
        1 << 20,
    ));
    builder
        .body(body)
        .unwrap_or_else(|e| refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 10), Some((0, 10)));
        assert_eq!(parse_range("bytes=4-", 10), Some((4, 10)));
        assert_eq!(parse_range("bytes=2-5", 10), Some((2, 6)));
        assert_eq!(parse_range("bytes=2-50", 10), Some((2, 10)));
        assert_eq!(parse_range("bytes=10-", 10), Some((10, 10)));
        assert_eq!(parse_range("bytes=11-", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }
}
