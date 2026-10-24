// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `Init{new_machine}`: a guest claimed from a warm pool, or restored or
//! forked from a shared memory snapshot, regenerates its per-machine
//! secrets; and a driver waiting for its first token holds no machine-seal
//! key a snapshot could copy.

use std::time::Duration;

use cua_proto::env::v1::*;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use tonic::Code;

const POOL_SECRET: &str = "pool-secret-0123456789abcdef";
const SPACE_TOKEN: &str = "space-token-0123456789abcdef";
const SNAPSHOT_MACHINE_ID: &str = "0e499f8038b24ee29a5185340af7be5e";

struct Driver {
    url: String,
    ctx: ServerContext,
    seal_file: std::path::PathBuf,
    machine_id: std::path::PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// A gateway-bootstrap driver (non-loopback config, no token), served on
/// loopback, with its own data dir and a stand-in machine-id file.
async fn start() -> Driver {
    let data = tempfile::tempdir().unwrap();
    let etc = tempfile::tempdir().unwrap();
    let machine_id = etc.path().join("machine-id");
    std::fs::write(&machine_id, format!("{SNAPSHOT_MACHINE_ID}\n")).unwrap();
    let config = ServerConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        insecure_bootstrap: true,
        data_dir: data.path().to_path_buf(),
        shutdown_grace: Duration::from_secs(1),
        machine_id_files: vec![machine_id.clone()],
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, None);
    let seal_file = ctx.machine_seal_path();
    let server = ServerBuilder::new(ctx.clone()).build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    Driver {
        url: format!("http://{addr}"),
        ctx,
        seal_file,
        machine_id,
        _dirs: vec![data, etc],
    }
}

async fn client(url: &str, token: Option<&str>) -> SpacesdClient {
    let mut o = ConnectOptions::parse(url)
        .unwrap()
        .transport(TransportPreference::Native)
        .probe(false);
    if let Some(t) = token {
        o = o.token(t);
    }
    SpacesdClient::connect(o).await.unwrap()
}

async fn seal_key(c: &SpacesdClient) -> Vec<u8> {
    c.system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner()
        .machine_seal_public_key
}

async fn stat_code(c: &SpacesdClient) -> Code {
    match c
        .filesystem()
        .stat(StatRequest {
            path: "/".into(),
            ..Default::default()
        })
        .await
    {
        Ok(_) => Code::Ok,
        Err(s) => s.code(),
    }
}

fn opens(ctx: &ServerContext, envelope: &cua_machine_seal::SealedEnvelope) -> bool {
    let keypair = ctx.machine_seal().expect("a key");
    cua_machine_seal::open(
        &keypair,
        "space-1",
        "test",
        &cua_machine_seal::ReplayGuard::new(),
        envelope,
    )
    .is_ok()
}

#[tokio::test]
async fn new_machine_regenerates_the_seal_key_rotates_the_token_and_the_old_key_no_longer_opens() {
    let d = start().await;
    let anon = client(&d.url, None).await;

    // Waiting for a token (what a golden or pool snapshot captures): no key
    // in memory, none on disk, none advertised.
    assert!(seal_key(&anon).await.is_empty());
    assert!(!d.seal_file.exists());

    // The daemon starts the pool member with its pool secret: a key now.
    anon.system()
        .init(InitRequest {
            token: POOL_SECRET.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let pool = client(&d.url, Some(POOL_SECRET)).await;
    let old_key = seal_key(&pool).await;
    assert_eq!(old_key.len(), 32);
    assert!(d.seal_file.exists());
    let to_old = cua_machine_seal::seal(
        cua_machine_seal::MachinePublicKey(old_key.clone().try_into().unwrap()),
        "space-1",
        "test",
        b"sealed to the snapshot's machine",
    )
    .unwrap();
    assert!(opens(&d.ctx, &to_old));

    // Too little entropy is refused before anything changes.
    let refused = pool
        .system()
        .init(InitRequest {
            token: SPACE_TOKEN.into(),
            new_machine: true,
            entropy: vec![1; 8],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::InvalidArgument);
    assert_eq!(stat_code(&pool).await, Code::Ok, "pool secret still valid");

    // The claim: new token, new machine.
    let response = pool
        .system()
        .init(InitRequest {
            token: SPACE_TOKEN.into(),
            new_machine: true,
            entropy: (0..32).collect(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert!(response.new_machine);
    assert!(response.token_changed);

    // The old token is refused from now on; the new one works.
    assert_eq!(stat_code(&pool).await, Code::Unauthenticated);
    let space = client(&d.url, Some(SPACE_TOKEN)).await;
    assert_eq!(stat_code(&space).await, Code::Ok);

    // A new key, persisted, and the old one no longer opens anything.
    let new_key = seal_key(&space).await;
    assert_eq!(new_key.len(), 32);
    assert_ne!(new_key, old_key);
    assert_eq!(
        cua_machine_seal::MachineKeypair::load_or_create(&d.seal_file)
            .unwrap()
            .public()
            .0
            .to_vec(),
        new_key
    );
    assert!(!opens(&d.ctx, &to_old));
    let to_new = cua_machine_seal::seal(
        cua_machine_seal::MachinePublicKey(new_key.try_into().unwrap()),
        "space-1",
        "test",
        b"sealed to this machine",
    )
    .unwrap();
    assert!(opens(&d.ctx, &to_new));

    // A new machine id where the driver may write it (Linux).
    let id = std::fs::read_to_string(&d.machine_id).unwrap();
    if cfg!(target_os = "linux") {
        let id = id.trim();
        assert_ne!(id, SNAPSHOT_MACHINE_ID);
        assert!(cua_spacesd_server::identity::valid_machine_id(id), "{id}");
    } else {
        assert_eq!(id.trim(), SNAPSHOT_MACHINE_ID);
    }
}

#[tokio::test]
async fn two_clones_of_one_snapshot_end_up_with_different_keys_and_ids() {
    // Two drivers that start from the same on-disk state (a snapshot's):
    // the same seal key file and machine id.
    let a = start().await;
    let b = start().await;
    for d in [&a, &b] {
        client(&d.url, None)
            .await
            .system()
            .init(InitRequest {
                token: POOL_SECRET.into(),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let shared = client(&a.url, Some(POOL_SECRET)).await;
    let _ = seal_key(&shared).await;
    std::fs::copy(&a.seal_file, &b.seal_file).unwrap();
    let mut keys = Vec::new();
    for (d, token) in [
        (&a, "space-a-0123456789abcdef"),
        (&b, "space-b-0123456789abcdef"),
    ] {
        let c = client(&d.url, Some(POOL_SECRET)).await;
        let r = c
            .system()
            .init(InitRequest {
                token: token.into(),
                new_machine: true,
                // The same entropy on purpose: OS randomness still separates them.
                entropy: vec![42; 32],
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        assert!(r.new_machine);
        keys.push(seal_key(&client(&d.url, Some(token)).await).await);
    }
    assert_ne!(keys[0], keys[1]);
    if cfg!(target_os = "linux") {
        assert_ne!(
            std::fs::read_to_string(&a.machine_id).unwrap(),
            std::fs::read_to_string(&b.machine_id).unwrap()
        );
    }
}

#[tokio::test]
async fn an_await_token_file_driver_creates_no_seal_key_before_its_token() {
    let data = tempfile::tempdir().unwrap();
    let run = tempfile::tempdir().unwrap();
    let token_file = run.path().join("env-token");
    let config = ServerConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        await_token_file: Some(token_file.clone()),
        data_dir: data.path().to_path_buf(),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, None);
    let mut renewals = ctx.machine_renewals();
    assert!(ctx.machine_seal().is_none());
    assert!(ctx.machine_seal_public_key().is_empty());
    assert!(!ctx.machine_seal_path().exists());
    // The claim's token arrives: the in-memory secrets a waiting snapshot
    // held are renewed, and a key may exist from now on.
    assert!(ctx.apply_file_token(Some(SPACE_TOKEN)));
    assert!(renewals.has_changed().unwrap());
    renewals.mark_unchanged();
    assert_eq!(ctx.machine_seal_public_key().len(), 32);
    assert!(ctx.machine_seal_path().exists());
    // A rotation is the same tenant: no renewal, the key stays.
    let key = ctx.machine_seal_public_key();
    assert!(ctx.apply_file_token(Some("rotated-token-0123456789")));
    assert!(!renewals.has_changed().unwrap());
    assert_eq!(ctx.machine_seal_public_key(), key);
}
