// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `GET/HEAD/PUT/POST /files?path&method&exp&sig` — signed-URL file access
//! for browsers and `<img>` tags.
//!
//! The signature is HMAC-SHA256 (key derived from the root token) over the
//! method, absolute path, expiry and the optional response overrides, the
//! user to act as and the create-parents flag, so none of them can be
//! altered. Expiry is mandatory. A full disk answers `507 Insufficient
//! Storage`.
//!
//! A URL signed for `PUT` takes the raw body with `PUT`, or the first file
//! part of a `POST multipart/form-data` body (the upload form of other
//! sandbox APIs); both write the file atomically and the `POST` answers
//! with the JSON list `[{"name","path","type"}]`. A URL signed with a
//! `run_as` user reads and writes as that user through the file helper (see
//! [`crate::filesystem::runas`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use cua_proto::env::v1::read_file_response::Message as ReadMessage;
use cua_proto::env::v1::write_file_request::Message as WriteMessage;
use cua_proto::env::v1::{
    ErrorReason, FileType, ReadFileRequest, ReadFileResponse, StatRequest, WriteFileHeader,
    WriteFileRequest, WriteFileResponse, WriteMode,
};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::context::ServerContext;
use crate::filesystem::runas::Acting;
use crate::filesystem::write::{AtomicWriter, WriteTarget};
use crate::filesystem::FsState;
use crate::http::multipart;

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

/// What a signature covers besides method, path and expiry.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Extras {
    /// Response `Content-Type` override (GET).
    pub content_type: String,
    /// Download file name (GET).
    pub download_name: String,
    /// Guest user to act as.
    pub run_as: String,
    /// Create missing parent directories (PUT).
    pub create_parents: bool,
}

fn signature(ctx: &ServerContext, method: &str, path: &str, exp: u64, extras: &Extras) -> Vec<u8> {
    let key = ctx.auth().derive_key("files-v1");
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("hmac key");
    for part in [
        method,
        path,
        &exp.to_string(),
        &extras.content_type,
        &extras.download_name,
    ] {
        mac.update(part.as_bytes());
        mac.update(b"\n");
    }
    // Optional parts are appended only when set, so URLs signed before they
    // existed (and URLs without them) keep their signatures.
    if !extras.run_as.is_empty() {
        mac.update(b"run_as=");
        mac.update(extras.run_as.as_bytes());
        mac.update(b"\n");
    }
    if extras.create_parents {
        mac.update(b"create_parents=1\n");
    }
    mac.finalize().into_bytes().to_vec()
}

/// Mints a signed URL for `method` on the absolute `path`.
#[allow(clippy::too_many_arguments)]
pub fn sign(
    ctx: &ServerContext,
    method: &str,
    path: &str,
    expires_at: SystemTime,
    content_type: &str,
    download_name: &str,
    run_as: &str,
    create_parents: bool,
) -> SignedUrl {
    let exp = expires_at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let extras = Extras {
        content_type: content_type.to_owned(),
        download_name: download_name.to_owned(),
        run_as: run_as.to_owned(),
        create_parents,
    };
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(signature(ctx, method, path, exp, &extras));
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
    if !run_as.is_empty() {
        url.push_str(&format!("&ru={}", utf8_percent_encode(run_as, QUERY)));
    }
    if create_parents {
        url.push_str("&pp=1");
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
    /// Guest user to act as (empty: the agent's own identity).
    pub run_as: String,
    /// Create missing parent directories (uploads).
    pub create_parents: bool,
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
    let extras = Extras {
        content_type: q.get("ct").cloned().unwrap_or_default(),
        download_name: q.get("dn").cloned().unwrap_or_default(),
        run_as: q.get("ru").cloned().unwrap_or_default(),
        create_parents: q.get("pp").is_some_and(|v| v == "1"),
    };
    let expected = signature(ctx, signed_method, path, exp, &extras);
    if !crate::util::constant_time_eq(&sig, &expected) {
        return Err(Refusal::Forbidden);
    }
    let allowed = match signed_method.as_str() {
        "GET" => *method == Method::GET || *method == Method::HEAD,
        // A PUT URL also takes the multipart form upload.
        "PUT" => *method == Method::PUT || *method == Method::POST,
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
        content_type: extras.content_type,
        download_name: extras.download_name,
        run_as: extras.run_as,
        create_parents: extras.create_parents,
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

/// State of the `/files` route: the server context and the filesystem state
/// (its helpers act as other users).
#[derive(Clone)]
pub struct FilesState {
    /// The server context (signing key, config).
    pub ctx: ServerContext,
    /// The filesystem service's state.
    pub fs: Arc<FsState>,
}

/// The `/files` handler (any method).
pub async fn handle(
    State(state): State<FilesState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let verified = match verify(&state.ctx, &method, &uri) {
        Ok(v) => v,
        Err(Refusal::BadRequest(m)) => return text(StatusCode::BAD_REQUEST, m),
        Err(Refusal::Expired) => return text(StatusCode::FORBIDDEN, "signed URL expired"),
        Err(Refusal::Forbidden) => return text(StatusCode::FORBIDDEN, "invalid signature"),
    };
    let acting = match state.fs.runas().acting(&verified.run_as).await {
        Ok(acting) => acting,
        Err(e) => return text(status_to_http(&e), e.message().to_owned()),
    };
    // `~` and relative paths cannot reach here: the path was resolved when
    // the URL was minted.
    let path = std::path::PathBuf::from(&verified.path);
    if method == Method::PUT {
        return put(acting, path, &verified, body).await;
    }
    if method == Method::POST {
        return post_multipart(acting, path, &verified, &headers, body).await;
    }
    get(acting, path, &verified, &headers, method == Method::HEAD).await
}

/// Where an upload's bytes go: an atomic writer in this process, or a
/// `WriteFile` call to the helper of the user the URL acts as.
enum Sink {
    Local(Box<AtomicWriter>),
    Remote {
        tx: tokio::sync::mpsc::Sender<WriteFileRequest>,
        call: tokio::task::JoinHandle<Result<WriteFileResponse, tonic::Status>>,
    },
}

/// A refusal ready to send (boxed: a response is large).
type Failure = Box<Response>;

fn failure(status: &tonic::Status) -> Failure {
    Box::new(text(status_to_http(status), status.message().to_owned()))
}

impl Sink {
    async fn open(
        acting: Acting,
        path: std::path::PathBuf,
        verified: &Verified,
    ) -> Result<Sink, Failure> {
        match acting {
            Acting::Own(_) => {
                let target =
                    WriteTarget::new(path, WriteMode::Overwrite, 0, verified.create_parents);
                AtomicWriter::create(target)
                    .await
                    .map(|w| Sink::Local(Box::new(w)))
                    .map_err(|e| failure(&e))
            }
            Acting::Other(_, mut client) => {
                let (tx, rx) = tokio::sync::mpsc::channel::<WriteFileRequest>(8);
                tx.send(WriteFileRequest {
                    message: Some(WriteMessage::Header(WriteFileHeader {
                        path: path.display().to_string(),
                        mode: WriteMode::Overwrite as i32,
                        create_parents: verified.create_parents,
                        ..Default::default()
                    })),
                })
                .await
                .map_err(|_| {
                    Box::new(text(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "file helper is gone",
                    ))
                })?;
                let call = tokio::spawn(async move {
                    client
                        .write_file(tokio_stream::wrappers::ReceiverStream::new(rx))
                        .await
                        .map(tonic::Response::into_inner)
                });
                Ok(Sink::Remote { tx, call })
            }
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        match self {
            Sink::Local(writer) => writer.write(bytes).await.map_err(|e| failure(&e)),
            Sink::Remote { tx, call } => {
                for piece in bytes.chunks(crate::config::MAX_CHUNK_BYTES as usize) {
                    let sent = tx
                        .send(WriteFileRequest {
                            message: Some(WriteMessage::Data(piece.to_vec())),
                        })
                        .await;
                    if sent.is_err() {
                        // The helper ended the call early: its status is
                        // the real reason.
                        return Err(match call.await {
                            Ok(Err(status)) => failure(&status),
                            _ => Box::new(text(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "file helper is gone",
                            )),
                        });
                    }
                }
                Ok(())
            }
        }
    }

    /// Publishes the file: its entry and digest.
    async fn finish(self) -> Result<(String, u64, String), Failure> {
        match self {
            Sink::Local(writer) => writer
                .finish(0, "")
                .await
                .map(|(entry, sha256)| (entry.path, entry.size, sha256))
                .map_err(|e| failure(&e)),
            Sink::Remote { tx, call } => {
                drop(tx);
                match call.await {
                    Ok(Ok(response)) => {
                        let entry = response.entry.unwrap_or_default();
                        Ok((entry.path, entry.size, response.sha256))
                    }
                    Ok(Err(status)) => Err(failure(&status)),
                    Err(_) => Err(Box::new(text(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "file helper is gone",
                    ))),
                }
            }
        }
    }

    /// Discards the partial file.
    async fn abort(self) {
        match self {
            Sink::Local(writer) => writer.abort().await,
            // Cancelling the call (not ending it) makes the helper drop the
            // partial file.
            Sink::Remote { tx, call } => {
                call.abort();
                drop(tx);
            }
        }
    }
}

async fn put(
    acting: Acting,
    path: std::path::PathBuf,
    verified: &Verified,
    body: Body,
) -> Response {
    let mut sink = match Sink::open(acting, path, verified).await {
        Ok(sink) => sink,
        Err(response) => return *response,
    };
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                sink.abort().await;
                return text(StatusCode::BAD_REQUEST, format!("body: {e}"));
            }
        };
        if let Err(response) = sink.write(&chunk).await {
            sink.abort().await;
            return *response;
        }
    }
    match sink.finish().await {
        Ok((path, size, sha256)) => (
            StatusCode::CREATED,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "path": path, "size": size, "sha256": sha256 }).to_string(),
        )
            .into_response(),
        Err(response) => *response,
    }
}

/// `POST multipart/form-data` on a URL signed for `PUT`: the first file
/// part replaces the file, exactly like `PUT`.
async fn post_multipart(
    acting: Acting,
    path: std::path::PathBuf,
    verified: &Verified,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(boundary) = multipart::boundary(content_type) else {
        return text(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "POST to a signed URL takes multipart/form-data (or use PUT with a raw body)",
        );
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shown = path.display().to_string();
    let mut reader = multipart::FirstFile::new(&boundary);
    let mut sink: Option<Sink> = None;
    let mut acting = Some(acting);
    let mut stream = body.into_data_stream();
    let mut out = Vec::new();
    let mut done = false;
    while !done {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                if let Some(sink) = sink {
                    sink.abort().await;
                }
                return text(StatusCode::BAD_REQUEST, format!("body: {e}"));
            }
        };
        out.clear();
        let progress = reader.feed(&chunk, &mut out);
        // The destination is opened when the file part begins, so a form
        // with no file never creates or touches anything.
        if sink.is_none() && reader.started() {
            match Sink::open(acting.take().expect("opened once"), path.clone(), verified).await {
                Ok(opened) => sink = Some(opened),
                Err(response) => return *response,
            }
        }
        if let Some(open) = sink.as_mut() {
            if !out.is_empty() {
                if let Err(response) = open.write(&out).await {
                    if let Some(sink) = sink {
                        sink.abort().await;
                    }
                    return *response;
                }
            }
        }
        match progress {
            Ok(multipart::Progress::More) => {}
            Ok(multipart::Progress::Done) => done = true,
            Err(error) => {
                if let Some(sink) = sink {
                    sink.abort().await;
                }
                return multipart_error(error);
            }
        }
    }
    if let Err(error) = reader.finish() {
        if let Some(sink) = sink {
            sink.abort().await;
        }
        return multipart_error(error);
    }
    let Some(sink) = sink else {
        return multipart_error(multipart::Error::NoFilePart);
    };
    match sink.finish().await {
        Ok((published, _, _)) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!([{ "name": name, "path": if published.is_empty() { shown } else { published }, "type": "file" }])
                .to_string(),
        )
            .into_response(),
        Err(response) => *response,
    }
}

fn multipart_error(error: multipart::Error) -> Response {
    match error {
        multipart::Error::NoFilePart => text(
            StatusCode::BAD_REQUEST,
            "the multipart body has no file part (a part with a filename)",
        ),
        multipart::Error::Malformed(why) => text(
            StatusCode::BAD_REQUEST,
            format!("malformed multipart body: {why}"),
        ),
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
    acting: Acting,
    path: std::path::PathBuf,
    verified: &Verified,
    headers: &HeaderMap,
    head: bool,
) -> Response {
    // Size (and existence) first, as the user the URL acts as.
    let mut remote = match acting {
        Acting::Own(_) => None,
        Acting::Other(_, client) => Some(client),
    };
    let len = match remote.as_mut() {
        None => match tokio::fs::metadata(&path).await {
            Ok(m) if m.is_dir() => return text(StatusCode::CONFLICT, "is a directory"),
            Ok(m) => m.len(),
            Err(e) => return *failure(&crate::error::io_status(&e, &path)),
        },
        Some(client) => {
            let stat = client
                .stat(StatRequest {
                    path: path.display().to_string(),
                    ..Default::default()
                })
                .await;
            match stat {
                Ok(r) => {
                    let entry = r.into_inner().entry.unwrap_or_default();
                    if entry.r#type == FileType::Directory as i32 {
                        return text(StatusCode::CONFLICT, "is a directory");
                    }
                    entry.size
                }
                Err(status) => return *failure(&status),
            }
        }
    };
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
    if let Some(mut client) = remote {
        let stream = client
            .read_file(ReadFileRequest {
                path: path.display().to_string(),
                offset: start,
                length: body_len,
                ..Default::default()
            })
            .await;
        let stream = match stream {
            Ok(r) => r.into_inner(),
            Err(status) => return *failure(&status),
        };
        let bytes = stream.filter_map(|message| async move {
            match message {
                Ok(ReadFileResponse {
                    message: Some(ReadMessage::Chunk(chunk)),
                }) => Some(Ok(bytes::Bytes::from(chunk.data))),
                Ok(_) => None,
                Err(status) => Some(Err(std::io::Error::other(status.message().to_owned()))),
            }
        });
        return builder.body(Body::from_stream(bytes)).expect("response");
    }
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return *failure(&crate::error::io_status(&e, &path)),
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
            "",
            false,
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
            "",
            false,
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
    fn the_user_and_create_parents_are_signed_and_post_uses_a_put_url() {
        let ctx = ctx();
        let exp = SystemTime::now() + Duration::from_secs(60);
        let signed = sign(&ctx, "PUT", "/tmp/u", exp, "", "", "bob", true);
        assert!(signed.url_path.contains("&ru=bob") && signed.url_path.contains("&pp=1"));
        let ok = verify(&ctx, &Method::PUT, &uri(&signed.url_path)).unwrap();
        assert_eq!((ok.run_as.as_str(), ok.create_parents), ("bob", true));
        // The same URL takes a multipart POST, but not a GET.
        assert!(verify(&ctx, &Method::POST, &uri(&signed.url_path)).is_ok());
        assert_eq!(
            verify(&ctx, &Method::GET, &uri(&signed.url_path)).unwrap_err(),
            Refusal::Forbidden
        );
        // Neither the user nor the flag can be changed, added or dropped.
        for tampered in [
            signed.url_path.replace("&ru=bob", "&ru=root"),
            signed.url_path.replace("&ru=bob", ""),
            signed.url_path.replace("&pp=1", ""),
        ] {
            assert_eq!(
                verify(&ctx, &Method::PUT, &uri(&tampered)).unwrap_err(),
                Refusal::Forbidden,
                "{tampered}"
            );
        }
        let plain = sign(&ctx, "PUT", "/tmp/u", exp, "", "", "", false);
        let added = format!("{}&ru=root", plain.url_path.replace("&sig=", "&x=1&sig="));
        assert_eq!(
            verify(&ctx, &Method::PUT, &uri(&added)).unwrap_err(),
            Refusal::Forbidden,
            "a user cannot be added to a URL signed without one"
        );
        // A POST never works on a URL signed for GET.
        let get = sign(&ctx, "GET", "/tmp/u", exp, "", "", "", false);
        assert_eq!(
            verify(&ctx, &Method::POST, &uri(&get.url_path)).unwrap_err(),
            Refusal::Forbidden
        );
    }

    #[test]
    fn urls_signed_without_the_new_parts_keep_their_signatures() {
        // The signature of a URL with no user and no create-parents flag is
        // the original one (method, path, expiry, content type, name), so
        // outstanding URLs stay valid across an upgrade.
        let ctx = ctx();
        let key = ctx.auth().derive_key("files-v1");
        let exp = 1_900_000_000u64;
        let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
        for part in ["GET", "/tmp/a", &exp.to_string(), "", ""] {
            mac.update(part.as_bytes());
            mac.update(b"\n");
        }
        let legacy = mac.finalize().into_bytes().to_vec();
        assert_eq!(
            legacy,
            signature(&ctx, "GET", "/tmp/a", exp, &Extras::default())
        );
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
