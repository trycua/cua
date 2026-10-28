//! envd's `/files` routes: `GET` streams a file out of the sandbox
//! (cua-spacesd `ReadFile`), `POST` streams uploads in (`WriteFile`, or the
//! resumable chunk protocol over gRPC-Web), as `application/octet-stream`
//! (optionally gzip) or `multipart/form-data` with one part per file.
//! Files written for a non-root user are owned by that user, like envd's.

use super::{E2b, HostSandbox, request_user, resolve_path};
use crate::state::Record;
use axum::Extension;
use axum::body::Body;
use axum::extract::{Query, State};
use bytes::Bytes;
use cua_spacesd_client::pb;
use futures_util::StreamExt;
use http::{HeaderMap, Response, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest;
use std::sync::Arc;

#[derive(Debug, Default, Deserialize)]
pub struct FileQuery {
    pub path: Option<String>,
    pub username: Option<String>,
    /// A signed URL's `v1_<base64 sha256>` (`download_url` / `upload_url`).
    pub signature: Option<String>,
    pub signature_expiration: Option<i64>,
}

/// envd's v1 file-URL signature: `v1_` + unpadded base64 of
/// sha256(`path:operation:user:token[:expiration]`).
pub fn signature(path: &str, operation: &str, user: &str, token: &str, exp: Option<i64>) -> String {
    use base64::Engine;
    let raw = match exp {
        Some(x) => format!("{path}:{operation}:{user}:{token}:{x}"),
        None => format!("{path}:{operation}:{user}:{token}"),
    };
    let digest = sha2::Sha256::digest(raw.as_bytes());
    format!(
        "v1_{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

/// The record a `/files` request is for: by `E2b-Sandbox-Id` (or the host
/// name) and access token, or, for a signed URL (which carries neither),
/// by the record whose token produced the signature.
async fn files_record(
    e: &E2b,
    headers: &HeaderMap,
    host_id: Option<&str>,
    q: &FileQuery,
    operation: &str,
) -> Result<Record, Response<Body>> {
    let Some(sig) = q.signature.as_deref() else {
        // A request that came in by the sandbox's public host name (not the
        // SDK, which names the sandbox in a header) needs a signature.
        if host_id.is_some() && headers.get("e2b-sandbox-id").is_none() {
            return Err(files_error(
                StatusCode::UNAUTHORIZED,
                "missing signature query parameter",
            ));
        }
        return e.envd_record(headers, host_id).await;
    };
    if let Some(exp) = q.signature_expiration
        && exp < (crate::state::now_ms() / 1000) as i64
    {
        return Err(files_error(
            StatusCode::UNAUTHORIZED,
            "signature is already expired",
        ));
    }
    let path = q.path.clone().unwrap_or_default();
    let user = q.username.clone().unwrap_or_default();
    let id = headers
        .get("e2b-sandbox-id")
        .and_then(|v| v.to_str().ok())
        .or(host_id)
        .map(str::to_string);
    for r in e.store.all().await {
        if id.as_ref().is_some_and(|id| *id != r.id) {
            continue;
        }
        let want = signature(
            &path,
            operation,
            &user,
            &r.access_token,
            q.signature_expiration,
        );
        if crate::auth::same(&want, sig) {
            if r.paused {
                if !r.auto_resume {
                    return Err(super::not_found_edge(&r.id));
                }
                e.resume(&r).await.map_err(|err| {
                    super::edge_error(StatusCode::BAD_GATEWAY, &format!("resuming: {err}"), &r.id)
                })?;
            }
            return Ok(r);
        }
    }
    Err(files_error(StatusCode::UNAUTHORIZED, "invalid signature"))
}

/// An envd `/files` error: `{"code": <status>, "message": ...}`.
pub fn files_error(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            // Go's JSON encoder ends its documents with a newline.
            format!(
                "{}\n",
                json!({"code": status.as_u16(), "message": message.into()})
            ),
        ))
        .expect("response")
}

async fn user_and_home(
    e: &E2b,
    r: &Record,
    c: &cua_spacesd_client::SpacesdClient,
    headers: &HeaderMap,
    q: &FileQuery,
) -> Result<(String, String), Response<Body>> {
    let user = q
        .username
        .clone()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| request_user(headers, super::default_user_of(e, r)));
    if e.check_user(r, c, &user).await.is_err() {
        return Err(files_error(
            StatusCode::UNAUTHORIZED,
            format!("error looking up user '{user}': user: unknown user {user}"),
        ));
    }
    let home = e.home(r, &user).await;
    Ok((user, home))
}

/// `GET /files?path=&username=`.
pub async fn read(
    State(e): State<Arc<E2b>>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
    Query(q): Query<FileQuery>,
) -> Response<Body> {
    let host_id = host.as_ref().map(|Extension(HostSandbox(id))| id.as_str());
    let r = match files_record(&e, &headers, host_id, &q, "read").await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // No path is the user's home directory (a directory: the error below).
    let path = q
        .path
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".into());
    let c = match e.spacesd(&r).await {
        Ok(c) => c,
        Err(err) => return files_error(StatusCode::BAD_GATEWAY, err.to_string()),
    };
    let (user, home) = match user_and_home(&e, &r, &c, &headers, &q).await {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    let _ = user;
    let path = resolve_path(&path, &home);
    let call = pb::ReadFileRequest {
        path: path.clone(),
        offset: 0,
        length: 0,
        chunk_size: 0,
        compute_sha256: false,
        ..Default::default()
    };
    let mut stream = match c.filesystem().read_file(call).await {
        Ok(s) => s.into_inner(),
        Err(s) if s.code() == tonic::Code::NotFound => {
            return files_error(
                StatusCode::NOT_FOUND,
                format!("path '{path}' does not exist"),
            );
        }
        Err(s) if s.code() == tonic::Code::PermissionDenied => {
            // envd reads with root's rights.
            let driver = e.drivers.of(&r.id, &c).await;
            return read_as_user(&c, &driver, "root", &path).await;
        }
        Err(s) => return read_error(&s, &path),
    };
    // The first message is the entry: a directory is an error before any
    // byte is sent.
    use pb::read_file_response::Message;
    let mut pending: Option<Bytes> = None;
    let name;
    match stream.next().await {
        Some(Ok(pb::ReadFileResponse {
            message: Some(Message::Entry(entry)),
        })) => {
            if entry.r#type == pb::FileType::Directory as i32 {
                return files_error(
                    StatusCode::BAD_REQUEST,
                    format!("path '{path}' is a directory"),
                );
            }
            name = entry.name;
        }
        Some(Ok(pb::ReadFileResponse {
            message: Some(Message::Chunk(chunk)),
        })) => {
            name = path.rsplit('/').next().unwrap_or_default().to_string();
            pending = Some(chunk.data.into());
        }
        Some(Ok(_)) | None => {
            name = path.rsplit('/').next().unwrap_or_default().to_string();
        }
        Some(Err(s)) if s.code() == tonic::Code::NotFound => {
            return files_error(
                StatusCode::NOT_FOUND,
                format!("path '{path}' does not exist"),
            );
        }
        Some(Err(s)) => return read_error(&s, &path),
    }
    let rest = stream.filter_map(|item| async move {
        match item {
            Ok(pb::ReadFileResponse {
                message: Some(Message::Chunk(c)),
            }) => Some(Ok::<Bytes, std::io::Error>(c.data.into())),
            Ok(_) => None,
            Err(s) => Some(Err(std::io::Error::other(s.message().to_string()))),
        }
    });
    let body = futures_util::stream::iter(pending.map(Ok)).chain(rest);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", name.replace('"', "")),
        )
        .body(Body::from_stream(body))
        .expect("response")
}

/// envd's answer for a read that failed in cua-spacesd.
fn read_error(s: &tonic::Status, path: &str) -> Response<Body> {
    if s.message().contains("is a directory") {
        return files_error(
            StatusCode::BAD_REQUEST,
            format!("path '{path}' is a directory"),
        );
    }
    files_error(status_of(s), s.message().to_string())
}

/// A file the driver may not read, read as `user` (`sudo -n -u ... cat`),
/// streamed.
async fn read_as_user(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
    path: &str,
) -> Response<Body> {
    use pb::process_data::Output;
    use pb::process_event::Event;
    let check = crate::guest::run_as(
        c,
        driver,
        user,
        "[ -e \"$1\" ] || exit 44; [ -d \"$1\" ] && exit 21; [ -r \"$1\" ] || exit 13; exit 0",
        &[path],
    )
    .await;
    match check.map(|o| o.status.code) {
        Ok(Some(0)) => {}
        Ok(Some(44)) => {
            return files_error(
                StatusCode::NOT_FOUND,
                format!("path '{path}' does not exist"),
            );
        }
        Ok(Some(21)) => {
            return files_error(
                StatusCode::BAD_REQUEST,
                format!("path '{path}' is a directory"),
            );
        }
        Ok(_) => return files_error(StatusCode::FORBIDDEN, format!("permission denied: {path}")),
        Err(e) => return files_error(StatusCode::BAD_GATEWAY, e),
    }
    let (command, args, u) =
        crate::guest::as_user(driver, user, "cat".into(), vec!["--".into(), path.into()]);
    let req = pb::StartProcessRequest {
        config: Some(pb::ProcessConfig {
            command,
            args,
            user: u,
            ..Default::default()
        }),
        kill_on_disconnect: true,
        ..Default::default()
    };
    let stream = match c.process().start_process(req).await {
        Ok(s) => s.into_inner(),
        Err(s) => return files_error(status_of(&s), s.message().to_string()),
    };
    let body = stream.filter_map(|m| async move {
        match m {
            Ok(m) => match m.event.and_then(|e| e.event) {
                Some(Event::Data(d)) => match d.output {
                    Some(Output::Stdout(b)) => Some(Ok::<Bytes, std::io::Error>(Bytes::from(b))),
                    _ => None,
                },
                Some(Event::End(e)) if e.exit_code != Some(0) => {
                    Some(Err(std::io::Error::other("reading the file failed")))
                }
                _ => None,
            },
            Err(s) => Some(Err(std::io::Error::other(s.message().to_string()))),
        }
    });
    let name = path.rsplit('/').next().unwrap_or_default().replace('"', "");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .body(Body::from_stream(body))
        .expect("response")
}

/// `POST /files?path=&username=`.
pub async fn write(
    State(e): State<Arc<E2b>>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
    Query(q): Query<FileQuery>,
    body: Body,
) -> Response<Body> {
    let host_id = host.as_ref().map(|Extension(HostSandbox(id))| id.as_str());
    let r = match files_record(&e, &headers, host_id, &q, "write").await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // E2B file metadata (envd 0.6.2): `X-Metadata-<key>` headers, stored
    // as `user.e2b.<key>` extended attributes on every file written.
    let metadata: std::collections::BTreeMap<String, String> = headers
        .iter()
        .filter_map(|(k, v)| {
            let key = k.as_str().strip_prefix("x-metadata-")?;
            Some((key.to_ascii_lowercase(), v.to_str().ok()?.to_string()))
        })
        .collect();
    let c = match e.spacesd(&r).await {
        Ok(c) => c,
        Err(err) => return files_error(StatusCode::BAD_GATEWAY, err.to_string()),
    };
    let (user, home) = match user_and_home(&e, &r, &c, &headers, &q).await {
        Ok(x) => x,
        Err(resp) => return resp,
    };
    let ctype = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let gzip = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
    let mut written: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(boundary) = multipart_boundary(&ctype) {
        let mut parts = Multipart::new(body.into_data_stream(), boundary);
        loop {
            let part = match parts.next_part().await {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(err) => return files_error(StatusCode::BAD_REQUEST, err),
            };
            // `path` in the query names the (first) file; otherwise each part
            // carries its own path as its file name.
            let dest = (if written.is_empty() {
                q.path.clone().filter(|p| !p.is_empty())
            } else {
                None
            })
            .or_else(|| part.filename.clone().filter(|f| !f.is_empty()))
            // No path at all is the user's home directory (a directory: an error below).
            .unwrap_or_else(|| ".".into());
            let path = resolve_path(&dest, &home);
            if seen.contains(&path) {
                // envd keeps the first file and refuses the request.
                return files_error(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "you cannot upload multiple files to the same path '{path}' in one upload request, only the first specified file was uploaded"
                    ),
                );
            }
            seen.insert(path.clone());
            let mut w = match Writer::begin(&c, &path, &user, &e.drivers.of(&r.id, &c).await).await
            {
                Ok(w) => w,
                Err(err) => return fs_response(err),
            };
            loop {
                match parts.read_chunk().await {
                    Ok(Some(chunk)) => {
                        if let Err(err) = w.write(chunk).await {
                            return fs_response(err);
                        }
                    }
                    Ok(None) => break,
                    Err(err) => return files_error(StatusCode::BAD_REQUEST, err),
                }
            }
            match w.finish().await {
                Ok(v) => written.push(finish_written(&e, &r, &c, v, &metadata).await),
                Err(err) => return fs_response(err),
            }
        }
    } else {
        let dest = q
            .path
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| ".".into());
        let path = resolve_path(&dest, &home);
        let mut w = match Writer::begin(&c, &path, &user, &e.drivers.of(&r.id, &c).await).await {
            Ok(w) => w,
            Err(err) => return fs_response(err),
        };
        let mut gz = gzip.then(|| flate2::write::GzDecoder::new(Vec::new()));
        let mut data = body.into_data_stream();
        while let Some(chunk) = data.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(err) => return files_error(StatusCode::BAD_REQUEST, err.to_string()),
            };
            let out = match gz.as_mut() {
                Some(d) => {
                    use std::io::Write;
                    if let Err(err) = d.write_all(&chunk) {
                        return files_error(StatusCode::BAD_REQUEST, format!("gzip: {err}"));
                    }
                    Bytes::from(std::mem::take(d.get_mut()))
                }
                None => chunk,
            };
            if !out.is_empty()
                && let Err(err) = w.write(out).await
            {
                return fs_response(err);
            }
        }
        if let Some(d) = gz {
            match d.finish() {
                Ok(rest) if !rest.is_empty() => {
                    if let Err(err) = w.write(Bytes::from(rest)).await {
                        return fs_response(err);
                    }
                }
                Ok(_) => {}
                Err(err) => return files_error(StatusCode::BAD_REQUEST, format!("gzip: {err}")),
            }
        }
        match w.finish().await {
            Ok(v) => written.push(finish_written(&e, &r, &c, v, &metadata).await),
            Err(err) => return fs_response(err),
        }
    }
    let _ = ctype;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(Value::Array(written).to_string()))
        .expect("response")
}

use crate::guestfs::{FsError, Multipart, Writer, Written, multipart_boundary, status_of};

fn fs_response(e: FsError) -> Response<Body> {
    files_error(e.0, e.1)
}

fn written_json(w: Written) -> Value {
    json!({"name": w.name, "type": "file", "path": w.path})
}

/// A written file's entry, with its metadata stored first.
async fn finish_written(
    e: &E2b,
    r: &Record,
    c: &cua_spacesd_client::SpacesdClient,
    w: Written,
    metadata: &std::collections::BTreeMap<String, String>,
) -> Value {
    if metadata.is_empty() {
        return written_json(w);
    }
    let driver = e.drivers.of(&r.id, c).await;
    if let Err(err) = crate::guest::set_metadata(c, &driver, &w.path, metadata).await {
        tracing::warn!("compat: metadata on {}: {err}", w.path);
    }
    e.store
        .update(&r.id, |r| {
            r.extra
                .insert("metadata".into(), serde_json::Value::Bool(true));
        })
        .await;
    let mut v = written_json(w);
    v["metadata"] = json!(metadata);
    v
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Signatures the real SDK made (`tests/compat/e2b/gen_signature_vectors.py`).
    #[test]
    fn signatures_match_the_sdk_vectors() {
        let v: Value = serde_json::from_str(include_str!(
            "../../../../../../tests/compat/e2b/fixtures/signature-vectors.json"
        ))
        .unwrap();
        let list = v["vectors"].as_array().unwrap();
        assert!(list.len() >= 50);
        for x in list {
            let got = signature(
                x["path"].as_str().unwrap(),
                x["operation"].as_str().unwrap(),
                x["user"].as_str().unwrap(),
                x["token"].as_str().unwrap(),
                x["expiration"].as_i64(),
            );
            assert_eq!(got, x["signature"].as_str().unwrap(), "{x}");
        }
    }

    #[test]
    fn signature_matches_the_sdk() {
        // e2b.sandbox.signature.get_signature("/tmp/a", "read", None, "tok")
        assert_eq!(
            signature("/tmp/a", "read", "", "tok", None),
            "v1_".to_string()
                + &{
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .encode(sha2::Sha256::digest(b"/tmp/a:read::tok"))
                }
        );
    }
}
