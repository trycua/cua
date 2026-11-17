// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The bulk lane against the real server: a plain file stands in for the
//! virtio disk (`CUA_SPACESD_BULK_DEVICE`), so the guest end (staging a file
//! into a slot, taking an upload chunk from one) and the client end are
//! exercised together over native gRPC.
#![cfg(unix)]

mod common;

use cua_proto::env::v1::*;
use cua_spacesd_client::{DownloadOptions, TransportPreference, UploadOptions};
use sha2::{Digest, Sha256};

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 31 + i / 7) % 251) as u8).collect()
}

#[tokio::test]
async fn files_move_through_a_slot_and_a_blank_disk_gets_an_identity() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let disk = dir.path().join("bulk.img");
    let f = std::fs::File::create(&disk).unwrap();
    f.set_len(cua_proto::bulk::DISK_BYTES).unwrap();
    std::fs::set_permissions(&disk, std::fs::Permissions::from_mode(0o600)).unwrap();
    // SAFETY: this test binary runs this one test, before any thread reads
    // the environment.
    unsafe { std::env::set_var("CUA_SPACESD_BULK_DEVICE", &disk) };

    let t = common::target().await;
    let env = t.client(TransportPreference::Native).await;
    let caps = env.refresh_capabilities().await.unwrap();
    let lane = caps.bulk.expect("the guest advertises its lane");
    assert_eq!(lane.slots, cua_proto::bulk::SLOTS);
    assert_eq!(lane.slot_bytes, cua_proto::bulk::SLOT_BYTES);
    assert_eq!(lane.id.len(), 16);
    // The identity is the one in the file, so the client can prove it.
    assert!(env.attach_bulk(&disk).await);

    // Upload 40 MiB, read it back through slots, byte for byte.
    let body = data(40 << 20);
    let guest_path = format!("{}/big.bin", t.scratch);
    let up = env
        .upload(&guest_path, body.clone(), UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(up.sha256, hex::encode(Sha256::digest(&body)));
    assert!(up.resumable, "the lane rides the resumable protocol");
    let on_disk = std::fs::read(&guest_path).unwrap();
    assert_eq!(on_disk, body, "the guest file holds the uploaded bytes");
    let got = env.download(&guest_path).await.unwrap();
    assert_eq!(got.as_ref(), body.as_slice());

    // A range ending mid-slot, and a short tail.
    let mut part = Vec::new();
    env.download_with(
        &guest_path,
        DownloadOptions {
            offset: 17 << 20,
            length: (19 << 20) + 11,
            ..Default::default()
        },
        &mut part,
    )
    .await
    .unwrap();
    assert_eq!(part, body[17 << 20..(17 << 20) + (19 << 20) + 11].to_vec());

    // A stream read with a bulk slot but no length is refused, and a slot
    // beyond the lane is invalid.
    let bad = env
        .filesystem()
        .read_file(ReadFileRequest {
            path: guest_path.clone(),
            bulk_slot: 1,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code(), tonic::Code::InvalidArgument);
    let bad = env
        .filesystem()
        .read_file(ReadFileRequest {
            path: guest_path.clone(),
            length: 1 << 20,
            bulk_slot: 9,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code(), tonic::Code::InvalidArgument);

    // A restored guest's new, blank file: capabilities write a fresh identity.
    std::fs::write(&disk, b"").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&disk)
        .unwrap()
        .set_len(cua_proto::bulk::DISK_BYTES)
        .unwrap();
    let caps2 = env.refresh_capabilities().await.unwrap();
    assert_ne!(caps2.bulk.unwrap().id, lane.id);
}
