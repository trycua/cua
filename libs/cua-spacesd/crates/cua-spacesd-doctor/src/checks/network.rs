// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `network`: DNS in the guest, and a `TunnelService.Forward` round trip
//! (client port -> `/tunnel` WebSocket -> spacesd -> a guest loopback port)
//! to an echo listener the doctor owns.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::ForwardOptions;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::{Ctx, Recorder};

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("network") {
        return;
    }
    let probe = ctx.manifest.manifest.dns_probe.clone();
    if probe.is_empty() {
        rec.skip(
            "network.dns",
            &[],
            "not_applicable",
            "the manifest names no DNS probe host".into(),
        )
        .await;
    } else {
        rec.run("network.dns", &[], Duration::from_secs(10), async {
            match tokio::time::timeout(
                Duration::from_secs(8),
                tokio::net::lookup_host((probe.as_str(), 443)),
            )
            .await
            {
                Ok(Ok(mut addrs)) => match addrs.next() {
                    Some(addr) => Check::new(
                        "network.dns",
                        Status::Pass,
                        format!("{probe} resolves ({})", addr.ip()),
                    ),
                    None => Check::new(
                        "network.dns",
                        Status::Fail,
                        format!("{probe} resolved to nothing"),
                    ),
                },
                Ok(Err(error)) => {
                    Check::new("network.dns", Status::Fail, format!("{probe}: {error}"))
                        .fix("no DNS or no egress in this sandbox (expected with network=none)")
                }
                Err(_) => Check::new(
                    "network.dns",
                    Status::Fail,
                    format!("{probe}: lookup timed out"),
                ),
            }
        })
        .await;
    }

    rec.run(
        "network.tunnel.forward",
        &["feature:tunnel.forward"],
        Duration::from_secs(30),
        async {
            let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                Ok(l) => l,
                Err(error) => {
                    return Check::new(
                        "network.tunnel.forward",
                        Status::Fail,
                        format!("bind echo: {error}"),
                    )
                }
            };
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            // One connection, bounded.
            let echo = tokio::spawn(async move {
                if let Ok(Ok((mut socket, _))) =
                    tokio::time::timeout(Duration::from_secs(20), listener.accept()).await
                {
                    let mut buf = [0u8; 256];
                    if let Ok(Ok(n)) =
                        tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf)).await
                    {
                        let _ = socket.write_all(&buf[..n]).await;
                    }
                }
            });
            let forward = match ctx
                .client
                .forward_tcp(ForwardOptions::new(port).bind("127.0.0.1:0".parse().expect("addr")))
                .await
            {
                Ok(f) => f,
                Err(error) => {
                    echo.abort();
                    return Check::new(
                        "network.tunnel.forward",
                        Status::Fail,
                        format!("Forward: {error}"),
                    );
                }
            };
            let message = format!("doctor-{}", ctx.nonce);
            let result = async {
                let mut stream = tokio::net::TcpStream::connect(forward.local_addr()).await?;
                stream.write_all(message.as_bytes()).await?;
                let mut got = vec![0u8; message.len()];
                tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut got))
                    .await
                    .map_err(|_| std::io::Error::other("echo timed out"))??;
                Ok::<_, std::io::Error>(got)
            }
            .await;
            let _ = forward.close().await;
            echo.abort();
            match result {
                Ok(got) => Check::new(
                    "network.tunnel.forward",
                    super::verdict(got == message.as_bytes()),
                    format!("echo through the /tunnel forward to guest port {port}"),
                ),
                Err(error) => Check::new(
                    "network.tunnel.forward",
                    Status::Fail,
                    format!("round trip: {error}"),
                ),
            }
        },
    )
    .await;
}
