// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `tunnel` and `hotspot`: the `TunnelService` lifecycle and the
//! reverse-SOCKS hotspot, end to end inside the guest.
//!
//! - `tunnel.lifecycle`: a forward is listed, a non-local target is
//!   refused, `CloseForward` revokes it (listed no more, its socket is gone,
//!   a second close is NOT_FOUND).
//! - `tunnel.tickets`: `/tunnel` and `/hotspot` refuse a forged ticket and
//!   a ticket minted for the other scope.
//! - `hotspot.status`: `GetHotspotStatus` answers with a defined state.
//! - `hotspot.egress` (effectful): the doctor itself is the egress peer. It
//!   starts a hotspot on a free guest loopback port, attaches the `/hotspot`
//!   WebSocket, and relays a SOCKS5 `CONNECT` to its own loopback echo
//!   listener; a `CONNECT` anywhere else is refused by its dialer, so
//!   nothing leaves the guest. It never sets the system proxy, and never
//!   replaces a hotspot someone else started (`StartHotspot` would stop it).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, Error, SpacesdClient};
use futures_util::{SinkExt as _, StreamExt as _};
use guest_tungstenite::tungstenite::Message;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

use crate::{Ctx, Recorder};

const TUNNEL: &str = "feature:tunnel.forward";
const HOTSPOT: &str = "feature:hotspot";
/// An address no guest may forward to (TEST-NET-1, RFC 5737).
const FOREIGN_HOST: &str = "192.0.2.1";

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if rec.wants_group("tunnel") {
        if !ctx.supports("tunnel.forward") {
            unsupported(ctx, rec, "tunnel.lifecycle", "tunnel.forward").await;
        } else {
            rec.run(
                "tunnel.lifecycle",
                &[TUNNEL],
                Duration::from_secs(30),
                forward_lifecycle(&ctx.client),
            )
            .await;
            rec.run(
                "tunnel.tickets",
                &[TUNNEL],
                Duration::from_secs(30),
                forged_tickets(&ctx.client, ctx.supports("hotspot")),
            )
            .await;
        }
    }
    if rec.wants_group("hotspot") {
        if !ctx.supports("hotspot") {
            unsupported(ctx, rec, "hotspot.status", "hotspot").await;
            return;
        }
        rec.run(
            "hotspot.status",
            &[HOTSPOT],
            Duration::from_secs(10),
            hotspot_status(&ctx.client),
        )
        .await;
        rec.run_effectful(
            "hotspot.egress",
            &[HOTSPOT],
            Duration::from_secs(45),
            hotspot_egress(&ctx.client, &ctx.nonce),
        )
        .await;
    }
}

/// An unsupported feature: a failure when the manifest requires it, an
/// allowed skip when it is optional or unclaimed.
async fn unsupported(ctx: &Ctx, rec: &mut Recorder<'_>, id: &str, feature: &str) {
    let claim = format!("feature:{feature}");
    let message = format!("{feature} unsupported: {}", ctx.limitation(feature));
    if ctx.manifest.manifest.requires(feature) {
        rec.push(Check::new(id, Status::Fail, message), &[claim.as_str()])
            .await;
    } else {
        rec.skip(id, &[claim.as_str()], "optional_unavailable", message)
            .await;
    }
}

fn code(error: &Error) -> String {
    error
        .code()
        .map(|c| format!("{c:?}"))
        .unwrap_or_else(|| "transport".into())
}

/// A loopback echo listener for up to `connections` connections of at most
/// 4 KiB each, stopped when the returned task is aborted.
async fn echo_listener(connections: usize) -> std::io::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let task = tokio::spawn(async move {
        for _ in 0..connections {
            let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_secs(30), listener.accept()).await
            else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                if let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf)).await
                {
                    let _ = socket.write_all(&buf[..n]).await;
                }
            });
        }
    });
    Ok((port, task))
}

/// `Forward` / `ListForwards` / `CloseForward`, and a refused foreign host.
pub async fn forward_lifecycle(client: &SpacesdClient) -> Check {
    let id = "tunnel.lifecycle";
    let mut tunnel = client.tunnel();
    // The target must exist only for the ticket to be listed; nothing
    // connects to it here.
    let (port, echo) = match echo_listener(0).await {
        Ok(v) => v,
        Err(error) => return Check::new(id, Status::Fail, format!("bind: {error}")),
    };
    let result = async {
        let forward = tunnel
            .forward(pb::ForwardRequest {
                port: port as u32,
                host: String::new(),
                ttl: Some(pbjson_types::Duration {
                    seconds: 60,
                    nanos: 0,
                }),
            })
            .await
            .map_err(|s| format!("Forward: {}", s.message()))?
            .into_inner();
        if forward.forward_id.is_empty() || !forward.ws_path.contains("ticket=") {
            return Err(format!(
                "Forward returned id {:?} and ws_path without a ticket",
                forward.forward_id
            ));
        }
        let listed = tunnel
            .list_forwards(pb::ListForwardsRequest {})
            .await
            .map_err(|s| format!("ListForwards: {}", s.message()))?
            .into_inner();
        let entry = listed
            .forwards
            .iter()
            .find(|f| f.forward_id == forward.forward_id);
        let Some(entry) = entry else {
            let _ = tunnel
                .close_forward(pb::CloseForwardRequest {
                    forward_id: forward.forward_id.clone(),
                })
                .await;
            return Err(format!("{} missing from ListForwards", forward.forward_id));
        };
        if entry.port != port as u32 {
            return Err(format!("listed port {} != {port}", entry.port));
        }
        // A foreign host must be refused at mint time.
        let foreign = tunnel
            .forward(pb::ForwardRequest {
                port: 9,
                host: FOREIGN_HOST.into(),
                ttl: None,
            })
            .await;
        let foreign_refused = match foreign {
            Ok(r) => {
                let _ = tunnel
                    .close_forward(pb::CloseForwardRequest {
                        forward_id: r.into_inner().forward_id,
                    })
                    .await;
                false
            }
            Err(_) => true,
        };
        tunnel
            .close_forward(pb::CloseForwardRequest {
                forward_id: forward.forward_id.clone(),
            })
            .await
            .map_err(|s| format!("CloseForward: {}", s.message()))?;
        let still_listed = tunnel
            .list_forwards(pb::ListForwardsRequest {})
            .await
            .map_err(|s| format!("ListForwards: {}", s.message()))?
            .into_inner()
            .forwards
            .iter()
            .any(|f| f.forward_id == forward.forward_id);
        let second_close = tunnel
            .close_forward(pb::CloseForwardRequest {
                forward_id: forward.forward_id.clone(),
            })
            .await;
        let second_not_found =
            matches!(&second_close, Err(s) if s.code() == tonic::Code::NotFound);
        // The revoked ticket no longer opens a socket.
        let revoked_socket = client.open_websocket(&forward.ws_path).await;
        let socket_refused = revoked_socket.is_err();
        let ok = foreign_refused && !still_listed && second_not_found && socket_refused;
        Ok(Check::new(
            id,
            super::verdict(ok),
            format!(
                "forward {} listed on port {port}; foreign host {}; after close: listed {still_listed}, second close {}, socket {}",
                forward.forward_id,
                if foreign_refused { "refused" } else { "ACCEPTED" },
                match &second_close {
                    Ok(_) => "ok (want NOT_FOUND)".to_owned(),
                    Err(s) => format!("{:?}", s.code()),
                },
                match &revoked_socket {
                    Ok(_) => "OPENED (want refused)".to_owned(),
                    Err(e) => format!("refused ({})", code(e)),
                },
            ),
        )
        .fact("foreign_host_refused", foreign_refused.to_string()))
    }
    .await;
    echo.abort();
    match result {
        Ok(check) => check,
        Err(message) => Check::new(id, Status::Fail, message),
    }
}

/// Forged and cross-scope tickets are refused by `/tunnel` and `/hotspot`.
pub async fn forged_tickets(client: &SpacesdClient, hotspot: bool) -> Check {
    let id = "tunnel.tickets";
    let tunnel_path = cua_proto::metadata::TUNNEL_WS_PATH;
    let hotspot_path = cua_proto::metadata::HOTSPOT_WS_PATH;
    let mut refused = Vec::new();
    let mut accepted = Vec::new();
    let mut probe = |label: String, result: Result<(), Error>| match result {
        Ok(()) => accepted.push(label),
        Err(error) => refused.push(format!("{label}: {}", code(&error))),
    };
    let open = |path: String| async move { client.open_websocket(&path).await.map(|_| ()) };
    probe(
        "forged /tunnel".into(),
        open(format!("{tunnel_path}?ticket=cua-doctor-forged")).await,
    );
    if hotspot {
        probe(
            "forged /hotspot".into(),
            open(format!("{hotspot_path}?ticket=cua-doctor-forged")).await,
        );
    }
    // A real forward ticket presented to /hotspot (wrong scope).
    let (port, echo) = match echo_listener(0).await {
        Ok(v) => v,
        Err(error) => return Check::new(id, Status::Fail, format!("bind: {error}")),
    };
    let mut tunnel = client.tunnel();
    match tunnel
        .forward(pb::ForwardRequest {
            port: port as u32,
            host: String::new(),
            ttl: None,
        })
        .await
    {
        Ok(forward) => {
            let forward = forward.into_inner();
            if hotspot {
                probe(
                    "tunnel ticket on /hotspot".into(),
                    open(format!("{hotspot_path}?ticket={}", forward.ticket)).await,
                );
            }
            let _ = tunnel
                .close_forward(pb::CloseForwardRequest {
                    forward_id: forward.forward_id,
                })
                .await;
        }
        Err(status) => {
            echo.abort();
            return Check::new(id, Status::Fail, format!("Forward: {}", status.message()));
        }
    }
    echo.abort();
    Check::new(
        id,
        super::verdict(accepted.is_empty()),
        if accepted.is_empty() {
            format!("refused: {}", refused.join(", "))
        } else {
            format!("ACCEPTED: {}", accepted.join(", "))
        },
    )
    .fix("the side-channel sockets must validate the ticket and its scope")
}

fn state_name(state: i32) -> &'static str {
    match pb::HotspotState::try_from(state).unwrap_or_default() {
        pb::HotspotState::Stopped => "stopped",
        pb::HotspotState::WaitingForPeer => "waiting_for_peer",
        pb::HotspotState::Active => "active",
        pb::HotspotState::Unspecified => "unspecified",
    }
}

/// `GetHotspotStatus` answers with a defined state.
pub async fn hotspot_status(client: &SpacesdClient) -> Check {
    match client
        .tunnel()
        .get_hotspot_status(pb::GetHotspotStatusRequest {})
        .await
    {
        Ok(r) => {
            let s = r.into_inner();
            let name = state_name(s.state);
            Check::new(
                "hotspot.status",
                super::verdict(name != "unspecified"),
                format!("state {name}"),
            )
            .fact("state", name)
        }
        Err(status) => Check::new(
            "hotspot.status",
            Status::Fail,
            format!("GetHotspotStatus: {}", status.message()),
        ),
    }
}

/// A SOCKS5 `CONNECT` to `host:port` (IPv4 literal) through `socks`.
/// Returns the stream on success, or the SOCKS reply code.
async fn socks_connect(
    socks: &str,
    host: [u8; 4],
    port: u16,
) -> std::io::Result<Result<TcpStream, u8>> {
    let mut stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(socks))
        .await
        .map_err(|_| std::io::Error::other("connect timed out"))??;
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut method = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut method))
        .await
        .map_err(|_| std::io::Error::other("greeting timed out"))??;
    if method != [0x05, 0x00] {
        return Err(std::io::Error::other(format!(
            "SOCKS greeting reply {method:?}"
        )));
    }
    let mut request = vec![0x05, 0x01, 0x00, 0x01];
    request.extend_from_slice(&host);
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut reply))
        .await
        .map_err(|_| std::io::Error::other("CONNECT reply timed out"))??;
    if reply[0] != 0x05 {
        return Err(std::io::Error::other(format!("SOCKS reply {reply:?}")));
    }
    Ok(if reply[1] == 0x00 {
        Ok(stream)
    } else {
        Err(reply[1])
    })
}

async fn free_loopback_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    listener.local_addr().map(|a| a.port())
}

/// The hotspot end to end, with the doctor as the egress peer.
pub async fn hotspot_egress(client: &SpacesdClient, nonce: &str) -> Check {
    let id = "hotspot.egress";
    let mut tunnel = client.tunnel();
    let before = match tunnel
        .get_hotspot_status(pb::GetHotspotStatusRequest {})
        .await
    {
        Ok(r) => r.into_inner(),
        Err(status) => {
            return Check::new(
                id,
                Status::Fail,
                format!("GetHotspotStatus: {}", status.message()),
            )
        }
    };
    if pb::HotspotState::try_from(before.state).unwrap_or_default() != pb::HotspotState::Stopped {
        // StartHotspot replaces the current hotspot: never do that to a
        // client's live egress.
        return Check::new(
            id,
            Status::Skip,
            format!(
                "a hotspot is already {} (id {}); not replacing it",
                state_name(before.state),
                before.hotspot_id
            ),
        )
        .skip_reason("hotspot_in_use");
    }
    let (echo_port, echo) = match echo_listener(4).await {
        Ok(v) => v,
        Err(error) => return Check::new(id, Status::Fail, format!("bind echo: {error}")),
    };
    let socks_port = match free_loopback_port().await {
        Ok(p) => p,
        Err(error) => {
            echo.abort();
            return Check::new(id, Status::Fail, format!("pick a SOCKS port: {error}"));
        }
    };
    let started = match tunnel
        .start_hotspot(pb::StartHotspotRequest {
            socks_port: socks_port as u32,
            set_system_proxy: false,
            bypass: vec![],
            ticket_ttl: Some(pbjson_types::Duration {
                seconds: 30,
                nanos: 0,
            }),
        })
        .await
    {
        Ok(r) => r.into_inner(),
        Err(status) => {
            echo.abort();
            return Check::new(
                id,
                Status::Fail,
                format!("StartHotspot: {}", status.message()),
            );
        }
    };
    let hotspot_id = started.hotspot_id.clone();
    let dials = Arc::new(AtomicU32::new(0));
    let refused_dials = Arc::new(AtomicU32::new(0));
    let mut peer: Option<tokio::task::JoinHandle<()>> = None;
    let result: Result<(String, u8, pb::GetHotspotStatusResponse), String> = async {
        let ws_path = if started.ws_path.is_empty() {
            format!(
                "{}?ticket={}",
                cua_proto::metadata::HOTSPOT_WS_PATH,
                started.ticket
            )
        } else {
            started.ws_path.clone()
        };
        let ws = client
            .open_websocket(&ws_path)
            .await
            .map_err(|e| format!("/hotspot socket: {e}"))?;
        let (sink, stream) = ws.split();
        let sink = sink.with(|bytes: Vec<u8>| async move {
            Ok::<_, guest_tungstenite::tungstenite::Error>(Message::Binary(bytes.into()))
        });
        let source = stream.filter_map(|m| async move {
            match m {
                Ok(Message::Binary(b)) => Some(b.to_vec()),
                _ => None,
            }
        });
        let (d, r) = (dials.clone(), refused_dials.clone());
        peer = Some(tokio::spawn(cua_hotspot::peer::serve_egress(
            Box::pin(source),
            Box::pin(sink),
            move |host: String, port: u16| {
                let allowed = (host == "127.0.0.1" || host == "localhost") && port == echo_port;
                let (d, r) = (d.clone(), r.clone());
                async move {
                    if allowed {
                        d.fetch_add(1, Ordering::SeqCst);
                        TcpStream::connect(("127.0.0.1", echo_port)).await
                    } else {
                        r.fetch_add(1, Ordering::SeqCst);
                        Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "the doctor's peer dials only its own echo listener",
                        ))
                    }
                }
            },
        )));
        // Bounded wait for the peer to register.
        let mut active = false;
        for _ in 0..50 {
            let s = tunnel
                .get_hotspot_status(pb::GetHotspotStatusRequest {})
                .await
                .map_err(|s| format!("GetHotspotStatus: {}", s.message()))?
                .into_inner();
            if pb::HotspotState::try_from(s.state).unwrap_or_default() == pb::HotspotState::Active {
                active = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !active {
            return Err("the hotspot never became active after the peer attached".into());
        }
        let socks = if started.socks_address.is_empty() {
            format!("127.0.0.1:{socks_port}")
        } else {
            started.socks_address.clone()
        };
        let message = format!("hotspot-{nonce}");
        let mut stream = socks_connect(&socks, [127, 0, 0, 1], echo_port)
            .await
            .map_err(|e| format!("SOCKS {socks}: {e}"))?
            .map_err(|rep| format!("SOCKS CONNECT to the echo refused (reply {rep:#04x})"))?;
        stream
            .write_all(message.as_bytes())
            .await
            .map_err(|e| format!("write: {e}"))?;
        let mut got = vec![0u8; message.len()];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut got))
            .await
            .map_err(|_| "echo through the hotspot timed out".to_owned())?
            .map_err(|e| format!("read: {e}"))?;
        drop(stream);
        if got != message.as_bytes() {
            return Err("echo through the hotspot returned different bytes".into());
        }
        // A destination the peer refuses must come back as a SOCKS failure.
        let foreign = socks_connect(&socks, [192, 0, 2, 1], 9)
            .await
            .map_err(|e| format!("SOCKS {socks}: {e}"))?;
        let foreign_reply = match foreign {
            Ok(_) => return Err("CONNECT to a destination the peer refused succeeded".into()),
            Err(rep) => rep,
        };
        let status = tunnel
            .get_hotspot_status(pb::GetHotspotStatusRequest {})
            .await
            .map_err(|s| format!("GetHotspotStatus: {}", s.message()))?
            .into_inner();
        Ok((socks, foreign_reply, status))
    }
    .await;
    // Always stop what this check started.
    let stopped = tunnel
        .stop_hotspot(pb::StopHotspotRequest {
            hotspot_id: hotspot_id.clone(),
        })
        .await;
    if let Some(peer) = peer.take() {
        peer.abort();
    }
    echo.abort();
    let (socks, foreign_reply, status) = match result {
        Ok(v) => v,
        Err(message) => return Check::new(id, Status::Fail, message),
    };
    if let Err(status) = stopped {
        return Check::new(
            id,
            Status::Fail,
            format!("StopHotspot: {}", status.message()),
        );
    }
    let after = tunnel
        .get_hotspot_status(pb::GetHotspotStatusRequest {})
        .await
        .map(|r| state_name(r.into_inner().state))
        .unwrap_or("error");
    // The listener closes with the hotspot (bounded retries). Windows
    // retries a refused loopback SYN for about 2 s before reporting it, so
    // the per-attempt budget is longer than that.
    let mut listener_closed = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&socks)).await {
            Ok(Err(_)) => {
                listener_closed = true;
                break;
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let through_peer = dials.load(Ordering::SeqCst) >= 1;
    let counted = status.bytes_out > 0 && status.bytes_in > 0;
    let ok = through_peer && counted && after == "stopped" && listener_closed;
    Check::new(
        id,
        super::verdict(ok),
        format!(
            "SOCKS5 {socks} -> peer -> echo round trip ok; refused destination got reply {foreign_reply:#04x}; \
             counters out {} in {}; after stop: {after}, listener {}",
            status.bytes_out,
            status.bytes_in,
            if listener_closed { "closed" } else { "STILL OPEN" },
        ),
    )
    .fact("peer_dials", dials.load(Ordering::SeqCst).to_string())
    .fact("peer_refused", refused_dials.load(Ordering::SeqCst).to_string())
    .fact("peer_principal", status.peer_principal_id)
}
