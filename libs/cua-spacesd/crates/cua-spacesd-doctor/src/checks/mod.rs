// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The check catalogue. Each module is one group (the id's first segment).
//!
//! | group | what |
//! |---|---|
//! | `meta` | reachability, versions and pins, health, `--print-config`, the manifest itself |
//! | `capabilities` | the claims diff: required features supported, limitations named, unclaimed features |
//! | `driver` | linked cua-driver: tool registry, contract tools, schema hash pin, `health_report` |
//! | `mcp` | `/mcp` initialize and `tools/list` agree with `DriverService` |
//! | `build` | running build identities (git sha, executable sha256) and injected overlays still in effect |
//! | `process` | run, exit codes, stdin, PTY, signals |
//! | `files` | chunked upload/download round trip, ops, watch, signed URL, chunk limit |
//! | `network` | DNS, tunnel forward round trip |
//! | `tunnel` | forward list/close/revoke, foreign hosts refused, forged and cross-scope tickets refused |
//! | `hotspot` | status; reverse-SOCKS egress with the doctor as the peer (loopback echo only) |
//! | `volume` | the Cua Volume guest mount end to end, with the doctor serving a throwaway volume |
//! | `teleport` | receive side: manifest, import verification, fixture import into a throwaway home, file transfer |
//! | `auth` | token delivery mode and its file contract, token not leaked, unauthenticated refused |
//! | `time` | clock skew, NTP sync (VMs), time zone |
//! | `resources` | disk, inodes, `/dev/shm`, memory, CPUs |
//! | `init` | units of the image's init system running, none failed, restart policy |
//! | `services` | the image's claimed network services answer (RFB banner, HTTP readiness, SSH banner) |
//! | `software` | claimed apps and tools answer at the claimed versions; simulator runtimes are exactly the claimed set |
//! | `viewer` | the HTML5 viewer page is served; viewer tickets are scoped |
//! | `compat` | pre-rename links, unit alias and environment spellings |
//! | `annotations` | manifest labels agree with the live guest |
//! | `fixtures`, `screenshot`, `windows`, `input`, `a11y` | desktop checks on doctor-owned fixture windows |
//! | `stream` | every compiled encoder (encode, decode, PSNR), a live media session, QUIC |
//! | `audio` | backend and devices, tone capture, uplink loopback, A/V sync |
//! | `presence` | join, own heartbeat, leave |

pub mod annotations;
pub mod audio;
pub mod auth;
pub mod build;
pub mod capabilities;
pub mod compat;
pub mod desktop;
pub mod driver;
pub mod fidelity;
pub mod files;
pub mod init;
pub mod mcp;
pub mod meta;
pub mod network;
pub mod presence;
pub mod process;
pub mod resources;
pub mod services;
pub mod software;
pub mod stream;
pub mod teleport;
pub mod time;
pub mod tunnel;
pub mod viewer;
pub mod volume;

use crate::{Ctx, Recorder};

/// Runs every group in dependency order.
pub async fn run_all(ctx: &Ctx, rec: &mut Recorder<'_>) {
    meta::run(ctx, rec).await;
    capabilities::run(ctx, rec).await;
    driver::run(ctx, rec).await;
    mcp::run(ctx, rec).await;
    build::run(ctx, rec).await;
    process::run(ctx, rec).await;
    files::run(ctx, rec).await;
    teleport::run(ctx, rec).await;
    network::run(ctx, rec).await;
    tunnel::run(ctx, rec).await;
    volume::run(ctx, rec).await;
    auth::run(ctx, rec).await;
    time::run(ctx, rec).await;
    resources::run(ctx, rec).await;
    init::run(ctx, rec).await;
    services::run(ctx, rec).await;
    software::run(ctx, rec).await;
    viewer::run(ctx, rec).await;
    compat::run(ctx, rec).await;
    annotations::run(ctx, rec).await;
    stream::run(ctx, rec).await;
    desktop::run(ctx, rec).await;
    audio::run(ctx, rec).await;
    presence::run(ctx, rec).await;
}

/// Short helper: `pass` / `fail` with a message.
pub(crate) fn verdict(ok: bool) -> cua_spacesd_client::diagnose::Status {
    if ok {
        cua_spacesd_client::diagnose::Status::Pass
    } else {
        cua_spacesd_client::diagnose::Status::Fail
    }
}
