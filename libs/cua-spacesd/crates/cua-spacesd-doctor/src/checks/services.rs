// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `services`: every network service the image claims answers inside the
//! guest (RFB banner on VNC, HTTP readiness on noVNC, SSH banner), the
//! host-free equivalent of the old smoke test's port probes. cua-spacesd's
//! own ports are covered by `meta` and `stream`.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::manifest::ServiceClaim;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::{Ctx, Recorder};

/// The per-step wait of a probe (connect, banner, HTTP reply): 5 s, stretched
/// by `CUA_DOCTOR_TIMEOUT_SCALE` like the check's own budget, so a slow guest
/// (QEMU TCG) is not failed by a service that is merely slow to answer.
fn step_timeout() -> Duration {
    crate::scaled(Duration::from_secs(5))
}

async fn banner(stream: &mut tokio::net::TcpStream, expect: &str) -> Result<String, String> {
    let mut buf = [0u8; 64];
    let wait = step_timeout();
    let n = tokio::time::timeout(wait, stream.read(&mut buf))
        .await
        .map_err(|_| format!("no banner within {} s", wait.as_secs()))?
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
    if text.starts_with(expect) {
        Ok(text)
    } else {
        Err(format!("banner {text:?}, want {expect}"))
    }
}

async fn probe(service: &ServiceClaim) -> Result<String, String> {
    let wait = step_timeout();
    let mut stream = tokio::time::timeout(
        wait,
        tokio::net::TcpStream::connect(("127.0.0.1", service.port)),
    )
    .await
    .map_err(|_| "connect timed out".to_owned())?
    .map_err(|e| format!("connect: {e}"))?;
    match (service.protocol.as_str(), service.readiness.as_str()) {
        ("rfb", _) => banner(&mut stream, "RFB 003").await,
        (_, _) if service.name == "ssh" => banner(&mut stream, "SSH-").await,
        ("http", readiness) => {
            let path = readiness.strip_prefix("http:").unwrap_or("/");
            let request =
                format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            let mut head = [0u8; 64];
            let n = tokio::time::timeout(wait, stream.read(&mut head))
                .await
                .map_err(|_| format!("no HTTP reply within {} s", wait.as_secs()))?
                .map_err(|e| e.to_string())?;
            let line = String::from_utf8_lossy(&head[..n])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            if line.contains(" 200") {
                Ok(format!("GET {path}: {line}"))
            } else {
                Err(format!("GET {path}: {line}"))
            }
        }
        _ => Ok("port open".into()),
    }
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("services") {
        return;
    }
    let services: Vec<ServiceClaim> = ctx
        .manifest
        .manifest
        .services
        .iter()
        .filter(|s| s.component != "cua-spacesd" && s.protocol != "udp" && s.port != 0)
        .cloned()
        .collect();
    for service in services {
        let id = format!("services.{}", service.name);
        rec.run(
            &id.clone(),
            &["manifest:services"],
            Duration::from_secs(15),
            async move {
                match probe(&service).await {
                    Ok(detail) => Check::new(
                        &id,
                        Status::Pass,
                        format!(":{} ({}) {detail}", service.port, service.protocol),
                    ),
                    Err(error) => Check::new(
                        &id,
                        Status::Fail,
                        format!(":{} ({}) {error}", service.port, service.protocol),
                    )
                    .fix("the image claims this service; check its unit/program"),
                }
            },
        )
        .await;
    }
}
