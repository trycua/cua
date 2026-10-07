// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `GET/HEAD/PUT /files?path&method&exp&sig` — signed-URL file access for
//! browsers and `<img>` tags.
//!
//! The signature is HMAC-SHA256 (key derived from the root token) over the
//! method, absolute path, expiry and the optional response overrides, so
//! none of them can be altered. Expiry is mandatory. A full disk answers
//! `507 Insufficient Storage`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use cua_proto::env::v1::{ErrorReason, WriteMode};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::context::ServerContext;
use crate::filesystem::write::{AtomicWriter, WriteTarget};

/// Longest signed-URL lifetime.
pub const MAX_SIGNED_URL_TTL: Duration = Duration::from_secs(24 * 60 * 60);

const QUERY: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// A minted URL.
pub struct SignedUrl {
    /// Path and query relative to the driver base URL.
    pub url_path: String,
    /// Expiry.
    pub expires_at: SystemTime,
}

fn signature(
    ctx: &ServerContext,
    method: &str,
    path: &str,
    exp: u64,
    content_type: &str,
    download_name: &str,
) -> Vec<u8> {
    let key = ctx.auth().derive_key("files-v1");
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("hmac key");
    for part in [method, path, &exp.to_string(), content_type, download_name] {
        mac.update(part.as_bytes());
        mac.update(b"\n");
    }
    mac.finalize().into_bytes().to_vec()
}

/// Mints a signed URL for `method` on the absolute `path`.
pub fn sign(
    ctx: &ServerContext,
    method: &str,
    path: &str,
    expires_at: SystemTime,
    content_type: &str,
    download_name: &str,
) -> SignedUrl {
    let exp = expires_at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature(
        ctx,
        method,
        path,
        exp,
        content_type,
        download_name,
    ));
    let mut url = format!(
        "{}?path={}&method={method}&exp={exp}",
        cua_proto::metadata::FILES_PATH,
        utf8_percent_encode(path, QUERY)
    );
    if !content_type.is_empty() {
        url.push_str(&format!("&ct={}", utf8_percent_encode(content_type, QUERY)));
    }
    if !download_name.is_empty() {
        url.push_str(&format!(
            "&dn={}",
            utf8_percent_encode(download_name, QUERY)
        ));
    }
    url.push_str(&format!("&sig={sig}"));
    SignedUrl {
        url_path: url,
        expires_at: UNIX_EPOCH + Duration::from_secs(exp),
    }
}

/// Why a signed request was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Missing or malformed parameters.
    BadRequest(&'static str),
    /// Expired.
    Expired,
    /// Wrong signature or method.
    Forbidden,
}

/// Verified parameters of a signed request.
#[derive(Debug)]
pub struct Verified {
    /// Absolute path.
    pub path: String,
    /// Response content type override.
    pub content_type: String,
    /// Download name override.
    pub download_name: String,
}

fn parse_query(uri: &Uri) -> HashMap<String, String> {
    uri.query()
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let v = percent_encoding::percent_decode_str(&v.replace('+', "%20"))
                .decode_utf8()
                .ok()?
                .into_owned();
            Some((k.to_owned(), v))
        })
        .collect()
}

/// Verifies method, expiry and signature.
pub fn verify(ctx: &ServerContext, method: &Method, uri: &Uri) -> Result<Verified, Refusal> {
    let q = parse_query(uri);
    let path = q.get("path").ok_or(Refusal::BadRequest("missing path"))?;
    let signed_method = q
        .get("method")
        .ok_or(Refusal::BadRequest("missing method"))?;
    let exp: u64 = q
        .get("exp")
        .ok_or(Refusal::BadRequest("missing exp"))?
        .parse()
        .map_err(|_| Refusal::BadRequest("invalid exp"))?;
    let sig = q.get("sig").ok_or(Refusal::BadRequest("missing sig"))?;
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig)
        .map_err(|_| Refusal::Forbidden)?;
    let content_type = q.get("ct").cloned().unwrap_or_default();
    let download_name = q.get("dn").cloned().unwrap_or_default();
    let expected = signature(ctx, signed_method, path, exp, &content_type, &download_name);
    if !crate::util::constant_time_eq(&sig, &expected) {
        return Err(Refusal::Forbidden);
    }
    let allowed = match signed_method.as_str() {
        "GET" => *method == Method::GET || *method == Method::HEAD,
        "PUT" => *method == Method::PUT,
        _ => false,
    };
    if !allowed {
        return Err(Refusal::Forbidden);
    }
    if crate::util::unix_now() >= exp {
        return Err(Refusal::Expired);
    }
    Ok(Verified {
        path: path.clone(),
        content_type,
        download_name,
    })
}

fn text(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

fn status_to_http(status: &tonic::Status) -> StatusCode {
    let reason = crate::error::error_info(status)
        .map(|i| i.reason)
        .unwrap_or_default();
    if reason == ErrorReason::DiskFull as i32 {
        return StatusCode::INSUFFICIENT_STORAGE;
    }
    match status.code() {
        tonic::Code::NotFound => StatusCode::NOT_FOUND,
        tonic::Code::PermissionDenied => StatusCode::FORBIDDEN,
        tonic::Code::AlreadyExists | tonic::Code::FailedPrecondition => StatusCode::CONFLICT,
        tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
        tonic::Code::ResourceExhausted => StatusCode::INSUFFICIENT_STORAGE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// The `/files` handler (any method).
pub async fn handle(
    State(ctx): State<ServerContext>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let verified = match verify(&ctx, &method, &uri) {
        Ok(v) => v,
        Err(Refusal::BadRequest(m)) => return text(StatusCode::BAD_REQUEST, m),
        Err(Refusal::Expired) => return text(StatusCode::FORBIDDEN, "signed URL expired"),
        Err(Refusal::Forbidden) => return text(StatusCode::FORBIDDEN, "invalid signature"),
    };
    let path = std::path::PathBuf::from(&verified.path);
    if method == Method::PUT {
        return put(path, body).await;
    }
    get(path, &verified, &headers, method == Method::HEAD).await
}

async fn put(path: std::path::PathBuf, body: Body) -> Response {
    let target = WriteTarget {
        dest: path,
        mode: WriteMode::Overwrite,
        permissions: 0,
        create_parents: false,
    };
    let mut writer = match AtomicWriter::create(target).await {
        Ok(w) => w,
        Err(e) => return text(status_to_http(&e), e.message().to_owned()),
    };
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                writer.abort().await;
                return text(StatusCode::BAD_REQUEST, format!("body: {e}"));
            }
        };
        if let Err(e) = writer.write(&chunk).await {
            writer.abort().await;
            return text(status_to_http(&e), e.message().to_owned());
        }
    }
    match writer.finish(0, "").await {
        Ok((entry, sha256)) => (
            StatusCode::CREATED,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "path": entry.path, "size": entry.size, "sha256": sha256 })
                .to_string(),
        )
            .into_response(),
        Err(e) => text(status_to_http(&e), e.message().to_owned()),
    }
}

/// Parses a single `bytes=` range against `len`.
fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        (len.saturating_sub(suffix), len.checked_sub(1)?)
    } else {
        let start: u64 = start.parse().ok()?;
        let end = if end.is_empty() {
            len.checked_sub(1)?
        } else {
            end.parse::<u64>().ok()?.min(len.checked_sub(1)?)
        };
        (start, end)
    };
    (start <= end && start < len).then_some((start, end))
}

async fn get(
    path: std::path::PathBuf,
    verified: &Verified,
    headers: &HeaderMap,
    head: bool,
) -> Response {
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) => m,
        Err(e) => {
            let status = crate::error::io_status(&e, &path);
            return text(status_to_http(&status), status.message().to_owned());
        }
    };
    if meta.is_dir() {
        return text(StatusCode::CONFLICT, "is a directory");
    }
    let len = meta.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, len));
    let (status, start, end) = match range {
        None => (StatusCode::OK, 0, len.saturating_sub(1)),
        Some(Some((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
        Some(None) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .expect("response");
        }
    };
    let body_len = if len == 0 { 0 } else { end - start + 1 };
    let content_type = if verified.content_type.is_empty() {
        mime_guess::from_path(&path)
            .first_or_octet_stream()
            .essence_str()
            .to_owned()
    } else {
        verified.content_type.clone()
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, body_len)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "private, no-store");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    if !verified.download_name.is_empty() {
        let encoded = utf8_percent_encode(&verified.download_name, QUERY).to_string();
        if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}"))
        {
            builder = builder.header(header::CONTENT_DISPOSITION, value);
        }
    }
    if head || body_len == 0 {
        return builder.body(Body::empty()).expect("response");
    }
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            let status = crate::error::io_status(&e, &path);
            return text(status_to_http(&status), status.message().to_owned());
        }
    };
    if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "seek failed");
    }
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(body_len), 256 * 1024);
    builder.body(Body::from_stream(stream)).expect("response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn ctx() -> ServerContext {
        ServerContext::new(ServerConfig::default(), Some("tok".into()))
    }

    fn uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    #[test]
    fn signed_url_verifies_and_rejects_tampering() {
        let ctx = ctx();
        let signed = sign(
            &ctx,
            "GET",
            "/tmp/a b.png",
            SystemTime::now() + Duration::from_secs(60),
            "",
            "x.png",
        );
        let ok = verify(&ctx, &Method::GET, &uri(&signed.url_path)).unwrap();
        assert_eq!(ok.path, "/tmp/a b.png");
        assert_eq!(ok.download_name, "x.png");
        assert!(verify(&ctx, &Method::HEAD, &uri(&signed.url_path)).is_ok());
        assert_eq!(
            verify(&ctx, &Method::PUT, &uri(&signed.url_path)).unwrap_err(),
            Refusal::Forbidden
        );
        let tampered = signed.url_path.replace("a%20b.png", "etc");
        assert_eq!(
            verify(&ctx, &Method::GET, &uri(&tampered)).unwrap_err(),
            Refusal::Forbidden
        );
        let renamed = signed.url_path.replace("dn=x.png", "dn=y.png");
        assert_eq!(
            verify(&ctx, &Method::GET, &uri(&renamed)).unwrap_err(),
            Refusal::Forbidden
        );
        let unsigned = signed.url_path.split("&sig=").next().unwrap().to_owned();
        assert!(matches!(
            verify(&ctx, &Method::GET, &uri(&unsigned)),
            Err(Refusal::BadRequest(_))
        ));
        // Another token's signature does not verify.
        let other = ServerContext::new(ServerConfig::default(), Some("other".into()));
        assert_eq!(
            verify(&other, &Method::GET, &uri(&signed.url_path)).unwrap_err(),
            Refusal::Forbidden
        );
    }

    #[test]
    fn expired_url_is_refused_and_expiry_is_mandatory() {
        let ctx = ctx();
        let signed = sign(
            &ctx,
            "PUT",
            "/tmp/x",
            SystemTime::now() - Duration::from_secs(1),
            "",
            "",
        );
        assert_eq!(
            verify(&ctx, &Method::PUT, &uri(&signed.url_path)).unwrap_err(),
            Refusal::Expired
        );
        let no_exp = signed.url_path.replace("&exp=", "&e=");
        assert!(matches!(
            verify(&ctx, &Method::PUT, &uri(&no_exp)),
            Err(Refusal::BadRequest(_))
        ));
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-500", 100), Some((50, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
    }
}
