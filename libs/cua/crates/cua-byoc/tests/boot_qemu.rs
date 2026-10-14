// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The cloud first boot, with no cloud: what `cua_byoc::bootstrap` renders
//! for a cloud VM must make the sandbox image join a relay as the machine
//! this device registered, exactly as on EC2 or Compute Engine.
//!
//! The relay is a local `cua-relay` in account mode (the mode relay.cua.ai
//! runs: machines registered by an account, guests that pin the relay's
//! assertion keys), with an in-test OIDC issuer (an Ed25519 key whose JWKS
//! is served on loopback). The test registers the machine with
//! `POST /v1/machines`, as `cua_byoc::RelayAccess::register` does.
//!
//! Two opt-in tests (ignored by default):
//!
//! - `the_sandbox_container_joins_an_account_relay`: the image run with the
//!   container environment and relay files cloud-init writes, under this
//!   machine's Docker (seconds; `CUA_BYOC_JOIN_TEST=1`);
//! - `a_cloud_vm_first_boot_joins_the_relay`: an Ubuntu cloud image booted
//!   in local QEMU with the whole user data (Docker installed, the image
//!   pulled and started), one VM of 4 GiB, HVF on macOS or KVM on Linux
//!   (minutes; `CUA_BYOC_BOOT_TEST=1`).
//!
//! ```text
//! CUA_RELAY_BIN=<cua-relay> CUA_BYOC_BOOT_TEST=1 CUA_BYOC_JOIN_TEST=1 \
//!   cargo test -p cua-byoc --test boot_qemu -- --ignored --nocapture --test-threads 1
//! ```
//!
//! - `CUA_RELAY_BIN`: a `cua-relay` binary (`cargo build -p cua-relay` in
//!   `libs/cua-spacesd`).
//! - `CUA_BYOC_CLOUD_IMAGE`: an Ubuntu 24.04 cloud image for this machine's
//!   architecture; default: downloaded once into
//!   `~/projects/.cua-work/cloud-providers/boot-test/`.
//! - `CUA_BYOC_BOOT_IMAGE`: the sandbox image (default
//!   `ghcr.io/trycua/linux:24.04`).
//!
//! The VM or container, the overlay disk and the relay are gone when a test
//! ends, pass or fail.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use cua_byoc::bootstrap::{VmBoot, cloud_init, container_env};
use cua_byoc::relay::Join;
use ring::signature::KeyPair as _;

const ISSUER: &str = "https://issuer.boot-test.invalid";

struct Kill(Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    use ring::rand::SecureRandom as _;
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new().fill(&mut b).unwrap();
    b
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The OIDC issuer: an Ed25519 key, its JWKS served on loopback, and
/// account tokens signed with it.
struct Issuer {
    key: ring::signature::Ed25519KeyPair,
    jwks_url: String,
}

impl Issuer {
    fn start() -> Self {
        let pkcs8 =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
                .unwrap();
        let key = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let jwks = serde_json::json!({"keys": [{
            "kty": "OKP", "crv": "Ed25519", "kid": "boot-test", "alg": "EdDSA", "use": "sig",
            "x": b64(key.public_key().as_ref()),
        }]})
        .to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let jwks_url = format!("http://{}/jwks", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                    if line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                let mut s = &stream;
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{jwks}",
                    jwks.len()
                );
            }
        });
        Issuer { key, jwks_url }
    }

    fn token(&self, sub: &str) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let header = b64(br#"{"alg":"EdDSA","kid":"boot-test","typ":"JWT"}"#);
        let claims = b64(serde_json::json!({
            "iss": ISSUER, "sub": sub, "aud": "cua-relay", "iat": now, "exp": now + 3600,
            "auth_time": now, "email": format!("{sub}@example.com"), "email_verified": true,
            "name": sub,
        })
        .to_string()
        .as_bytes());
        let signing = format!("{header}.{claims}");
        let sig = self.key.sign(signing.as_bytes());
        format!("{signing}.{}", b64(sig.as_ref()))
    }
}

/// A local relay in account mode, and the account `ada` on it.
struct Relay {
    _proc: Kill,
    port: u16,
    token: String,
    _issuer: Issuer,
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn curl(args: &[&str]) -> (bool, String) {
    let out = Command::new("curl")
        .args(["-sS", "-w", "\n%{http_code}"])
        .args(args)
        .output()
        .expect("curl");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or((&text, "000"));
    (code.starts_with('2'), body.to_string())
}

impl Relay {
    fn start(log: &Path) -> Self {
        let bin = std::env::var("CUA_RELAY_BIN").expect("CUA_RELAY_BIN: a cua-relay binary");
        let issuer = Issuer::start();
        let port = free_port();
        let proc = Kill(
            Command::new(bin)
                .args(["--listen", &format!("0.0.0.0:{port}")])
                .args(["--oidc-issuer", ISSUER, "--oidc-jwks-url", &issuer.jwks_url])
                .args(["--public-url", &format!("http://127.0.0.1:{port}")])
                .args(["--device-enrollment", "off"])
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(log).unwrap())
                .spawn()
                .expect("start cua-relay"),
        );
        let t = Instant::now();
        while !curl(&[&format!("http://127.0.0.1:{port}/healthz")]).0 {
            assert!(
                t.elapsed() < Duration::from_secs(20),
                "the relay did not start"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        let token = issuer.token("ada");
        Relay {
            _proc: proc,
            port,
            token,
            _issuer: issuer,
        }
    }

    /// Registers machine `id`, as `RelayAccess::register` does, and returns
    /// what the guest joins with (the relay reached at `guest_host`).
    fn register(&self, id: &str, guest_host: &str) -> Join {
        let (ok, body) = curl(&[
            "-X",
            "POST",
            "-H",
            &format!("authorization: Bearer {}", self.token),
            "-H",
            "content-type: application/json",
            "-d",
            &serde_json::json!({"id": id, "name": id, "allow": []}).to_string(),
            &format!("http://127.0.0.1:{}/v1/machines", self.port),
        ]);
        assert!(ok, "register {id}: {body}");
        let reg: serde_json::Value = serde_json::from_str(&body).unwrap();
        Join {
            relay_url: format!("http://{guest_host}:{}", self.port),
            machine_id: reg["machine"]["id"].as_str().unwrap().to_string(),
            machine_token: reg["machine_token"].as_str().unwrap().to_string(),
            jwks_json: reg["jwks"].to_string(),
            owner: reg["machine"]["owner"]["id"].as_str().unwrap().to_string(),
            owner_email: String::new(),
        }
    }

    fn online(&self, id: &str) -> bool {
        let (ok, body) = curl(&[
            "-H",
            &format!("authorization: Bearer {}", self.token),
            &format!("http://127.0.0.1:{}/v1/machines/{id}", self.port),
        ]);
        ok && serde_json::from_str::<serde_json::Value>(&body)
            .is_ok_and(|m| m["online"].as_bool() == Some(true))
    }

    /// Waits until `id` is online; on timeout, panics with `diag()`.
    fn wait_online(&self, id: &str, budget: Duration, diag: impl Fn() -> String) {
        let t = Instant::now();
        while !self.online(id) {
            if t.elapsed() > budget {
                panic!("{id} did not join within {budget:?}\n{}", diag());
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        eprintln!("{id} joined the relay after {:?}", t.elapsed());
    }
}

fn image() -> String {
    std::env::var("CUA_BYOC_BOOT_IMAGE").unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04".into())
}

fn work_dir() -> tempfile::TempDir {
    let home = std::env::var("HOME").unwrap();
    let cache = PathBuf::from(home).join("projects/.cua-work/cloud-providers/boot-test");
    std::fs::create_dir_all(&cache).unwrap();
    tempfile::Builder::new()
        .prefix("byoc-boot-")
        .tempdir_in(&cache)
        .unwrap()
}

fn tail(path: &Path, n: usize) -> String {
    let log = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn env() -> BTreeMap<String, String> {
    BTreeMap::from([("CUA_ENV_TOKEN".to_string(), hex(&rand_bytes::<12>()))])
}

#[test]
#[ignore = "runs the sandbox image under this machine's Docker (CUA_BYOC_JOIN_TEST=1)"]
fn the_sandbox_container_joins_an_account_relay() {
    if std::env::var("CUA_BYOC_JOIN_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_BYOC_JOIN_TEST=1");
        return;
    }
    let work = work_dir();
    let relay = Relay::start(&work.path().join("relay.log"));
    let id = format!("join-test-{}", hex(&rand_bytes::<4>()));
    let join = relay.register(&id, "host.docker.internal");
    let env = env();
    let boot = VmBoot {
        join: &join,
        image: &image(),
        env: &env,
        expires: 0,
        ttl_secs: 0,
        shm_mb: 1024,
    };
    // The files and the environment file cloud-init writes, as the VM's
    // first boot hands them to Docker (owned by the image's user).
    let files = work.path().join("relay");
    std::fs::create_dir_all(&files).unwrap();
    for (name, body) in [
        ("machine-token", join.machine_token.clone()),
        ("machine-id", format!("{}\n", join.machine_id)),
        ("jwks.json", join.jwks_json.clone()),
        ("policy.json", join.policy_json()),
    ] {
        std::fs::write(files.join(name), body).unwrap();
    }
    let env_file = work.path().join("sandbox.env");
    std::fs::write(&env_file, container_env(&boot)).unwrap();
    // Docker Desktop and Colima run the container as root over a shared
    // folder: the files must be readable by the image's user.
    let _ = Command::new("chmod")
        .arg("-R")
        .arg("a+rX")
        .arg(&files)
        .status();
    let name = format!("cua-join-test-{}", hex(&rand_bytes::<4>()));
    let ok = Command::new("docker")
        .args(["run", "-d", "--name", &name, "--memory", "3g", "--env-file"])
        .arg(&env_file)
        .arg("-v")
        .arg(format!("{}:/run/cua-relay:ro", files.display()))
        .arg(image())
        .status()
        .unwrap()
        .success();
    assert!(ok, "docker run");
    struct Rm(String);
    impl Drop for Rm {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", &self.0]).output();
        }
    }
    let _rm = Rm(name.clone());
    relay.wait_online(&id, Duration::from_secs(180), || {
        let logs = Command::new("docker")
            .args([
                "exec",
                &name,
                "sh",
                "-c",
                "tail -30 /var/log/supervisor/cua-*d.log",
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        format!(
            "guest log:\n{logs}\nrelay log:\n{}",
            tail(&work.path().join("relay.log"), 20)
        )
    });
}

fn arch() -> (&'static str, &'static str) {
    if cfg!(target_arch = "aarch64") {
        ("arm64", "qemu-system-aarch64")
    } else {
        ("amd64", "qemu-system-x86_64")
    }
}

fn cloud_image(dir: &Path) -> PathBuf {
    if let Ok(p) = std::env::var("CUA_BYOC_CLOUD_IMAGE") {
        return PathBuf::from(p);
    }
    let (arch, _) = arch();
    let path = dir.join(format!("noble-server-cloudimg-{arch}.img"));
    if !path.exists() {
        let url = format!(
            "https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-{arch}.img"
        );
        let tmp = path.with_extension("part");
        let ok = Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&tmp)
            .arg(&url)
            .status()
            .expect("curl")
            .success();
        assert!(ok, "download {url}");
        std::fs::rename(&tmp, &path).unwrap();
    }
    path
}

#[test]
#[ignore = "boots a local QEMU VM and pulls a 1.2 GB image (CUA_BYOC_BOOT_TEST=1)"]
fn a_cloud_vm_first_boot_joins_the_relay() {
    if std::env::var("CUA_BYOC_BOOT_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_BYOC_BOOT_TEST=1");
        return;
    }
    let work = work_dir();
    let base = cloud_image(work.path().parent().unwrap());
    let relay = Relay::start(&work.path().join("relay.log"));
    let id = format!("boot-test-{}", hex(&rand_bytes::<4>()));
    // The guest reaches this machine at QEMU's host address.
    let join = relay.register(&id, "10.0.2.2");
    let env = env();
    let user_data = cloud_init(&VmBoot {
        join: &join,
        image: &image(),
        env: &env,
        expires: 0,
        // The backstop still runs: the VM powers itself off within the hour.
        ttl_secs: 3600,
        shm_mb: 1024,
    });
    let seed = cua_vmm::cloudinit::Seed {
        meta_data: format!("instance-id: {id}\nlocal-hostname: {id}\n"),
        user_data,
        network_config: None,
    };
    let seed_iso = work.path().join("seed.iso");
    seed.write_iso(&seed_iso).unwrap();
    let disk = work.path().join("disk.qcow2");
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2", "-F", "qcow2", "-b"])
            .arg(&base)
            .arg(&disk)
            .arg("20G")
            .status()
            .unwrap()
            .success()
    );
    let (arch, qemu) = arch();
    let mut q = Command::new(qemu);
    q.args(["-m", "4096", "-smp", "4", "-nographic", "-no-reboot"]);
    if arch == "arm64" {
        let fw = [
            "/opt/homebrew/share/qemu/edk2-aarch64-code.fd",
            "/usr/share/AAVMF/AAVMF_CODE.fd",
        ]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .expect("an aarch64 UEFI firmware");
        q.args(["-M", "virt", "-cpu", "host", "-bios", fw]);
    } else {
        q.args(["-M", "q35", "-cpu", "host"]);
    }
    q.args([
        "-accel",
        if cfg!(target_os = "macos") {
            "hvf"
        } else {
            "kvm"
        },
    ]);
    q.arg("-drive")
        .arg(format!("if=virtio,format=qcow2,file={}", disk.display()));
    q.arg("-drive").arg(format!(
        "if=virtio,format=raw,readonly=on,file={}",
        seed_iso.display()
    ));
    q.args([
        "-netdev",
        "user,id=n0",
        "-device",
        "virtio-net-pci,netdev=n0",
    ]);
    let serial = work.path().join("serial.log");
    q.stdin(Stdio::null())
        .stdout(std::fs::File::create(&serial).unwrap())
        .stderr(Stdio::null());
    let _vm = Kill(q.spawn().expect("start QEMU"));
    relay.wait_online(&id, Duration::from_secs(20 * 60), || {
        format!(
            "serial log tail:\n{}\nrelay log:\n{}",
            tail(&serial, 40),
            tail(&work.path().join("relay.log"), 20)
        )
    });
}
