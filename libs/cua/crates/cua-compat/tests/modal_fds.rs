//! The Modal gateway's file descriptors over many Sandbox cycles. Its own
//! test binary: descriptors are counted for the whole process, so no other
//! test may run beside it.

use cua_compat::backend::mock::MockBackend;
use cua_compat::modal::pb::api as pb;
use cua_compat::modal::pb::api::modal_client_client::ModalClientClient;
use cua_compat::modal::pb::tcr;
use cua_compat::modal::pb::tcr::task_command_router_client::TaskCommandRouterClient;
use cua_compat::{Gateway, GatewayOptions, Provider};
use std::sync::Arc;
use tonic::Request;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;

struct Gw {
    gw: Gateway,
    chan: Channel,
    _dir: tempfile::TempDir,
}

async fn start() -> Gw {
    let dir = tempfile::tempdir().unwrap();
    // Image builds are logged, not run (the mock Space has no shell).
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any gateway thread of this test binary reads it.
    ONCE.call_once(|| unsafe { std::env::set_var("CUA_MODAL_BUILD_DRY_RUN", "1") });
    let mut o = GatewayOptions::new(Provider::Modal);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(Arc::new(MockBackend::new())))
        .await
        .unwrap();
    let chan = Channel::from_shared(format!("http://{}", gw.api_addr))
        .unwrap()
        .connect()
        .await
        .unwrap();
    Gw {
        gw,
        chan,
        _dir: dir,
    }
}

impl Gw {
    fn client(&self) -> ModalClientClient<Channel> {
        ModalClientClient::new(self.chan.clone())
    }

    fn token(&self) -> (String, String) {
        let env: std::collections::HashMap<_, _> = self.gw.env().into_iter().collect();
        (
            env["MODAL_TOKEN_ID"].clone(),
            env["MODAL_TOKEN_SECRET"].clone(),
        )
    }

    fn authed<T>(&self, msg: T) -> Request<T> {
        let (id, secret) = self.token();
        let mut r = Request::new(msg);
        r.metadata_mut()
            .insert("x-modal-token-id", MetadataValue::try_from(id).unwrap());
        r.metadata_mut().insert(
            "x-modal-token-secret",
            MetadataValue::try_from(secret).unwrap(),
        );
        r
    }
}

fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
}

/// Creating and ending Sandboxes (with an exec and both kinds of tunnel)
/// returns the gateway's file descriptors to where they were.
#[tokio::test]
async fn sandbox_cycles_do_not_leak_file_descriptors() {
    let g = start().await;
    let mut c = g.client();
    let app = c
        .app_get_or_create(g.authed(pb::AppGetOrCreateRequest {
            app_name: "fds".into(),
            object_creation_type: pb::ObjectCreationType::CreateIfMissing as i32,
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let image = c
        .image_get_or_create(g.authed(pb::ImageGetOrCreateRequest {
            app_id: app.app_id.clone(),
            image: Some(pb::Image {
                dockerfile_commands: vec!["FROM ghcr.io/trycua/linux:24.04".into()],
                ..Default::default()
            }),
            builder_version: "2025.06".into(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner()
        .image_id;
    let auth = c
        .auth_token_get(g.authed(pb::AuthTokenGetRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .token;
    let cycle = |n: usize| {
        let c = c.clone();
        let chan = g.chan.clone();
        let (image, app_id, auth) = (image.clone(), app.app_id.clone(), auth.clone());
        let (id, secret) = g.token();
        async move {
            let mut c = c;
            let with_token = |mut r: Request<pb::SandboxCreateV2Request>| {
                r.metadata_mut().insert(
                    "x-modal-token-id",
                    MetadataValue::try_from(id.clone()).unwrap(),
                );
                r.metadata_mut().insert(
                    "x-modal-token-secret",
                    MetadataValue::try_from(secret.clone()).unwrap(),
                );
                r.metadata_mut().insert(
                    "x-modal-auth-token",
                    MetadataValue::try_from(auth.clone()).unwrap(),
                );
                r
            };
            let sb = c
                .sandbox_create_v2(with_token(Request::new(pb::SandboxCreateV2Request {
                    app_id,
                    definition: Some(pb::Sandbox {
                        entrypoint_args: vec!["sleep".into(), "600000".into()],
                        image_id: image,
                        timeout_secs: 60,
                        open_ports_oneof: Some(pb::sandbox::OpenPortsOneof::OpenPorts(
                            pb::PortSpecs {
                                ports: vec![
                                    pb::PortSpec {
                                        port: 8080,
                                        unencrypted: false,
                                        ..Default::default()
                                    },
                                    pb::PortSpec {
                                        port: 9090,
                                        unencrypted: true,
                                        ..Default::default()
                                    },
                                ],
                            },
                        )),
                        ..Default::default()
                    }),
                    ..Default::default()
                })))
                .await
                .unwrap()
                .into_inner();
            let access = sb.command_router_access.unwrap();
            let mut r = TaskCommandRouterClient::new(chan);
            let mut exec = Request::new(tcr::TaskExecStartRequest {
                task_id: sb.task_id.clone(),
                exec_id: format!("ex-{n}"),
                command_args: vec!["sleep".into(), "600000".into()], // still running when the sandbox ends
                ..Default::default()
            });
            exec.metadata_mut().insert(
                "authorization",
                MetadataValue::try_from(format!("Bearer {}", access.jwt)).unwrap(),
            );
            r.task_exec_start(exec).await.unwrap();
            let mut term = Request::new(pb::SandboxTerminateRequest {
                sandbox_id: sb.sandbox_id,
            });
            term.metadata_mut()
                .insert("x-modal-token-id", MetadataValue::try_from(id).unwrap());
            term.metadata_mut().insert(
                "x-modal-token-secret",
                MetadataValue::try_from(secret).unwrap(),
            );
            term.metadata_mut()
                .insert("x-modal-auth-token", MetadataValue::try_from(auth).unwrap());
            c.sandbox_terminate_v2(term).await.unwrap();
        }
    };
    // Warm up (connections, caches), then measure.
    for n in 0..3 {
        cycle(n).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let before = open_fds();
    for n in 3..33 {
        cycle(n).await;
    }
    let mut after = open_fds();
    for _ in 0..40 {
        if after <= before + 8 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        after = open_fds();
    }
    if after > before + 8 {
        let out = std::process::Command::new("lsof")
            .args(["-n", "-P", "-p", &std::process::id().to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        eprintln!("{out}");
    }
    assert!(
        after <= before + 8,
        "file descriptors grew from {before} to {after} over 30 sandbox cycles"
    );
}

#[tokio::test]
async fn the_mock_backend_alone_does_not_leak() {
    use cua_compat::backend::{Backend, CreateSpec, Identity, Placement, Source};
    let b = MockBackend::new();
    let who = Identity::default();
    let (b, who) = (&b, &who);
    let mk = |i: usize| async move {
        let c = b
            .create(
                who,
                CreateSpec {
                    name: format!("m{i}"),
                    source: Source::Image(None),
                    env: Default::default(),
                    internet: true,
                    place: Placement::default(),
                },
            )
            .await
            .unwrap();
        let _ = b.spacesd(who, &c.name).await.unwrap();
        b.delete(who, &c.name).await.unwrap();
    };
    for i in 0..3 {
        mk(i).await;
    }
    let before = open_fds();
    for i in 3..33 {
        mk(i).await;
    }
    let after = open_fds();
    assert!(after <= before + 8, "mock backend fds {before} -> {after}");
}
