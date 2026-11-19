// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The real binary answers as a hardware encoder probe child: started with
//! `CUA_CODEC_PROBE_CHILD`, it probes that one backend, prints the result
//! and exits instead of serving. The daemon re-runs itself this way so a GPU
//! driver that hangs during the probe hangs a child it kills, not the
//! daemon.
#![cfg(not(cua_headless))]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cua_media_codec::probe::{EncoderInfo, PROBE_CHILD_ENV};
use cua_media_codec::Backend;

#[test]
fn the_binary_answers_as_a_probe_child() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cua-spacesd"))
        .env(PROBE_CHILD_ENV, "openh264")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cua-spacesd did not exit as a probe child (it started serving)");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "probe child exited with {status}");
    let mut output = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut output).unwrap();
    let info: EncoderInfo = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(info.backend, Backend::OpenH264);
}
