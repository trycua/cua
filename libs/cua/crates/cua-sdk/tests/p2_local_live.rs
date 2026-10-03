//! Opt-in local container lane for sandbox parity (`CUA_TEST_UNIFIED_LOCAL=1`,
//! a Docker-API engine such as colima; never on by default):
//!
//! - a `python:3.12-slim` sandbox with a `redis:7-alpine` sidecar reaches it
//!   on `localhost:6379`, and `services={"db": 6379}` reaches it from here;
//! - a private image in a throwaway `registry:2` with htpasswd pulls with a
//!   `RegistrySecret` and is refused without one.
//!
//! Everything is named `cua-e2e-p2-*`, memory-capped, and removed in the
//! test. The docker CLI uses a temp `DOCKER_CONFIG` (the user's is never
//! read or written).

use cua_sdk::{Container, Cua, CuaConfig, ReadinessProbe, RegistrySecret, SandboxCreateOptions};
use futures_util::FutureExt;
use std::{
    collections::HashMap, panic::AssertUnwindSafe, process::Command, sync::Arc, time::Duration,
};

fn enabled() -> bool {
    std::env::var("CUA_TEST_UNIFIED_LOCAL").as_deref() == Ok("1")
}

fn suffix() -> String {
    format!(
        "{:08x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    )
}

fn cua(dirs: &tempfile::TempDir) -> Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dirs.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dirs.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dirs.path().join("pools").display().to_string()),
        fleet_from_env: false,
        ..Default::default()
    })
    .unwrap()
}

/// The engine the docker CLI talks to (its current context), so the CLI can
/// run with a throwaway `DOCKER_CONFIG` and still reach it.
fn docker_host() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        if let Ok(h) = std::env::var("DOCKER_HOST") {
            return h;
        }
        let out = Command::new("docker")
            .args([
                "context",
                "inspect",
                "--format",
                "{{.Endpoints.docker.Host}}",
            ])
            .output()
            .expect("docker CLI");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    })
    .clone()
}

/// `docker <args>` with a throwaway client config; panics on failure.
fn docker(config: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("docker")
        .env("DOCKER_HOST", docker_host())
        .env("DOCKER_CONFIG", config)
        .args(args)
        .output()
        .expect("docker CLI");
    assert!(
        out.status.success(),
        "docker {}: {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn docker_quiet(config: &std::path::Path, args: &[&str]) {
    let _ = Command::new("docker")
        .env("DOCKER_HOST", docker_host())
        .env("DOCKER_CONFIG", config)
        .args(args)
        .output();
}

const PROBE: &str = r#"
import socket, time, http.server
def ping():
    for _ in range(120):
        try:
            s = socket.create_connection(("127.0.0.1", 6379), 1)
            s.sendall(b"PING\r\n")
            r = s.recv(64)
            s.close()
            return r
        except OSError:
            time.sleep(0.5)
    return b"unreachable"
reply = ping()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(reply)
    def log_message(self, *a):
        pass
http.server.HTTPServer(("0.0.0.0", 8000), H).serve_forever()
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_shares_the_sandbox_localhost() {
    if !enabled() {
        eprintln!("skipped: set CUA_TEST_UNIFIED_LOCAL=1 (starts containers)");
        return;
    }
    let dirs = tempfile::tempdir().unwrap();
    let cua = cua(&dirs);
    let name = format!("cua-e2e-p2-sc-{}", suffix());
    let mut o = SandboxCreateOptions::new("local", "python:3.12-slim");
    o.name = Some(name.clone());
    o.cpus = Some(1);
    o.memory_mb = Some(256);
    o.command = Some(vec!["python".into(), "-c".into(), PROBE.into()]);
    o.services = HashMap::from([("web".to_string(), 8000), ("db".to_string(), 6379)]);
    o.wait_for = vec![ReadinessProbe::http("web", "/")];
    o.ready_timeout_ms = Some(240_000);
    o.sidecars = vec![Container {
        image: "redis:7-alpine".into(),
        command: None,
        env: HashMap::new(),
        ports: vec![6379],
        name: None,
    }];
    // Without an explicit runc, an engine with gVisor refuses the group
    // before anything is created (no silent downgrade).
    let refused = cua.sandboxes().create(o.clone()).await;
    let cfg0 = dirs.path().join("docker0");
    std::fs::create_dir_all(&cfg0).unwrap();
    let has_runsc = docker(&cfg0, &["info", "--format", "{{json .Runtimes}}"]).contains("runsc");
    if has_runsc {
        let err = refused.err().expect("sidecars on gVisor must be refused");
        assert!(
            matches!(err, cua_sdk::CuaError::Unsupported(_))
                && err.to_string().contains("runtime='runc'"),
            "{err}"
        );
        let left = docker(
            &cfg0,
            &[
                "ps",
                "-a",
                "--filter",
                &format!("name={name}"),
                "--format",
                "{{.Names}}",
            ],
        );
        assert!(left.is_empty(), "refused but created: {left}");
    } else if let Ok(sb) = refused {
        let _ = sb.delete().await;
    }
    o.runtime = Some("runc".into());
    let sb = cua.sandboxes().create(o).await;
    let result = AssertUnwindSafe(async {
        let sb = sb.map_err(|e| format!("create: {e}"))?;
        let r = sb
            .service("web".into())
            .map_err(|e| e.to_string())?
            .request("GET".into(), "/".into(), None, Some(30_000), None)
            .await
            .map_err(|e| e.to_string())?;
        let body = String::from_utf8_lossy(&r.body).to_string();
        if body.trim() != "+PONG" {
            return Err(format!("sandbox -> localhost:6379 answered {body:?}"));
        }
        // The sidecar's port is a service reachable from this machine too.
        let url = sb
            .service("db".into())
            .map_err(|e| e.to_string())?
            .url()
            .await
            .map_err(|e| e.to_string())?;
        let addr = url.trim_start_matches("http://").trim_end_matches('/');
        let mut s = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| format!("db service {addr}: {e}"))?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        s.write_all(b"PING\r\n").await.map_err(|e| e.to_string())?;
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .map_err(|_| "db service: no reply".to_string())?
            .map_err(|e| e.to_string())?;
        if &buf[..n] != b"+PONG\r\n" {
            return Err(format!("db service answered {:?}", &buf[..n]));
        }
        Ok::<_, String>(())
    })
    .catch_unwind()
    .await;
    // Removes the sandbox and its sidecar (the backend deletes the group).
    let _ = cua.sandboxes().delete(name.clone()).await;
    let cfg = dirs.path().join("docker");
    std::fs::create_dir_all(&cfg).unwrap();
    let left = docker(
        &cfg,
        &[
            "ps",
            "-a",
            "--filter",
            &format!("name={name}"),
            "--format",
            "{{.Names}}",
        ],
    );
    assert!(left.is_empty(), "containers left behind: {left}");
    match result {
        Ok(r) => r.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_registry_pull_needs_the_registry_secret() {
    if !enabled() {
        eprintln!("skipped: set CUA_TEST_UNIFIED_LOCAL=1 (starts containers)");
        return;
    }
    let dirs = tempfile::tempdir().unwrap();
    let cfg = dirs.path().join("docker");
    let auth = dirs.path().join("auth");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&auth).unwrap();
    let id = suffix();
    let registry = format!("cua-e2e-p2-reg-{id}");
    let (user, pass) = ("cua-e2e", format!("pw-{id}"));
    // bcrypt htpasswd from the httpd image's htpasswd (registry:2 has none).
    let line = docker(
        &cfg,
        &[
            "run",
            "--rm",
            "--memory=64m",
            "--entrypoint",
            "htpasswd",
            "httpd:2-alpine",
            "-Bbn",
            user,
            &pass,
        ],
    );
    std::fs::write(auth.join("htpasswd"), format!("{line}\n")).unwrap();
    // A bind mount of a macOS temp dir is not visible inside colima's VM, so
    // the file is copied into the created container instead.
    docker(
        &cfg,
        &[
            "create",
            "--name",
            &registry,
            "--memory=256m",
            "-p",
            "127.0.0.1::5000",
            "-e",
            "REGISTRY_AUTH=htpasswd",
            "-e",
            "REGISTRY_AUTH_HTPASSWD_REALM=cua-e2e",
            "-e",
            "REGISTRY_AUTH_HTPASSWD_PATH=/auth/htpasswd",
            "registry:2",
        ],
    );
    // Panics (a failing docker step) still reach the cleanup below.
    let result = AssertUnwindSafe(async {
        docker(
            &cfg,
            &["cp", auth.to_str().unwrap(), &format!("{registry}:/auth")],
        );
        docker(&cfg, &["start", &registry]);
        let port = docker(&cfg, &["port", &registry, "5000/tcp"]);
        let port = port.rsplit(':').next().unwrap().trim().to_string();
        let host = format!("localhost:{port}");
        // SAFETY: tests in this file run the registry case alone; the
        // resolver reads plain-HTTP registries from this variable.
        unsafe { std::env::set_var("CUA_INSECURE_REGISTRIES", &host) };
        let image = format!("{host}/private/busybox:1");
        // Publish a small image with a login kept in the temp config.
        let login = Command::new("docker")
            .env("DOCKER_HOST", docker_host())
            .env("DOCKER_CONFIG", &cfg)
            .args(["login", &host, "-u", user, "--password-stdin"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(pass.as_bytes())?;
                c.wait()
            })
            .map_err(|e| e.to_string())?;
        if !login.success() {
            return Err("docker login to the throwaway registry failed".to_string());
        }
        docker(&cfg, &["pull", "-q", "busybox:1"]);
        docker(&cfg, &["tag", "busybox:1", &image]);
        docker(&cfg, &["push", "-q", &image]);
        // Nothing of it stays in the engine: the sandbox must pull.
        docker_quiet(&cfg, &["rmi", &image]);
        docker_quiet(&cfg, &["logout", &host]);

        let cua = cua(&dirs);
        let opts = |name: &str| {
            let mut o = SandboxCreateOptions::new("local", format!("container:{image}"));
            o.name = Some(name.into());
            o.cpus = Some(1);
            o.memory_mb = Some(64);
            o.command = Some(vec!["sleep".into(), "300".into()]);
            o.ready_timeout_ms = Some(120_000);
            o
        };
        let denied = format!("cua-e2e-p2-noauth-{id}");
        let err = cua.sandboxes().create(opts(&denied)).await;
        let _ = cua.sandboxes().delete(denied).await;
        if err.is_ok() {
            return Err("pulled a private image without credentials".into());
        }
        let ok = format!("cua-e2e-p2-auth-{id}");
        let mut o = opts(&ok);
        o.registry_secret = Some(RegistrySecret::Basic {
            username: user.into(),
            password: pass.clone(),
            registry: None,
        });
        let created = cua.sandboxes().create(o).await;
        let _ = cua.sandboxes().delete(ok).await;
        let sb = created.map_err(|e| format!("create with the secret: {e}"))?;
        drop(sb);
        // The sandbox pulled by digest (untagged): remove by repository.
        let ids = docker(
            &cfg,
            &[
                "images",
                "-q",
                "--filter",
                &format!("reference={host}/private/*"),
            ],
        );
        for id in ids.split_whitespace() {
            docker_quiet(&cfg, &["rmi", "-f", id]);
        }
        Ok::<_, String>(())
    })
    .catch_unwind()
    .await;
    docker_quiet(&cfg, &["rm", "-f", &registry]);
    match result {
        Ok(r) => r.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
