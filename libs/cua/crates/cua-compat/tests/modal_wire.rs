//! The Modal gateway over mock Spaces, at the gRPC wire, with the client
//! stubs generated from the same descriptors as the server: token
//! authentication, the error codes the Modal client maps to its exceptions,
//! the reasons given for RPCs that are not served, the command router's JWT
//! rules, and the V2 sandbox flow. The unmodified Python, JavaScript and Go
//! SDKs are driven against the same gateway by libs/cua/tests/compat/modal.

use cua_compat::backend::mock::MockBackend;
use cua_compat::modal::pb::api as pb;
use cua_compat::modal::pb::api::modal_client_client::ModalClientClient;
use cua_compat::modal::pb::tcr;
use cua_compat::modal::pb::tcr::task_command_router_client::TaskCommandRouterClient;
use cua_compat::{Gateway, GatewayOptions, Provider};
use std::sync::Arc;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request};

struct Gw {
    gw: Gateway,
    chan: Channel,
    _dir: tempfile::TempDir,
}

async fn start() -> Gw {
    start_with(&[]).await
}

/// A gateway whose mock Spaces have these guest ports served by these local addresses.
async fn start_with(ports: &[(u16, String)]) -> Gw {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(MockBackend::new());
    for (g, a) in ports {
        backend.ports.lock().await.insert(*g, a.clone());
    }
    // Image builds are logged, not run (the mock Space has no shell).
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before any gateway thread of this test binary reads it.
    ONCE.call_once(|| unsafe { std::env::set_var("CUA_MODAL_BUILD_DRY_RUN", "1") });
    let mut o = GatewayOptions::new(Provider::Modal);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    let gw = Gateway::start(o, Some(backend)).await.unwrap();
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

#[tokio::test]
async fn calls_need_the_token_pair() {
    let g = start().await;
    let mut c = g.client();
    let e = c.client_hello(Request::new(())).await.unwrap_err();
    assert_eq!(e.code(), Code::Unauthenticated);
    assert!(e.message().contains("MODAL_TOKEN_ID"), "{}", e.message());

    let mut bad = Request::new(());
    bad.metadata_mut()
        .insert("x-modal-token-id", MetadataValue::from_static("ak-nope"));
    bad.metadata_mut().insert(
        "x-modal-token-secret",
        MetadataValue::from_static("as-nope"),
    );
    let e = c.client_hello(bad).await.unwrap_err();
    assert_eq!(e.code(), Code::Unauthenticated);

    c.client_hello(g.authed(())).await.unwrap();
}

#[tokio::test]
async fn token_info_names_the_workspace() {
    let g = start().await;
    let r = g
        .client()
        .token_info_get(g.authed(pb::TokenInfoGetRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(r.token_id, g.token().0);
    assert!(!r.workspace_name.is_empty());
}

#[tokio::test]
async fn unserved_rpcs_say_why() {
    let g = start().await;
    let mut c = g.client();
    // Billing, workspace administration and networking products are Modal's.
    let e = c
        .workspace_billing_report(g.authed(pb::WorkspaceBillingReportRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::Unimplemented);
    assert!(
        e.message().contains("WorkspaceBillingReport") && e.message().contains("Cua Spaces"),
        "{}",
        e.message()
    );
    let e = c
        .proxy_get(g.authed(pb::ProxyGetRequest::default()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::Unimplemented);
    assert!(e.message().contains("ProxyGet"), "{}", e.message());
}

#[tokio::test]
async fn errors_carry_the_codes_the_client_maps() {
    let g = start().await;
    let mut c = g.client();
    // An unknown id is NOT_FOUND (NotFoundError).
    let e = c
        .sandbox_wait(g.authed(pb::SandboxWaitRequest {
            sandbox_id: "sb-doesnotexist00000000000".into(),
            timeout: 0.0,
        }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::NotFound, "{e}");
    // A name used twice is ALREADY_EXISTS (AlreadyExistsError).
    let req = pb::VolumeGetOrCreateRequest {
        deployment_name: "wire-vol".into(),
        object_creation_type: pb::ObjectCreationType::CreateIfMissing as i32,
        ..Default::default()
    };
    c.volume_get_or_create(g.authed(req.clone())).await.unwrap();
    let again = pb::VolumeGetOrCreateRequest {
        object_creation_type: pb::ObjectCreationType::CreateFailIfExists as i32,
        ..req
    };
    let e = c.volume_get_or_create(g.authed(again)).await.unwrap_err();
    assert_eq!(e.code(), Code::AlreadyExists, "{e}");
    // Version 2 volumes are not served; the error says so (InvalidError).
    let v2 = pb::VolumeGetOrCreateRequest {
        deployment_name: "wire-vol2".into(),
        object_creation_type: pb::ObjectCreationType::CreateIfMissing as i32,
        version: pb::VolumeFsVersion::V2 as i32,
        ..Default::default()
    };
    let e = c.volume_get_or_create(g.authed(v2)).await.unwrap_err();
    assert_eq!(e.code(), Code::InvalidArgument, "{e}");
    assert!(e.message().contains("version"), "{}", e.message());
}

#[tokio::test]
async fn a_sandbox_runs_through_the_v2_flow_and_the_router_checks_its_token() {
    let g = start().await;
    let mut c = g.client();
    let app = c
        .app_get_or_create(g.authed(pb::AppGetOrCreateRequest {
            app_name: "wire".into(),
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
                dockerfile_commands: vec![
                    "FROM ghcr.io/trycua/linux:24.04".into(),
                    "RUN echo layer".into(),
                ],
                ..Default::default()
            }),
            builder_version: "2025.06".into(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let image_id = image.image_id;
    assert!(image_id.starts_with("im-"), "{image_id}");

    // The auth token V2 sandbox RPCs carry.
    let auth = c
        .auth_token_get(g.authed(pb::AuthTokenGetRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .token;
    assert!(!auth.is_empty());

    let mut create = g.authed(pb::SandboxCreateV2Request {
        app_id: app.app_id.clone(),
        definition: Some(pb::Sandbox {
            entrypoint_args: vec!["sleep".into(), "600000".into()],
            image_id,
            timeout_secs: 60,
            ..Default::default()
        }),
        ..Default::default()
    });
    create.metadata_mut().insert(
        "x-modal-auth-token",
        MetadataValue::try_from(auth.clone()).unwrap(),
    );
    let sb = c.sandbox_create_v2(create).await.unwrap().into_inner();
    assert!(!sb.sandbox_id.is_empty() && !sb.task_id.is_empty());
    let access = sb.command_router_access.expect("router access");
    assert!(
        access.url.starts_with("http://127.0.0.1:"),
        "{}",
        access.url
    );

    // V2 RPCs without the auth token are refused.
    let e = c
        .sandbox_wait_v2(g.authed(pb::SandboxWaitRequest {
            sandbox_id: sb.sandbox_id.clone(),
            timeout: 0.0,
        }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::Unauthenticated, "{e}");

    // The command router only takes its own JWT.
    let mut r = TaskCommandRouterClient::new(g.chan.clone());
    let exec = tcr::TaskExecStartRequest {
        task_id: sb.task_id.clone(),
        exec_id: "ex-wire".into(),
        command_args: vec!["echo".into(), "hi".into()],
        ..Default::default()
    };
    let e = r
        .task_exec_start(Request::new(exec.clone()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Code::Unauthenticated, "{e}");
    let mut req = Request::new(exec.clone());
    req.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from("Bearer not.a.jwt").unwrap(),
    );
    let e = r.task_exec_start(req).await.unwrap_err();
    assert_eq!(e.code(), Code::Unauthenticated, "{e}");
    let mut req = Request::new(exec);
    req.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {}", access.jwt)).unwrap(),
    );
    r.task_exec_start(req).await.unwrap();

    // A router token is for one task.
    let mut req = Request::new(tcr::TaskExecStartRequest {
        task_id: "ta-someoneelse".into(),
        exec_id: "ex-x".into(),
        command_args: vec!["echo".into()],
        ..Default::default()
    });
    req.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {}", access.jwt)).unwrap(),
    );
    let e = r.task_exec_start(req).await.unwrap_err();
    assert!(
        matches!(
            e.code(),
            Code::PermissionDenied | Code::NotFound | Code::InvalidArgument
        ),
        "{e}"
    );

    let mut term = g.authed(pb::SandboxTerminateRequest {
        sandbox_id: sb.sandbox_id,
    });
    term.metadata_mut()
        .insert("x-modal-auth-token", MetadataValue::try_from(auth).unwrap());
    c.sandbox_terminate_v2(term).await.unwrap();
}

/// A guest HTTP/2 server on a tunnel declared with `h2_ports`: the router's TLS
/// negotiates `h2` for it and only `http/1.1` for an ordinary encrypted port.
#[tokio::test]
async fn h2_ports_negotiate_http2_and_others_do_not() {
    use http_body_util::{BodyExt, Empty};
    use hyper::body::Bytes;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};

    // The "guest" server speaks HTTP/2 without TLS (prior knowledge).
    let guest = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let guest_addr = guest.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (s, _) = guest.accept().await.unwrap();
            tokio::spawn(async move {
                let svc =
                    hyper::service::service_fn(|_r: hyper::Request<hyper::body::Incoming>| async {
                        Ok::<_, std::convert::Infallible>(hyper::Response::new(
                            http_body_util::Full::new(Bytes::from_static(b"h2-ok")),
                        ))
                    });
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    let g = start_with(&[(8080, guest_addr.clone()), (9091, guest_addr)]).await;
    let mut c = g.client();
    let app = c
        .app_get_or_create(g.authed(pb::AppGetOrCreateRequest {
            app_name: "h2".into(),
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
    let mut create = g.authed(pb::SandboxCreateV2Request {
        app_id: app.app_id.clone(),
        definition: Some(pb::Sandbox {
            entrypoint_args: vec!["sleep".into(), "600000".into()],
            image_id: image,
            timeout_secs: 60,
            open_ports_oneof: Some(pb::sandbox::OpenPortsOneof::OpenPorts(pb::PortSpecs {
                ports: vec![
                    pb::PortSpec {
                        port: 8080,
                        unencrypted: false,
                        tunnel_type: Some(pb::TunnelType::H2 as i32),
                    },
                    pb::PortSpec {
                        port: 9091,
                        unencrypted: false,
                        tunnel_type: None,
                    },
                ],
            })),
            ..Default::default()
        }),
        ..Default::default()
    });
    create
        .metadata_mut()
        .insert("x-modal-auth-token", MetadataValue::try_from(auth).unwrap());
    let sb = c.sandbox_create_v2(create).await.unwrap().into_inner();
    let host_port = sb
        .tunnels
        .iter()
        .find(|t| t.container_port == 8080)
        .unwrap()
        .port;
    let sb_id = sb.sandbox_id.to_ascii_lowercase();

    let env: std::collections::HashMap<_, _> = g.gw.env().into_iter().collect();
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(&env["SSL_CERT_FILE"]).unwrap() {
        let _ = roots.add(cert.unwrap());
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
    let negotiated = |port: u16| {
        let connector = connector.clone();
        let name = format!("{port}-{sb_id}.modal.localhost");
        async move {
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", host_port as u16))
                .await
                .unwrap();
            let tls = connector
                .connect(ServerName::try_from(name).unwrap(), tcp)
                .await
                .unwrap();
            let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
            (tls, alpn)
        }
    };

    let (tls, alpn) = negotiated(8080).await;
    assert_eq!(alpn.as_deref(), Some(&b"h2"[..]));
    let (mut send, conn) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tokio::spawn(conn);
    let req = hyper::Request::builder()
        .uri("https://8080/")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = send.send_request(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"h2-ok");

    let (_tls, alpn) = negotiated(9091).await;
    assert_eq!(alpn.as_deref(), Some(&b"http/1.1"[..]));
}

/// Every served RPC takes an empty request, random bytes and a truncated frame
/// without the gateway dying: each answer is a gRPC status, never a dropped
/// connection, and the gateway still answers afterwards.
#[tokio::test]
async fn garbage_requests_never_crash_the_gateway() {
    let g = start().await;
    let (id, secret) = g.token();
    let http = reqwest::Client::builder()
        .http2_prior_knowledge()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap();
    let mut failures: Vec<String> = Vec::new();
    let served = include_str!("../modal-proto/served-rpcs.txt");
    let mut rng = 0x9e3779b97f4a7c15u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut asked = 0;
    for rpc in served.lines().filter(|l| l.contains("ModalClient/")) {
        let url = format!("http://{}/{rpc}", g.gw.api_addr);
        // Streaming and blocking RPCs: the request is bad, so they answer at once.
        let noise: Vec<u8> = (0..64).map(|_| (next() & 0xff) as u8).collect();
        let mut framed = vec![0u8, 0, 0, 0, noise.len() as u8];
        framed.extend_from_slice(&noise);
        let bodies: [Vec<u8>; 4] = [
            vec![0, 0, 0, 0, 0],      // an empty message
            framed,                   // garbage inside a valid frame
            vec![0, 0, 0, 0, 200, 1], // a frame longer than its body
            vec![0xff; 7],            // not a frame at all
        ];
        for body in bodies {
            let sent = http
                .post(&url)
                .header("content-type", "application/grpc")
                .header("te", "trailers")
                .header("x-modal-token-id", &id)
                .header("x-modal-token-secret", &secret)
                .body(body.clone())
                .send()
                .await;
            asked += 1;
            let resp = match sent {
                Ok(r) => r,
                Err(e) => {
                    failures.push(format!("{rpc} {body:?}: {e}"));
                    continue;
                }
            };
            if !resp.status().is_success() && resp.status().as_u16() != 415 {
                failures.push(format!("{rpc}: HTTP {}", resp.status()));
                continue;
            }
            // The status is a header (trailers-only answer) or a trailer after the body.
            let in_headers = resp.headers().contains_key("grpc-status");
            if resp.bytes().await.is_err() && !in_headers {
                failures.push(format!("{rpc}: no gRPC status"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {asked} requests got no answer: {failures:#?}",
        failures.len()
    );
    assert!(asked > 500, "asked {asked} requests");
    g.client().client_hello(g.authed(())).await.unwrap();
}
