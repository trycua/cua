// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `files`: `FilesystemService` round trips in the doctor's own scratch
//! directory (removed at the end): a chunked 8 MiB upload and download with
//! matching SHA-256, move/list/remove, a directory watch event, a signed
//! `/files` URL fetched without credentials, and the chunk size limit.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, UploadOptions};
use futures_util::StreamExt as _;
use sha2::{Digest, Sha256};

use crate::{Ctx, Recorder};

/// Size of the round-trip blob.
const BLOB_BYTES: usize = 8 * 1024 * 1024;

fn blob() -> Vec<u8> {
    (0..BLOB_BYTES).map(|i| (i % 251) as u8).collect()
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("files") {
        return;
    }
    let dir = ctx.scratch.clone();
    if let Err(error) = ctx.client.make_dir(&dir).await {
        rec.push(
            Check::new(
                "files.roundtrip",
                Status::Fail,
                format!("MakeDir {dir}: {error}"),
            ),
            &["core"],
        )
        .await;
        return;
    }
    let blob_path = join(&dir, "blob.bin");
    rec.run(
        "files.roundtrip",
        &["core"],
        Duration::from_secs(60),
        async {
            let data = blob();
            let want = hex::encode(Sha256::digest(&data));
            let options = UploadOptions {
                chunk_size: 1024 * 1024,
                ..UploadOptions::default()
            };
            if let Err(error) = ctx.client.upload(&blob_path, data, options).await {
                return Check::new("files.roundtrip", Status::Fail, format!("upload: {error}"));
            }
            let size = match ctx.client.stat(&blob_path).await {
                Ok(entry) => entry.size,
                Err(error) => {
                    return Check::new("files.roundtrip", Status::Fail, format!("stat: {error}"))
                }
            };
            match ctx.client.download(&blob_path).await {
                Ok(bytes) => {
                    let got = hex::encode(Sha256::digest(&bytes));
                    Check::new(
                        "files.roundtrip",
                        super::verdict(got == want && size == BLOB_BYTES as u64),
                        format!(
                            "8 MiB in 1 MiB chunks: size {size}, sha256 {}",
                            if got == want { "matches" } else { "DIFFERS" }
                        ),
                    )
                    .fact("sha256", got)
                }
                Err(error) => Check::new(
                    "files.roundtrip",
                    Status::Fail,
                    format!("download: {error}"),
                ),
            }
        },
    )
    .await;

    rec.run("files.ops", &["core"], Duration::from_secs(20), async {
        let moved = join(&dir, "moved.bin");
        let mut fs = ctx.client.filesystem();
        if let Err(status) = fs
            .r#move(pb::MoveRequest {
                source: blob_path.clone(),
                destination: moved.clone(),
                overwrite: true,
                create_parents: false,
            })
            .await
        {
            return Check::new(
                "files.ops",
                Status::Fail,
                format!("Move: {}", status.message()),
            );
        }
        let listed = match ctx.client.list_dir(&dir, 1).await {
            Ok(entries) => entries,
            Err(error) => {
                return Check::new("files.ops", Status::Fail, format!("ListDir: {error}"))
            }
        };
        let names: Vec<String> = listed
            .iter()
            .map(|e| {
                e.path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        if let Err(error) = ctx.client.remove(&moved, false).await {
            return Check::new("files.ops", Status::Fail, format!("Remove: {error}"));
        }
        let gone = ctx.client.stat(&moved).await.is_err();
        Check::new(
            "files.ops",
            super::verdict(
                names.iter().any(|n| n == "moved.bin")
                    && !names.iter().any(|n| n == "blob.bin")
                    && gone,
            ),
            format!("move, list ({names:?}) and remove"),
        )
    })
    .await;

    rec.run(
        "files.watch",
        &["feature:fs_watch"],
        Duration::from_secs(20),
        async {
            let mut fs = ctx.client.filesystem();
            let mut stream = match fs
                .watch_dir(pb::WatchDirRequest {
                    path: dir.clone(),
                    recursive: false,
                    keepalive_interval: None,
                })
                .await
            {
                Ok(r) => r.into_inner(),
                Err(status) => {
                    return Check::new(
                        "files.watch",
                        Status::Fail,
                        format!("WatchDir: {}", status.message()),
                    )
                }
            };
            // Wait for `started` so the watch is armed before the write.
            for _ in 0..10 {
                match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
                    Ok(Some(Ok(m)))
                        if matches!(
                            m.message,
                            Some(pb::watch_dir_response::Message::Started(_))
                        ) =>
                    {
                        break
                    }
                    Ok(Some(Ok(_))) => continue,
                    _ => return Check::new("files.watch", Status::Fail, "WatchDir never started"),
                }
            }
            let target = join(&dir, "watched.txt");
            if let Err(error) = ctx
                .client
                .upload(&target, &b"watch"[..], UploadOptions::default())
                .await
            {
                return Check::new("files.watch", Status::Fail, format!("write: {error}"));
            }
            for _ in 0..200 {
                match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
                    Ok(Some(Ok(m))) => {
                        if let Some(pb::watch_dir_response::Message::Event(event)) = m.message {
                            if event.path.ends_with("watched.txt") {
                                return Check::new(
                                    "files.watch",
                                    Status::Pass,
                                    format!(
                                        "{:?} event for watched.txt",
                                        pb::FsEventType::try_from(event.r#type).unwrap_or_default()
                                    ),
                                );
                            }
                        }
                    }
                    _ => break,
                }
            }
            Check::new(
                "files.watch",
                Status::Fail,
                "no watch event for a new file within 10 s",
            )
        },
    )
    .await;

    rec.run(
        "files.signed_url",
        &["core"],
        Duration::from_secs(20),
        async {
            let path = join(&dir, "signed.txt");
            let body = format!("signed-{}", ctx.nonce);
            if let Err(error) = ctx
                .client
                .upload(&path, body.as_bytes(), UploadOptions::default())
                .await
            {
                return Check::new("files.signed_url", Status::Fail, format!("write: {error}"));
            }
            let signed = match ctx
                .client
                .filesystem()
                .create_signed_url(pb::CreateSignedUrlRequest {
                    path: path.clone(),
                    method: pb::SignedUrlMethod::Get as i32,
                    ttl: Some(pbjson_types::Duration {
                        seconds: 60,
                        nanos: 0,
                    }),
                    ..Default::default()
                })
                .await
            {
                Ok(r) => r.into_inner(),
                Err(status) => {
                    return Check::new(
                        "files.signed_url",
                        Status::Fail,
                        format!("CreateSignedUrl: {}", status.message()),
                    )
                }
            };
            // Fetch with no credentials at all: only the signature authorizes it.
            let endpoint = ctx.client.endpoint();
            match plain_get(endpoint.host(), endpoint.port(), &signed.url_path).await {
                Ok((status, got)) => Check::new(
                    "files.signed_url",
                    super::verdict(status == 200 && got == body.as_bytes()),
                    format!(
                        "unauthenticated GET of the signed URL: HTTP {status}, {} bytes",
                        got.len()
                    ),
                ),
                Err(error) => Check::new("files.signed_url", Status::Fail, format!("GET: {error}")),
            }
        },
    )
    .await;

    rec.run(
        "files.chunk_limit",
        &["core"],
        Duration::from_secs(30),
        async {
            let limit = ctx
                .caps
                .limits
                .as_ref()
                .map(|l| l.max_chunk_bytes)
                .unwrap_or(0) as usize;
            if limit == 0 {
                return Check::new(
                    "files.chunk_limit",
                    Status::Fail,
                    "GetCapabilities reports no max_chunk_bytes",
                );
            }
            let mut fs = ctx.client.filesystem();
            let begun = match fs
                .begin_upload(pb::BeginUploadRequest {
                    header: Some(pb::WriteFileHeader {
                        path: join(&dir, "oversize.bin"),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .await
            {
                Ok(r) => r.into_inner(),
                Err(status) => {
                    return Check::new(
                        "files.chunk_limit",
                        Status::Fail,
                        format!("BeginUpload: {}", status.message()),
                    )
                }
            };
            let over = vec![0u8; limit + 1];
            let result = fs
                .upload_chunk(pb::UploadChunkRequest {
                    upload_id: begun.upload_id.clone(),
                    offset: 0,
                    data: over,
                })
                .await;
            let _ = fs
                .abort_upload(pb::AbortUploadRequest {
                    upload_id: begun.upload_id,
                })
                .await;
            match result {
                Err(status) => Check::new(
                    "files.chunk_limit",
                    super::verdict(matches!(
                        status.code(),
                        tonic::Code::ResourceExhausted
                            | tonic::Code::InvalidArgument
                            | tonic::Code::OutOfRange
                    )),
                    format!(
                        "a {} byte chunk was refused ({:?})",
                        limit + 1,
                        status.code()
                    ),
                )
                .fact("max_chunk_bytes", limit),
                Ok(_) => Check::new(
                    "files.chunk_limit",
                    Status::Fail,
                    format!("a chunk over max_chunk_bytes ({limit}) was accepted"),
                ),
            }
        },
    )
    .await;

    let _ = ctx.client.remove(&dir, true).await;
}

/// A bare HTTP/1.1 GET with no credentials (bounded to 1 MiB).
pub(crate) async fn plain_get(
    host: &str,
    port: u16,
    path: &str,
) -> std::io::Result<(u16, Vec<u8>)> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let mut stream = tokio::net::TcpStream::connect((host, port)).await?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    (&mut stream)
        .take(1024 * 1024)
        .read_to_end(&mut raw)
        .await?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("no HTTP header terminator"))?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut body = raw[split + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body = dechunk(&body);
    }
    Ok((status, body))
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..10_000 {
        let Some(line_end) = data.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size = usize::from_str_radix(String::from_utf8_lossy(&data[..line_end]).trim(), 16)
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = line_end + 2;
        if data.len() < start + size {
            break;
        }
        out.extend_from_slice(&data[start..start + size]);
        data = &data[(start + size + 2).min(data.len())..];
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn dechunks_http_bodies() {
        assert_eq!(
            super::dechunk(b"5\r\nhello\r\n1\r\n!\r\n0\r\n\r\n"),
            b"hello!"
        );
        assert_eq!(super::dechunk(b"0\r\n\r\n"), b"");
    }
}
