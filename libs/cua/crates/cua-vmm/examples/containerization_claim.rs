//! Cold start versus warm-pool claim of Apple Containerization microVMs,
//! at the backend: `start` of a new Space until its probes pass (cua-spacesd
//! listening), against `claim` of an already running pool member plus the
//! first command in it.
//!
//!     cargo run -p cua-vmm --example containerization_claim -- [--image REF] [--n N]
//!
//! Uses cua's own `container` install under `CUA_HOME` (it starts its
//! services when needed; stop them with `container system stop` and the
//! same roots when done). Prints one JSON line per sample and a summary.

use std::time::{Duration, Instant};

use cua_vmm::containerization::ContainerizationRuntime;
use cua_vmm::{ClaimIdentity, ExecRequest, ImageSource, Probe, Runtime, StartSpec};

fn spec(name: &str, image: &str, token: &str) -> StartSpec {
    StartSpec::new(name, ImageSource::oci(image))
        .cpus(2)
        .memory_mb(2048)
        .port(3211)
        .probe(Probe::tcp(3211))
        .env("CUA_ENV_TOKEN", token)
        .ready_timeout(Duration::from_secs(300))
}

fn p(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() as f64 - 1.0) * q).round() as usize]
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (mut image, mut n) = ("ghcr.io/trycua/linux:24.04-slim".to_string(), 5usize);
    while let Some(k) = args.next() {
        let v = args.next().unwrap_or_default();
        match k.as_str() {
            "--image" => image = v,
            "--n" => n = v.parse().expect("--n needs a number"),
            other => panic!("unknown option {other}"),
        }
    }
    let rt = ContainerizationRuntime::with_defaults();
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let (mut cold, mut claim) = (Vec::new(), Vec::new());
    for i in 0..n {
        let name = format!("cz-cold-{i}");
        let t = Instant::now();
        rt.start(&spec(&name, &image, &format!("cold-token-{i:032}")))
            .await
            .expect("cold start");
        let c = ms(t);
        cold.push(c);
        println!(
            "{}",
            serde_json::json!({"mode": "cold", "i": i, "ready_ms": c})
        );
        rt.delete(&name).await.expect("delete");
    }
    for i in 0..n {
        let member = format!("cz-pool-{i}");
        let mut s = spec(&member, &image, &format!("pool-secret-{i:032}"));
        s.pool_member = true;
        rt.start(&s).await.expect("pool member start");
        let name = format!("cz-space-{i}");
        let t = Instant::now();
        rt.claim(&member, &name, &ClaimIdentity::default())
            .await
            .expect("claim");
        let claimed = ms(t);
        let exec = rt.guest_exec(&name).await.expect("exec").expect("exec");
        let out = exec
            .exec(ExecRequest::sh("true"))
            .await
            .expect("first command");
        assert_eq!(out.exit_code, 0);
        let c = ms(t);
        claim.push(c);
        println!(
            "{}",
            serde_json::json!({"mode": "claim", "i": i, "claim_ms": claimed, "first_command_ms": c})
        );
        rt.delete(&name).await.expect("delete");
    }
    println!(
        "{}",
        serde_json::json!({"summary": true, "image": image, "n": n,
            "cold_p50_ms": p(&mut cold, 0.5), "cold_p95_ms": p(&mut cold, 0.95),
            "claim_first_command_p50_ms": p(&mut claim, 0.5),
            "claim_first_command_p95_ms": p(&mut claim, 0.95)})
    );
}
