// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Prints the encoder/decoder probe as JSON. Also serves as the isolated
//! probe child (`CUA_CODEC_PROBE_CHILD=<backend>`).
//!
//! Usage: `cua-codec-probe [--isolated]`

use std::time::Duration;

use cua_media_codec::probe::{self, Isolation};

fn main() {
    probe::maybe_run_probe_child();
    let isolated = std::env::args().any(|a| a == "--isolated");
    let isolation = if isolated {
        Isolation::current_exe(Duration::from_secs(30)).expect("current exe")
    } else {
        Isolation::InProcess
    };
    let encoders = probe::probe_with(&isolation);
    let decoders = probe::probe_decoders();
    let out =
        serde_json::json!({ "isolated": isolated, "encoders": encoders, "decoders": decoders });
    println!("{}", serde_json::to_string_pretty(&out).expect("json"));
}
