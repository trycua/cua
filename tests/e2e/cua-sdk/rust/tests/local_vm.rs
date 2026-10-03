//! local-qemu / local-lume: the reference image (linux disk,
//! built locally by libs/images/build.sh --outputs disk) booted by cua-vmm,
//! then driven through the cua SDK by URL + token: env smoke -> marker ->
//! stop -> checkpoint -> fork -> boot the fork -> marker survived.
//!
//! These lanes pass their own token through cloud-init `write_files`
//! (/etc/cua/env-token) and use cua-vmm for lifecycle and the SDK for
//! everything in the guest. `local_qemu_canonical_disk_gets_an_env_token`
//! goes through `Sandboxes.create(local)` instead, where the SDK mints and
//! delivers the token itself. Limits: one VM at a time, <= 4 GiB.

use std::sync::Arc;
use std::time::Duration;

use cua_sdk_e2e::*;
use cua_vmm::lume::{LumeConfig, LumeRuntime};
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{ImageSource, Probe, Runtime, StartSpec, Status};

const MIN: Duration = Duration::from_secs(60);

/// At most one VM at a time per agent/runner: the QEMU and Lume tests in this
/// binary take this lock (cargo runs tests on several threads).
static ONE_VM: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn desktop_disk() -> std::path::PathBuf {
    // CUA_E2E_DISK_CUA_DESKTOP_LINUX: the pre-rename name, read for one release.
    std::env::var("CUA_E2E_DISK_LINUX")
        .or_else(|_| std::env::var("CUA_E2E_DISK_CUA_DESKTOP_LINUX"))
        .map(Into::into)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap())
                .join(".cache/cua-images-e2e/linux")
                .join(host_arch())
                .join("disk.img")
        })
}

fn user_data(token: &str) -> String {
    format!(
        "#cloud-config\nwrite_files:\n  - path: /etc/cua/env-token\n    permissions: '0640'\n    content: '{token}'\n"
    )
}

async fn smoke_by_url(url: &str, token: &str, marker: Option<&str>) -> Res<String> {
    let c = embedded_local();
    let sb = c
        .sandboxes()
        .connect_url(url.into(), Some(token.into()), Some(name("vm-direct")))
        .await?;
    let r = async {
        let env = wait_env(&sb, 300).await?;
        env_smoke(&env, true, false).await?;
        let path = "/var/tmp/cua-e2e-marker".to_string();
        match marker {
            Some(m) => {
                env.upload(path, m.as_bytes().to_vec(), None).await?;
                let _ = env.sh("sync".into(), Some(10_000)).await;
                Res::Ok(m.to_string())
            }
            None => Ok(String::from_utf8(env.download(path).await?)?),
        }
    }
    .await;
    sb.delete().await?;
    r
}

async fn boot_smoke_fork_checkpoint(rt: Arc<dyn Runtime>, what: &str, via_ip: bool) -> Res {
    let _one_vm = ONE_VM.lock().await;
    let disk = desktop_disk();
    if !disk.exists() {
        return skip(format!(
            "missing {}; build it with libs/images/build.sh linux --tag e2e --outputs rootfs,disk --out ~/.cache/cua-images-e2e",
            disk.display()
        ));
    }
    let name = name(what);
    let ck = format!("{name}-ck");
    let fork = format!("{name}-fork");
    let token = hex(16);
    let url_of = |inst: &cua_vmm::Instance| -> Res<String> {
        let addr = if via_ip {
            format!("{}:3211", inst.endpoints.host)
        } else {
            inst.endpoints.addr(3211).ok_or("3211 not published")?
        };
        Ok(format!("http://{addr}"))
    };
    let mut spec = StartSpec::new(&name, ImageSource::disk(&disk))
        .cpus(2)
        .memory_mb(3072)
        .port(3211)
        .probe(Probe::tcp(3211))
        .ready_timeout(Duration::from_secs(600));
    spec.cloud_init_user_data = Some(user_data(&token));
    let marker = hex(8);
    let r = async {
        let inst = rt.start(&spec).await?;
        let url = url_of(&inst)?;
        eprintln!("{what}: booted, env at {url}");
        smoke_by_url(&url, &token, Some(&marker)).await?;
        rt.stop(&name).await?;
        assert_eq!(rt.status(&name).await?, Status::Stopped);
        rt.checkpoint(&name, &ck).await?;
        rt.fork(&ck, &fork).await?;
        // One VM at a time: the original is stopped before the fork boots.
        let fspec = StartSpec::new(&fork, ImageSource::Existing)
            .cpus(2)
            .memory_mb(3072)
            .port(3211)
            .probe(Probe::tcp(3211))
            .ready_timeout(Duration::from_secs(600));
        let finst = rt.start(&fspec).await?;
        let got = smoke_by_url(&url_of(&finst)?, &token, None).await?;
        assert_eq!(got, marker, "the fork must carry the original's disk state");
        rt.stop(&fork).await?;
        Res::Ok(())
    }
    .await;
    for n in [&fork, &ck, &name] {
        let _ = rt.stop(n).await;
        let _ = rt.delete(n).await;
    }
    r
}

#[tokio::test]
async fn local_qemu_reference_image() {
    e2e(
        "local-qemu",
        "qemu",
        "reference disk: boot -> env smoke -> checkpoint -> fork -> boot fork",
        40 * MIN,
        None,
        || async {
            let root = tmp_dir("qemu");
            let rt = QemuRuntime::new(QemuConfig {
                root,
                graceful_stop: Duration::from_secs(30),
                ..Default::default()
            });
            boot_smoke_fork_checkpoint(Arc::new(rt), "qemu", false).await
        },
    )
    .await;
}

#[tokio::test]
async fn local_lume_reference_image() {
    e2e(
        "local-lume",
        "lume",
        "reference disk: boot -> env smoke -> checkpoint -> fork -> boot fork",
        40 * MIN,
        None,
        || async {
            if !cfg!(target_os = "macos") {
                return skip("Lume runs on macOS hosts only");
            }
            let root = tmp_dir("lume");
            let rt = LumeRuntime::new(LumeConfig {
                root,
                ..Default::default()
            });
            rt.ensure_serving().await?;
            boot_smoke_fork_checkpoint(Arc::new(rt), "lume", true).await
        },
    )
    .await;
}

/// The canonical containerDisk through `Sandboxes.create(local)` with no
/// token and no manual steps: the SDK mints a per-sandbox token, cua-vmm
/// delivers it through the NoCloud seed as the Fleet claim-token contract
/// (root 0600 `/run/cua/env-token` on a private tmpfs, spacesd in
/// await-token-file mode, token-sync mirroring it for the driver's user),
/// and `sb.spacesd()` runs a command and takes a screenshot.
/// `CUA_E2E_QEMU_CANONICAL_IMAGE` overrides the image.
#[tokio::test]
async fn local_qemu_canonical_disk_gets_an_env_token() {
    e2e(
        "local-qemu",
        "qemu",
        "canonical linux:24.04-disk: minted env token -> env command + screenshot",
        30 * MIN,
        None,
        || async {
            let _one_vm = ONE_VM.lock().await;
            let image = std::env::var("CUA_E2E_QEMU_CANONICAL_IMAGE")
                .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-disk".into());
            let state = tmp_dir("state-canonical");
            let c = cua::Cua::embedded(cua::CuaConfig {
                state_dir: Some(state.display().to_string()),
                fleet: None,
                fleet_from_env: false,
                env_probe_timeout_ms: None,
                spaces_home: Some(tmp_dir("spaces").display().to_string()),
                ..Default::default()
            })?;
            let name = name("qemu-canon");
            let mut o = opts("local");
            o.image = image.clone();
            o.name = Some(name.clone());
            o.cpus = Some(2);
            o.memory_mb = Some(3072);
            o.ready_timeout_ms = Some(900_000);
            let sb = c.sandboxes().create(o).await?;
            let r = async {
                assert_eq!(sb.runtime_type(), "qemu", "{image} runs on QEMU");
                // The token is kept with the record, owner-only.
                let record = state.join(format!("{name}.json"));
                let raw: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&record)?)?;
                let token = raw["env_token"]
                    .as_str()
                    .ok_or("no env_token in the record")?;
                assert!(token.len() >= 32);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    let mode = std::fs::metadata(&record)?.permissions().mode() & 0o777;
                    assert_eq!(mode, 0o600, "record mode {mode:o}");
                }
                let env = wait_env(&sb, 600).await?;
                let summary = env_smoke(&env, true, false).await?;
                eprintln!("canonical disk env smoke: {summary}");
                // The Fleet contract inside the guest: a private mount at
                // /run/cua and the driver user's 0600 copy.
                let mount = sh_ok(&env, "findmnt -no FSTYPE,OPTIONS /run/cua").await?;
                assert!(mount.starts_with("tmpfs"), "/run/cua: {mount}");
                let copy = sh_ok(&env, "stat -c '%U %a' /run/cua-env/env-token").await?;
                assert_eq!(copy.trim(), "cua 600", "synced token file");
                let who = sh_ok(&env, "id -un").await?;
                assert_eq!(who.trim(), "cua", "spacesd's user");
                // Nothing but root reads the token source.
                let denied = env
                    .sh("cat /run/cua/env-token".into(), Some(10_000))
                    .await?;
                assert!(
                    !denied.exit.success,
                    "the driver user must not read /run/cua/env-token"
                );
                let envfile = env
                    .sh(
                        "grep -c CUA_ENV_TOKEN /etc/cua/spacesd.env".into(),
                        Some(10_000),
                    )
                    .await?;
                assert!(!envfile.exit.success, "no token in the env file");
                // A second handle (another process) authenticates from the record.
                let again = c.sandboxes().connect(name.clone()).await?;
                let env2 = wait_env(&again, 30).await?;
                let out = env2.run(cmd("echo", &["again"])).await?;
                assert_eq!(out.stdout, b"again\n");
                Res::Ok(())
            }
            .await;
            sb.delete().await?;
            r
        },
    )
    .await;
}
