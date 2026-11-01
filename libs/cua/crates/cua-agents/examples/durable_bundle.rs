// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Writes the durable driver `durable_driver_spec` builds for a harness into a directory, ready to
//! run with `sh driver.sh` under the session env contract. For the crash campaign
//! (`CRASH_MODE=acp`, libs/cua/typescript/integrations/pi-durable/test/crash/acp.mjs) and for
//! looking at what a re-driver would launch.
//!
//! `cargo run -p cua-agents --example durable_bundle -- <harness> <out_dir> <prompt_file>
//!  [--endpoint URL] [--model M] [--session NAME] [--no-guard] [--no-install] [--policy rerun-safe]
//!  [--cwd DIR] [--max-agent-restarts N] [--env NAME=VALUE]...`
//!
//! Prints a JSON object `{argv, env, state_dir, label, bundle_bytes}` on stdout (the env holds
//! only what the options name: no key is read from the process env).

use cua_agents::RunOptions;
use cua_agents::durable::{DurableOptions, Policy, durable_driver_spec};
use cua_agents::harness::Endpoint;
use std::io::Read;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(harness), Some(out), Some(prompt_file)) = (args.next(), args.next(), args.next())
    else {
        eprintln!(
            "usage: durable_bundle <harness> <out_dir> <prompt_file> [--endpoint URL] [--model M] [--session NAME] [--no-guard] [--no-install] [--policy rerun-safe] [--cwd DIR] [--max-agent-restarts N] [--env NAME=VALUE]..."
        );
        std::process::exit(2);
    };
    let prompt = std::fs::read_to_string(&prompt_file).expect("prompt file");
    let mut opts = RunOptions::default();
    let mut d = DurableOptions::default();
    while let Some(a) = args.next() {
        let mut val = || args.next().expect("a value");
        match a.as_str() {
            "--endpoint" => {
                opts.endpoint = Some(Endpoint {
                    base_url: val(),
                    ..Default::default()
                })
            }
            "--model" => opts.model = Some(val()),
            "--session" => d.session = Some(val()),
            "--cwd" => opts.cwd = Some(val()),
            "--env" => {
                let kv = val();
                let (k, v) = kv.split_once('=').expect("NAME=VALUE");
                opts.env.insert(k.into(), v.into());
            }
            "--no-guard" => d.guard = false,
            "--no-install" => opts.install = Some(false),
            "--policy" => {
                d.policy = if val() == "rerun-safe" {
                    Policy::RerunSafe
                } else {
                    Policy::AskModel
                }
            }
            "--max-agent-restarts" => d.max_agent_restarts = val().parse().expect("a number"),
            other => panic!("unknown argument {other}"),
        }
    }
    opts.durable = Some(d);
    let spec = durable_driver_spec(&harness, &prompt, &opts).expect("driver spec");
    use base64::Engine;
    let gz = base64::engine::general_purpose::STANDARD
        .decode(&spec.bundle)
        .expect("bundle");
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut tar)
        .expect("gunzip");
    std::fs::create_dir_all(&out).expect("out dir");
    tar::Archive::new(&tar[..]).unpack(&out).expect("unpack");
    println!(
        "{}",
        serde_json::json!({
            "argv": spec.argv, "env": spec.env, "state_dir": spec.state_dir,
            "label": spec.label, "bundle_bytes": spec.bundle_bytes,
        })
    );
}
