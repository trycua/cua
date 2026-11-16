// SPDX-License-Identifier: MIT
//! Opt-in: a delete removes a Space's resolv.conf only when it was given one
//! (#6917). Under the `CUA_SECURE_DEFAULT=0` override a Space has none, so
//! its delete must not start the privileged `cua-runsc-helper` to remove one;
//! under the secure default the file is written at create and removed by the
//! delete.
//!
//! Needs a Docker engine with no `cua-runsc-helper` container yet (another
//! Space may rely on one); the secure half leaves the helper running, as any
//! secure Space does. Use a throwaway `CUA_HOME`:
//!
//! ```text
//! CUA_HOME=$(mktemp -d) CUA_TEST_ENGINE=unix:///path/docker.sock \
//!   cargo test -p cua-vmm --test container_delete_live -- --ignored --nocapture
//! ```
#![cfg(feature = "container")]

use std::process::Command;

use cua_vmm::container::snapshot::HELPER_NAME;
use cua_vmm::container::{ContainerConfig, ContainerRuntime};
use cua_vmm::network::{Mode, hold_mode};
use cua_vmm::{ImageSource, Runtime, StartSpec};

/// `docker --host <engine> <args>` succeeded.
fn docker(engine: &str, args: &[&str]) -> bool {
    Command::new("docker")
        .args(["--host", engine])
        .args(args)
        .output()
        .expect("docker CLI")
        .status
        .success()
}

/// Whether the engine has a container called `name`.
fn exists(engine: &str, name: &str) -> bool {
    docker(engine, &["container", "inspect", name])
}

/// Whether the engine host holds a resolv.conf for container `name`.
fn has_resolv(engine: &str, name: &str) -> bool {
    let path = format!("/host/var/lib/cua/resolv/{name}.conf");
    docker(engine, &["exec", HELPER_NAME, "test", "-f", &path])
}

fn spec(name: &str, image: &str) -> StartSpec {
    let mut s = StartSpec::new(
        name,
        ImageSource::Oci {
            reference: image.into(),
        },
    )
    .command(["sleep", "300"]);
    s.container_runtime = Some("runc".into());
    s
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a Docker engine"]
async fn a_delete_removes_a_resolv_conf_only_when_it_wrote_one() {
    let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
    assert!(!home.ends_with("/.cua"), "never the real cua home");
    let engine = std::env::var("CUA_TEST_ENGINE").expect("CUA_TEST_ENGINE=<docker uri>");
    let image = std::env::var("CUA_TEST_IMAGE").unwrap_or_else(|_| "alpine:3.20".into());
    assert!(
        !exists(&engine, HELPER_NAME),
        "remove {HELPER_NAME} first (only when no Space uses it)"
    );
    let rt = ContainerRuntime::connect(ContainerConfig {
        endpoint: Some(engine.clone()),
        ..Default::default()
    })
    .await
    .unwrap();

    // The open override: no resolv.conf, so no helper at create or delete.
    let open = hold_mode(Mode::Legacy);
    let name = "delete-open";
    let _ = rt.delete(name).await;
    rt.start(&spec(name, &image)).await.unwrap();
    assert!(!exists(&engine, HELPER_NAME), "the start made the helper");
    rt.delete(name).await.unwrap();
    assert!(!exists(&engine, name), "the Space is gone");
    assert!(!exists(&engine, HELPER_NAME), "the delete made the helper");
    drop(open);

    // The secure default: the file is written at create and removed by the delete.
    let _secure = hold_mode(Mode::Secure);
    let name = "delete-secure";
    let _ = rt.delete(name).await;
    rt.start(&spec(name, &image)).await.unwrap();
    assert!(has_resolv(&engine, name), "the start wrote no resolv.conf");
    rt.delete(name).await.unwrap();
    assert!(!exists(&engine, name), "the Space is gone");
    assert!(
        !has_resolv(&engine, name),
        "the delete left its resolv.conf"
    );
}
