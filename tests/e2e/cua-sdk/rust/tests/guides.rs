//! Guide scenarios in Rust: your-first-cloud-fleet, create-pool,
//! expire-pools-and-claims, run-omarchy, connect-with-viewer,
//! local-container. Definitions: ../python/test_{fleet_guides,desktop}.py.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use cua_sdk_e2e::cua::{Cua, Fleet, Sandbox};
use cua_sdk_e2e::*;

const MIN: Duration = Duration::from_secs(60);
const FAKE_IMAGE: &str = "registry.test/cua-e2e:fake";

/// Fleet "gone": NotFound (404, or a read's 403 in a deleted namespace), or
/// the 403 live Fleet answers a *delete* in a namespace that is already gone.
fn gone(e: &CuaError) -> bool {
    match e {
        CuaError::NotFound(_) => true,
        CuaError::Fleet(m) => m.contains("403"),
        _ => false,
    }
}

async fn cleanup(fleet: &Fleet, pool: &str) -> Res {
    match fleet.delete_pool(pool.into()).await {
        Ok(()) => Ok(()),
        Err(e) if gone(&e) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn assert_gone(fleet: &Fleet, pool: &str) -> Res {
    poll(
        &format!("pool {pool} deleted"),
        90,
        Duration::from_secs(2),
        no_retry,
        || async {
            match fleet.get_pool(pool.into()).await {
                Ok(_) => Ok(None),
                Err(e) if gone(&e) => Ok(Some(())),
                Err(e) => Err(e),
            }
        },
    )
    .await
}

fn spec(name: &str, image: &str, runtime: &str, services: &[(&str, u16)]) -> cua::FleetPoolSpec {
    let mut s = pool_spec(name, image);
    s.runtime = Some(runtime.into());
    s.services = services.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    s.ttl_seconds_after_created = Some(7200);
    s
}

fn claim_opts(pool: &str, name: &str) -> cua::SandboxCreateOptions {
    let mut o = opts("cloud");
    o.pool = Some(pool.into());
    o.name = Some(name.into());
    o.ready_timeout_ms = Some(1_200_000);
    o
}

// ------------------------------------------------------------ your-first-cloud-fleet

async fn first_fleet<F, Fut>(
    c: &Arc<Cua>,
    image: &str,
    runtime: &str,
    services: &[(&str, u16)],
    desktop: F,
) -> Res
where
    F: FnOnce(Arc<Sandbox>) -> Fut,
    Fut: std::future::Future<Output = Res<(String, Vec<u8>)>>,
{
    first_fleet_with(c, image, runtime, services, None, desktop).await
}

/// `token`: the spacesd token, installed in a gVisor pod through the
/// entrypoint override (Fleet pools have no env/secret field yet).
async fn first_fleet_with<F, Fut>(
    c: &Arc<Cua>,
    image: &str,
    runtime: &str,
    services: &[(&str, u16)],
    token: Option<String>,
    desktop: F,
) -> Res
where
    F: FnOnce(Arc<Sandbox>) -> Fut,
    Fut: std::future::Future<Output = Res<(String, Vec<u8>)>>,
{
    let fleet = c.fleet()?;
    // Distinct per test: a just-deleted pool's namespace stays forbidden
    // while Fleet finalizes it.
    let pool = name(if token.is_some() {
        "first-env"
    } else {
        "first"
    });
    let mut s = spec(&pool, image, runtime, services);
    s.replicas = Some(1);
    s.cpu = Some(4);
    s.memory_mb = Some(4096);
    s.command = token.as_deref().map(env_token_command);
    fleet.apply_pool(s).await?;
    let r = async {
        let mut o = claim_opts(&pool, &format!("{pool}-claim"));
        o.token = token.clone();
        let sb = c.sandboxes().create(o).await?;
        let r = desktop(sb.clone()).await;
        sb.delete().await?;
        let (uname, png) = r?;
        assert!(uname.contains("Linux"), "{uname}");
        assert!(is_png(&png));
        Res::Ok(())
    }
    .await;
    cleanup(&fleet, &pool).await?;
    r?;
    assert_gone(&fleet, &pool).await
}

#[tokio::test]
async fn first_cloud_fleet_fake() {
    e2e(
        "your-first-cloud-fleet",
        "hermetic",
        "pool -> claim -> uname -> screenshot -> delete (fake Fleet)",
        MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            let c = embedded_fleet(Some((&fx.fleet_base_url, &fx.fleet_token)));
            let (url, token) = (fx.env_url.clone(), fx.env_token.clone());
            let c2 = c.clone();
            first_fleet(
                &c,
                FAKE_IMAGE,
                "kubevirt",
                &[("server", 8000)],
                move |sb| async move {
                    assert_eq!(
                        sb.service("server".into())?
                            .request("GET".into(), "/status".into(), None, Some(5000), None)
                            .await?
                            .status,
                        200
                    );
                    let d = c2
                        .sandboxes()
                        .connect_url(url, Some(token), Some(name("first-env")))
                        .await?;
                    let env = d.spacesd(Some(5000)).await?;
                    let out = env.run(cmd("echo", &["Linux", "mock"])).await?;
                    let png = env.screenshot(None).await?.image;
                    d.delete().await?;
                    Ok((String::from_utf8_lossy(&out.stdout).into_owned(), png))
                },
            )
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn first_cloud_fleet_env_image() {
    e2e(
        "your-first-cloud-fleet",
        "fleet-env",
        "spacesd image on Fleet (gVisor)",
        30 * MIN,
        None,
        || async {
            let image = std::env::var("CUA_E2E_FLEET_ENV_IMAGE")?;
            let token = hex(16);
            first_fleet_with(
                &embedded_fleet(None),
                &image,
                "gvisor",
                &[("env", 3211)],
                Some(token),
                |sb| async move {
                    let env = wait_env(&sb, 180).await?;
                    let out = env.run(cmd("uname", &["-a"])).await?;
                    let png = env.screenshot(None).await?.image;
                    Ok((String::from_utf8_lossy(&out.stdout).into_owned(), png))
                },
            )
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn first_cloud_fleet_env_image_kubevirt() {
    e2e(
        "your-first-cloud-fleet",
        "fleet-env",
        "spacesd image on Fleet (KubeVirt)",
        MIN,
        None,
        || async { skip(KUBEVIRT_ENV_SKIP) },
    )
    .await;
}

#[tokio::test]
async fn first_cloud_fleet_live() {
    e2e(
        "your-first-cloud-fleet",
        "fleet",
        "the tutorial's KubeVirt pool (legacy /cmd)",
        30 * MIN,
        None,
        || async {
            first_fleet(
                &embedded_fleet(None),
                LEGACY_FLEET_IMAGE,
                "kubevirt",
                &[("server", 8000)],
                |sb| async move {
                    let svc = sb.service("server".into())?;
                    poll(
                        "computer-server",
                        90,
                        Duration::from_secs(5),
                        |_| true,
                        || async {
                            Ok((svc
                                .request("GET".into(), "/status".into(), None, Some(30_000), None)
                                .await?
                                .status
                                == 200)
                                .then_some(()))
                        },
                    )
                    .await?;
                    let legacy = |command: &str, params: serde_json::Value| {
                        let svc = svc.clone();
                        let body = serde_json::json!({"command": command, "params": params})
                            .to_string()
                            .into_bytes();
                        async move {
                            let r = svc
                                .request(
                                    "POST".into(),
                                    "/cmd".into(),
                                    Some(body),
                                    Some(120_000),
                                    None,
                                )
                                .await?;
                            let text = String::from_utf8_lossy(&r.body).into_owned();
                            let data = text
                                .lines()
                                .find_map(|l| l.strip_prefix("data: "))
                                .ok_or("no data frame")?
                                .to_string();
                            Res::Ok(serde_json::from_str::<serde_json::Value>(&data)?)
                        }
                    };
                    let out =
                        legacy("run_command", serde_json::json!({"command": "uname -a"})).await?;
                    let shot = legacy("screenshot", serde_json::json!({})).await?;
                    let b64 = shot["image_data"]
                        .as_str()
                        .or(shot["result"]["image_data"].as_str())
                        .ok_or("no image")?;
                    Ok((
                        out["stdout"].as_str().unwrap_or_default().to_string(),
                        base64::engine::general_purpose::STANDARD.decode(b64)?,
                    ))
                },
            )
            .await
        },
    )
    .await;
}

// ------------------------------------------------------------ create-pool

async fn create_pool(c: &Arc<Cua>, image: &str, live: bool) -> Res {
    let fleet = c.fleet()?;
    let pool = name("pool");
    let claim = format!("{pool}-claim");
    let mut s = spec(&pool, image, "gvisor", &[("server", 8000)]);
    s.replicas = Some(1);
    s.cpu = Some(1);
    s.memory_mb = Some(2048);
    let r = async {
        assert_eq!(fleet.apply_pool(s.clone()).await?.replicas, 1);
        assert_eq!(fleet.apply_pool(s.clone()).await?.name, pool);
        if live {
            assert!(
                fleet
                    .wait_pool_ready(pool.clone(), 900_000)
                    .await?
                    .ready_replicas
                    .unwrap_or(0)
                    >= 1
            );
        }
        let sbx = c.sandboxes();
        let sb = sbx.create(claim_opts(&pool, &claim)).await?;
        let sb2 = sbx.create(claim_opts(&pool, &claim)).await?;
        assert_eq!(
            fleet
                .list_claims(pool.clone())
                .await?
                .iter()
                .filter(|x| x.name == claim)
                .count(),
            1
        );
        assert_eq!(sb2.name(), sb.name());
        let svc = sb2.service("server".into())?;
        poll(
            "server",
            if live { 60 } else { 3 },
            Duration::from_secs(if live { 5 } else { 0 }),
            |_| true,
            || async {
                Ok((svc
                    .request("GET".into(), "/status".into(), None, Some(30_000), None)
                    .await?
                    .status
                    == 200)
                    .then_some(()))
            },
        )
        .await?;
        assert_eq!(fleet.set_pool_replicas(pool.clone(), 2).await?.replicas, 2);
        sb.delete().await?;
        poll(
            "claim released",
            90,
            Duration::from_secs(2),
            no_retry,
            || async {
                Ok((!fleet
                    .list_claims(pool.clone())
                    .await?
                    .iter()
                    .any(|x| x.name == claim))
                .then_some(()))
            },
        )
        .await?;
        assert_eq!(fleet.get_pool(pool.clone()).await?.name, pool);
        Res::Ok(())
    }
    .await;
    cleanup(&fleet, &pool).await?;
    r
}

async fn ephemeral(c: &Arc<Cua>, image: &str) -> Res {
    let fleet = c.fleet()?;
    let mut o = opts("cloud");
    o.image = image.into();
    o.runtime = Some("gvisor".into());
    o.services = HashMap::from([("server".to_string(), 8000)]);
    o.cpus = Some(1);
    o.memory_mb = Some(2048);
    o.fleet_ttl_seconds = Some(3600);
    o.ready_timeout_ms = Some(900_000);
    let sb = c.sandboxes().create(o).await?;
    let pool = sb
        .info()
        .endpoints
        .values()
        .find_map(|u| {
            u.split("/api/svc/")
                .nth(1)
                .and_then(|r| r.split('/').next())
                .map(str::to_string)
        })
        .ok_or("no gateway endpoint")?;
    let r = async {
        let r = async {
            assert!(sb.is_ephemeral());
            // Managed pools are cua-auto-<tenant/spec hash> and outlive the
            // sandbox (reused by the next create with the same spec).
            assert!(pool.starts_with("cua-auto-"), "{pool}");
            assert_eq!(fleet.get_pool(pool.clone()).await?.name, pool);
            Res::Ok(())
        }
        .await;
        sb.delete().await?;
        r?;
        poll(
            &format!("claims of {pool} released"),
            90,
            Duration::from_secs(2),
            no_retry,
            || async {
                Ok(fleet
                    .list_claims(pool.clone())
                    .await?
                    .is_empty()
                    .then_some(()))
            },
        )
        .await?;
        assert_eq!(
            fleet.get_pool(pool.clone()).await?.name,
            pool,
            "the managed pool stays for reuse"
        );
        Res::Ok(())
    }
    .await;
    // Scoped GC through the SDK: only this pool, only once it has no claims.
    let report = fleet.pools().gc_pools(vec![pool.clone()], Some(0)).await?;
    r?;
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_gone(&fleet, &pool).await
}

#[tokio::test]
async fn create_pool_fake() {
    e2e(
        "create-pool",
        "hermetic",
        "warm pool, named claim reattach, scaling, ephemeral cleanup (fake Fleet)",
        MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            let c = embedded_fleet(Some((&fx.fleet_base_url, &fx.fleet_token)));
            create_pool(&c, FAKE_IMAGE, false).await?;
            ephemeral(&c, FAKE_IMAGE).await
        },
    )
    .await;
}

#[tokio::test]
async fn create_pool_live() {
    e2e(
        "create-pool",
        "fleet",
        "warm pool, named claim reattach, scaling",
        30 * MIN,
        None,
        || async { create_pool(&embedded_fleet(None), LEGACY_FLEET_ROOTFS, true).await },
    )
    .await;
}

#[tokio::test]
async fn ephemeral_pool_live() {
    e2e(
        "create-pool",
        "fleet",
        "ephemeral sandbox on a managed cua-auto-* pool; the test GCs the pool",
        30 * MIN,
        None,
        || async { ephemeral(&embedded_fleet(None), LEGACY_FLEET_ROOTFS).await },
    )
    .await;
}

// ------------------------------------------------------------ expire-pools-and-claims

fn has_ttl(json: &str, seconds: u64) -> bool {
    fn walk(v: &serde_json::Value, s: u64) -> bool {
        match v {
            serde_json::Value::Object(m) => m.iter().any(|(k, v)| {
                ((k == "ttlSecondsAfterCreated" || k == "ttl_seconds_after_created")
                    && v.as_u64() == Some(s))
                    || walk(v, s)
            }),
            serde_json::Value::Array(a) => a.iter().any(|v| walk(v, s)),
            _ => false,
        }
    }
    serde_json::from_str(json)
        .map(|v| walk(&v, seconds))
        .unwrap_or(false)
}

async fn expire(c: &Arc<Cua>, image: &str, live: bool) -> Res {
    let fleet = c.fleet()?;
    let pool = name("ttl");
    let mut s = spec(&pool, image, "gvisor", &[("server", 8000)]);
    s.replicas = Some(if live { 0 } else { 1 });
    s.cpu = Some(1);
    s.memory_mb = Some(1024);
    s.ttl_seconds_after_created = Some(86400);
    let r = async {
        fleet.apply_pool(s).await?;
        let got = fleet.get_pool(pool.clone()).await?;
        assert!(
            has_ttl(&got.json, 86400),
            "{}",
            &got.json[..got.json.len().min(400)]
        );
        let claim = fleet
            .claim(pool.clone(), Some(format!("{pool}-claim")), Some(3600))
            .await?;
        let body = fleet
            .list_claims(pool.clone())
            .await?
            .into_iter()
            .find(|x| x.name == claim.name)
            .ok_or("claim missing")?
            .json;
        assert!(
            has_ttl(&body, 3600) || body.contains("shutdownTime"),
            "{body}"
        );
        fleet.release(pool.clone(), claim.name).await?;
        Res::Ok(())
    }
    .await;
    cleanup(&fleet, &pool).await?;
    r
}

#[tokio::test]
async fn expire_fake() {
    e2e(
        "expire-pools-and-claims",
        "hermetic",
        "pool + claim TTLs reach Fleet (fake)",
        MIN,
        None,
        || async {
            let fx = Fixtures::start()?;
            expire(
                &embedded_fleet(Some((&fx.fleet_base_url, &fx.fleet_token))),
                FAKE_IMAGE,
                false,
            )
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn expire_live() {
    e2e(
        "expire-pools-and-claims",
        "fleet",
        "pool + claim TTLs reach Fleet",
        15 * MIN,
        None,
        || async { expire(&embedded_fleet(None), LEGACY_FLEET_ROOTFS, true).await },
    )
    .await;
}

// ------------------------------------------------------------ desktop

async fn with_local_desktop<F, Fut>(what: &str, f: F) -> Res
where
    F: FnOnce(Arc<Sandbox>, String) -> Fut,
    Fut: std::future::Future<Output = Res>,
{
    require_image(&desktop_image())?;
    let c = embedded_local();
    let token = hex(16);
    let sb = c
        .sandboxes()
        .create(local_desktop_opts(&name(what), &token))
        .await?;
    let r = f(sb.clone(), token).await;
    sb.delete().await?;
    r
}

#[tokio::test]
async fn local_container_desktop() {
    e2e(
        "local-container",
        "container",
        "desktop image as a local container sandbox + suspend/resume",
        10 * MIN,
        None,
        || async {
            with_local_desktop("desk-lc", |sb, _| async move {
                // gVisor when Docker has it (the CI runner installs it), else
                // runc; runtime_type names the backend.
                let expected = if has_runsc() { "gvisor" } else { "container" };
                assert_eq!(sb.runtime_type(), expected);
                env_smoke(&*wait_env(&sb, 120).await?, true, false).await?;
                sb.suspend().await?;
                sb.resume().await?;
                let env = wait_env(&sb, 120).await?;
                assert_eq!(env.run(cmd("echo", &["back"])).await?.stdout, b"back\n");
                if has_runsc() {
                    assert_eq!(
                        docker(&["inspect", "--format", "{{.HostConfig.Runtime}}", &sb.name()])?
                            .trim(),
                        "runsc"
                    );
                }
                Ok(())
            })
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn run_omarchy_local() {
    e2e(
        "run-omarchy",
        "container",
        "dimensions, clipboard, click + keys in the grid fixture, /mcp initialize",
        10 * MIN,
        None,
        || async {
            with_local_desktop("desk-om", |sb, token| async move {
                let env = wait_env(&sb, 120).await?;
                desktop_checks(&env).await?;
                let fwd = sb.forward(3211).await?;
                let addr = fwd.local_addr().unwrap();
                let r = tokio::task::spawn_blocking(move || mcp_initialize(&addr, &token)).await?;
                fwd.close().await?;
                assert!(r?["serverInfo"]["name"].is_string());
                Ok(())
            })
            .await
        },
    )
    .await;
}

#[tokio::test]
async fn viewer_local() {
    e2e(
        "connect-with-viewer",
        "container",
        "env service /viewer/ page + forward + viewer_url ticket link",
        10 * MIN,
        None,
        || async {
            with_local_desktop("desk-view", |sb, _| async move {
                let env = sb.service("env".into())?;
                let body = poll(
                    "viewer /viewer/",
                    30,
                    Duration::from_secs(2),
                    |_| true,
                    || async {
                        let r = env
                            .request("GET".into(), "/viewer/".into(), None, Some(30_000), None)
                            .await?;
                        Ok((r.status == 200).then_some(r.body))
                    },
                )
                .await?;
                assert!(String::from_utf8_lossy(&body).contains("viewer.js"));
                let link = sb
                    .viewer_url(Some(cua::ViewerOptions {
                        ttl_seconds: Some(600),
                        view_only: true,
                        ..Default::default()
                    }))
                    .await?;
                assert!(link.url.contains("/viewer/#ticket="), "{}", link.url);
                assert!(link.expires_at_unix > 0);
                let fwd = sb.forward(3211).await?;
                let addr = fwd.local_addr().unwrap();
                let page = tokio::task::spawn_blocking(move || http_get(&addr, "/viewer/")).await?;
                fwd.close().await?;
                let (status, page) = page?;
                assert_eq!(status, 200);
                assert!(page.contains("viewer.js"));
                Ok(())
            })
            .await
        },
    )
    .await;
}

// ------------------------------------------------------------ images

fn image_spec(nm: &str, marker: &str) -> String {
    serde_json::json!({
        "apiVersion": "images.cua.ai/v1alpha1", "kind": "Image", "metadata": {"name": nm, "namespace": nm},
        "spec": {"recipe": {"osType": "linux", "distro": "ubuntu", "version": "24.04", "kind": "vm",
            "layers": [{"type": "run", "command": format!("mkdir -p /opt/cua-e2e && echo {marker} > /opt/cua-e2e/marker")}],
            "env": {"CUA_E2E_BUILT": marker}, "ports": [8080]}}
    })
    .to_string()
}

/// A throwaway registry:2 on loopback; removed on drop.
struct Registry(String);
impl Drop for Registry {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

#[tokio::test]
async fn images_sdk_local_layers() {
    e2e(
        "images",
        "container",
        "SDK build_image layers apply; the pushed rootfs runs as a sandbox",
        20 * MIN,
        None,
        || async {
            let base = plain_image("ubuntu-server");
            require_image(&base)?;
            let reg = Registry(name("registry-sdk"));
            let _ = docker(&["rm", "-f", &reg.0]);
            docker(&[
                "run",
                "-d",
                "--name",
                &reg.0,
                "--memory=256m",
                "-p",
                "127.0.0.1::5000",
                "registry:2",
            ])?;
            let port = docker(&["port", &reg.0, "5000/tcp"])?;
            let host = format!(
                "127.0.0.1:{}",
                port.lines()
                    .next()
                    .and_then(|l| l.rsplit(':').next())
                    .ok_or("port")?
                    .trim()
            );
            for _ in 0..30 {
                if read_banner(&host, 1).is_empty() && std::net::TcpStream::connect(&host).is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            // SAFETY: tests in this binary that read the variable run after it is set.
            unsafe { std::env::set_var("CUA_INSECURE_REGISTRIES", &host) };
            let c = embedded_local();
            let marker = hex(6);
            let dest = format!("{host}/cua-e2e/sdk-built:{}-rs", run_id());
            let r = c
                .local()
                .build_image(
                    image_spec(&name("img-sdk"), &marker),
                    format!("container:{base}"),
                    Some(dest.clone()),
                )
                .await?;
            assert!(r.contains("@sha256:"), "{r}");
            let nm = name("built");
            let mut o = opts("local");
            o.image = format!("container:{dest}");
            o.name = Some(nm.clone());
            o.cpus = Some(1);
            o.memory_mb = Some(512);
            let sb = c.sandboxes().create(o).await?;
            let out = docker(&[
                "exec",
                &nm,
                "sh",
                "-c",
                "cat /opt/cua-e2e/marker; . /etc/profile.d/cua-env.sh; echo $CUA_E2E_BUILT",
            ]);
            sb.delete().await?;
            let out = out?;
            assert_eq!(
                out.split_whitespace().collect::<Vec<_>>(),
                vec![marker.as_str(), marker.as_str()]
            );
            Ok(())
        },
    )
    .await;
}

#[tokio::test]
async fn images_fleet_remote_build() {
    e2e(
        "images",
        "fleet",
        "remote build via create_image, or a typed error",
        10 * MIN,
        None,
        || async {
            let c = embedded_fleet(None);
            let fleet = c.fleet()?;
            let pool = name("img");
            let mut s = spec(&pool, LEGACY_FLEET_ROOTFS, "gvisor", &[]);
            s.replicas = Some(0);
            fleet.apply_pool(s).await?;
            let mut img: serde_json::Value =
                serde_json::from_str(&image_spec(&name("img-remote"), "remote"))?;
            img["metadata"]["namespace"] = pool.clone().into();
            let r = match fleet.create_image(pool.clone(), img.to_string()).await {
                Ok(_) => fleet
                    .delete_image(pool.clone(), name("img-remote"))
                    .await
                    .map_err(Into::into),
                Err(
                    e @ (CuaError::Fleet(_)
                    | CuaError::Unsupported(_)
                    | CuaError::InvalidArgument(_)
                    | CuaError::PermissionDenied(_)),
                ) => {
                    eprintln!("remote build refused (typed): {e}");
                    Ok(())
                }
                Err(e) => Err(e.into()),
            };
            cleanup(&fleet, &pool).await?;
            r
        },
    )
    .await;
}
