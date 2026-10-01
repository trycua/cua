// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `volume`: the guest mount of Cua Volume, end to end, with the doctor as
//! the client. The doctor serves a throwaway volume (a temporary directory,
//! never the user's), asks the driver to mount it at a temporary path,
//! reads a file it wrote, writes one back through the mount, checks the
//! write landed in its volume, and detaches. A claimed `volume.mount`
//! (image.json `features_required`, with the backend in
//! `feature_attributes`) fails `--strict` when any step fails.

use std::sync::Arc;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, SpacesdClient};
use futures_util::{SinkExt as _, StreamExt as _};
use guest_tungstenite::tungstenite::Message;

use crate::{Ctx, Recorder};

const VOLUME: &str = "feature:volume.mount";

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("volume") {
        return;
    }
    if !ctx.supports("volume.mount") {
        let message = format!(
            "volume.mount unsupported: {}",
            ctx.limitation("volume.mount")
        );
        if ctx.manifest.manifest.requires("volume.mount") {
            rec.push(Check::new("volume.mount", Status::Fail, message), &[VOLUME])
                .await;
        } else {
            rec.skip("volume.mount", &[VOLUME], "optional_unavailable", message)
                .await;
        }
        return;
    }
    rec.run_effectful(
        "volume.mount",
        &[VOLUME],
        Duration::from_secs(90),
        mount_round_trip(&ctx.client, &ctx.nonce),
    )
    .await;
}

/// Mounts a throwaway volume in the guest and uses it like any program.
pub async fn mount_round_trip(client: &SpacesdClient, nonce: &str) -> Check {
    let id = "volume.mount";
    let mut volume = client.volume();
    let before = match volume
        .get_volume_status(pb::GetVolumeStatusRequest {})
        .await
    {
        Ok(r) => r.into_inner(),
        Err(s) => {
            return Check::new(
                id,
                Status::Fail,
                format!("GetVolumeStatus: {}", s.message()),
            )
        }
    };
    if pb::VolumeState::try_from(before.state).unwrap_or_default() != pb::VolumeState::Detached {
        // AttachVolume replaces the current mount: never do that to a
        // client's live volume.
        return Check::new(
            id,
            Status::Skip,
            format!(
                "a volume is attached at {}; not replacing it",
                before.mount_path
            ),
        );
    }
    let dir = match tempfile::Builder::new()
        .prefix("cua-doctor-volume-")
        .tempdir()
    {
        Ok(d) => d,
        Err(e) => return Check::new(id, Status::Fail, format!("temporary directory: {e}")),
    };
    // The doctor usually runs as root and the driver as the desktop user,
    // which creates the mount point (as itself): the directory is sticky and
    // world-writable, like /tmp.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o1777));
    }
    let result = round_trip(client, dir.path(), nonce).await;
    // Always leave nothing behind.
    let _ = volume
        .detach_volume(pb::DetachVolumeRequest {
            volume_id: String::new(),
        })
        .await;
    match result {
        Ok((backend, ms)) => Check::new(
            id,
            Status::Pass,
            format!("mounted over {backend}, read and wrote through it ({ms} ms)"),
        )
        .fact("backend", backend),
        Err(e) => Check::new(id, Status::Fail, e)
            .fix("check the image's FUSE (Linux: fuse3, /dev/fuse, sudo for the driver's user) or NFS client (macOS: /sbin/mount_nfs; Windows: Client for NFS, and 127.0.0.1:111 free for its portmapper)"),
    }
}

async fn round_trip(
    client: &SpacesdClient,
    dir: &std::path::Path,
    nonce: &str,
) -> Result<(String, u128), String> {
    use cua_volume::{Condition, Context, Drive};
    let t0 = std::time::Instant::now();
    let drive = Drive::open_local(&dir.join("home"));
    let probe = format!("doctor volume probe {nonce}");
    drive
        .session(Context::user())
        .write(
            "public/doctor.txt",
            probe.clone().into_bytes(),
            Condition::None,
        )
        .await
        .map_err(|e| format!("the doctor's volume: {e}"))?;
    let vfs = cua_volume::vfs::Vfs::new(&drive, Context::user(), None, None, &dir.join("state"))
        .map_err(|e| e.to_string())?;
    let servers = Arc::new(
        cua_volume::guest::GuestServers::start(vfs)
            .await
            .map_err(|e| e.to_string())?,
    );
    let mount = dir.join("mnt");
    let attached = client
        .volume()
        .attach_volume(pb::AttachVolumeRequest {
            mount_path: mount.to_string_lossy().into_owned(),
            ticket_ttl: None,
        })
        .await
        .map_err(|s| format!("AttachVolume: {}", s.message()))?
        .into_inner();
    let ws_path = if attached.ws_path.is_empty() {
        format!(
            "{}?ticket={}",
            cua_proto::metadata::VOLUME_WS_PATH,
            attached.ticket
        )
    } else {
        attached.ws_path.clone()
    };
    let ws = client
        .open_websocket(&ws_path)
        .await
        .map_err(|e| format!("/volume socket: {e}"))?;
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
    let peer = tokio::spawn(cua_hotspot::peer::serve_egress(
        Box::pin(source),
        Box::pin(sink),
        servers.dialer(),
    ));
    let outcome = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let s = client
                .volume()
                .get_volume_status(pb::GetVolumeStatusRequest {})
                .await
                .map_err(|s| format!("GetVolumeStatus: {}", s.message()))?
                .into_inner();
            match pb::VolumeState::try_from(s.state).unwrap_or_default() {
                pb::VolumeState::Mounted => break,
                pb::VolumeState::Error => return Err(format!("the mount failed: {}", s.detail)),
                _ if tokio::time::Instant::now() > deadline => {
                    return Err("the volume did not mount in 30 s".into());
                }
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
        let (m, want) = (mount.clone(), probe.clone());
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let got = std::fs::read_to_string(m.join("public/doctor.txt"))
                .map_err(|e| format!("read through the mount: {e}"))?;
            if got != want {
                return Err(format!("read {got:?} through the mount, expected {want:?}"));
            }
            std::fs::write(m.join("public/from-guest.txt"), b"written in the guest")
                .map_err(|e| format!("write through the mount: {e}"))?;
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())??;
        // The write lands in the doctor's volume (on close, or 1.5 s idle
        // over NFS).
        for _ in 0..50 {
            if let Ok((b, _)) = drive
                .session(Context::user())
                .read("public/from-guest.txt", None)
                .await
            {
                if b == b"written in the guest" {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err("the guest's write never reached the volume".into())
    }
    .await;
    let _ = client
        .volume()
        .detach_volume(pb::DetachVolumeRequest {
            volume_id: attached.volume_id.clone(),
        })
        .await;
    peer.abort();
    outcome?;
    // Detaching must leave no mount behind.
    for _ in 0..50 {
        if !cua_spacesd_server::services::volume::is_mounted_async(&mount).await {
            return Ok((attached.backend, t0.elapsed().as_millis()));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "{} is still mounted after detaching",
        mount.display()
    ))
}
