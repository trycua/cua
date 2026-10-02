//! Shared helpers for the local-runtime e2e suite.
//!
//! Every test is gated by an env var so `cargo test` stays hermetic:
//!
//! | env | what runs |
//! |---|---|
//! | `CUA_E2E_DOCKER=1` | container backend against the local engine (gVisor if present) |
//! | `CUA_E2E_QEMU=1` | QEMU backend: arm64 cloud image under hvf/kvm, SSH exec, checkpoint |
//! | `CUA_E2E_QEMU_X86=1` | QEMU backend: Fleet x86_64 containerDisk under TCG (slow) |
//! | `CUA_E2E_LUME=1` | Lume backend against `lume serve` |
//! | `CUA_E2E_IMAGE=1` | cua-image registry pulls + local builder |
//! | `CUA_E2E_FLEET_CATALOG=1` | every Fleet catalog image through the SDK local provider (`tests/fleet_catalog.rs`) |

use std::time::Instant;

/// `true` when `var` is set to `1`; prints a skip line otherwise.
pub fn gated(var: &str) -> bool {
    let on = std::env::var(var).map(|v| v == "1").unwrap_or(false);
    if !on {
        eprintln!("skipping: set {var}=1 to run");
    }
    on
}

/// Initialise tracing once (RUST_LOG controls verbosity).
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,cua_vmm=debug,cua_image=debug".into()),
        )
        .with_test_writer()
        .try_init();
}

/// Prints `label: <secs>` timings so the suite output doubles as a report.
pub struct Timer(Instant);

impl Timer {
    pub fn start() -> Self {
        Self(Instant::now())
    }
    pub fn lap(&self, label: &str) -> f64 {
        let s = self.0.elapsed().as_secs_f64();
        eprintln!("TIMING {label}: {s:.1}s");
        s
    }
}

/// Unique-ish suffix for resource names in this run.
pub fn run_id() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            % 0xffffff
    )
}
