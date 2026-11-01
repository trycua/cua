// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Commands and files as another user on a running guest (a Space under
//! gVisor, runc or a VM): the agent must switch users itself, with no
//! `sudo`. With `CUA_ENV_TEST_TARGET=http://host:port` and
//! `CUA_ENV_TEST_TOKEN` it drives that guest through the client and prints
//! the evidence; `CUA_TEST_FS_AS_USER` (default `alice`) is the other
//! account, which must exist in the guest, and `CUA_TEST_FS_AS_GROUP`
//! (default `devs`) a supplementary group of it. Without a target the test
//! does nothing (the hermetic lane is `process_run_as.rs`).
#![cfg(unix)]

use cua_spacesd_client::{
    Command, ConnectOptions, FileAccess, RunAsMode, SpacesdClient, TransportPreference,
    UploadOptions,
};

async fn sh(c: &SpacesdClient, user: Option<&str>, line: &str) -> String {
    let mut cmd = Command::shell(line);
    if let Some(u) = user {
        cmd = cmd.user(u);
    }
    let out = c.run(cmd).await.unwrap();
    let text = out.stdout_str().trim().to_owned();
    println!(
        "$ {}{line}\n{text}",
        user.map(|u| format!("[as {u}] ")).unwrap_or_default()
    );
    assert_eq!(out.status.code, Some(0), "{line}: {}", out.stderr_str());
    text
}

#[tokio::test]
async fn the_guest_agent_switches_users_without_sudo() {
    let Ok(url) = std::env::var("CUA_ENV_TEST_TARGET") else {
        return;
    };
    let token = std::env::var("CUA_ENV_TEST_TOKEN").unwrap_or_default();
    let other = std::env::var("CUA_TEST_FS_AS_USER").unwrap_or_else(|_| "alice".into());
    let group = std::env::var("CUA_TEST_FS_AS_GROUP").unwrap_or_else(|_| "devs".into());
    let c = SpacesdClient::connect(
        ConnectOptions::parse(&url)
            .unwrap()
            .transport(TransportPreference::Native)
            .probe(false)
            .token(token),
    )
    .await
    .unwrap();

    let mode = c.run_as_mode().await;
    println!("process.run_as mode = {mode:?}");
    assert!(
        mode.is_native(),
        "the agent does not switch users itself: {mode:?}"
    );
    assert_eq!(mode, RunAsMode::Capabilities);

    // Whether the guest has a usable sudo is its own business (the Linux
    // images keep one for `cua`); the switch below must not depend on it.
    // `CUA_TEST_EXPECT_NO_SUDO=1` (a guest image without sudo, or a
    // non-sudoer agent) asserts that the agent has none to fall back on.
    let sudo = sh(&c, None, "if sudo -n true 2>/dev/null; then echo 'sudo: usable'; else echo 'sudo: absent or refused'; fi").await;
    if std::env::var_os("CUA_TEST_EXPECT_NO_SUDO").is_some() {
        assert_eq!(sudo, "sudo: absent or refused");
    }
    let handle = c.spawn(Command::shell("id -u").user(&other)).await.unwrap();
    assert!(!handle.via_sudo(), "the command went through sudo");
    handle.wait().await.unwrap();
    let default_user = sh(&c, None, "id -un").await;
    assert_ne!(default_user, other);
    let uid = sh(&c, Some(&other), "id -u").await;
    assert_ne!(uid, sh(&c, None, "id -u").await);
    sh(&c, Some(&other), "id").await;
    assert!(sh(&c, Some(&other), "id -Gn")
        .await
        .split_whitespace()
        .any(|g| g == group));
    assert!(!sh(&c, None, "id -Gn")
        .await
        .split_whitespace()
        .any(|g| g == group));
    assert_eq!(
        sh(&c, Some(&other), "echo $HOME $USER $LOGNAME").await,
        format!("/home/{other} {other} {other}")
    );
    assert_eq!(sh(&c, Some(&other), "pwd").await, format!("/home/{other}"));
    for f in ["CapEff", "CapPrm", "CapInh", "CapAmb"] {
        let line = sh(&c, Some(&other), &format!("grep ^{f}: /proc/self/status")).await;
        assert!(line.ends_with("0000000000000000"), "{line}");
    }
    assert_eq!(sh(&c, Some("root"), "id -u").await, "0");

    // Files: owner, group and mode are the other user's.
    let path = format!("/home/{other}/r3-evidence.txt");
    c.upload(
        &path,
        bytes::Bytes::from_static(b"evidence"),
        UploadOptions {
            run_as: Some(other.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let owner = sh(&c, Some(&other), &format!("stat -c '%U:%G %a' {path}")).await;
    assert!(owner.starts_with(&format!("{other}:")), "{owner}");
    assert_eq!(c.download_as(&path, &other).await, b"evidence".to_vec());
    sh(&c, Some(&other), &format!("rm -f {path}")).await;
}

trait DownloadAs {
    async fn download_as(&self, path: &str, user: &str) -> Vec<u8>;
}

impl DownloadAs for SpacesdClient {
    async fn download_as(&self, path: &str, user: &str) -> Vec<u8> {
        let st = self
            .stat_with(path, false, &FileAccess::user(user))
            .await
            .unwrap();
        assert_eq!(st.owner, user);
        let mut sink = Vec::new();
        self.download_with(
            path,
            cua_spacesd_client::DownloadOptions {
                run_as: Some(user.to_owned()),
                ..Default::default()
            },
            &mut sink,
        )
        .await
        .unwrap();
        sink
    }
}
