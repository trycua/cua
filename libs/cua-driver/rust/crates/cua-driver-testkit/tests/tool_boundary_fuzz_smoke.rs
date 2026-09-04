//! Bounded, deterministic runs of the tool-call boundary fuzz targets.
//!
//! The target bodies live in `cua_driver_testkit::boundary_fuzz` and are the
//! same functions the detached `fuzz/` crate hands to libFuzzer. Running them
//! here over the checked-in seed corpus plus a seeded pseudo-random byte
//! stream keeps them compiling on the pinned stable toolchain and turns every
//! past finding into a regression test, without needing `cargo fuzz`.

use cua_driver_testkit::boundary_fuzz::{self, TARGETS};
use std::path::{Path, PathBuf};

const RANDOM_CASES: usize = 1_500;
const MAX_RANDOM_LEN: usize = 4096;

fn corpus_dir(target: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target)
}

/// Small xorshift64* stream so every run explores the same inputs.
struct Xorshift(u64);

impl Xorshift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u64() as u8).collect()
    }
}

fn run_seed_corpus(target: &str, body: fn(&[u8])) -> usize {
    let dir = corpus_dir(target);
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("seed corpus {} unreadable: {error}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();
    for path in &entries {
        let data = std::fs::read(path).unwrap();
        body(&data);
    }
    entries.len()
}

fn run_random(target: &str, body: fn(&[u8])) {
    // Seed from the target name so each target walks a distinct stream.
    let seed = target.bytes().fold(0x9E37_79B9_7F4A_7C15_u64, |acc, byte| {
        (acc ^ u64::from(byte)).wrapping_mul(0x100_0000_01B3)
    });
    let mut rng = Xorshift(seed | 1);
    for case in 0..RANDOM_CASES {
        let len = (rng.next_u64() as usize) % (MAX_RANDOM_LEN + 1);
        let data = rng.bytes(len);
        // Mix in structurally plausible prefixes so random bytes still reach
        // the deeper parsing layers some of the time.
        let data = match case % 4 {
            0 => data,
            1 => [
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":".as_slice(),
                &data,
            ]
            .concat(),
            2 => [
                b"{\"session\":\"s\",\"pid\":7,\"window_id\":9,".as_slice(),
                &data,
            ]
            .concat(),
            _ => [b"{".as_slice(), &data].concat(),
        };
        body(&data);
    }
}

#[test]
fn every_target_has_a_seed_corpus() {
    for (name, body) in TARGETS {
        let seeds = run_seed_corpus(name, *body);
        assert!(seeds > 0, "fuzz target {name} has no seed corpus");
    }
}

#[test]
fn mcp_request_survives_random_bytes() {
    run_random("mcp_request", boundary_fuzz::mcp_request);
}

#[test]
fn tool_arguments_survive_random_bytes() {
    run_random("tool_arguments", boundary_fuzz::tool_arguments);
}

#[test]
fn typed_input_json_survives_random_bytes() {
    run_random("typed_input_json", boundary_fuzz::typed_input_json);
}

#[test]
fn registry_invoke_survives_random_bytes() {
    run_random("registry_invoke", boundary_fuzz::registry_invoke);
}

#[test]
fn targets_table_matches_fuzz_target_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/fuzz_targets");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    let mut names: Vec<String> = TARGETS.iter().map(|(name, _)| name.to_string()).collect();
    names.sort();
    assert_eq!(
        files, names,
        "fuzz/fuzz_targets must mirror boundary_fuzz::TARGETS"
    );
}
