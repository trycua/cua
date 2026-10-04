//! Guide scenarios in Rust: run-omarchy, connect-with-viewer,
//! local-container, images. Definitions: ../python/test_{desktop,images}.py.

use std::sync::Arc;
use std::time::Duration;

use cua_sdk_e2e::cua::Sandbox;
use cua_sdk_e2e::*;

const MIN: Duration = Duration::from_secs(60);

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
