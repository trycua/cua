//! Sandbox and pool sizes: the SDK refuses only nonsense (Fleet's absolute
//! ceiling) and leaves the per-account limits to Fleet, whose admission
//! denial reaches the caller as `Error::AdmissionDenied` with Fleet's own
//! message.

use cua_fleet::{
    Error, PoolOptions, RuntimeKind, SandboxSpec,
    testing::{FakeFleet, SIZE_LIMIT_MESSAGE},
};

const IMAGE: &str = "ghcr.io/trycua/cua-e2e-plain:1";

fn sized(cpu: u32, memory_mb: u32) -> SandboxSpec {
    SandboxSpec {
        cpu: Some(cpu),
        memory_mb: Some(memory_mb),
        ..SandboxSpec::new(IMAGE)
    }
}

fn gvisor() -> PoolOptions {
    PoolOptions {
        runtime: Some(RuntimeKind::Gvisor),
        ..Default::default()
    }
}

/// Sizes exempt accounts run today are written as asked.
#[tokio::test]
async fn sizes_above_the_everyday_range_reach_fleet() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    for (name, cpu, mem) in [
        ("cua-e2e-big16", 16, 64 * 1024),
        ("cua-e2e-big64", 64, 100 * 1024),
    ] {
        fleet
            .apply(name, &sized(cpu, mem), &gvisor())
            .await
            .unwrap();
        let t = fake.object("template", name, name).unwrap();
        assert_eq!(t["spec"]["vmTemplate"]["cpuCores"], cpu);
        assert_eq!(t["spec"]["vmTemplate"]["memory"], format!("{mem}Mi"));
    }
}

/// Nonsense never leaves the SDK: nothing is written.
#[tokio::test]
async fn nonsense_sizes_are_refused_before_any_write() {
    let fake = FakeFleet::new();
    let fleet = fake.client();
    for spec in [
        sized(0, 4096),
        sized(65, 4096),
        sized(4, 256),
        sized(4, u32::MAX),
    ] {
        let e = fleet
            .apply("cua-e2e-nonsense", &spec, &gvisor())
            .await
            .unwrap_err();
        assert!(matches!(e, Error::InvalidArgument(_)), "{e:?}");
    }
    let e = fleet
        .apply(
            "cua-e2e-nonsense",
            &sized(4, 4096),
            &PoolOptions {
                max_pool_size: Some(51),
                ..gvisor()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Error::InvalidArgument(m) if m.contains("at most 50")),
        "{e:?}"
    );
    assert!(fake.requests().iter().all(|r| r.method == "GET"));
    assert!(!fake.namespace_exists("cua-e2e-nonsense"));
}

/// An account under Fleet's size cap gets Fleet's 403 as a typed error
/// carrying Fleet's message, not a generic status or the pool-name hint.
#[tokio::test]
async fn fleet_size_admission_denial_surfaces_its_message() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().size_cap = Some((8, 32 * 1024));
    let fleet = fake.client();

    // Create: the template POST is refused and the pool rolled back.
    let e = fleet
        .apply("cua-e2e-capped", &sized(16, 64 * 1024), &gvisor())
        .await
        .unwrap_err();
    match &e {
        Error::AdmissionDenied {
            operation,
            status,
            message,
        } => {
            assert_eq!(operation, "create template");
            assert_eq!(*status, 403);
            assert_eq!(message, SIZE_LIMIT_MESSAGE);
        }
        other => panic!("expected AdmissionDenied, got {other:?}"),
    }
    let shown = e.to_string();
    assert!(
        shown.contains("sandbox size is over the Fleet limits"),
        "{shown}"
    );
    assert!(!shown.contains("pool name"), "{shown}");
    assert!(!e.is_not_found());
    assert!(!fake.exists("pool", "cua-e2e-capped", "cua-e2e-capped"));

    // Update: a pool within the cap, then resized over it (PATCH).
    fleet
        .apply("cua-e2e-capped", &sized(8, 32 * 1024), &gvisor())
        .await
        .unwrap();
    let e = fleet
        .apply("cua-e2e-capped", &sized(9, 4096), &gvisor())
        .await
        .unwrap_err();
    assert!(
        matches!(&e, Error::AdmissionDenied { operation, message, .. }
            if operation == "update template" && message == SIZE_LIMIT_MESSAGE),
        "{e:?}"
    );
}
