//! direct-connect, daemon-agnostic and daemon-vs-embedded (Rust). Scenario
//! definitions: ../python/test_{direct_connect,daemon_agnostic,daemon_vs_embedded}.py.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cua_sdk_e2e::cua::{Cua, CuaMode, SandboxStatus};
use cua_sdk_e2e::*;

const MIN: Duration = Duration::from_secs(60);

async fn direct(c: &Arc<Cua>, url: &str, token: &str, mock: bool) -> Res<serde_json::Value> {
    let sbx = c.sandboxes();
    let sb = sbx
        .connect_url(url.into(), Some(token.into()), Some(name("direct")))
        .await?;
    let res = async {
        assert_eq!(sb.location(), "direct");
        let env = wait_env(&sb, 90).await?;
        let summary = env_smoke(&env, true, mock).await?;
        let bad = sbx
            .connect_url(
                url.into(),
                Some("wrong-token".into()),
                Some(name("direct-bad")),
            )
            .await?;
        assert!(matches!(
            bad.spacesd(Some(5000)).await,
            Err(CuaError::Unauthenticated(_))
        ));
        bad.delete().await?;
        Res::Ok(summary)
    }
    .await;
    sb.delete().await?;
    res
}

#[tokio::test]
async fn direct_connect_mock() {
    e2e(
        "direct-connect",
        "hermetic",
        "MockServer by URL + token",
        MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            direct(&embedded_local(), &fx.env_url, &fx.env_token, true).await?;
            Ok(())
        },
    )
    .await;
}

#[tokio::test]
async fn direct_connect_docker() {
    e2e(
        "direct-connect",
        "container",
        "spacesd in docker by URL + token",
        5 * MIN,
        None,
        || async {
            let d = DriverContainer::start("direct")?;
            let s = direct(&embedded_local(), &d.url, &d.token, false).await?;
            assert_eq!(s["os_family"], "linux");
            Ok(())
        },
    )
    .await;
}

/// The spacesd's own conformance suite (libs/cua-spacesd) against the
/// same docker target: `cargo test --test conformance` with
/// CUA_ENV_TEST_TARGET/TOKEN. Heavy (builds the driver workspace).
#[tokio::test]
async fn direct_connect_conformance_suite() {
    e2e(
        "direct-connect",
        "conformance",
        "spacesd conformance suite over the docker target",
        60 * MIN,
        None,
        || async {
            let d = DriverContainer::start("conformance")?;
            let _ = wait_env(
                &embedded_local()
                    .sandboxes()
                    .connect_url(d.url.clone(), Some(d.token.clone()), None)
                    .await?,
                90,
            )
            .await?;
            let dir = repo().join("libs/cua-spacesd");
            let status = tokio::process::Command::new("timeout")
                .args([
                    "2700",
                    "cargo",
                    "test",
                    "-p",
                    "cua-spacesd-server",
                    "--test",
                    "conformance",
                    "--",
                    "--test-threads=4",
                ])
                .current_dir(&dir)
                .env("CUA_ENV_TEST_TARGET", &d.url)
                .env("CUA_ENV_TEST_TOKEN", &d.token)
                .env("CUA_ENV_TEST_BIG_BYTES", (64u64 << 20).to_string())
                .env("CARGO_BUILD_JOBS", "4")
                .status()
                .await?;
            assert!(status.success(), "conformance suite failed: {status}");
            Ok(())
        },
    )
    .await;
}

// ------------------------------------------------------------ daemon-agnostic

async fn plain_local(
    c: &Arc<Cua>,
    image: &str,
    port: u16,
    banner: &str,
    what: &str,
    memory_mb: u64,
) -> Res {
    let sbx = c.sandboxes();
    let nm = name(what);
    let mut o = opts("local");
    o.image = image.into();
    o.name = Some(nm.clone());
    o.cpus = Some(1);
    o.memory_mb = Some(memory_mb);
    o.ports = vec![port];
    o.services = HashMap::from([("plain".to_string(), port)]);
    o.wait_for = vec![probe(port, None)];
    o.ready_timeout_ms = Some(300_000);
    let sb = sbx.create(o).await?;
    let res = async {
        assert_eq!(sb.refresh().await?.status, SandboxStatus::Running);
        assert!(
            sbx.list(Some("local".into()))
                .await?
                .iter()
                .any(|s| s.name == nm)
        );
        let fwd = sb.forward(port).await?;
        let r = banner_via(&fwd.local_addr().unwrap(), banner).await;
        fwd.close().await?;
        r?;
        sb.wait_ready(vec![probe(port, None)], 30_000).await?;
        assert!(
            sb.wait_ready(vec![probe(3999, Some("/"))], 3_000)
                .await
                .is_err()
        );
        assert!(matches!(
            sb.spacesd(Some(3000)).await,
            Err(CuaError::SpacesdNotAvailable(_))
        ));
        Res::Ok(())
    }
    .await;
    sb.delete().await?;
    assert!(
        !sbx.list(Some("local".into()))
            .await?
            .iter()
            .any(|s| s.name == nm)
    );
    res
}

#[tokio::test]
async fn agnostic_fake_fleet_and_direct() {
    e2e(
        "daemon-agnostic",
        "hermetic",
        "fake Fleet pool without env + direct URL without a driver",
        MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            let c = embedded_fleet(Some((&fx.fleet_base_url, &fx.fleet_token)));
            let fleet = c.fleet()?;
            let pool = name("agnostic");
            let mut spec = pool_spec(&pool, "img:plain");
            spec.services = HashMap::from([("server".to_string(), 8000)]);
            fleet.apply_pool(spec).await?;
            let mut o = opts("cloud");
            o.pool = Some(pool.clone());
            o.name = Some(format!("{pool}-c"));
            let sb = c.sandboxes().create(o).await?;
            let r = async {
                assert_eq!(
                    sb.service("server".into())?
                        .request("GET".into(), "/status".into(), None, Some(5000), None)
                        .await?
                        .status,
                    200
                );
                assert!(matches!(
                    sb.spacesd(Some(2000)).await,
                    Err(CuaError::SpacesdNotAvailable(_))
                ));
                Res::Ok(())
            }
            .await;
            sb.delete().await?;
            fleet.delete_pool(pool).await?;
            r?;
            let d = c
                .sandboxes()
                .connect_url(
                    fx.fleet_base_url.clone(),
                    Some("t".into()),
                    Some(name("nodriver")),
                )
                .await?;
            assert!(matches!(
                d.spacesd(Some(2000)).await,
                Err(CuaError::SpacesdNotAvailable(_))
            ));
            d.delete().await?;
            Ok(())
        },
    )
    .await;
}

#[tokio::test]
async fn agnostic_container_ssh_only() {
    e2e(
        "daemon-agnostic",
        "container",
        "plain ubuntu-server (sshd only)",
        10 * MIN,
        None,
        || async {
            let image = plain_image("ubuntu-server");
            require_image(&image)?;
            plain_local(
                &embedded_local(),
                &format!("container:{image}"),
                22,
                "SSH-2.0",
                "plain-ssh",
                512,
            )
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn agnostic_container_vnc_only() {
    e2e(
        "daemon-agnostic",
        "container",
        "plain ubuntu-xfce-vnc (Xvnc only)",
        10 * MIN,
        None,
        || async {
            let image = plain_image("ubuntu-xfce-vnc");
            require_image(&image)?;
            plain_local(
                &embedded_local(),
                &format!("container:{image}"),
                5901,
                "RFB 003",
                "plain-vnc",
                1024,
            )
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn agnostic_qemu_ssh_only() {
    e2e(
        "daemon-agnostic",
        "qemu",
        "plain ubuntu-server disk under QEMU",
        15 * MIN,
        None,
        || async {
            let disk = disk_path("ubuntu-server");
            if !disk.exists() {
                return skip(format!("missing {}", disk.display()));
            }
            plain_local(
                &embedded_local(),
                &format!("vm:{}", disk.display()),
                22,
                "SSH-2.0",
                "qemu-ssh",
                1024,
            )
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn agnostic_fleet_legacy_image() {
    e2e(
        "daemon-agnostic",
        "fleet",
        "legacy computer-server image on Fleet (no spacesd)",
        25 * MIN,
        None,
        || async {
            let c = embedded_fleet(None);
            let fleet = c.fleet()?;
            let pool = name("agnostic");
            let mut spec = pool_spec(&pool, LEGACY_FLEET_ROOTFS);
            spec.runtime = Some("gvisor".into());
            spec.services = HashMap::from([("server".to_string(), 8000)]);
            spec.cpu = Some(1);
            spec.memory_mb = Some(2048);
            spec.ttl_seconds_after_created = Some(3600);
            fleet.apply_pool(spec).await?;
            let r = async {
                let mut o = opts("cloud");
                o.pool = Some(pool.clone());
                o.name = Some(format!("{pool}-c"));
                o.ready_timeout_ms = Some(900_000);
                let sb = c.sandboxes().create(o).await?;
                let r = async {
                    let svc = sb.service("server".into())?;
                    poll(
                        "server /status",
                        60,
                        Duration::from_secs(5),
                        |_| true,
                        || async {
                            Ok(svc
                                .request("GET".into(), "/status".into(), None, Some(30_000), None)
                                .await?
                                .status
                                .eq(&200)
                                .then_some(()))
                        },
                    )
                    .await?;
                    assert!(matches!(
                        sb.spacesd(Some(10_000)).await,
                        Err(CuaError::SpacesdNotAvailable(_))
                    ));
                    Res::Ok(())
                }
                .await;
                sb.delete().await?;
                r
            }
            .await;
            let _ = fleet.delete_pool(pool).await;
            r
        },
    )
    .await;
}

// ------------------------------------------------------------ daemon-vs-embedded

async fn script(c: &Arc<Cua>, target: &Target, suffix: &str) -> Res<serde_json::Value> {
    let sbx = c.sandboxes();
    let nm = name(&format!("dve-{}-{suffix}", target.lane()));
    let sb = match target {
        Target::Direct { url, token } => {
            sbx.connect_url(url.clone(), Some(token.clone()), Some(nm.clone()))
                .await?
        }
        Target::Local { token } => {
            let mut o = local_desktop_opts(&nm, token);
            o.services = HashMap::new();
            o.wait_for = vec![probe(3211, None)];
            sbx.create(o).await?
        }
    };
    let res = async {
        let env = wait_env(&sb, 120).await?;
        let mut s = env_smoke(&env, true, matches!(target, Target::Direct { .. })).await?;
        let info = sbx.get(nm.clone()).await?;
        s["sandbox"] = serde_json::json!([
            info.location,
            info.runtime_type,
            info.ephemeral,
            format!("{:?}", info.status)
        ]);
        s["listed"] = sbx.list(None).await?.iter().any(|x| x.name == nm).into();
        let mut services: Vec<_> = sb.services().into_keys().collect();
        services.sort();
        s["services"] = services.into();
        Res::Ok(s)
    }
    .await;
    sb.delete().await?;
    res
}

enum Target {
    Direct { url: String, token: String },
    Local { token: String },
}

impl Target {
    /// Part of every sandbox name, so the mock and container lanes, which
    /// run at once in this binary, never create two sandboxes with one name
    /// (the embedded client sees both local containers and direct Spaces).
    fn lane(&self) -> &'static str {
        match self {
            Target::Direct { .. } => "mock",
            Target::Local { .. } => "ctr",
        }
    }
}

async fn compare(target: Target) -> Res {
    let embedded = script(&embedded_local(), &target, "emb").await?;
    let d = Daemon::start()?;
    let c = d.client();
    assert_eq!(c.mode(), CuaMode::Daemon);
    let via_daemon = script(&c, &target, "dmn").await?;
    assert_eq!(via_daemon, embedded);

    // Two clients share one daemon-held sandbox: client A creates it and
    // writes a marker; client B (a separate connection) reattaches by name.
    // The Python and TS halves do this across OS processes.
    let nm = name(&format!("dve-{}-shared", target.lane()));
    let marker = hex(8);
    let a = d.client();
    let sb = match &target {
        Target::Direct { url, token } => {
            a.sandboxes()
                .connect_url(url.clone(), Some(token.clone()), Some(nm.clone()))
                .await?
        }
        Target::Local { token } => {
            let mut o = local_desktop_opts(&nm, token);
            o.services = HashMap::new();
            o.wait_for = vec![probe(3211, None)];
            a.sandboxes().create(o).await?
        }
    };
    wait_env(&sb, 120)
        .await?
        .upload(
            "/tmp/cua-e2e-shared-marker".into(),
            marker.clone().into_bytes(),
            None,
        )
        .await?;
    drop((sb, a));
    let b = d.client();
    assert_eq!(
        b.sandboxes()
            .list(None)
            .await?
            .iter()
            .filter(|s| s.name == nm)
            .count(),
        1
    );
    let sb = b.sandboxes().connect(nm).await?;
    let got = sb
        .spacesd(Some(10_000))
        .await?
        .download("/tmp/cua-e2e-shared-marker".into())
        .await;
    sb.delete().await?;
    assert_eq!(String::from_utf8(got?)?, marker);
    b.shutdown_daemon().await?;
    Ok(())
}

#[tokio::test]
async fn daemon_vs_embedded_mock() {
    e2e(
        "daemon-vs-embedded",
        "hermetic",
        "MockServer: identical results + shared sandbox",
        2 * MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            compare(Target::Direct {
                url: fx.env_url.clone(),
                token: fx.env_token.clone(),
            })
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn daemon_vs_embedded_local_container() {
    e2e(
        "daemon-vs-embedded",
        "container",
        "local desktop container: identical results + shared sandbox",
        20 * MIN,
        None,
        || async {
            require_image(&desktop_image())?;
            compare(Target::Local { token: hex(16) }).await
        },
    )
    .await;
}
