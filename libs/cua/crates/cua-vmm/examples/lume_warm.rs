//! Makes the warm states of a macOS image (`LumeRuntime::prepare_warm`):
//! its base booted to a listening cua-spacesd with no identity and saved,
//! twice (one per macOS guest a host may run). A later create of that image
//! with the same CPU and memory resumes a copy-on-write clone of one
//! instead of booting macOS.
//!
//!     cargo run -p cua-vmm --example lume_warm -- REF [--cpus N] [--memory-mb M] [--settle-secs S]
//!
//! Uses the `lume serve` of `LUME_API` (default `http://127.0.0.1:7777`, or
//! `http://127.0.0.1:7787` on the Local channel) and
//! the `CUA_HOME` the daemon uses. Prints one JSON line with the states made
//! and how long it took.

use std::time::{Duration, Instant};

use cua_vmm::lume::LumeRuntime;
use cua_vmm::{GuestOs, ImageSource, Probe, StartSpec};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(reference) = args.next() else {
        eprintln!("usage: lume_warm REF [--cpus N] [--memory-mb M] [--settle-secs S]");
        std::process::exit(2);
    };
    let (mut cpus, mut memory_mb, mut settle) = (4u32, 8192u64, 5u64);
    while let Some(k) = args.next() {
        let v = args.next().unwrap_or_default();
        let n: u64 = v.parse().unwrap_or_else(|_| {
            eprintln!("{k} needs a number");
            std::process::exit(2)
        });
        match k.as_str() {
            "--cpus" => cpus = n as u32,
            "--memory-mb" => memory_mb = n,
            "--settle-secs" => settle = n,
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2)
            }
        }
    }
    let spec = StartSpec::new("warm", ImageSource::oci(reference.as_str()))
        .os(GuestOs::Macos)
        .cpus(cpus)
        .memory_mb(memory_mb)
        .port(3211)
        .probe(Probe::tcp(3211))
        .ready_timeout(Duration::from_secs(600));
    let started = Instant::now();
    match LumeRuntime::with_defaults()
        .prepare_warm(&spec, Duration::from_secs(settle))
        .await
    {
        Ok(states) => println!(
            "{}",
            serde_json::json!({
                "image": reference, "states": states, "cpus": cpus,
                "memory_mb": memory_mb, "ms": started.elapsed().as_millis() as u64,
            })
        ),
        Err(e) => {
            eprintln!("lume_warm: {e}");
            std::process::exit(1);
        }
    }
}
