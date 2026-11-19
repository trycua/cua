// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The SwiftUI Spaces app's bundle id (`com.trycua.spaces.macos`) is a Cua
//! identifier, and only a Cua-team, Apple-anchored, hardened signature of it
//! is first party. A real child process signed ad hoc with that identifier
//! (what `apps/cua-spaces-macos/scripts/build-app.sh` produces) connects to
//! a socket and is identified: the identifier is read from its signature,
//! and the production policy still refuses it.
#![cfg(target_os = "macos")]

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

use cua_keyvault::caller::{CUA_IDENTIFIERS, identify_peer};
use cua_keyvault::{Signing, TrustPolicy};

const SWIFTUI_ID: &str = "com.trycua.spaces.macos";

#[test]
fn swiftui_bundle_id_is_a_cua_identifier() {
    assert!(CUA_IDENTIFIERS.contains(&SWIFTUI_ID));
    let req = TrustPolicy::production().macos_requirement;
    let parsed: Result<security_framework::os::macos::code_signing::SecRequirement, _> =
        req.parse();
    assert!(parsed.is_ok(), "the production requirement compiles: {req}");
}

/// Builds a tiny client that connects to argv[1] and waits for stdin EOF,
/// signed ad hoc with `identifier` and the hardened runtime.
fn signed_client(dir: &std::path::Path, identifier: &str) -> Option<std::path::PathBuf> {
    let src = dir.join("client.c");
    std::fs::write(
        &src,
        r#"#include <sys/socket.h>
#include <sys/un.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  struct sockaddr_un a; memset(&a, 0, sizeof a); a.sun_family = AF_UNIX;
  strncpy(a.sun_path, argv[1], sizeof a.sun_path - 1);
  if (connect(fd, (struct sockaddr *)&a, sizeof a) != 0) return 2;
  char b; while (read(0, &b, 1) > 0) {}
  return 0;
}
"#,
    )
    .ok()?;
    let exe = dir.join("client");
    let cc = Command::new("cc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .status()
        .ok()?;
    if !cc.success() {
        return None;
    }
    let sign = Command::new("codesign")
        .args(["-s", "-", "-f", "-o", "runtime", "-i", identifier])
        .arg(&exe)
        .status()
        .ok()?;
    sign.success().then_some(exe)
}

#[test]
fn an_ad_hoc_swiftui_build_is_identified_but_not_first_party() {
    let dir = tempfile::tempdir().unwrap();
    let Some(exe) = signed_client(dir.path(), SWIFTUI_ID) else {
        eprintln!("skipped: no C compiler or codesign");
        return;
    };
    let sock = dir.path().join("kv.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let mut child = Command::new(&exe)
        .arg(&sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let (stream, _) = listener.accept().unwrap();

    let production = identify_peer(stream.as_raw_fd(), &TrustPolicy::production()).unwrap();
    match &production.signing {
        Signing::AdHoc { identifier, .. } => assert_eq!(identifier, SWIFTUI_ID),
        other => panic!("expected an ad hoc signature, got {other:?}"),
    }
    assert!(
        !production.first_party,
        "an ad hoc build must never satisfy the production requirement"
    );
    // The identifier alone (a test policy) matches, so the refusal above is
    // the anchor and team clause, not a misread identifier.
    let by_id = TrustPolicy::for_tests(format!("identifier \"{SWIFTUI_ID}\""));
    let tested = identify_peer(stream.as_raw_fd(), &by_id).unwrap();
    assert!(tested.first_party);

    drop(child.stdin.take());
    let _ = child.wait();
    let mut rest = Vec::new();
    let _ = (&stream).read_to_end(&mut rest);
}
