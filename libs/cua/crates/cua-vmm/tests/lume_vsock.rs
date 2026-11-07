//! A macOS guest's published ports go over vsock when Lume forwards them:
//! the start asks for them, waits for its services on the host loopback
//! ports Lume reports, and hands those out, so neither macOS Local Network
//! privacy nor a VPN's routes stand between this Mac and its own VM.
//! Hermetic: a fake `lume serve` and a loopback listener as the guest.

#![cfg(feature = "lume")]

#[allow(dead_code)]
#[path = "support/fake_lume.rs"]
mod fake_lume;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_vmm::lume::{VSOCK_HOST, base_vm_name};
use cua_vmm::{GuestOs, ImageSource, Probe, Runtime, StartSpec};
use fake_lume::{FakeLume, serve};
use serde_json::{Value, json};

const REF: &str = "ghcr.io/trycua/macos:26";

/// A guest service as Lume's forward presents it: a loopback port that
/// accepts and keeps connections open.
fn guest_service() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for c in l.incoming().flatten() {
            held.push(c);
        }
    });
    port
}

async fn fake(forwards: Option<Value>) -> Arc<Mutex<FakeLume>> {
    let mut st = FakeLume {
        vsock_forwards: forwards,
        ..Default::default()
    };
    let base = base_vm_name(REF);
    st.vms.insert(
        base.clone(),
        json!({
            "name": base, "os": "macOS", "cpuCount": 4, "memorySize": 8u64 << 30,
            "diskSize": { "allocated": 1u64 << 30, "total": 150u64 << 30 },
            "display": "1024x768", "status": "stopped"
        }),
    );
    Arc::new(Mutex::new(st))
}

fn spec(name: &str) -> StartSpec {
    StartSpec::new(
        name,
        ImageSource::Oci {
            reference: REF.into(),
        },
    )
    .os(GuestOs::Macos)
    .port(3211)
    .probe(Probe::tcp(3211))
    .ready_timeout(Duration::from_secs(20))
}

#[tokio::test]
async fn a_macos_start_uses_the_vsock_forwards_lume_reports() {
    // About the vsock path, not the network gateway Secure Spaces (on by default) puts a VM behind.
    let _open = cua_vmm::network::hold_mode(cua_vmm::network::Mode::Legacy);
    let host_port = guest_service();
    let state = fake(Some(json!({ "3211": host_port }))).await;
    let rt = fake_lume::runtime(serve(state.clone()).await);

    // The VM's address (TEST-NET-1) never answers; the vsock path does.
    let inst = rt.start(&spec("cua-e2e-vsock")).await.unwrap();
    assert_eq!(inst.endpoints.host, VSOCK_HOST);
    assert_eq!(inst.endpoints.host_port(3211), Some(host_port));
    assert_eq!(
        inst.endpoints.host_port(22),
        None,
        "never the host's own sshd"
    );

    let run = {
        let st = state.lock().unwrap();
        st.calls
            .iter()
            .find(|(m, p, _)| m == "POST" && p.ends_with("/run"))
            .map(|(_, _, body)| body.clone())
            .unwrap()
    };
    assert_eq!(run["forwards"], json!([3211]));

    // Another process reattaching by name sees the same path.
    let again = rt.endpoints("cua-e2e-vsock").await.unwrap();
    assert_eq!(again.host, VSOCK_HOST);
    assert_eq!(again.host_port(3211), Some(host_port));
}

#[tokio::test]
async fn without_vsock_a_start_keeps_the_vm_address() {
    let _open = cua_vmm::network::hold_mode(cua_vmm::network::Mode::Legacy);
    let state = fake(None).await;
    let rt = fake_lume::runtime(serve(state.clone()).await);
    let mut s = spec("cua-e2e-no-vsock");
    s.probes.clear();
    let inst = rt.start(&s).await.unwrap();
    assert_eq!(inst.endpoints.host, "192.0.2.10");
    assert_eq!(inst.endpoints.host_port(3211), Some(3211));
}
