//! Warm-pool claims of Firecracker microVM Spaces, timed at the runtime
//! level (no daemon): start pool members, claim each one as a Space, then
//! time the first command and the first screenshot through cua-spacesd with
//! the Space's own token. Also reports each member's memory while it waits
//! and checks that two claimed Spaces do not share an identity.
//!
//!     cargo run --release -p cua-daemon --example microvm_pool -- [options]
//!
//! Options:
//!   --image REF     default ghcr.io/trycua/linux:24.04-slim
//!   --n N           claims to time (default 10)
//!   --cpus N        default 2
//!   --memory-mb M   default 2048
//!   --pool N        members started ahead of the claims (default 2)
//!   --no-shot       skip the screenshot
//!   --again         also time a second command and screenshot
//!   --keep          keep the last two claimed Spaces and the members
//!   --running-ms MS resume each member and let it run MS before its claim
//!
//! `MICROVM_POOL_TRACE=1` prints every readiness attempt to stderr.
//! Uses `CUA_HOME` like the daemon (its microVM state lives under
//! `$CUA_HOME/vmm/microvm`), and the same golden snapshots.

use std::time::{Duration, Instant};

use cua_spacesd_client::{
    Command, ConnectOptions, ScreenshotOptions, SpacesdClient, TransportPreference,
};
use cua_vmm::microvm::{MicroVmConfig, MicroVmRuntime};
use cua_vmm::{ClaimIdentity, ImageSource, Probe, Runtime, StartSpec};

const SPACESD_PORT: u16 = 3211;

struct Args {
    image: String,
    n: usize,
    cpus: u32,
    memory_mb: u64,
    pool: usize,
    shot: bool,
    /// Also time a second command and screenshot (warm guest paths).
    again: bool,
    /// Keep the claimed Spaces (and members) for inspection.
    keep: bool,
    /// Resume each member after it starts and wait this long before the
    /// claim (members otherwise wait paused).
    running_ms: Option<u64>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        image: "ghcr.io/trycua/linux:24.04-slim".into(),
        n: 10,
        cpus: 2,
        memory_mb: 2048,
        pool: 2,
        shot: true,
        again: false,
        keep: false,
        running_ms: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--image" => a.image = v()?,
            "--n" => a.n = v()?.parse().map_err(|e| format!("--n: {e}"))?,
            "--cpus" => a.cpus = v()?.parse().map_err(|e| format!("--cpus: {e}"))?,
            "--memory-mb" => a.memory_mb = v()?.parse().map_err(|e| format!("--memory-mb: {e}"))?,
            "--pool" => a.pool = v()?.parse().map_err(|e| format!("--pool: {e}"))?,
            "--no-shot" => a.shot = false,
            "--again" => a.again = true,
            "--keep" => a.keep = true,
            "--running-ms" => {
                a.running_ms = Some(v()?.parse().map_err(|e| format!("--running-ms: {e}"))?)
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(a)
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 10_000.0).round() / 10.0
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn token() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

/// `(rss_kib, pss_kib)` of a process.
fn memory_of(pid: u32) -> (u64, u64) {
    let field = |text: &str, key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let s = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
    (field(&s, "Rss:"), field(&s, "Pss:"))
}

async fn connect(port: u16, token: &str) -> Result<SpacesdClient, String> {
    let mut o = ConnectOptions::parse(&format!("http://127.0.0.1:{port}"))
        .map_err(|e| e.to_string())?
        .transport(TransportPreference::Native)
        .probe(false);
    o.token = Some(token.to_string());
    SpacesdClient::connect(o).await.map_err(|e| e.to_string())
}

/// Connects and runs the first command (`true`) until cua-spacesd accepts
/// `token` (it follows the token file the claim wrote).
async fn ready_client(port: u16, token: &str) -> Result<SpacesdClient, String> {
    let start = Instant::now();
    let deadline = start + Duration::from_secs(20);
    let trace = std::env::var_os("MICROVM_POOL_TRACE").is_some();
    loop {
        let t = Instant::now();
        let last = match connect(port, token).await {
            Ok(c) => {
                let connected = ms(t);
                // The first command doubles as the token check (Metrics
                // would add its 200 ms CPU sample).
                match sh(&c, "true").await {
                    Ok(_) => return Ok(c),
                    Err(e) => format!(
                        "{e} (connect {connected} ms, metrics {} ms)",
                        ms(t) - connected
                    ),
                }
            }
            Err(e) => format!("{e} (connect {} ms)", ms(t)),
        };
        if trace {
            eprintln!("+{} ms: {last}", ms(start));
        }
        if Instant::now() >= deadline {
            return Err(format!("cua-spacesd never accepted the token: {last}"));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn sh(c: &SpacesdClient, line: &str) -> Result<String, String> {
    let out = c
        .run(Command::shell(line).timeout(Duration::from_secs(20)))
        .await
        .map_err(|e| e.to_string())?;
    if out.status.code != Some(0) {
        return Err(format!("{line:?} exited {:?}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn member_spec(a: &Args, name: &str) -> StartSpec {
    let mut s = StartSpec::new(name, ImageSource::oci(a.image.clone()))
        .cpus(a.cpus)
        .memory_mb(a.memory_mb)
        .port(SPACESD_PORT)
        .probe(Probe::tcp(SPACESD_PORT))
        .ready_timeout(Duration::from_secs(180));
    s.pool_member = true;
    s
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = parse().map_err(|e| format!("microvm_pool: {e}"))?;
    let rt = MicroVmRuntime::new(MicroVmConfig {
        resolver: Some(cua_image::RootfsLayerResolver::shared()),
        ..MicroVmConfig::default()
    });
    let tag = format!("{:06x}", std::process::id() & 0xff_ffff);
    let mut members: std::collections::VecDeque<String> = Default::default();
    let mut next = 0usize;
    let mut start_member = async |members: &mut std::collections::VecDeque<String>| {
        let name = format!("pool-{tag}-{next}");
        next += 1;
        let t = Instant::now();
        let r = rt.start(&member_spec(&a, &name)).await;
        let st = rt.load(&name).ok();
        let (rss, pss) = st
            .as_ref()
            .and_then(|s| s.fc_pid)
            .map(memory_of)
            .unwrap_or_default();
        println!(
            "{}",
            serde_json::json!({"member": name, "start_ms": ms(t), "ok": r.is_ok(),
                "how": st.and_then(|s| s.last_start), "paused_rss_kib": rss,
                "paused_pss_kib": pss, "error": r.as_ref().err().map(|e| e.to_string())})
        );
        if r.is_ok() {
            members.push_back(name);
        }
    };
    for _ in 0..a.pool.max(1) {
        start_member(&mut members).await;
    }
    let mut ready = Vec::new();
    let mut claimed: Vec<(String, String, u16)> = Vec::new();
    for i in 0..a.n {
        // Let the refill settle so each claim finds a waiting member.
        while members.len() < a.pool.max(1) {
            start_member(&mut members).await;
        }
        let Some(member) = members.pop_front() else {
            break;
        };
        if let Some(wait) = a.running_ms {
            rt.resume(&member).await?;
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
        let name = format!("space-{tag}-{i}");
        let tok = token();
        let t0 = Instant::now();
        let id = ClaimIdentity {
            token: Some(tok.clone()),
            hostname: None,
        };
        let mut line = serde_json::json!({"mode": "claim", "i": i, "name": name});
        let outcome: Result<(u16, f64), String> = async {
            let inst = rt
                .claim(&member, &name, &id)
                .await
                .map_err(|e| e.to_string())?;
            line["claim_ms"] = ms(t0).into();
            let port = *inst
                .endpoints
                .ports
                .get(&SPACESD_PORT)
                .ok_or("no spacesd port")?;
            let c = ready_client(port, &tok).await?;
            line["exec_ms"] = ms(t0).into();
            if a.shot {
                let s = c
                    .screenshot(ScreenshotOptions::default())
                    .await
                    .map_err(|e| e.to_string())?;
                if s.image.is_empty() || s.width == 0 {
                    return Err("empty screenshot".into());
                }
                line["shot_px"] = format!("{}x{}", s.width, s.height).into();
                line["shot_ms"] = ms(t0).into();
            }
            let ready_at = ms(t0);
            if a.again {
                let t = Instant::now();
                sh(&c, "true").await?;
                line["exec2_ms"] = ms(t).into();
                let t = Instant::now();
                c.screenshot(ScreenshotOptions::default())
                    .await
                    .map_err(|e| e.to_string())?;
                line["shot2_ms"] = ms(t).into();
            }
            Ok((port, ready_at))
        }
        .await;
        match outcome {
            Ok((port, r)) => {
                line["ready_ms"] = r.into();
                line["ok"] = true.into();
                ready.push(r);
                claimed.push((name.clone(), tok, port));
            }
            Err(e) => {
                line["ok"] = false.into();
                line["error"] = e.into();
                let _ = rt.delete(&name).await;
            }
        }
        println!("{line}");
        // Keep two claimed Spaces for the identity check.
        while claimed.len() > 2 {
            let (old, _, _) = claimed.remove(0);
            let _ = rt.delete(&old).await;
        }
    }
    ready.sort_by(|x, y| x.partial_cmp(y).unwrap());
    println!(
        "{}",
        serde_json::json!({"summary": true, "mode": "claim", "image": a.image,
            "n": ready.len(), "p50_ms": pct(&ready, 50.0), "p95_ms": pct(&ready, 95.0),
            "min_ms": ready.first(), "max_ms": ready.last()})
    );

    // Two claimed Spaces: nothing that makes a machine its own is shared.
    if claimed.len() == 2 {
        let probe = "cat /etc/machine-id; hostname; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \\n'; echo; date +%s.%N";
        let mut seen = Vec::new();
        for (name, tok, port) in &claimed {
            let c = ready_client(*port, tok).await?;
            let host_now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs_f64();
            let out = sh(&c, probe).await?;
            let f: Vec<String> = out.lines().map(str::to_string).collect();
            let guest_now: f64 = f.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            seen.push(f.clone());
            println!(
                "{}",
                serde_json::json!({"identity": name, "machine_id": f.first(), "hostname": f.get(1),
                    "urandom32": f.get(2), "clock_skew_s": guest_now - host_now})
            );
        }
        let (x, y) = (&claimed[0], &claimed[1]);
        // Each Space's token opens only its own Space.
        let cross = match connect(x.2, &y.1).await {
            Ok(c) => sh(&c, "true").await.is_err(),
            Err(_) => true,
        };
        let differ = (0..3).all(|k| seen[0].get(k) != seen[1].get(k));
        println!(
            "{}",
            serde_json::json!({"identity_check": true, "fields_differ": differ,
                "other_token_refused": cross})
        );
    }
    if a.keep {
        return Ok(());
    }
    for (name, _, _) in &claimed {
        let _ = rt.delete(name).await;
    }
    while let Some(m) = members.pop_front() {
        let _ = rt.delete(&m).await;
    }
    Ok(())
}
