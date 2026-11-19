//! Hermetic `cua images release` run: the tiny fixture image
//! (tests/fixtures/release-images/tiny) through every phase against a local
//! registry (a `registry:2` container on a loopback port): build, gate,
//! stage, push, publish, verify and promote, then a `--resume` rerun that
//! skips everything and a `--from verify` rerun.
//!
//! Opt-in (it starts a container and builds an image):
//!
//! ```sh
//! CUA_E2E_IMAGES_RELEASE=1 cargo test -p cua-cli --test images_release -- --nocapture
//! ```
//!
//! Needs docker, crane, qemu-img, zstd and python3 on the host. The registry
//! container, the images and the work directory are removed afterwards.

use std::path::{Path, PathBuf};
use std::process::Command;

fn enabled() -> bool {
    std::env::var("CUA_E2E_IMAGES_RELEASE").as_deref() == Ok("1")
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

fn sh(cmd: &mut Command) -> String {
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "{cmd:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Registry {
    name: String,
    port: u16,
}

impl Registry {
    fn start() -> Self {
        let name = format!("cua-e2e-release-registry-{}", std::process::id());
        sh(Command::new("docker").args([
            "run",
            "-d",
            "--rm",
            "--name",
            &name,
            "-p",
            "127.0.0.1::5000",
            "registry:2",
        ]));
        let port = sh(Command::new("docker").args(["port", &name, "5000/tcp"]));
        let port: u16 = port
            .lines()
            .next()
            .and_then(|l| l.rsplit(':').next())
            .and_then(|p| p.parse().ok())
            .expect("registry port");
        // Wait until it answers (bounded).
        for _ in 0..50 {
            if Command::new("curl")
                .args(["-fsS", &format!("http://127.0.0.1:{port}/v2/")])
                .output()
                .is_ok_and(|o| o.status.success())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Registry { name, port }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

fn release(dir: &Path, work: &Path, repo: &str, extra: &[&str]) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cua"));
    c.args(["--json", "images", "release"])
        .arg(dir)
        .args(["--arch", host_arch(), "--work"])
        .arg(work)
        .args([
            "--var",
            &format!("repo={repo}"),
            "--stamp",
            "20260926-e2e0001",
        ])
        .args(extra)
        .env("CUA_INSECURE_REGISTRIES", repo.split('/').next().unwrap())
        .env("CUA_TELEMETRY", "0");
    let out = c.output().expect("cua");
    eprintln!("{}", String::from_utf8_lossy(&out.stderr));
    out
}

#[test]
fn tiny_image_releases_to_a_local_registry() {
    if !enabled() {
        eprintln!("skipped: set CUA_E2E_IMAGES_RELEASE=1");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/release-images/tiny");
    let work = tempfile::Builder::new()
        .prefix("cua-e2e-release-")
        .tempdir_in(
            std::env::var("CUA_E2E_WORK_DIR")
                .unwrap_or_else(|_| std::env::temp_dir().display().to_string()),
        )
        .unwrap();
    let reg = Registry::start();
    let repo = format!("localhost:{}/cua-e2e-release/tiny", reg.port);
    let arch = host_arch();

    // Plan only: nothing runs, every phase is listed.
    let out = release(
        &dir,
        work.path(),
        &repo,
        &["--dry-run", "--publish", "--promote"],
    );
    assert!(out.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["steps"].as_array().unwrap().len(), 8, "{plan:#}");

    // The whole pipeline.
    let out = release(&dir, work.path(), &repo, &["--publish", "--promote"]);
    assert!(out.status.success(), "release failed");
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let statuses: Vec<(String, String)> = summary["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_str().unwrap().to_string(),
                s["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(statuses.iter().all(|(_, s)| s == "passed"), "{statuses:?}");
    let evidence = work.path().join("evidence");
    for f in [
        "pins.json".to_string(),
        format!("pushed-{arch}/pushed.json"),
        format!("artifacts-{arch}.sha256"),
        format!("logs/build__{arch}.log"),
        format!("summary-{arch}.md"),
    ] {
        assert!(evidence.join(&f).exists(), "missing evidence {f}");
    }
    let pins: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(evidence.join("pins.json")).unwrap())
            .unwrap();
    // The floating tags point at the pins.
    let digest = |r: &str| sh(Command::new("crane").args(["digest", r]));
    assert_eq!(
        digest(&format!("{repo}:1")),
        pins["primary"]["digest"].as_str().unwrap()
    );
    assert_eq!(
        digest(&format!("{repo}:1-disk")),
        pins["containerdisk"]["digest"].as_str().unwrap()
    );
    assert_eq!(
        digest(&format!("{repo}:1-20260926-e2e0001")),
        pins["primary"]["digest"].as_str().unwrap()
    );

    // --resume: every step already passed with the same inputs.
    let out = release(
        &dir,
        work.path(),
        &repo,
        &["--publish", "--promote", "--resume"],
    );
    assert!(out.status.success());
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        summary["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["status"] == "done"),
        "{summary:#}"
    );

    // --from verify reruns verify and promote only (promote is idempotent).
    let out = release(
        &dir,
        work.path(),
        &repo,
        &["--publish", "--promote", "--from", "verify"],
    );
    assert!(out.status.success());
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let ran: Vec<&str> = summary["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["status"] == "passed")
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ran, ["verify", "promote"]);

    // A second release of the same stamp refuses to overwrite the pins.
    let out = release(
        &dir,
        work.path(),
        &repo,
        &["--publish", "--steps", "publish"],
    );
    assert!(
        !out.status.success(),
        "republishing an existing pin must fail"
    );

    let _ = Command::new("docker")
        .args([
            "image",
            "rm",
            "-f",
            &format!("{repo}:docker-build-20260926-e2e0001-{arch}"),
        ])
        .output();
}
