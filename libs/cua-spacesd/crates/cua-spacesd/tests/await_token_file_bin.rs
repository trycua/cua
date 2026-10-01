// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The real binary in await-token-file mode behind the `token-sync` helper
//! (both as the current user, loopback only, no desktop): the source file
//! drives install, rotation and revocation end to end. Unix only.
#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cua_proto::env::v1::*;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::token_file::write_token_atomically;
use cua_spacesd_server::tonic::Code;

const TOKEN_A: &str = "binA-0123456789abcdef0123456789";
const TOKEN_B: &str = "binB-0123456789abcdef0123456789";

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn current_user() -> String {
    let out = Command::new("id").arg("-un").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

async fn client(url: &str, token: Option<&str>, t: TransportPreference) -> SpacesdClient {
    let mut o = ConnectOptions::parse(url)
        .unwrap()
        .transport(t)
        .probe(false);
    if let Some(token) = token {
        o = o.token(token);
    }
    SpacesdClient::connect(o).await.unwrap()
}

/// Outcome of a Stat with `token`, retried for at most 10 s until `want`.
async fn wait_code(url: &str, token: Option<&str>, want: Option<Code>) {
    let mut last = None;
    for _ in 0..200 {
        for t in [TransportPreference::Native, TransportPreference::GrpcWeb] {
            let got = match client(url, token, t)
                .await
                .filesystem()
                .stat(StatRequest {
                    path: "/".into(),
                    ..Default::default()
                })
                .await
            {
                Ok(_) => None,
                Err(e) => Some(e.code()),
            };
            last = Some(got);
            if got != want {
                break;
            }
            if t == TransportPreference::GrpcWeb {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("stat with {token:?}: wanted {want:?}, last {last:?}");
}

fn write(path: &Path, token: Option<&str>) {
    write_token_atomically(path, token, None).unwrap();
}

#[tokio::test]
async fn binary_follows_the_synced_token_file() {
    let bin = env!("CARGO_BIN_EXE_cua-spacesd");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("secret-env-token");
    let synced = dir.path().join("synced").join("env-token");
    write(&source, None);
    let _sync = Kill(
        Command::new(bin)
            .args([
                "token-sync",
                "--interval-ms",
                "50",
                "--owner",
                &current_user(),
            ])
            .arg("--from")
            .arg(&source)
            .arg("--to")
            .arg(&synced)
            .env("CUA_ENV_LOG", "warn")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let port = free_port();
    let _driver = Kill(
        Command::new(bin)
            .args([
                "serve",
                "--await-token-file",
                "--no-desktop",
                "--no-driver",
                "--no-mcp",
                "--quic-port",
                "0",
            ])
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--token-file")
            .arg(&synced)
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .env("CUA_ENV_TOKEN_POLL_MS", "50")
            .env("CUA_ENV_LOG", "warn")
            .env_remove("CUA_ENV_TOKEN")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    // Awaiting.
    wait_code(&url, None, Some(Code::FailedPrecondition)).await;
    wait_code(&url, Some(TOKEN_A), Some(Code::FailedPrecondition)).await;
    // Install through the helper.
    write(&source, Some(TOKEN_A));
    wait_code(&url, Some(TOKEN_A), None).await;
    wait_code(&url, None, Some(Code::Unauthenticated)).await;
    // Rotate.
    write(&source, Some(TOKEN_B));
    wait_code(&url, Some(TOKEN_B), None).await;
    wait_code(&url, Some(TOKEN_A), Some(Code::Unauthenticated)).await;
    // Revoke.
    write(&source, None);
    wait_code(&url, Some(TOKEN_B), Some(Code::FailedPrecondition)).await;
    // The synced copy is 0600 in a directory the driver's user cannot write
    // into only when the helper runs as root; here both run as the test user.
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(&synced).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
