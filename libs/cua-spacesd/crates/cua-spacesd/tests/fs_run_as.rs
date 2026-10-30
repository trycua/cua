// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `FilesystemService` `run_as`: file operations and signed URLs performed
//! by the `cua-spacesd fs-as` helper, the real binary, as another guest
//! user. Unix only.
//!
//! Lanes sharing the assertions:
//! - the helper for the user the test runs as (a server option makes the
//!   agent use a helper even for its own user), which exercises the whole
//!   forwarding machinery without privilege and runs everywhere;
//! - a different user (`nobody`, or `CUA_TEST_FS_AS_USER`) when the test
//!   process is root or has passwordless `sudo`, which also proves the
//!   identity switch: ownership, permission denials and groups;
//! - a running guest: with `CUA_ENV_TEST_TARGET=http://host:port` and
//!   `CUA_ENV_TEST_TOKEN` the same assertions run against that cua-spacesd
//!   (a Linux Space whose agent runs as `cua` with passwordless `sudo`),
//!   where the other user is `CUA_TEST_FS_AS_USER` (default `nobody`).
//!   Every file the test touches is made and checked through the agent, so
//!   the test process needs no access to the guest.
#![cfg(unix)]

use std::process::Command;

use cua_proto::env::v1::*;
use cua_spacesd_client::{
    ConnectOptions, DownloadOptions, FileAccess, SpacesdClient, TransportPreference, UploadOptions,
};
use cua_spacesd_server::tonic::Code;
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "fs-run-as-token-0123456789abcdef";

struct Agent {
    client: SpacesdClient,
    /// Where `/files` is reached (plain HTTP).
    addr: std::net::SocketAddr,
    /// A guest under test (`CUA_ENV_TEST_TARGET`), not an in-process agent.
    remote: bool,
    _dirs: Vec<tempfile::TempDir>,
}

/// The guest under test, when the lane is a running one.
fn remote_target() -> Option<(String, String)> {
    let url = std::env::var("CUA_ENV_TEST_TARGET")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some((url, std::env::var("CUA_ENV_TEST_TOKEN").unwrap_or_default()))
}

fn current_user_local() -> String {
    let out = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

async fn agent(for_self: bool) -> Agent {
    if let Some((url, token)) = remote_target() {
        let addr = url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .parse()
            .expect("CUA_ENV_TEST_TARGET is http://<ip>:<port>");
        let client = SpacesdClient::connect(
            ConnectOptions::parse(&url)
                .unwrap()
                .transport(TransportPreference::Native)
                .probe(false)
                .token(token),
        )
        .await
        .unwrap();
        return Agent {
            client,
            addr,
            remote: true,
            _dirs: vec![],
        };
    }
    let data = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        data_dir: data.path().to_path_buf(),
        fs_helper_exe: Some(env!("CARGO_BIN_EXE_cua-spacesd").into()),
        fs_helper_for_self: for_self,
        shutdown_grace: std::time::Duration::from_secs(1),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some(TOKEN.into()));
    let server = ServerBuilder::new(ctx).build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    let client = SpacesdClient::connect(
        ConnectOptions::parse(&format!("http://{addr}"))
            .unwrap()
            .transport(TransportPreference::Native)
            .probe(false)
            .token(TOKEN),
    )
    .await
    .unwrap();
    Agent {
        client,
        addr,
        remote: false,
        _dirs: vec![data],
    }
}

/// The user the agent itself runs as (the guest's, for a running guest).
async fn agent_user(a: &Agent) -> String {
    sh(a, "id -un").await.trim().to_owned()
}

/// Runs a shell line in the guest through the agent; stdout.
async fn sh(a: &Agent, script: &str) -> String {
    let out = a
        .client
        .run(cua_spacesd_client::Command::shell(script))
        .await
        .unwrap();
    assert_eq!(
        out.status.code,
        Some(0),
        "{script}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A fresh directory every user can create files in, removed on drop of the
/// returned guard by [`Scratch::remove`] (the agent's own user owns it).
struct Scratch {
    path: String,
}

impl Scratch {
    async fn new(a: &Agent, mode: &str) -> Self {
        let path = sh(
            a,
            &format!(
                "d=$(mktemp -d /tmp/cua-fs-as-XXXXXX) && chmod {mode} \"$d\" && printf %s \"$d\""
            ),
        )
        .await;
        Scratch { path }
    }

    async fn remove(self, a: &Agent) {
        let _ = a
            .client
            .run(cua_spacesd_client::Command::shell(format!(
                "rm -rf '{}'",
                self.path
            )))
            .await;
    }
}

/// One HTTP/1.1 request, `Connection: close`; returns status, headers
/// (lowercased block) and body.
async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut request =
        format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response head");
    let head = String::from_utf8_lossy(&raw[..split]).to_lowercase();
    let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut body = raw[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    (status, head, body)
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&data[..line_end]).unwrap().trim(), 16)
                .unwrap();
        data = &data[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

async fn signed(
    client: &SpacesdClient,
    path: &str,
    method: SignedUrlMethod,
    run_as: &str,
    create_parents: bool,
) -> String {
    client
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: path.into(),
            method: method as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 300,
                nanos: 0,
            }),
            run_as: run_as.into(),
            create_parents,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .url_path
}

fn multipart(boundary: &str, name: &str, data: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// Downloads `path` as `user`.
async fn read_as(
    client: &SpacesdClient,
    path: &str,
    user: &str,
) -> cua_spacesd_client::Result<Vec<u8>> {
    let mut out = Vec::new();
    client
        .download_with(
            path,
            DownloadOptions {
                run_as: Some(user.into()),
                ..Default::default()
            },
            &mut out,
        )
        .await?;
    Ok(out)
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 + 7) as u8).collect()
}

/// Everything `run_as` covers, as `user`, in `dir` (writable by `user`).
/// `owner_is_user`: also assert that created files are owned by `user`.
async fn exercise(a: &Agent, user: &str, dir: &str, owner_is_user: bool) {
    let access = FileAccess::user(user);
    let d = dir.to_owned();
    let client = &a.client;

    // mkdir -p, stat, list.
    let made = client
        .make_dir_with(&format!("{d}/sub/deeper"), &access)
        .await
        .unwrap();
    if owner_is_user {
        assert_eq!(made.owner, user);
    }
    let stat = client
        .stat_with(&format!("{d}/sub"), false, &access)
        .await
        .unwrap();
    assert_eq!(stat.r#type, FileType::Directory as i32);
    let listed = client.list_dir_with(&d, 2, &access).await.unwrap();
    let names: Vec<_> = listed.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"sub") && names.contains(&"deeper"),
        "{names:?}"
    );

    // Upload (WriteFile stream and resumable), owned by the user, with metadata.
    let small = pattern(1000);
    let big = pattern(5 * 1024 * 1024 + 123);
    for (name, data, resumable) in [("a.bin", &small, false), ("b.bin", &big, true)] {
        let options = UploadOptions {
            run_as: Some(user.into()),
            resumable: Some(resumable),
            create_parents: true,
            ..Default::default()
        };
        let path = format!("{d}/up/{name}");
        let result = client.upload(&path, data.clone(), options).await.unwrap();
        assert_eq!(result.size as usize, data.len());
        assert_eq!(result.resumable, resumable);
        let entry = client.stat_with(&path, false, &access).await.unwrap();
        assert_eq!(entry.size as usize, data.len());
        if owner_is_user {
            assert_eq!(entry.owner, user, "{name}");
        }
        let got = read_as(client, &path, user).await.unwrap();
        assert!(got == *data, "{name} round-trips");
    }
    // The parent created by create_parents belongs to the user too.
    if owner_is_user {
        let up = client
            .stat_with(&format!("{d}/up"), false, &access)
            .await
            .unwrap();
        assert_eq!(up.owner, user);
    }

    // Metadata (skipped on a file system without extended attributes).
    let meta_path = format!("{d}/meta.txt");
    let with_meta = client
        .upload(
            &meta_path,
            b"m".to_vec(),
            UploadOptions {
                run_as: Some(user.into()),
                metadata: [("origin".to_owned(), "run-as".to_owned())].into(),
                xattr_namespace: Some("user.e2b.".into()),
                ..Default::default()
            },
        )
        .await;
    match with_meta {
        Ok(_) => {
            let e2b = FileAccess {
                run_as: Some(user.into()),
                xattr_namespace: Some("user.e2b.".into()),
            };
            let entry = client.stat_with(&meta_path, false, &e2b).await.unwrap();
            assert_eq!(
                entry.metadata.get("origin").map(String::as_str),
                Some("run-as")
            );
            // Another namespace sees none.
            let other = client.stat_with(&meta_path, false, &access).await.unwrap();
            assert!(other.metadata.is_empty());
        }
        Err(e) => eprintln!("skipping metadata: {e}"),
    }

    // Move and remove.
    let moved = client
        .move_path(
            &format!("{d}/up/a.bin"),
            &format!("{d}/up/moved/a2.bin"),
            false,
            true,
            &access,
        )
        .await
        .unwrap();
    assert!(moved.path.ends_with("up/moved/a2.bin"));
    client
        .remove_with(&format!("{d}/up/moved"), true, &access)
        .await
        .unwrap();
    let gone = client
        .stat_with(&format!("{d}/up/moved/a2.bin"), false, &access)
        .await
        .unwrap_err();
    assert!(
        matches!(gone, cua_spacesd_client::Error::PathNotFound(_)),
        "{gone:?}"
    );

    // A watcher created as the user, polled through the agent.
    let watcher = client
        .filesystem()
        .create_watcher(CreateWatcherRequest {
            path: d.clone(),
            recursive: true,
            run_as: user.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner()
        .watcher_id;
    // Let the watch settle (FSEvents latency, first poll scan).
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    client
        .upload(
            &format!("{d}/watched.txt"),
            b"w".to_vec(),
            UploadOptions {
                run_as: Some(user.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut seen = false;
    for _ in 0..100 {
        let events = client
            .filesystem()
            .get_watcher_events(GetWatcherEventsRequest {
                watcher_id: watcher.clone(),
                max_events: 0,
            })
            .await
            .unwrap()
            .into_inner()
            .events;
        if events.iter().any(|e| e.path.ends_with("watched.txt")) {
            seen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(seen, "the watcher saw the write");
    client
        .filesystem()
        .remove_watcher(RemoveWatcherRequest {
            watcher_id: watcher,
        })
        .await
        .unwrap();

    // Signed URLs that act as the user: PUT, GET, HEAD, Range, multipart POST.
    let url_dir = format!("{d}/signed");
    let put_path = format!("{url_dir}/put.bin");
    let put = signed(client, &put_path, SignedUrlMethod::Put, user, true).await;
    assert!(put.contains("&ru="), "the user is in the URL: {put}");
    let body = pattern(300_000);
    let (status, _, _) = http(a.addr, "PUT", &put, &[], &body).await;
    assert_eq!(status, 201);
    let get = signed(client, &put_path, SignedUrlMethod::Get, user, false).await;
    let (status, _, got) = http(a.addr, "GET", &get, &[], b"").await;
    assert_eq!((status, got == body), (200, true));
    let (status, head, got) = http(a.addr, "HEAD", &get, &[], b"").await;
    assert_eq!(status, 200);
    assert!(
        head.contains(&format!("content-length: {}", body.len())),
        "{head}"
    );
    assert!(got.is_empty());
    let (status, head, got) = http(a.addr, "GET", &get, &[("Range", "bytes=10-19")], b"").await;
    assert_eq!(status, 206, "{head}");
    assert_eq!(got, body[10..20]);
    if owner_is_user {
        let entry = client.stat_with(&put_path, false, &access).await.unwrap();
        assert_eq!(entry.owner, user);
        let parent = client.stat_with(&url_dir, false, &access).await.unwrap();
        assert_eq!(parent.owner, user, "create_parents made it as the user");
    }

    let form_path = format!("{url_dir}/form.bin");
    let form = signed(client, &form_path, SignedUrlMethod::Put, user, false).await;
    let payload = pattern(70_000);
    let boundary = "----cua-boundary-xyz";
    let (status, head, response) = http(
        a.addr,
        "POST",
        &form,
        &[(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )],
        &multipart(boundary, "ignored-name.txt", &payload),
    )
    .await;
    assert_eq!(status, 200, "{head} {}", String::from_utf8_lossy(&response));
    let listing: serde_json::Value = serde_json::from_slice(&response).unwrap();
    assert_eq!(listing[0]["name"], "form.bin");
    assert_eq!(listing[0]["path"], form_path.as_str());
    assert_eq!(listing[0]["type"], "file");
    let stored = read_as(client, &form_path, user).await.unwrap();
    assert!(stored == payload);
    if owner_is_user {
        assert_eq!(
            client
                .stat_with(&form_path, false, &access)
                .await
                .unwrap()
                .owner,
            user
        );
    }

    // A signature covers the user: swapping it is refused.
    let swapped = put.replace(&format!("&ru={user}"), "&ru=root");
    if swapped != put {
        let (status, _, _) = http(a.addr, "PUT", &swapped, &[], b"x").await;
        assert_eq!(status, 403);
    }
}

#[tokio::test]
async fn a_helper_for_the_agents_own_user_serves_every_operation() {
    let a = agent(true).await;
    let dir = Scratch::new(&a, "777").await;
    let user = agent_user(&a).await;
    exercise(&a, &user, &dir.path, true).await;
    dir.remove(&a).await;
}

#[tokio::test]
async fn errors_keep_their_codes_through_the_helper() {
    let a = agent(true).await;
    let dir = Scratch::new(&a, "777").await;
    let user = agent_user(&a).await;
    let access = FileAccess::user(&user);
    // Not found, exists, not a directory: the helper's own statuses.
    let missing = format!("{}/nope", dir.path);
    let e = a
        .client
        .stat_with(&missing, false, &access)
        .await
        .unwrap_err();
    assert!(
        matches!(e, cua_spacesd_client::Error::PathNotFound(_)),
        "{e:?}"
    );
    let file = format!("{}/f", dir.path);
    a.client
        .upload(
            &file,
            b"x".to_vec(),
            UploadOptions {
                run_as: Some(user.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let e = a
        .client
        .upload(
            &file,
            b"y".to_vec(),
            UploadOptions {
                run_as: Some(user.clone()),
                mode: WriteMode::CreateNew,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(e, cua_spacesd_client::Error::PathExists(_)),
        "{e:?}"
    );
    let e = a.client.list_dir_with(&file, 1, &access).await.unwrap_err();
    assert!(
        matches!(e, cua_spacesd_client::Error::NotADirectory(_)),
        "{e:?}"
    );

    // An unknown user is rejected before anything runs.
    let e = a
        .client
        .stat_with(&file, false, &FileAccess::user("no-such-user-cua-test"))
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("no-such-user-cua-test"),
        "names the user: {e}"
    );

    // A checksum mismatch through the helper's upload session discards the
    // partial file, and the session id is routed to the helper throughout.
    let target = format!("{}/bad.bin", dir.path);
    let mut fs = a.client.filesystem();
    let begun = fs
        .begin_upload(BeginUploadRequest {
            header: Some(WriteFileHeader {
                path: target.clone(),
                expected_sha256: "0".repeat(64),
                run_as: user.clone(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    fs.upload_chunk(UploadChunkRequest {
        upload_id: begun.upload_id.clone(),
        offset: 0,
        data: b"data".to_vec(),
        ..Default::default()
    })
    .await
    .unwrap();
    let e = fs
        .commit_upload(CommitUploadRequest {
            upload_id: begun.upload_id.clone(),
            sha256: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::FailedPrecondition);
    assert_eq!(
        sh(
            &a,
            &format!("ls -A '{}' | grep -c 'bad.bin\\|part' || true", dir.path)
        )
        .await
        .trim(),
        "0",
        "neither the file nor a partial one is left"
    );
    // The session is gone: a second commit finds nothing.
    let again = fs
        .commit_upload(CommitUploadRequest {
            upload_id: begun.upload_id,
            sha256: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(again.code(), Code::NotFound);
    dir.remove(&a).await;
}

#[tokio::test]
async fn an_agent_without_the_helper_binary_refuses_with_a_reason() {
    // The helper cannot start (no such executable): the call is a clear
    // PERMISSION_DENIED naming the user, never a silent run as the agent.
    let data = tempfile::tempdir().unwrap();
    let ctx = ServerContext::new(
        ServerConfig {
            data_dir: data.path().to_path_buf(),
            fs_helper_exe: Some("/nonexistent/cua-spacesd".into()),
            fs_helper_for_self: true,
            ..ServerConfig::default()
        },
        Some(TOKEN.into()),
    );
    let addr = cua_spacesd_server::spawn_local(ServerBuilder::new(ctx).build())
        .await
        .unwrap();
    let client = SpacesdClient::connect(
        ConnectOptions::parse(&format!("http://{addr}"))
            .unwrap()
            .probe(false)
            .token(TOKEN),
    )
    .await
    .unwrap();
    let user = current_user_local();
    let e = client
        .stat_with("/", false, &FileAccess::user(&user))
        .await
        .unwrap_err();
    let text = e.to_string();
    assert!(
        matches!(e, cua_spacesd_client::Error::PermissionDenied(_)),
        "{text}"
    );
    assert!(
        text.contains(&user) && text.contains("cannot act as"),
        "{text}"
    );
}

#[tokio::test]
async fn the_helper_features_are_advertised() {
    let a = agent(false).await;
    let caps = a.client.capabilities().await.unwrap();
    for name in ["fs.xattr", "files.multipart"] {
        let f = caps.features.iter().find(|f| f.name == name).unwrap();
        assert!(f.supported, "{name}");
    }
    let run_as = caps
        .features
        .iter()
        .find(|f| f.name == "fs.run_as")
        .unwrap();
    // The field is understood on every Unix guest; whether this agent may
    // take another identity is its `privilege` attribute.
    assert!(run_as.supported, "{run_as:?}");
    let privilege = run_as.attributes.get("privilege").unwrap();
    assert!(
        privilege == "root" || privilege == "sudo" || privilege.starts_with("none: "),
        "{privilege}"
    );
}

/// Whether the agent can take `user`'s identity: its `privilege` attribute
/// is root or sudo, and (for sudo) `sudo -n` really works as the agent's
/// user. For an in-process agent that is this process.
async fn can_switch(a: &Agent) -> bool {
    let caps = a.client.capabilities().await.unwrap();
    let privilege = caps
        .features
        .iter()
        .find(|f| f.name == "fs.run_as")
        .and_then(|f| f.attributes.get("privilege").cloned())
        .unwrap_or_default();
    match privilege.as_str() {
        "root" => true,
        "sudo" => {
            let out = a
                .client
                .run(cua_spacesd_client::Command::new("sudo").args(["-n", "true"]))
                .await
                .unwrap();
            out.status.code == Some(0)
        }
        _ => false,
    }
}

/// The second user to switch to, when the agent can.
async fn other_user(a: &Agent) -> Option<String> {
    let wanted = std::env::var("CUA_TEST_FS_AS_USER").unwrap_or_else(|_| "nobody".into());
    if wanted == agent_user(a).await {
        return None;
    }
    let exists = a
        .client
        .run(cua_spacesd_client::Command::new("id").args(["-u", &wanted]))
        .await
        .unwrap()
        .status
        .code
        == Some(0);
    if !exists {
        eprintln!("skipped: no user {wanted:?}");
        return None;
    }
    if !can_switch(a).await {
        eprintln!("skipped: the agent can neither be root nor use passwordless sudo");
        return None;
    }
    Some(wanted)
}

#[tokio::test]
async fn acting_as_a_different_user_switches_identity() {
    let a = agent(false).await;
    let Some(user) = other_user(&a).await else {
        if a.remote {
            panic!("the guest under test must be able to act as another user");
        }
        return;
    };
    let access = FileAccess::user(&user);
    let me = agent_user(&a).await;
    let dir = Scratch::new(&a, "777").await;
    exercise(&a, &user, &dir.path, true).await;

    // The kernel's checks are that user's: a private file of the agent's own
    // is unreadable to them, and a closed directory is closed.
    let p = format!("{}/private.txt", dir.path);
    sh(&a, &format!("printf secret > '{p}' && chmod 600 '{p}'")).await;
    let e = read_as(&a.client, &p, &user).await.unwrap_err();
    assert!(
        matches!(e, cua_spacesd_client::Error::PermissionDenied(_)),
        "{e:?}"
    );
    // Without run_as the agent reads it.
    assert_eq!(a.client.download(&p).await.unwrap().as_ref(), b"secret");
    // As the agent's own user by name it can too.
    assert_eq!(
        read_as(&a.client, &p, &me).await.unwrap(),
        b"secret".to_vec()
    );

    // Writing where only the agent's user may write is refused for them.
    let ours = Scratch::new(&a, "700").await;
    let e = a
        .client
        .upload(
            &format!("{}/x", ours.path),
            b"x".to_vec(),
            UploadOptions {
                run_as: Some(user.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(e, cua_spacesd_client::Error::PermissionDenied(_)),
        "{e:?}"
    );
    ours.remove(&a).await;

    // `~` is that user's home, and the supplementary groups are theirs.
    // The shell's `~user` (macOS has no getent).
    let home = sh(&a, &format!("eval echo ~{user}"))
        .await
        .trim()
        .to_owned();
    let home_exists = a
        .client
        .run(cua_spacesd_client::Command::new("test").args(["-d", &home]))
        .await
        .unwrap()
        .status
        .code
        == Some(0);
    if home_exists {
        let stat = a.client.stat_with("~", false, &access).await.unwrap();
        // macOS homes can sit under a symlink (/var is /private/var).
        let physical = sh(&a, &format!("cd '{home}' && pwd -P"))
            .await
            .trim()
            .to_owned();
        assert!(
            stat.path == home || stat.path == physical,
            "{} vs {home}",
            stat.path
        );
    }
    // Watchers and uploads started as the user are routed to their helper
    // and keep working after another user's helper starts.
    let both = a
        .client
        .stat_with(&dir.path, false, &FileAccess::user(&me))
        .await
        .unwrap();
    assert_eq!(both.r#type, FileType::Directory as i32);
    dir.remove(&a).await;
}

#[tokio::test]
async fn without_privilege_acting_as_another_user_is_refused_with_the_reason() {
    // Only meaningful where the agent can neither be root nor use sudo.
    let a = agent(false).await;
    if a.remote || can_switch(&a).await {
        eprintln!("skipped: the agent has the privilege (or is a guest under test)");
        return;
    }
    let me = agent_user(&a).await;
    let other = ["nobody", "daemon", "root"]
        .into_iter()
        .find(|u| *u != me)
        .unwrap();
    let e = a
        .client
        .stat_with("/tmp", false, &FileAccess::user(other))
        .await
        .unwrap_err();
    let text = e.to_string();
    assert!(
        matches!(e, cua_spacesd_client::Error::PermissionDenied(_)),
        "{text}"
    );
    assert!(
        text.contains(other) && text.contains("cannot act as"),
        "{text}"
    );
    // Signed URLs refuse at mint time, not at the holder, when there is no
    // sudo at all; with sudo present the URL is minted and the helper
    // starts on use.
    let minted = a
        .client
        .filesystem()
        .create_signed_url(CreateSignedUrlRequest {
            path: "/tmp/x".into(),
            method: SignedUrlMethod::Put as i32,
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 60,
                nanos: 0,
            }),
            run_as: other.into(),
            ..Default::default()
        })
        .await;
    if let Err(status) = minted {
        assert_eq!(status.code(), Code::PermissionDenied);
    }
}
