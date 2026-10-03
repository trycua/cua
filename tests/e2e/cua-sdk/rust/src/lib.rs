//! Shared helpers for the cua SDK e2e suite (Rust half). Mirrors
//! `../python/e2e.py`: `cua-e2e-<run>-*` names, env-gated lanes, bounded
//! polls, results appended to `$CUA_E2E_RESULTS/rust.jsonl`.
//!
//! The SDK is used exactly as a Rust application would: the `cua-sdk`
//! crate's exported API (`Cua`, `Sandboxes`, `SpacesdClient`, `Fleet`, ...).

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub use cua_sdk::{self as cua, CuaError};
use cua_sdk::{
    Cua, CuaConfig, SpacesdClient, SpacesdCommand, FleetPoolSpec, FleetSettings, ReadinessProbe, SandboxCreateOptions,
};

pub type Res<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
}

pub fn run_id() -> &'static str {
    static RUN: OnceLock<String> = OnceLock::new();
    RUN.get_or_init(|| std::env::var("CUA_E2E_RUN").unwrap_or_else(|_| hex(3)))
}

pub fn hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .expect("urandom");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `cua-e2e-<run>-<what>-rs` (a DNS label).
pub fn name(what: &str) -> String {
    let mut n = format!("cua-e2e-{}-{what}-rs", run_id());
    n.truncate(63);
    n.trim_end_matches('-').to_string()
}

pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

pub fn desktop_image() -> String {
    std::env::var("CUA_E2E_DESKTOP_IMAGE").unwrap_or_else(|_| {
        format!(
            "cua-e2e-local/linux:docker-local-{}",
            host_arch()
        )
    })
}

pub fn plain_image(n: &str) -> String {
    let var = if n == "ubuntu-server" {
        "CUA_E2E_PLAIN_SERVER_IMAGE"
    } else {
        "CUA_E2E_PLAIN_VNC_IMAGE"
    };
    std::env::var(var).unwrap_or_else(|_| format!("cua-e2e-local/{n}:docker-local-{}", host_arch()))
}

pub fn disk_path(image: &str) -> PathBuf {
    std::env::var(format!(
        "CUA_E2E_DISK_{}",
        image.to_uppercase().replace('-', "_")
    ))
    .map(PathBuf::from)
    .unwrap_or_else(|_| {
        PathBuf::from(std::env::var("HOME").unwrap())
            .join(".cache/cua-images-e2e")
            .join(image)
            .join(host_arch())
            .join("disk.img")
    })
}

pub const LEGACY_FLEET_ROOTFS: &str =
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81";
pub const LEGACY_FLEET_IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:82702ebdd32d1f8fc05f2ea409a7c67d0ba9f8f8e4e9f1a89ce40989d5f4475d";

fn binary(var: &str, exe: &str) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(var) {
        return Some(p.into());
    }
    ["debug", "release"]
        .iter()
        .map(|p| repo().join("libs/cua/target").join(p).join(exe))
        .find(|p| p.exists())
}

fn flag(v: &str) -> bool {
    std::env::var(v).map(|x| x == "1").unwrap_or(false)
}

/// Fleet pools have no env/secret field yet: a gVisor pod of the spacesd
/// image gets its token through an entrypoint override (the image reads
/// /etc/cua/env-token). KubeVirt containerDisk guests have no such hook.
pub const KUBEVIRT_ENV_SKIP: &str = "KubeVirt spacesd lane needs a per-claim secret field (cloud PR): \
     no way to deliver the env token to a containerDisk guest yet";

/// Pod command that installs `token` as the spacesd token, then runs the
/// image's entrypoint.
pub fn env_token_command(token: &str) -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        format!(
            "mkdir -p /etc/cua && printf %s {token} >/etc/cua/env-token && exec /opt/cua/desktop/entrypoint.sh"
        ),
    ]
}

/// `None` when `lane` can run here, else the skip reason.
pub fn lane_enabled(lane: &str) -> Option<String> {
    match lane {
        "hermetic" => binary("CUA_TEST_FIXTURES", "cua-test-fixtures")
            .is_none()
            .then(|| "cua-test-fixtures is not built".into()),
        "container" => (!flag("CUA_E2E_CONTAINER")).then(|| "set CUA_E2E_CONTAINER=1 for the container lane".into()),
        "qemu" => (!flag("CUA_E2E_QEMU")).then(|| "set CUA_E2E_QEMU=1 for the QEMU lane".into()),
        "lume" => (!flag("CUA_E2E_LUME")).then(|| "set CUA_E2E_LUME=1 for the Lume lane".into()),
        "fleet" => {
            if !flag("CUA_E2E_FLEET") {
                Some("set CUA_E2E_FLEET=1 for live Fleet".into())
            } else if std::env::var("FLEETS_TOKEN").is_err() && std::env::var("CUA_CLIENT_ID").is_err() {
                Some("no Fleet credentials".into())
            } else {
                None
            }
        }
        "fleet-env" => lane_enabled("fleet").or_else(|| {
            std::env::var("CUA_E2E_FLEET_ENV_IMAGE").is_err().then(|| {
                "CUA_E2E_FLEET_ENV_IMAGE is unset: no linux (spacesd) image in a registry Fleet can pull".into()
            })
        }),
        "conformance" => (!flag("CUA_E2E_CONFORMANCE") || !flag("CUA_E2E_CONTAINER"))
            .then(|| "set CUA_E2E_CONTAINER=1 CUA_E2E_CONFORMANCE=1 to run the spacesd conformance suite".into()),
        other => panic!("unknown lane {other}"),
    }
}

/// A test body asked to be skipped with a reason.
#[derive(Debug)]
pub struct Skip(pub String);
impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "skip: {}", self.0)
    }
}
impl std::error::Error for Skip {}

pub fn skip<T>(reason: impl Into<String>) -> Res<T> {
    Err(Box::new(Skip(reason.into())))
}

fn record(scenario: &str, lane: &str, test: &str, status: &str, secs: f64, reason: &str) {
    let Ok(dir) = std::env::var("CUA_E2E_RESULTS") else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let line = serde_json::json!({
        "scenario": scenario, "lang": "rust", "lane": lane, "test": test, "status": status,
        "secs": (secs * 10.0).round() / 10.0, "reason": reason, "run": run_id(),
    });
    // One write_all per record under a lock: tests run on several threads and
    // `writeln!` may split a record into several writes.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(&dir).join("rust.jsonl"))
    {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// Runs one scenario test: skips (recorded) when the lane is off, times it,
/// records the outcome and panics on failure. `xfail`: a known bug.
pub async fn e2e<F, Fut>(
    scenario: &str,
    lane: &str,
    test: &str,
    timeout: Duration,
    xfail: Option<&str>,
    body: F,
) where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Res>,
{
    if let Some(why) = lane_enabled(lane) {
        eprintln!("skipping {scenario}/{lane}/{test}: {why}");
        record(scenario, lane, test, "skip", 0.0, &why);
        return;
    }
    // Lanes that start a local container or VM run one at a time even when
    // the harness uses several test threads: two desktops booting at once
    // double the host's memory footprint (see AGENT_BRIEF memory rules).
    // Fleet and hermetic lanes stay parallel.
    static LOCAL_RUNTIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _local = if matches!(lane, "container" | "qemu" | "lume" | "conformance") {
        Some(LOCAL_RUNTIME.lock().await)
    } else {
        None
    };
    let t0 = Instant::now();
    let out = match tokio::time::timeout(timeout, body()).await {
        Ok(r) => r,
        Err(_) => Err(format!("timed out after {timeout:?}").into()),
    };
    let secs = t0.elapsed().as_secs_f64();
    match (out, xfail) {
        (Ok(()), None) => record(scenario, lane, test, "pass", secs, ""),
        (Ok(()), Some(x)) => {
            record(scenario, lane, test, "xpass", secs, x);
            panic!("xpass: {x} (the bug is fixed; drop the xfail)");
        }
        (Err(e), _) if e.is::<Skip>() => {
            eprintln!("skipping {scenario}/{lane}/{test}: {e}");
            record(
                scenario,
                lane,
                test,
                "skip",
                secs,
                &e.to_string()["skip: ".len()..],
            );
        }
        (Err(e), Some(x)) => {
            eprintln!("xfail {scenario}/{lane}/{test}: {e}");
            record(scenario, lane, test, "xfail", secs, x);
        }
        (Err(e), None) => {
            let msg = e.to_string();
            record(
                scenario,
                lane,
                test,
                "fail",
                secs,
                &msg.chars().take(400).collect::<String>(),
            );
            panic!("{scenario}/{lane}/{test}: {msg}");
        }
    }
}

// ---------------------------------------------------------------- SDK builders

pub fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cua-e2e-{}-{tag}-{}", run_id(), hex(3)));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn embedded_local() -> Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(tmp_dir("state").display().to_string()),
        fleet: None,
        fleet_from_env: false,
        env_probe_timeout_ms: None,
        spaces_home: Some(tmp_dir("spaces").display().to_string()),
        ..Default::default()
    })
    .unwrap()
}

pub fn embedded_fleet(fake: Option<(&str, &str)>) -> Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(tmp_dir("state").display().to_string()),
        fleet: fake.map(|(base, token)| FleetSettings {
            base_url: Some(base.into()),
            token_url: None,
            client_id: None,
            client_secret: None,
            token: Some(token.into()),
        }),
        fleet_from_env: fake.is_none(),
        env_probe_timeout_ms: None,
        spaces_home: Some(tmp_dir("spaces").display().to_string()),
        ..Default::default()
    })
    .unwrap()
}

pub fn opts(on: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        on: Some(on.into()),
        kind: None,
        runtime: None,
        image: String::new(),
        name: None,
        token: None,
        pool: None,
        os: None,
        cpus: None,
        memory_mb: None,
        ports: vec![],
        services: HashMap::new(),
        wait_for: vec![],
        ready_timeout_ms: None,
        env: HashMap::new(),
        fleet_replicas: None,
        fleet_ttl_seconds: None,
        warm: None,
        max_pool_size: None,
        command: None,
        cloud: None,
        sidecars: vec![],
        registry_secret: None,
        build: None,
        network: None,
        overlays: vec![],
        keep_on_failure: false,
        gpu: None,
    }
}

pub fn pool_spec(name: &str, image: &str) -> FleetPoolSpec {
    FleetPoolSpec {
        name: name.into(),
        image: image.into(),
        runtime: None,
        replicas: None,
        cpu: None,
        memory_mb: None,
        services: HashMap::new(),
        readiness_tcp_port: None,
        efi: false,
        command: None,
        ttl_seconds_after_created: None,
    }
}

pub fn probe(port: u16, http_path: Option<&str>) -> ReadinessProbe {
    ReadinessProbe {
        port,
        http_path: http_path.map(str::to_string),
        http_status: None,
        service: None,
    }
}

pub fn cmd(program: &str, args: &[&str]) -> SpacesdCommand {
    SpacesdCommand {
        program: program.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: HashMap::new(),
        cwd: None,
        user: None,
        timeout_ms: None,
        tag: None,
        stdin: false,
        pty: None,
    }
}

pub fn local_desktop_opts(name: &str, token: &str) -> SandboxCreateOptions {
    let mut o = opts("local");
    o.image = format!("container:{}", desktop_image());
    o.name = Some(name.into());
    o.token = Some(token.into());
    o.env = HashMap::from([("CUA_ENV_TOKEN".to_string(), token.to_string())]);
    o.cpus = Some(2);
    o.memory_mb = Some(2048);
    o.services = HashMap::from([("env".to_string(), 3211)]);
    o.wait_for = vec![probe(3211, Some("/viewer/"))];
    o.ready_timeout_ms = Some(300_000);
    o
}

// ---------------------------------------------------------------- polling

/// Awaits `f()` until it yields `Some` (bounded). `retry` decides which
/// errors are retried; others propagate at once.
pub async fn poll<T, F, Fut>(
    what: &str,
    attempts: u32,
    delay: Duration,
    retry: fn(&CuaError) -> bool,
    mut f: F,
) -> Res<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<Option<T>, CuaError>>,
{
    let mut last = String::new();
    for _ in 0..attempts {
        match f().await {
            Ok(Some(v)) => return Ok(v),
            Ok(None) => last = "not yet".into(),
            Err(e) if retry(&e) => last = e.to_string(),
            Err(e) => return Err(e.into()),
        }
        tokio::time::sleep(delay).await;
    }
    Err(format!("timed out waiting for {what}; last={last}").into())
}

pub fn env_retry(e: &CuaError) -> bool {
    matches!(
        e,
        CuaError::SpacesdNotAvailable(_) | CuaError::Transport(_) | CuaError::Timeout(_)
    )
}
pub fn no_retry(_: &CuaError) -> bool {
    false
}

pub async fn wait_env(sb: &Arc<cua::Sandbox>, attempts: u32) -> Res<Arc<SpacesdClient>> {
    poll(
        "spacesd",
        attempts,
        Duration::from_secs(1),
        env_retry,
        || async { sb.spacesd(Some(5000)).await.map(Some) },
    )
    .await
}

// ---------------------------------------------------------------- docker

pub fn docker(args: &[&str]) -> Res<String> {
    let out = Command::new("docker").args(args).output()?;
    if !out.status.success() {
        return Err(format!(
            "docker {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn has_runsc() -> bool {
    docker(&["info", "--format", "{{json .Runtimes}}"])
        .map(|s| s.contains("runsc"))
        .unwrap_or(false)
}

pub fn require_image(image: &str) -> Res {
    if docker(&["image", "inspect", image]).is_err() {
        return skip(format!(
            "image {image} is not present; build it with libs/images/build.sh"
        ));
    }
    Ok(())
}

/// A linux container started with plain `docker run` (the
/// direct-connect topology). Removed on drop.
pub struct DriverContainer {
    pub name: String,
    pub token: String,
    pub url: String,
}

impl DriverContainer {
    pub fn start(what: &str) -> Res<Self> {
        let image = desktop_image();
        require_image(&image)?;
        let name = name(what);
        let token = hex(16);
        let _ = docker(&["rm", "-f", &name]);
        let runtime = format!("--runtime={}", if has_runsc() { "runsc" } else { "runc" });
        let env = format!("CUA_ENV_TOKEN={token}");
        let label = format!("cua-e2e-run={}", run_id());
        docker(&[
            "run",
            "-d",
            "--name",
            &name,
            &runtime,
            "--memory=2g",
            "--memory-swap=2g",
            "--shm-size=512m",
            "--label",
            &label,
            "-e",
            &env,
            "-p",
            "127.0.0.1::3211",
            &image,
        ])?;
        // Constructed first so the container is removed if the port lookup fails.
        let mut c = DriverContainer {
            name: name.clone(),
            token,
            url: String::new(),
        };
        let port = docker(&["port", &name, "3211/tcp"])?;
        let port = port
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .ok_or("no port")?
            .trim()
            .to_string();
        c.url = format!("http://127.0.0.1:{port}");
        Ok(c)
    }
}

impl Drop for DriverContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

// ---------------------------------------------------------------- fixtures + daemon

pub struct Fixtures {
    pub env_url: String,
    pub env_token: String,
    pub fleet_base_url: String,
    pub fleet_token: String,
    child: std::process::Child,
}

impl Fixtures {
    pub fn start() -> Res<Self> {
        let bin = binary("CUA_TEST_FIXTURES", "cua-test-fixtures")
            .ok_or("cua-test-fixtures is not built")?;
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let mut line = String::new();
        let mut out = child.stdout.take().unwrap();
        let mut b = [0u8; 1];
        for _ in 0..4096 {
            if out.read(&mut b)? == 0 || b[0] == b'\n' {
                break;
            }
            line.push(b[0] as char);
        }
        let v: serde_json::Value = serde_json::from_str(&line)?;
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Ok(Fixtures {
            env_url: s("env_url"),
            env_token: s("env_token"),
            fleet_base_url: s("fleet_base_url"),
            fleet_token: s("fleet_token"),
            child,
        })
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Daemon {
    pub socket: PathBuf,
    home: PathBuf,
    child: std::process::Child,
}

impl Daemon {
    pub fn start() -> Res<Self> {
        let bin = binary("CUA_CLI", "cua").ok_or("the cua CLI is not built")?;
        // /tmp: macOS caps Unix socket paths at 104 bytes.
        let home = PathBuf::from(format!("/tmp/cua-e2e-{}-{}", run_id(), hex(3)));
        std::fs::create_dir_all(&home)?;
        let socket = home.join("cua.sock");
        let child = Command::new(bin)
            .args(["daemon", "start", "--foreground", "--socket"])
            .arg(&socket)
            .arg("--state-dir")
            .arg(home.join("sandboxes"))
            .env("CUA_HOME", &home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        for _ in 0..150 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !socket.exists() {
            return Err("daemon did not start".into());
        }
        Ok(Daemon {
            socket,
            home,
            child,
        })
    }

    pub fn client(&self) -> Arc<Cua> {
        Cua::connect(Some(self.socket.display().to_string()), None).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

// ---------------------------------------------------------------- env smoke

pub fn is_png(b: &[u8]) -> bool {
    b.starts_with(b"\x89PNG\r\n\x1a\n")
}

/// The spacesd smoke every topology runs; returns a normalized summary.
pub async fn env_smoke(env: &SpacesdClient, desktop: bool, mock: bool) -> Res<serde_json::Value> {
    let caps = env.capabilities().await?;
    assert!(!caps.version.is_empty());
    let out = env.run(cmd("echo", &["hi"])).await?;
    assert!(out.exit.success && out.stdout == b"hi\n", "{out:?}");
    let fail = env
        .sh(if mock { "fail 4" } else { "exit 4" }.into(), None)
        .await?;
    assert_eq!(fail.exit.code, Some(4));
    let mut c = cmd("cat", &[]);
    c.stdin = true;
    let p = env.spawn(c).await?;
    p.write_stdin(b"xyz".to_vec()).await?;
    p.close_stdin().await?;
    assert_eq!(p.wait().await?.stdout, b"xyz");
    let blob: Vec<u8> = (0..(1usize << 20)).map(|i| (i % 251) as u8).collect();
    let path = format!("/tmp/cua-e2e-{}/blob-rs.bin", run_id());
    let up = env.upload(path.clone(), blob.clone(), None).await?;
    assert_eq!(up.size, blob.len() as u64);
    assert!(env.download(path).await? == blob);
    assert!(matches!(
        env.download("/definitely/not/here".into()).await,
        Err(CuaError::NotFound(_))
    ));
    let health: serde_json::Value = serde_json::from_str(
        &env.call_json("SystemService/Health".into(), "{}".into())
            .await?,
    )?;
    let mut keys: Vec<String> = health
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    let mut summary = serde_json::json!({
        "echo": "hi\n", "exit4": 4, "blob_size": up.size, "health_keys": keys, "os_family": caps.os_family,
    });
    if desktop {
        env.set_clipboard("cua-e2e clipboard rs".into()).await?;
        assert_eq!(
            env.get_clipboard().await?.as_deref(),
            Some("cua-e2e clipboard rs")
        );
        let shot = env.screenshot(None).await?;
        assert!(is_png(&shot.image) && shot.width > 0);
        summary["screen"] = serde_json::json!([shot.width, shot.height]);
    }
    Ok(summary)
}

// ---------------------------------------------------------------- desktop checks

pub async fn sh_ok(env: &SpacesdClient, line: &str) -> Res<String> {
    let out = env.sh(line.into(), Some(60_000)).await?;
    if !out.exit.success {
        return Err(format!("{line}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub async fn fixture_log(env: &SpacesdClient, fixture: &str) -> Res<Vec<serde_json::Value>> {
    match env
        .download(format!("/tmp/cua-fixtures/{fixture}.jsonl"))
        .await
    {
        Ok(b) => Ok(String::from_utf8_lossy(&b)
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()),
        Err(CuaError::NotFound(_)) => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// Waits for the WM to manage the window (WM_STATE) and for a stable
/// origin (see ../python/e2e.py::window_origin).
pub async fn window_origin(env: &SpacesdClient, title: &str) -> Res<(f64, f64)> {
    let script = format!(
        "wid=$(xdotool search --name \"{title}\" | head -1); [ -n \"$wid\" ] || exit 3; \
         for i in $(seq 1 50); do xprop -id \"$wid\" WM_STATE 2>/dev/null | grep -q \"window state\" && break; sleep 0.1; done; \
         xdotool windowactivate --sync \"$wid\" >/dev/null 2>&1 || true; xdotool windowraise \"$wid\"; \
         o=; for i in $(seq 1 20); do sleep 0.3; \
         n=$(xwininfo -id \"$wid\" | awk \"/Absolute upper-left X/{{x=\\$4}} /Absolute upper-left Y/{{y=\\$4}} END{{print x, y}}\"); \
         [ \"$n\" = \"$o\" ] && break; o=$n; done; echo \"$n\""
    );
    for _ in 0..30 {
        let o = env
            .sh(format!("desktop-env bash -c '{script}'"), Some(20_000))
            .await?;
        if o.exit.success {
            let s = String::from_utf8_lossy(&o.stdout).to_string();
            let v: Vec<f64> = s
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            if v.len() == 2 {
                return Ok((v[0], v[1]));
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(format!("window {title} not found").into())
}

/// run-omarchy's desktop checks (see ../python/e2e.py::desktop_checks).
pub async fn desktop_checks(env: &SpacesdClient) -> Res {
    let shot = env.screenshot(None).await?;
    assert!(is_png(&shot.image));
    env.set_clipboard("hello from cua-e2e rs".into()).await?;
    assert_eq!(
        env.get_clipboard().await?.as_deref(),
        Some("hello from cua-e2e rs")
    );
    sh_ok(env, "cua-fixtures start grid").await?;
    let (gx, gy) = window_origin(env, "CUA Fixture Grid").await?;
    let req = serde_json::json!({"target": {"delivery": "DELIVERY_FOREGROUND"},
        "click": {"position": {"x": gx + 4.0 * 80.0 + 40.0, "y": gy + 40.0}}});
    env.pointer_json(req.to_string()).await?;
    let mut ok = false;
    for _ in 0..20 {
        if fixture_log(env, "grid")
            .await?
            .iter()
            .any(|e| e["type"] == "button_press" && e["cell"] == serde_json::json!([4, 0]))
        {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ok, "grid never saw the click in cell [4,0]");
    env.press("a".into()).await?;
    env.hotkey(vec!["ctrl".into(), "b".into()]).await?;
    let mut keys = false;
    for _ in 0..20 {
        let ev = fixture_log(env, "grid").await?;
        let a = ev
            .iter()
            .any(|e| e["type"] == "key_press" && e["key"] == "a");
        let b = ev.iter().any(|e| {
            e["type"] == "key_press" && e["key"] == "b" && e["mods"].to_string().contains("ctrl")
        });
        if a && b {
            keys = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(keys, "grid never saw a and ctrl+b");
    let _ = env.sh("cua-fixtures stop grid".into(), Some(10_000)).await;
    Ok(())
}

/// Minimal blocking HTTP/1.1 GET (for example the viewer page).
pub fn http_get(addr: &str, path: &str) -> Res<(u16, String)> {
    http_request(addr, "GET", path, &[], "")
}

/// Minimal blocking HTTP/1.1 POST (JSON) for `/mcp`.
pub fn http_post(
    addr: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Res<(u16, String)> {
    http_request(addr, "POST", path, headers, body)
}

fn http_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Res<(u16, String)> {
    let mut s = TcpStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    for _ in 0..1024 {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|x| x.1.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

pub fn mcp_initialize(addr: &str, token: &str) -> Res<serde_json::Value> {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                   "clientInfo": {"name": "cua-e2e-rs", "version": "0.1.0"}}});
    let auth = format!("Bearer {token}");
    let (status, text) = http_post(
        addr,
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("authorization", &auth),
        ],
        &body.to_string(),
    )?;
    if status != 200 {
        return Err(format!("mcp {status}: {text}").into());
    }
    let json = text
        .lines()
        .find_map(|l| l.strip_prefix("data:"))
        .map(str::to_string)
        .unwrap_or(text);
    let v: serde_json::Value = serde_json::from_str(json.trim())?;
    if v["result"]["serverInfo"].is_null() {
        return Err(format!("mcp: {v}").into());
    }
    Ok(v["result"].clone())
}

/// Reads up to `n` bytes (a service banner) with a timeout.
pub fn read_banner(addr: &str, n: usize) -> Vec<u8> {
    let Ok(mut s) = TcpStream::connect(addr) else {
        return vec![];
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut out = Vec::new();
    let mut b = vec![0u8; n];
    for _ in 0..16 {
        match s.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                out.extend_from_slice(&b[..k]);
                if out.len() >= n {
                    break;
                }
            }
        }
    }
    out
}

pub async fn banner_via(addr: &str, banner: &str) -> Res {
    for _ in 0..60 {
        let a = addr.to_string();
        let n = banner.len();
        let got = tokio::task::spawn_blocking(move || read_banner(&a, n)).await?;
        if got.starts_with(banner.as_bytes()) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(format!("no {banner} banner at {addr}").into())
}
