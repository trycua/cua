//! Any OCI image is a full Space (`CUA_E2E_OCI_ANY=1`): `alpine:3.20` and
//! `python:3.12-slim` carry no cua-spacesd, so the runtime injects a static
//! one and runs it beside the image's own command. For each image and kind
//! the test creates a Space through the Spaces SDK the apps use, then runs a
//! command, moves a file up and back, and reaches a server the Space starts
//! through a port forward. A Space with a command of its own, and an image
//! whose user is not root, are checked too.
//!
//! | env | meaning |
//! |---|---|
//! | `CUA_HOME` | required, a temporary directory (never `~/.cua`); keep it short |
//! | `CUA_E2E_OCI_ANY_KINDS` | comma separated: `container` (default), `microvm` |
//! | `CUA_E2E_OCI_ANY_RUNTIME` | the container engine (default `auto`; `runc` on a runner with no gVisor) |
//! | `CUA_SPACESD_STATIC` | the static Linux cua-spacesd (else `cua-spacesd-linux-<arch>` next to the test, in `~/.cua/bin` or on `PATH`) |

use std::sync::Arc;
use std::time::Duration;

use cua_daemon::local::VmmLocal;
use cua_e2e_local_runtimes::{gated, init_tracing, run_id};
use cua_sandbox_core::placement::{Kind, On};
use cua_spaces::{SpaceCreate, SpaceCreated, Spaces};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn spaces(home: &std::path::Path) -> Spaces {
    Spaces::builder()
        .home(home)
        .download_dir(home.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .local_runtime(Arc::new(VmmLocal::default()))
        .probe_timeout(Duration::from_secs(20))
        .build()
}

fn kinds() -> Vec<Kind> {
    std::env::var("CUA_E2E_OCI_ANY_KINDS")
        .unwrap_or_else(|_| "container".into())
        .split(',')
        .map(|k| Kind::parse(k.trim()).expect("a kind"))
        .collect()
}

async fn create(
    spaces: &Spaces,
    image: &str,
    kind: Kind,
    name: &str,
    command: Option<Vec<String>>,
) -> cua_spaces::SpaceInfo {
    let created = spaces
        .create(SpaceCreate {
            image: Some(image.into()),
            on: Some(On::Local),
            kind,
            // The engine asked for applies to containers (a microVM has its
            // own runtimes).
            runtime: std::env::var("CUA_E2E_OCI_ANY_RUNTIME")
                .ok()
                .filter(|_| kind == Kind::Container)
                .map(|r| cua_sandbox_core::placement::Runtime::parse(&r).expect("a runtime"))
                .unwrap_or_default(),
            name: Some(name.into()),
            cpus: Some(1),
            memory_mb: Some(1024),
            command,
            timeout: Some(Duration::from_secs(600)),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| panic!("create {image} as {kind}: {e}"));
    let SpaceCreated::Ready { info, .. } = created else {
        panic!("{image} as {kind} is not ready: {created:?}");
    };
    info
}

/// `GET path` from `addr`: (status line, body).
async fn http_get(addr: std::net::SocketAddr, path: &str) -> (String, String) {
    let mut s = tokio::net::TcpStream::connect(addr).await.expect("connect");
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (
        head.lines().next().unwrap_or_default().to_string(),
        body.to_string(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn any_oci_image_is_a_full_space() {
    if !gated("CUA_E2E_OCI_ANY") {
        return;
    }
    init_tracing();
    let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
    assert!(
        !home.is_empty() && !home.ends_with("/.cua"),
        "never the real cua home"
    );
    let reg = tempfile::tempdir().unwrap();
    let spaces = spaces(reg.path());
    let scratch = tempfile::tempdir().unwrap();

    // (image, what /etc/os-release says, a server the image can start)
    let images = [
        (
            "alpine:3.20",
            "Alpine",
            r#"sh -c 'while true; do printf "HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\nAlpine\n" | nc -l -p 8000 >/dev/null; done'"#,
        ),
        (
            "python:3.12-slim",
            "Debian",
            "python -m http.server 8000 --directory /etc",
        ),
    ];
    for kind in kinds() {
        for (image, marker, server) in images {
            let name = format!("e2e-oci-{}-{}", run_id(), image.split(':').next().unwrap());
            let info = create(&spaces, image, kind, &name, None).await;
            eprintln!("{image} as {kind}: {}", info.id);
            let space = spaces.space(&info.id).await.unwrap();

            // A command in the image's own userland.
            let os = space
                .bash("cat /etc/os-release", Duration::from_secs(60))
                .await
                .unwrap();
            assert!(os.success(), "{image}: {os:?}");
            assert!(os.stdout.contains(marker), "{image}: {}", os.stdout);

            // A file up and back.
            let local = scratch.path().join(format!("hello-{name}.txt"));
            std::fs::write(&local, format!("hello {image}\n")).unwrap();
            let up = space
                .upload(&local, Some("/tmp/hello.txt"))
                .await
                .unwrap_or_else(|e| panic!("{image}: upload: {e}"));
            assert_eq!(up.files, 1, "{up:?}");
            let cat = space
                .bash("cat /tmp/hello.txt", Duration::from_secs(30))
                .await
                .unwrap();
            assert_eq!(cat.stdout, format!("hello {image}\n"), "{image}");
            let down_dir = scratch.path().join("down");
            let down = space.download("/etc/os-release", &down_dir).await.unwrap();
            assert!(down.bytes > 0, "{down:?}");
            assert!(
                std::fs::read_to_string(&down.dest)
                    .unwrap()
                    .contains(marker),
                "{image}: downloaded os-release"
            );

            // A server the Space starts, reached through a port forward.
            let started = space
                .bash(
                    &format!("nohup {server} >/tmp/server.log 2>&1 & sleep 1; echo started"),
                    Duration::from_secs(60),
                )
                .await
                .unwrap();
            assert!(started.success(), "{image}: {started:?}");
            let fwd = space
                .spacesd()
                .unwrap()
                .forward_tcp(cua_spacesd_client::ForwardOptions::new(8000))
                .await
                .unwrap_or_else(|e| panic!("{image}: forward: {e}"));
            let (status, body) = http_get(fwd.local_addr(), "/os-release").await;
            assert!(status.contains("200"), "{image}: {status}");
            assert!(body.contains(marker), "{image}: {body}");
            drop(fwd);

            spaces.delete(&info.id).await.unwrap();
        }

        // A command of the Space's own runs beside cua-spacesd and ends the
        // Space when it exits.
        let name = format!("e2e-oci-{}-cmd", run_id());
        let info = create(
            &spaces,
            "alpine:3.20",
            kind,
            &name,
            Some(vec!["sleep".into(), "600".into()]),
        )
        .await;
        let space = spaces.space(&info.id).await.unwrap();
        let ps = space
            .bash(
                "for p in /proc/[0-9]*; do tr '\\0' ' ' < $p/cmdline; echo; done",
                Duration::from_secs(30),
            )
            .await
            .unwrap();
        assert!(ps.stdout.contains("sleep 600"), "{}", ps.stdout);
        assert!(ps.stdout.contains("supervise"), "{}", ps.stdout);
        spaces.delete(&info.id).await.unwrap();

        // An image whose user is not root: the command drops to it, and
        // cua-spacesd stays root (it can still read everything).
        let name = format!("e2e-oci-{}-user", run_id());
        let info = create(
            &spaces,
            "nginxinc/nginx-unprivileged:alpine",
            kind,
            &name,
            None,
        )
        .await;
        let space = spaces.space(&info.id).await.unwrap();
        let uids = space
            .bash(
                "sleep 2; for p in /proc/[0-9]*; do \
                   c=$(tr '\\0' ' ' < $p/cmdline); \
                   case \"$c\" in *'nginx: master'*) grep ^Uid: $p/status;; esac; done; id -u",
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert!(uids.stdout.contains("Uid:\t101\t"), "{}", uids.stdout);
        spaces.delete(&info.id).await.unwrap();
    }
}
