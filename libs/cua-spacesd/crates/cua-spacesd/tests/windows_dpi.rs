// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Check the DPI context of the shipped executable, not this test process.
//! No desktop services, screen capture, or input are enabled.
#![cfg(target_os = "windows")]

use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cua_proto::env::v1::GetCapabilitiesRequest;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::UI::HiDpi::{GetProcessDpiAwareness, PROCESS_PER_MONITOR_DPI_AWARE};

struct Host(Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn host_starts_per_monitor_dpi_aware() {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let token = uuid::Uuid::new_v4().to_string();
    let mut host = Host(
        Command::new(env!("CARGO_BIN_EXE_cua-spacesd"))
            .args([
                "serve",
                "--no-desktop",
                "--no-driver",
                "--no-mcp",
                "--quic-port",
                "0",
            ])
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .arg("--token-file")
            .arg(dir.path().join("token"))
            .env("CUA_ENV_TOKEN", &token)
            .env("CUA_ENV_TEST_SANDBOX", "1")
            .env("CUA_ENV_LOG", "warn")
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let options = ConnectOptions::parse(&format!("http://127.0.0.1:{port}"))
        .unwrap()
        .transport(TransportPreference::Native)
        .probe(false)
        .token(token);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                host.0.try_wait().unwrap().is_none(),
                "Host exited before becoming ready"
            );
            if let Ok(client) = SpacesdClient::connect(options.clone()).await {
                if client
                    .system()
                    .get_capabilities(GetCapabilitiesRequest::default())
                    .await
                    .is_ok()
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("Host did not become ready");

    // Query Windows independently of Host tool responses. This also catches a
    // missing DPI declaration on CI machines whose display scaling is 100%.
    let awareness = unsafe { GetProcessDpiAwareness(HANDLE(host.0.as_raw_handle())).unwrap() };
    assert_eq!(awareness, PROCESS_PER_MONITOR_DPI_AWARE);
}
