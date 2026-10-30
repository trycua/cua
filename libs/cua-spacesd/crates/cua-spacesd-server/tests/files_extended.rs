// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Extended attributes as file metadata, and `POST multipart/form-data` on
//! signed `PUT` URLs. Runs against the in-process server, or any driver with
//! `CUA_ENV_TEST_TARGET` (see `common`).

mod common;

use cua_proto::env::v1::watch_dir_response::Message as WatchMessage;
use cua_proto::env::v1::*;
use cua_spacesd_client::TransportPreference;
use tonic::Code;

use common::*;

fn ttl() -> Option<cua_proto::wkt::Duration> {
    Some(cua_proto::wkt::Duration {
        seconds: 300,
        nanos: 0,
    })
}

async fn signed_put(t: &Target, path: &str, create_parents: bool) -> String {
    let c = t.client(TransportPreference::Native).await;
    c.filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: path.into(),
            method: SignedUrlMethod::Put as i32,
            ttl: ttl(),
            create_parents,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .url_path
}

fn form(boundary: &str, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, filename, data) in parts {
        out.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(f) = filename {
            out.extend_from_slice(format!("; filename=\"{f}\"").as_bytes());
        }
        out.extend_from_slice(b"\r\n\r\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

async fn post(
    t: &Target,
    url_path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (http::StatusCode, Vec<u8>) {
    let mut request = http::Request::post(t.endpoint().http_url(url_path));
    if let Some(ct) = content_type {
        request = request.header("content-type", ct);
    }
    let response = http_client()
        .request(request.body(full(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (status, body_bytes(response, 1 << 20).await)
}

async fn read_all(t: &Target, path: &str) -> Result<Vec<u8>, tonic::Status> {
    let c = t.client(TransportPreference::Native).await;
    let mut stream = c
        .filesystem()
        .read_file(ReadFileRequest {
            path: path.into(),
            ..Default::default()
        })
        .await?
        .into_inner();
    let mut out = Vec::new();
    while let Some(m) = stream.message().await? {
        if let Some(read_file_response::Message::Chunk(chunk)) = m.message {
            out.extend_from_slice(&chunk.data);
        }
    }
    Ok(out)
}

#[tokio::test]
async fn multipart_post_writes_the_first_file_part_like_put() {
    let t = target().await;
    let multipart = "multipart/form-data; boundary=XXboundaryXX";
    let content: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let path = t.path("upload/form.bin");

    // The parent is created only when the URL says so.
    let no_parents = signed_put(&t, &path, false).await;
    let (status, _) = post(
        &t,
        &no_parents,
        Some(multipart),
        form("XXboundaryXX", &[("file", Some("a.bin"), &content)]),
    )
    .await;
    assert_ne!(status, http::StatusCode::OK, "missing parent");
    assert!(read_all(&t, &path).await.is_err());

    let url = signed_put(&t, &path, true).await;
    let (status, body) = post(
        &t,
        &url,
        Some(multipart),
        form(
            "XXboundaryXX",
            &[
                ("note", None, b"ignored field"),
                ("file", Some("whatever.bin"), &content),
                ("file", Some("second.bin"), b"ignored second file"),
            ],
        ),
    )
    .await;
    assert_eq!(
        status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let listing: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listing.as_array().unwrap().len(), 1);
    assert_eq!(listing[0]["name"], "form.bin", "named by the signed path");
    assert_eq!(listing[0]["type"], "file");
    // The listing reports host paths: `\` separators on Windows.
    assert!(listing[0]["path"]
        .as_str()
        .unwrap()
        .replace('\\', "/")
        .ends_with("upload/form.bin"));
    assert!(read_all(&t, &path).await.unwrap() == content);

    // The same URL replaces it, atomically, and PUT still works on it.
    let (status, _) = post(
        &t,
        &url,
        Some(multipart),
        form("XXboundaryXX", &[("file", Some("x"), b"short")]),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(read_all(&t, &path).await.unwrap(), b"short");
    let put = http_client()
        .request(
            http::Request::put(t.endpoint().http_url(&url))
                .body(full(b"raw".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(put.status(), http::StatusCode::CREATED);
    assert_eq!(read_all(&t, &path).await.unwrap(), b"raw");
    t.cleanup().await;
}

#[tokio::test]
async fn multipart_refusals_leave_the_destination_alone() {
    let t = target().await;
    let path = t.path("keep.txt");
    let c = t.client(TransportPreference::Native).await;
    let url = signed_put(&t, &path, false).await;
    let (status, _) = post(
        &t,
        &url,
        Some("multipart/form-data; boundary=b"),
        form("b", &[("file", Some("keep"), b"original")]),
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);

    // Not multipart.
    let (status, _) = post(&t, &url, Some("application/json"), b"{}".to_vec()).await;
    assert_eq!(status, http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (status, _) = post(&t, &url, None, b"raw".to_vec()).await;
    assert_eq!(status, http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    // A form without a file part.
    let (status, body) = post(
        &t,
        &url,
        Some("multipart/form-data; boundary=b"),
        form("b", &[("field", None, b"v")]),
    )
    .await;
    assert_eq!(
        status,
        http::StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    // A body cut off inside the file part.
    let mut cut = form("b", &[("file", Some("f"), &vec![7u8; 100_000])]);
    cut.truncate(60_000);
    let (status, _) = post(&t, &url, Some("multipart/form-data; boundary=b"), cut).await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    // A URL signed for GET never takes a POST.
    let get = c
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: path.clone(),
            method: SignedUrlMethod::Get as i32,
            ttl: ttl(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .url_path;
    let (status, _) = post(
        &t,
        &get,
        Some("multipart/form-data; boundary=b"),
        form("b", &[("file", Some("f"), b"nope")]),
    )
    .await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);

    assert_eq!(read_all(&t, &path).await.unwrap(), b"original");
    // No temporary file is left beside it.
    let listed = c
        .filesystem()
        .list_dir(ListDirRequest {
            path: t.scratch.clone(),
            include_hidden: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let names: Vec<_> = listed.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["keep.txt".to_string()], "{names:?}");
    t.cleanup().await;
}

fn header(path: &str, namespace: &str, metadata: &[(&str, &str)]) -> WriteFileHeader {
    WriteFileHeader {
        path: path.into(),
        xattr_namespace: namespace.into(),
        metadata: metadata
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..Default::default()
    }
}

/// Writes `data` through `BeginUpload`, returning the status of the commit.
async fn write(
    t: &Target,
    transport: TransportPreference,
    h: WriteFileHeader,
    data: &[u8],
) -> Result<EntryInfo, tonic::Status> {
    let c = t.client(transport).await;
    let begun = c
        .filesystem()
        .begin_upload(BeginUploadRequest {
            header: Some(h),
            ..Default::default()
        })
        .await?
        .into_inner();
    c.filesystem()
        .upload_chunk(UploadChunkRequest {
            upload_id: begun.upload_id.clone(),
            offset: 0,
            data: data.to_vec(),
            ..Default::default()
        })
        .await?;
    Ok(c.filesystem()
        .commit_upload(CommitUploadRequest {
            upload_id: begun.upload_id,
            sha256: String::new(),
        })
        .await?
        .into_inner()
        .entry
        .unwrap())
}

#[tokio::test]
async fn metadata_is_stored_as_extended_attributes_and_reported() {
    let t = target().await;
    let c = t.client(TransportPreference::Native).await;
    let caps = c
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    let xattr = caps.features.iter().find(|f| f.name == "fs.xattr").unwrap();
    if !xattr.supported {
        eprintln!("skipped: {}", xattr.limitation);
        return;
    }
    let path = t.path("tagged.txt");
    let entry = match write(
        &t,
        TransportPreference::Native,
        header(
            &path,
            "",
            &[("owner", "alice"), ("note", "h\u{e9}llo, w\u{f6}rld")],
        ),
        b"payload",
    )
    .await
    {
        Ok(entry) => entry,
        Err(status) if status.code() == Code::FailedPrecondition => {
            eprintln!("skipped: {}", status.message());
            return;
        }
        Err(status) => panic!("{status}"),
    };
    // The commit already reports them, under the default namespace.
    assert_eq!(
        entry.metadata.get("owner").map(String::as_str),
        Some("alice")
    );
    assert_eq!(
        entry.metadata.get("note").map(String::as_str),
        Some("h\u{e9}llo, w\u{f6}rld")
    );

    for transport in TRANSPORTS {
        let c = t.client(transport).await;
        let stat = c
            .filesystem()
            .stat(StatRequest {
                path: path.clone(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner()
            .entry
            .unwrap();
        assert_eq!(stat.metadata.len(), 2, "{transport:?}");
        let listed = c
            .filesystem()
            .list_dir(ListDirRequest {
                path: t.scratch.clone(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let tagged = listed
            .entries
            .iter()
            .find(|e| e.name == "tagged.txt")
            .unwrap();
        assert_eq!(
            tagged.metadata.get("owner").map(String::as_str),
            Some("alice")
        );
        let mut stream = c
            .filesystem()
            .read_file(ReadFileRequest {
                path: path.clone(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let first = stream.message().await.unwrap().unwrap();
        let Some(read_file_response::Message::Entry(read_entry)) = first.message else {
            panic!("ReadFile starts with the entry");
        };
        assert_eq!(
            read_entry.metadata.get("owner").map(String::as_str),
            Some("alice")
        );
    }

    // Another namespace sees nothing; its own values stay separate.
    let stat_in = |ns: &'static str| {
        let c = c.clone();
        let path = path.clone();
        async move {
            c.filesystem()
                .stat(StatRequest {
                    path,
                    xattr_namespace: ns.into(),
                    ..Default::default()
                })
                .await
                .map(|r| r.into_inner().entry.unwrap().metadata)
        }
    };
    assert!(stat_in("user.e2b.").await.unwrap().is_empty());
    let entry = write(
        &t,
        TransportPreference::GrpcWeb,
        header(&path, "user.e2b.", &[("owner", "bob")]),
        b"second",
    )
    .await
    .unwrap();
    assert_eq!(entry.metadata.get("owner").map(String::as_str), Some("bob"));
    assert_eq!(entry.metadata.len(), 1);
    // The rewrite replaced the file, so the default namespace is gone with it.
    assert!(stat_in("").await.unwrap().is_empty());
    assert_eq!(
        stat_in("user.e2b.")
            .await
            .unwrap()
            .get("owner")
            .map(String::as_str),
        Some("bob")
    );

    // Watch events carry the entry's attributes.
    let mut watch = c
        .filesystem()
        .watch_dir(WatchDirRequest {
            path: t.scratch.clone(),
            recursive: false,
            xattr_namespace: "user.e2b.".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let Some(WatchMessage::Started(_)) = watch.message().await.unwrap().unwrap().message else {
        panic!("started first");
    };
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let watched = t.path("watched.txt");
    write(
        &t,
        TransportPreference::Native,
        header(&watched, "user.e2b.", &[("seen", "yes")]),
        b"w",
    )
    .await
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut carried = false;
    while std::time::Instant::now() < deadline && !carried {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), watch.message())
            .await
            .ok()
            .and_then(|m| m.ok().flatten());
        if let Some(WatchDirResponse {
            message: Some(WatchMessage::Event(e)),
        }) = message
        {
            carried = e.path.ends_with("watched.txt")
                && e.metadata.get("seen").map(String::as_str) == Some("yes");
        }
    }
    assert!(
        carried,
        "an event for the written file carried its metadata"
    );
    t.cleanup().await;
}

#[tokio::test]
async fn metadata_limits_and_namespaces_are_validated() {
    let t = target().await;
    let path = t.path("v.txt");
    let invalid = |status: tonic::Status| {
        assert_eq!(status.code(), Code::InvalidArgument, "{status}");
    };
    invalid(
        write(
            &t,
            TransportPreference::Native,
            header(&path, "", &[("bad key", "v")]),
            b"x",
        )
        .await
        .unwrap_err(),
    );
    invalid(
        write(
            &t,
            TransportPreference::Native,
            header(&path, "", &[("", "v")]),
            b"x",
        )
        .await
        .unwrap_err(),
    );
    invalid(
        write(
            &t,
            TransportPreference::Native,
            header(&path, "", &[("k", &"v".repeat(1025))]),
            b"x",
        )
        .await
        .unwrap_err(),
    );
    let many: Vec<(String, String)> = (0..33).map(|i| (format!("k{i}"), "v".into())).collect();
    let many_ref: Vec<(&str, &str)> = many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    invalid(
        write(
            &t,
            TransportPreference::Native,
            header(&path, "", &many_ref),
            b"x",
        )
        .await
        .unwrap_err(),
    );
    for namespace in ["trusted.x.", "user.x", "e2b.", "user.a b."] {
        invalid(
            write(
                &t,
                TransportPreference::Native,
                header(&path, namespace, &[("k", "v")]),
                b"x",
            )
            .await
            .unwrap_err(),
        );
    }
    let c = t.client(TransportPreference::Native).await;
    invalid(
        c.filesystem()
            .stat(StatRequest {
                path: t.scratch.clone(),
                xattr_namespace: "system.".into(),
                ..Default::default()
            })
            .await
            .unwrap_err(),
    );
    // Nothing was created by the refused writes.
    assert!(read_all(&t, &path).await.is_err());
    t.cleanup().await;
}
