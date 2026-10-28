//! Hermetic tests of the Apple Containerization backend against a fake
//! `container` CLI (`support/fake_container.rs`).

#![cfg(all(unix, feature = "containerization"))]

#[path = "support/fake_container.rs"]
mod fake_container;

use cua_vmm::{ExecRequest, ImageSource, Runtime, StartSpec, Status, VmmError};
use fake_container::FakeContainer;

fn spec(name: &str) -> StartSpec {
    StartSpec::new(name, ImageSource::oci("ghcr.io/trycua/linux:24.04-slim"))
        .cpus(2)
        .memory_mb(2048)
        .port(3211)
        .env("CUA_ENV_TOKEN", "tok-0123456789")
}

#[tokio::test]
async fn a_space_runs_stops_resumes_and_is_deleted() {
    let f = FakeContainer::new();
    let rt = f.runtime();
    let inst = rt.start(&spec("sb-one")).await.unwrap();
    assert_eq!(inst.status, Status::Running);
    assert_eq!(inst.backend, cua_vmm::BackendKind::Containerization);
    // Reached at the guest's own address, port for port.
    assert_eq!(inst.endpoints.host_port(3211), Some(3211));
    assert_eq!(inst.endpoints.host, "192.168.64.9");

    let calls = f.calls();
    // The services were started with cua's own roots, once.
    let app_root = rt.config().root.join("data");
    let starts: Vec<_> = calls
        .iter()
        .filter(|c| c.starts_with("system start"))
        .collect();
    assert_eq!(starts.len(), 1, "{calls:#?}");
    assert!(
        starts[0].contains(&format!("--app-root {}", app_root.display())),
        "{}",
        starts[0]
    );
    assert!(
        starts[0].contains("--enable-kernel-install"),
        "{}",
        starts[0]
    );
    // Every call names cua's application root.
    assert!(
        calls
            .iter()
            .all(|c| c.ends_with(&format!("APP_ROOT={}", app_root.display()))),
        "{calls:#?}"
    );
    let run = calls.iter().find(|c| c.starts_with("run ")).unwrap();
    assert!(
        run.contains("--name sb-one --cpus 2 --memory 2048M"),
        "{run}"
    );
    assert!(!run.contains("--publish"), "{run}");
    assert!(run.contains("--label ai.cua.ports=3211"), "{run}");
    assert!(
        run.contains("--label cua.vmm.backend=containerization"),
        "{run}"
    );
    // The token went through an owner-only env file, never the command line,
    // and the file is gone.
    assert!(!run.contains("tok-0123456789"), "{run}");
    assert_eq!(
        f.file("c/sb-one.env").as_deref(),
        Some("CUA_ENV_TOKEN=tok-0123456789\n")
    );
    assert_eq!(f.file("c/sb-one.envmode").unwrap().trim(), "600");
    assert!(!rt.config().root.join("run").join("sb-one.env").exists());

    // Lookups.
    assert_eq!(rt.status("sb-one").await.unwrap(), Status::Running);
    let listed = rt.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "sb-one");
    let ep = rt.endpoints("sb-one").await.unwrap();
    assert_eq!(ep.host_port(3211), Some(3211));
    assert!(matches!(
        rt.status("nope").await,
        Err(VmmError::NotFound(_))
    ));

    // Commands, through `container exec`.
    let exec = rt.guest_exec("sb-one").await.unwrap().unwrap();
    let out = exec
        .exec(ExecRequest::sh("echo \"hi $X\"").env_var("X", "there"))
        .await
        .unwrap();
    assert_eq!(out.stdout_str(), "hi there\n");
    assert_eq!(
        exec.exec(ExecRequest::sh("exit 3"))
            .await
            .unwrap()
            .exit_code,
        3
    );

    // The guest's own view of its listeners.
    assert_eq!(
        rt.guest_tcp_listening("sb-one", 3211).await.unwrap(),
        Some(false)
    );
    f.touch("c/sb-one.listen", "");
    assert_eq!(
        rt.guest_tcp_listening("sb-one", 3211).await.unwrap(),
        Some(true)
    );

    // Starting a running Space again is a no-op.
    rt.start(&spec("sb-one")).await.unwrap();
    assert_eq!(
        f.calls().iter().filter(|c| c.starts_with("run ")).count(),
        1
    );

    // No pause: suspending stops it, its disk kept.
    rt.suspend("sb-one").await.unwrap();
    assert_eq!(rt.status("sb-one").await.unwrap(), Status::Stopped);
    rt.resume("sb-one").await.unwrap();
    rt.stop("sb-one").await.unwrap();
    assert_eq!(rt.status("sb-one").await.unwrap(), Status::Stopped);
    rt.stop("sb-one").await.unwrap();
    let back = rt.resume("sb-one").await.unwrap();
    assert_eq!(back.status, Status::Running);
    assert_eq!(back.endpoints.host_port(3211), Some(3211));

    rt.delete("sb-one").await.unwrap();
    assert!(matches!(
        rt.status("sb-one").await,
        Err(VmmError::NotFound(_))
    ));
    assert!(matches!(
        rt.delete("sb-one").await,
        Err(VmmError::NotFound(_))
    ));
    // cua never stops the services.
    assert!(!f.calls().iter().any(|c| c.contains("SYSTEM STOP")));
}

#[tokio::test]
async fn the_image_command_replaces_entrypoint_and_cmd() {
    let f = FakeContainer::new();
    let rt = f.runtime();
    let mut s = StartSpec::new("sb-cmd", ImageSource::oci("alpine"));
    s.command = Some(vec!["/bin/sleep".into(), "100".into()]);
    rt.start(&s).await.unwrap();
    let calls = f.calls();
    let run = calls.iter().find(|c| c.starts_with("run ")).unwrap();
    assert!(
        run.contains("--entrypoint /bin/sleep alpine 100 |"),
        "{run}"
    );
    assert!(!run.contains("--env-file"), "{run}");
}

#[tokio::test]
async fn another_install_of_container_is_left_alone() {
    // Its services are running: cua neither uses nor stops them.
    let f = FakeContainer::new();
    f.touch(
        "services",
        "/Users/someone/Library/Application Support/com.apple.container",
    );
    let rt = f.runtime();
    let e = rt.start(&spec("sb-x")).await.unwrap_err().to_string();
    assert!(e.contains("another install"), "{e}");
    assert!(e.contains("container system stop"), "{e}");
    // Lookups do not see its containers either.
    assert!(rt.list().await.unwrap().is_empty());
    assert!(matches!(
        rt.status("sb-x").await,
        Err(VmmError::NotFound(_))
    ));
    let calls = f.calls();
    assert!(
        calls.iter().all(|c| c.starts_with("system status")),
        "{calls:#?}"
    );
    let r = rt.report().await;
    assert_eq!(r.services, "other");
}

#[tokio::test]
async fn a_start_that_launchd_ignored_is_caught() {
    // `system start` succeeds while launchd keeps another install's service.
    let f = FakeContainer::new();
    f.touch("hijack", "/elsewhere/data");
    let rt = f.runtime();
    let e = rt.start(&spec("sb-y")).await.unwrap_err().to_string();
    assert!(e.contains("another install"), "{e}");
    assert!(!f.calls().iter().any(|c| c.starts_with("run ")));
}

#[tokio::test]
async fn lookups_never_start_the_services() {
    let f = FakeContainer::new();
    let rt = f.runtime();
    assert!(matches!(
        rt.status("sb-z").await,
        Err(VmmError::NotFound(_))
    ));
    assert!(rt.list().await.unwrap().is_empty());
    assert_eq!(rt.report().await.services, "stopped");
    assert!(!f.exists("services"));
    assert!(!f.calls().iter().any(|c| c.starts_with("system start")));
}

#[tokio::test]
async fn a_guest_that_stops_while_booting_fails_with_its_log() {
    let f = FakeContainer::new();
    f.touch("boot_fails", "");
    let rt = f.runtime();
    let e = rt.start(&spec("sb-boot")).await.unwrap_err().to_string();
    assert!(e.contains("stopped while starting"), "{e}");
    assert!(e.contains("boot failed"), "{e}");
}

#[tokio::test]
async fn what_containerization_cannot_do_is_refused() {
    let f = FakeContainer::new();
    let rt = f.runtime();
    for (s, what) in [
        (spec("bad name!"), "not a valid"),
        (
            {
                let mut s = spec("sb-net");
                s.restrict_network = true;
                s
            },
            "network",
        ),
        (
            {
                let mut s = spec("sb-disk");
                s.disk_size_gb = Some(40);
                s
            },
            "disk size",
        ),
        (
            {
                let mut s = spec("sb-gpu");
                s.gpu = Some("nvidia".into());
                s
            },
            "GPU",
        ),
        (
            spec("sb-mac").os(cua_vmm::GuestOs::Macos),
            "other than Linux",
        ),
    ] {
        let e = rt.start(&s).await.unwrap_err().to_string();
        assert!(e.contains(what), "{what}: {e}");
    }
    assert!(matches!(
        rt.checkpoint("sb", "ck").await,
        Err(VmmError::Unsupported { .. })
    ));
    assert!(matches!(
        rt.fork("sb", "sb2").await,
        Err(VmmError::Unsupported { .. })
    ));
    assert!(!f.calls().iter().any(|c| c.starts_with("run ")));
}

#[tokio::test]
async fn concurrent_spaces_start_the_services_once() {
    let f = FakeContainer::new();
    let rt = std::sync::Arc::new(f.runtime());
    let mut tasks = Vec::new();
    for i in 0..6 {
        let rt = rt.clone();
        tasks.push(tokio::spawn(async move {
            rt.start(&spec(&format!("sb-c{i}"))).await.unwrap()
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap().status, Status::Running);
    }
    assert_eq!(rt.list().await.unwrap().len(), 6);
    let starts = f
        .calls()
        .iter()
        .filter(|c| c.starts_with("system start"))
        .count();
    assert_eq!(starts, 1, "the services start once");
}

/// A warm-pool member is claimed under a Space's name: `container` cannot
/// rename, so the name is an alias every call resolves; the member's
/// container keeps running untouched and its hostname becomes the Space's.
#[tokio::test]
async fn a_pool_member_is_claimed_under_the_spaces_name() {
    let f = FakeContainer::new();
    let rt = f.runtime();
    let mut member = spec("pool-a1");
    member.pool_member = true;
    rt.start(&member).await.unwrap();
    rt.start(&spec("other")).await.unwrap();

    // A name in use is refused.
    let err = rt
        .claim("pool-a1", "other", &cua_vmm::ClaimIdentity::default())
        .await
        .unwrap_err();
    assert!(matches!(err, VmmError::AlreadyExists(_)), "{err}");

    let inst = rt
        .claim("pool-a1", "space-1", &cua_vmm::ClaimIdentity::default())
        .await
        .unwrap();
    assert_eq!(inst.name, "space-1");
    assert_eq!(inst.status, Status::Running);
    assert_eq!(inst.endpoints.container_id.as_deref(), Some("pool-a1"));
    assert_eq!(rt.status("space-1").await.unwrap(), Status::Running);
    assert!(matches!(
        rt.status("pool-a1").await,
        Ok(Status::Running) // the container id still resolves to itself
    ));
    let mut names: Vec<_> = rt
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    names.sort();
    assert_eq!(names, ["other", "space-1"]);
    // The guest got the Space's hostname (set in the background), through
    // the member's container.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.file("c/pool-a1.hostname").is_none() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        f.file("c/pool-a1.hostname").unwrap().contains("space-1"),
        "{:?}",
        f.file("c/pool-a1.hostname")
    );
    // Commands and lifecycle go to the member's container.
    let exec = rt.guest_exec("space-1").await.unwrap().unwrap();
    let out = exec.exec(ExecRequest::sh("echo hi")).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    rt.stop("space-1").await.unwrap();
    assert_eq!(rt.status("space-1").await.unwrap(), Status::Stopped);
    rt.delete("space-1").await.unwrap();
    assert!(!f.exists("c/pool-a1.image"));
    assert!(matches!(
        rt.status("space-1").await,
        Err(VmmError::NotFound(_))
    ));
    // The alias went with it: a new container may take the name.
    rt.start(&spec("space-1")).await.unwrap();
    assert_eq!(rt.status("space-1").await.unwrap(), Status::Running);
}
