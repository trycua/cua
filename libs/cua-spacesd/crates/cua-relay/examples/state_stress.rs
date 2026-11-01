// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Stress and chaos runs for the relay's state volume (CUA-1297). Run them
//! on a small, size-limited volume (a disk image or tmpfs), never a disk
//! that matters: they fill it on purpose.
//!
//! ```text
//! state_stress seed     <devices.json> <accounts> <events-per-account>
//! state_stress churn    <devices.json> <seconds>
//! state_stress scenario <state-dir>
//! state_stress mesh     <state-dir> <cua-relay binary> <port>
//! ```
//!
//! - `seed` writes a device store whose accounts each hold that many audit
//!   events; `churn` records `machine_access` events as fast as it can (kill
//!   it with `kill -9` mid-write and run it again to see what an interrupted
//!   write leaves).
//! - `scenario` runs a relay in-process on `<state-dir>`: leftover temp files
//!   at startup, pool appends until the reserve refuses them, device, team
//!   and pairing code writes meanwhile, then the volume filled to the last
//!   byte, `/readyz`, `/metrics` and recovery. Prints one JSON line per step.
//! - `mesh` starts the relay binary on `<state-dir>`, a machine and a client
//!   mesh node with a direct session, then kills the relay (`SIGKILL`),
//!   fills the volume, starts it again and checks the session and the
//!   relay's answers throughout.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use cua_relay::devices::{
    device_id, register_message, session_message, AuditEvent, DevicePolicy, DeviceStore,
};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_relay::storage::StorageLimits;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let args: Vec<String> = std::env::args().collect();
    let usage = "state_stress seed <file> <accounts> <events> | churn <file> <secs> | scenario <dir> | mesh <dir> <relay-bin> <port>";
    let arg = |i: usize| args.get(i).expect(usage).clone();
    match args.get(1).map(String::as_str) {
        Some("seed") => seed(
            Path::new(&arg(2)),
            arg(3).parse().expect(usage),
            arg(4).parse().expect(usage),
        ),
        Some("churn") => churn(
            Path::new(&arg(2)),
            Duration::from_secs(arg(3).parse().expect(usage)),
        ),
        Some("scenario") => runtime().block_on(scenario(PathBuf::from(arg(2)))),
        Some("mesh") => runtime().block_on(mesh(
            PathBuf::from(arg(2)),
            PathBuf::from(arg(3)),
            arg(4).parse().expect(usage),
        )),
        _ => panic!("{usage}"),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
}

/// Writes a devices file whose accounts each hold `events` audit events (a
/// legacy, pre-chain log inside the devices file), then opens it once so
/// the store takes it over in its own format.
fn seed(path: &Path, accounts: usize, events: usize) {
    let now = cua_relay::assertion::now_secs();
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    write!(
        out,
        "{{\"grace_started_at\":{now},\"devices\":{{}},\"audit\":{{"
    )
    .unwrap();
    for a in 0..accounts {
        if a > 0 {
            out.write_all(b",").unwrap();
        }
        write!(out, "\"account-{a:04}\":[").unwrap();
        for e in 0..events {
            if e > 0 {
                out.write_all(b",").unwrap();
            }
            let ev = AuditEvent::new("machine_access")
                .device(Some(&format!("dev_{a:04}_{:04}", e % 64)))
                .machine(&format!("machine-{a:04}-{:04}", e % 32))
                .subject(format!("user-{a}@example.com"))
                .detail("seeded".to_string());
            serde_json::to_writer(&mut out, &ev).unwrap();
        }
        out.write_all(b"]").unwrap();
    }
    out.write_all(b"}}").unwrap();
    out.flush().unwrap();
    drop(out);
    let t = Instant::now();
    DeviceStore::open(path.to_path_buf(), DevicePolicy::default()).expect("open seeded store");
    println!(
        "seeded {accounts} accounts x {events} events in {:?}",
        t.elapsed()
    );
}

/// Records `machine_access` audit events, round-robin over 16 seeded
/// accounts, for `secs`.
fn churn(path: &Path, secs: Duration) {
    let t = Instant::now();
    let store = DeviceStore::open(path.to_path_buf(), DevicePolicy::default()).expect("open");
    println!("opened in {:?}", t.elapsed());
    let start = Instant::now();
    let mut n = 0u64;
    let mut last = Instant::now();
    while start.elapsed() < secs {
        let a = n % 16;
        store.audit(
            &format!("account-{a:04}"),
            AuditEvent::new("machine_access")
                .device(Some("dev_churn"))
                .machine(&format!("machine-{a:04}-churn"))
                .detail(format!("churn {n}")),
        );
        n += 1;
        if last.elapsed() >= Duration::from_secs(1) {
            println!(
                "events={n} elapsed={:?} writes_ok={}",
                start.elapsed(),
                store.writes_ok()
            );
            last = Instant::now();
        }
    }
    println!(
        "done events={n} elapsed={:?} writes_ok={}",
        start.elapsed(),
        store.writes_ok()
    );
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn std64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// An enrolled device with a live session.
struct Device {
    token: String,
    session: String,
    id: String,
    pair: ring::signature::EcdsaKeyPair,
    pool_key: String,
}

impl Device {
    async fn call(
        &self,
        method: reqwest::Method,
        url: String,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut r = reqwest::Client::new()
            .request(method, url)
            .bearer_auth(&self.token)
            .header("x-cua-device-session", &self.session);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }
}

/// Enrolls a new device of `sub`: `Err((status, body))` when refused.
async fn enroll(issuer: &FakeIssuer, base: &str, sub: &str) -> Result<Device, (u16, Value)> {
    issuer.set_auth_time(sub, cua_relay::assertion::now_secs() as i64);
    let token = issuer.token(sub, Some(&format!("{sub}@example.com")), "cua-relay", 3600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = device_id(pair.public_key().as_ref());
    let sign = |m: &str| b64().encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let ts = cua_relay::assertion::now_secs();
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64().encode(pair.public_key().as_ref()),
            "name": "stress", "ts": ts, "sig": sign(&register_message(&id, ts)), "bootstrap": true,
            "machine_id": format!("machine-{}", uuid::Uuid::new_v4().simple()),
        }))
        .send()
        .await
        .unwrap();
    if !r.status().is_success() {
        return Err((r.status().as_u16(), r.json().await.unwrap_or(Value::Null)));
    }
    let r = http
        .post(format!("{base}/v1/devices/session"))
        .bearer_auth(&token)
        .json(&json!({"device_id": id, "ts": ts, "sig": sign(&session_message(&id, ts))}))
        .send()
        .await
        .unwrap();
    if !r.status().is_success() {
        return Err((r.status().as_u16(), r.json().await.unwrap_or(Value::Null)));
    }
    let v: Value = r.json().await.unwrap();
    let key: [u8; 32] = rand::random();
    Ok(Device {
        token,
        session: v["session"].as_str().unwrap().to_owned(),
        id,
        pair,
        pool_key: std64().encode(key),
    })
}

/// Sealed data of about `bytes` bytes, as devices send it.
fn sealed(bytes: usize) -> Value {
    let filler: Vec<u8> = (0..bytes * 3 / 4).map(|_| rand::random::<u8>()).collect();
    json!({"v": 1, "g": 1, "n": "bm9uY2Vub25jZW5vbmNlbm9uY2Vub25j", "c": std64().encode(filler)})
}

fn df(dir: &Path) -> Value {
    match cua_relay::storage::space(dir) {
        Ok(s) => json!({"total": s.total, "available": s.available,
                         "free_percent": (s.free_percent() * 10.0).round() / 10.0}),
        Err(e) => json!({"error": e.to_string()}),
    }
}

fn temps(dir: &Path) -> (usize, u64) {
    let mut n = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let m = e.metadata().unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else if e.file_name().to_string_lossy().ends_with(".tmp") {
                n.0 += 1;
                n.1 += m.len();
            }
        }
    }
    n
}

fn step(name: &str, v: Value) {
    println!("{}", json!({"step": name, "result": v}));
}

async fn get(url: String) -> (u16, Value) {
    let r = reqwest::get(url).await.unwrap();
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

async fn metric_lines(base: &str, names: &[&str]) -> Vec<String> {
    reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && names.iter().any(|n| l.starts_with(n)))
        .map(str::to_owned)
        .collect()
}

/// Fills `dir`'s volume with one file until the write fails (ENOSPC).
fn fill(dir: &Path) -> (PathBuf, u64, String) {
    let path = dir.join("filler.bin");
    let mut f = std::fs::File::create(&path).unwrap();
    let block = vec![0u8; 1 << 20];
    let mut written = 0u64;
    let err = loop {
        match f.write_all(&block) {
            Ok(()) => written += block.len() as u64,
            Err(e) => break e,
        }
    };
    // The last bytes, block by smaller block.
    let mut small = vec![0u8; 4096];
    while f.write_all(&small).is_ok() {
        written += small.len() as u64;
    }
    small.truncate(1);
    while f.write_all(&small).is_ok() {
        written += 1;
    }
    let _ = f.sync_all();
    (path, written, err.to_string())
}

async fn scenario(dir: PathBuf) {
    std::fs::create_dir_all(&dir).unwrap();
    let state = dir.join("relay.json");
    // What the incident left behind: temp files of interrupted writes, of
    // the devices file and of every other store next to it.
    for _ in 0..3 {
        let p = dir.join(format!(
            ".relay.devices.json.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&p, vec![b'{'; 16 << 20]).unwrap();
    }
    for file in [
        "relay.teams.json",
        "relay.pairing.json",
        "relay.grants.json",
        "relay.usage.json",
    ] {
        let p = dir.join(format!(".{file}.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&p, vec![b'['; 1 << 20]).unwrap();
    }
    step(
        "orphans planted",
        json!({"temps": temps(&dir), "disk": df(&dir)}),
    );

    let issuer = FakeIssuer::new(ISSUER);
    let limits = StorageLimits {
        reserve_percent: 15,
        reserve_min_bytes: 8 << 20,
        pool_account_quota_bytes: 1 << 30,
    };
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state.clone()),
        storage_limits: limits,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    step(
        "relay started",
        json!({"temps": temps(&dir), "disk": df(&dir),
               "metrics": metric_lines(&base, &["cua_relay_state_temp"]).await}),
    );

    // A coordinator fills its pool log.
    let a = enroll(&issuer, &base, "ada").await.expect("enroll a");
    let account = a
        .call(reqwest::Method::GET, format!("{base}/v1/pool"), None)
        .await
        .1["account"]
        .as_str()
        .unwrap()
        .to_owned();
    let rng = ring::rand::SystemRandom::new();
    let m = cua_relay::pool::member_message(&account, &a.id, &a.pool_key);
    let sig = b64().encode(a.pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let (s, v) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/pool/member"),
            Some(json!({"pool_key": a.pool_key, "sig": sig})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let wrap =
        json!({"device": a.id, "key": a.pool_key, "from": a.id, "enc": "ZW5j", "ct": "Y3Q="});
    let (s, v) = a
        .call(
            reqwest::Method::PUT,
            format!("{base}/v1/pool/keys"),
            Some(json!({"version": 0, "keyring": {"gen": 1, "wraps": [wrap]}})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (s, v) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/pool/lease"),
            Some(json!({"name": "a", "priority": 10, "epoch": 0, "ttl_ms": 120_000})),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let epoch = v["epoch"].as_u64().unwrap();
    let mut head = 0u64;
    let started = Instant::now();
    let refusal = loop {
        let (s, v) = a
            .call(
                reqwest::Method::POST,
                format!("{base}/v1/pool/log"),
                Some(json!({"epoch": epoch, "from": head,
                            "entries": [{"kind": "submit", "run": format!("r{head}"), "data": sealed(512 << 10)}]})),
            )
            .await;
        if s != 200 {
            break (s, v);
        }
        head = v["head"].as_u64().unwrap();
    };
    let (_, ready) = get(format!("{base}/readyz")).await;
    step(
        "pool appends until refused",
        json!({"appended": head, "secs": started.elapsed().as_secs_f64(),
               "refusal": {"status": refusal.0, "body": refusal.1},
               "disk": df(&dir), "readyz_disk": ready["disk"], "ready": ready["ready"]}),
    );

    // Devices, sign-ins, teams and pairing codes keep working in the
    // reserve.
    let b = enroll(&issuer, &base, "ada").await;
    let c = enroll(&issuer, &base, "bob").await;
    let team = |n: &str| Some(json!({"name": n}));
    let code = || Some(json!({"ttl_secs": 600}));
    let (team_s, _) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/teams"),
            team("Ops"),
        )
        .await;
    let (code_s, _) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/pairing-codes"),
            code(),
        )
        .await;
    let (s, info) = get(format!("{base}/v1/info")).await;
    step(
        "device writes while pool appends are refused",
        json!({"second_device_of_ada": b.is_ok(), "first_device_of_bob": c.is_ok(),
               "team_created": team_s, "pairing_code_minted": code_s,
               "info_status": s, "state_writable": info["state_writable"], "disk": df(&dir)}),
    );

    // Something else fills the volume to the last byte.
    let (filler, written, err) = fill(&dir);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let refused = enroll(&issuer, &base, "carol").await.err();
    let (team_refused, team_body) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/teams"),
            team("Lost"),
        )
        .await;
    let (code_refused, code_body) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/pairing-codes"),
            code(),
        )
        .await;
    let (rs, ready) = get(format!("{base}/readyz")).await;
    let live = reqwest::get(format!("{base}/healthz"))
        .await
        .unwrap()
        .status()
        .as_u16();
    let (_, info) = get(format!("{base}/v1/info")).await;
    step(
        "volume full",
        json!({"filler_bytes": written, "fill_error": err, "disk": df(&dir),
               "new_device_refused": refused,
               "new_team_refused": {"status": team_refused, "body": team_body},
               "new_pairing_code_refused": {"status": code_refused, "body": code_body},
               "readyz_status": rs, "readyz": ready,
               "healthz_status": live, "state_writable": info["state_writable"],
               "metrics": metric_lines(&base, &["cua_relay_state_persist_failures_total",
                 "cua_relay_audit_events_dropped_total", "cua_relay_state_probe",
                 "cua_relay_pool_writes_refused_total"]).await}),
    );

    // Space comes back: the next writes succeed and readiness recovers.
    std::fs::remove_file(&filler).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let d = enroll(&issuer, &base, "carol").await;
    let (team_s, _) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/teams"),
            team("Back"),
        )
        .await;
    let (code_s, _) = a
        .call(
            reqwest::Method::POST,
            format!("{base}/v1/pairing-codes"),
            code(),
        )
        .await;
    let (rs, ready) = get(format!("{base}/readyz")).await;
    step(
        "recovered",
        json!({"new_device_enrolled": d.is_ok(), "team_created": team_s,
               "pairing_code_minted": code_s, "readyz_status": rs,
               "failing": ready["failing"], "disk": df(&dir)}),
    );
}

/// Writes `label` padded to 32 bytes through the echo service and reads it
/// back: whether it came back, how long it took, and the stream.
async fn echo(mut s: cua_mesh::MeshStream, label: &str) -> (bool, u64, cua_mesh::MeshStream) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let msg = format!("{label:-<32}");
    let t = Instant::now();
    let ok = tokio::time::timeout(Duration::from_secs(10), async {
        s.write_all(msg.as_bytes()).await?;
        let mut buf = vec![0u8; msg.len()];
        s.read_exact(&mut buf).await?;
        Ok::<_, std::io::Error>(buf == msg.as_bytes())
    })
    .await
    .map(|r| r.unwrap_or(false))
    .unwrap_or(false);
    (ok, t.elapsed().as_millis() as u64, s)
}

async fn mesh(dir: PathBuf, relay_bin: PathBuf, port: u16) {
    use cua_mesh::{Mesh, MeshConfig, NodeKind};

    std::fs::create_dir_all(&dir).unwrap();
    let nodes = tempfile::tempdir().unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let start_relay = || {
        std::process::Command::new(&relay_bin)
            .args([
                "--listen",
                &format!("127.0.0.1:{port}"),
                "--tokens",
                "reg",
                "--admin-token",
                "admin",
                "--state-file",
                dir.join("state.json").to_str().unwrap(),
            ])
            .env("RUST_LOG", "warn")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let wait_up = || async {
        for _ in 0..100 {
            if reqwest::get(format!("{url}/healthz")).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the relay did not start");
    };
    let mut relay = start_relay();
    wait_up().await;

    let mut m = MeshConfig::new(
        url.clone(),
        NodeKind::Machine,
        Some(nodes.path().join("m.json")),
        cua_mesh::static_auth(vec![
            ("authorization".into(), "Bearer reg".into()),
            ("x-cua-machine-id".into(), "machine-0001".into()),
        ]),
    );
    m.approved_path = Some(nodes.path().join("approved.json"));
    m.pair_verifier = Some(Arc::new(|client, ts, proof| {
        cua_mesh::proto::verify_pair_proof(
            "env-1",
            "machine-0001",
            client,
            ts,
            proof,
            cua_mesh::proto::now_secs(),
        )
    }));
    let machine = Mesh::start(m).await.unwrap();
    machine.wait_ready(Duration::from_secs(10)).await.unwrap();
    let server = machine.clone();
    tokio::spawn(async move {
        while let Some(i) = server.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(i.stream);
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let client = Mesh::start(MeshConfig::new(
        url.clone(),
        NodeKind::Client,
        Some(nodes.path().join("c.json")),
        cua_mesh::static_auth(Vec::new()),
    ))
    .await
    .unwrap();
    client.wait_ready(Duration::from_secs(10)).await.unwrap();
    client.pair("machine-0001", "env-1").await.unwrap();
    let peer = client
        .wait_machine("machine-0001", Duration::from_secs(10))
        .await
        .expect("paired");
    let mut s = client
        .open_stream(&peer.node_key, "echo", None)
        .await
        .unwrap();
    // Wait for the direct path.
    for _ in 0..100 {
        if matches!(client.path(&peer.node_key), Some(cua_mesh::Path::Direct(_))) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let path0 = client.path(&peer.node_key).map(|p| p.to_string());
    let (ok, ms, s2) = echo(s, "before").await;
    s = s2;
    step(
        "session up",
        json!({"path": path0, "echo": ok, "ms": ms, "disk": df(&dir)}),
    );

    // kill -9 the relay.
    relay.kill().unwrap();
    let _ = relay.wait();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut echoes = Vec::new();
    for i in 0..5 {
        let (ok, ms, s2) = echo(s, "relay killed").await;
        s = s2;
        echoes.push(json!({"i": i, "ok": ok, "ms": ms}));
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    step(
        "relay killed (SIGKILL)",
        json!({"relay_reachable": reqwest::get(format!("{url}/healthz")).await.is_ok(),
               "path": client.path(&peer.node_key).map(|p| p.to_string()), "echoes": echoes}),
    );

    // The volume fills; the relay comes back on it.
    let (filler, written, err) = fill(&dir);
    let mut relay = start_relay();
    wait_up().await;
    let reconnected = client.wait_ready(Duration::from_secs(20)).await.is_ok()
        && machine.wait_ready(Duration::from_secs(20)).await.is_ok();
    let (ok, ms, s2) = echo(s, "disk full").await;
    s = s2;
    let (rs, ready) = get(format!("{url}/readyz")).await;
    // A new client's pairing cannot be saved: it is refused, not confirmed.
    let other = Mesh::start(MeshConfig::new(
        url.clone(),
        NodeKind::Client,
        Some(nodes.path().join("c2.json")),
        cua_mesh::static_auth(Vec::new()),
    ))
    .await
    .unwrap();
    let other_ready = other.wait_ready(Duration::from_secs(20)).await.is_ok();
    let pairing = other
        .pair("machine-0001", "env-1")
        .await
        .map_err(|e| e.to_string());
    step(
        "relay restarted on a full volume",
        json!({"filler_bytes": written, "fill_error": err, "disk": df(&dir),
               "nodes_reconnected": reconnected, "existing_session_echo": ok, "ms": ms,
               "path": client.path(&peer.node_key).map(|p| p.to_string()),
               "readyz_status": rs, "probe": ready["probe"], "failing": ready["failing"],
               "new_client_ready": other_ready, "new_pairing": pairing}),
    );

    // Space back: the new pairing goes through.
    std::fs::remove_file(&filler).unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let pairing = other
        .pair("machine-0001", "env-1")
        .await
        .map_err(|e| e.to_string());
    let (ok, ms, _s) = echo(s, "space back").await;
    let (rs, ready) = get(format!("{url}/readyz")).await;
    step(
        "space back",
        json!({"existing_session_echo": ok, "ms": ms, "new_pairing": pairing,
               "readyz_status": rs, "failing": ready["failing"]}),
    );
    relay.kill().unwrap();
    let _ = relay.wait();
}
