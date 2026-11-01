//! Do these Spaces have identities of their own? For two or more running
//! Spaces (typically clones: forks or restores of one snapshot, or members
//! claimed from one warm pool), checks that each pair differs in
//! `/etc/machine-id` (macOS: the platform UUID), the machine-seal public
//! key, the hostname, 32 bytes of `/dev/urandom` and the Spaces token (each
//! Space's own token works, every other Space's is refused), and that each
//! guest's wall clock is within a second of this host's and its outbound
//! network works.
//!
//!     cargo run -p cua-sdk --example space_identity -- NAME NAME [NAME...] [options]
//!
//! Options:
//!   --net-url URL     what the outbound check fetches (default https://example.com)
//!   --no-network      skip the outbound check
//!   --clock-ms MS     allowed clock error (default 1000)
//!
//! Prints one JSON line per Space, one per failed check, and a summary line
//! (`"summary":true`, `"ok"`); exits 1 when any check fails. Reads the
//! Spaces from the state of `CUA_HOME` in this process (not through
//! `cua daemon`, whose env passthrough serves every Space under the daemon's
//! own token): each Space is reached at its own endpoint with its own token.
//! Run it with the daemon's `CUA_HOME`.
//! Linux and macOS guests.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cua_sdk::{Cua, CuaConfig};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};

struct Args {
    names: Vec<String>,
    net_url: Option<String>,
    clock_ms: f64,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        names: Vec::new(),
        net_url: Some("https://example.com".into()),
        clock_ms: 1000.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--net-url" => a.net_url = Some(it.next().ok_or("--net-url needs a value")?),
            "--no-network" => a.net_url = None,
            "--clock-ms" => {
                a.clock_ms = it
                    .next()
                    .ok_or("--clock-ms needs a value")?
                    .parse()
                    .map_err(|e| format!("--clock-ms: {e}"))?
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            name => a.names.push(name.to_owned()),
        }
    }
    if a.names.len() < 2 {
        return Err("give two or more Space names".into());
    }
    Ok(a)
}

/// What one Space reports about itself.
struct Identity {
    name: String,
    endpoint: String,
    token: String,
    machine_id: String,
    seal_key: String,
    hostname: String,
    random: String,
    clock_error_ms: f64,
    network: Option<String>,
}

fn unix_secs(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// One round trip: machine id, hostname, 32 random bytes, the wall clock.
const PROBE: &str = r#"set -e
if [ -s /etc/machine-id ]; then cat /etc/machine-id
elif command -v ioreg >/dev/null; then ioreg -rd1 -c IOPlatformExpertDevice | awk -F'"' '/IOPlatformUUID/{print $4}'
else echo; fi
hostname
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'; echo
if command -v python3 >/dev/null; then python3 -c 'import time; print("%.6f" % time.time())'
else date +%s.%N; fi"#;

fn net_probe(url: &str) -> String {
    format!(
        "if command -v curl >/dev/null; then curl -sS -o /dev/null -w '%{{http_code}}' --max-time 15 '{url}'; \
         else python3 -c 'import urllib.request,sys; print(urllib.request.urlopen(sys.argv[1], timeout=15).status, end=\"\")' '{url}'; fi"
    )
}

async fn probe(
    cua: &Cua,
    name: &str,
    net_url: Option<&str>,
) -> Result<Identity, Box<dyn std::error::Error>> {
    let sb = cua.sandboxes().connect(name.to_owned()).await?;
    let env = sb.spacesd(Some(30_000)).await?;
    let client = env.inner().clone();
    let token = client
        .token()
        .ok_or("the daemon gave no token for this Space")?
        .to_owned();
    let caps = client.refresh_capabilities().await?;
    let before = SystemTime::now();
    let out = client
        .run(cua_spacesd_client::Command::shell(PROBE).timeout(Duration::from_secs(30)))
        .await?;
    let after = SystemTime::now();
    if !out.success() {
        return Err(format!("probe failed: {:?} {}", out.status, out.stderr_str()).into());
    }
    let text = out.stdout_str();
    let lines: Vec<&str> = text.lines().collect();
    let [machine_id, hostname, random, clock] = lines[..] else {
        return Err(format!("unexpected probe output {text:?}").into());
    };
    // The guest read its clock somewhere inside the round trip: compare it
    // with the midpoint and allow for half the round trip either way.
    let guest: f64 = clock.trim().parse()?;
    let (t0, t1) = (unix_secs(before), unix_secs(after));
    let off = guest - (t0 + t1) / 2.0;
    let slack = (t1 - t0) / 2.0;
    let clock_error_ms = ((off.abs() - slack).max(0.0)) * 1000.0;
    let network = match net_url {
        Some(url) => {
            let out = client
                .run(
                    cua_spacesd_client::Command::shell(net_probe(url))
                        .timeout(Duration::from_secs(30)),
                )
                .await?;
            Some(if out.success() {
                out.stdout_str().trim().to_owned()
            } else {
                format!("failed: {}", out.stderr_str().trim())
            })
        }
        None => None,
    };
    Ok(Identity {
        name: name.to_owned(),
        endpoint: env.endpoint(),
        token,
        machine_id: machine_id.trim().to_owned(),
        seal_key: hex::encode(&caps.machine_seal_public_key),
        hostname: hostname.trim().to_owned(),
        random: random.trim().to_owned(),
        clock_error_ms,
        network,
    })
}

/// Whether `token` opens `endpoint` (an authenticated call succeeds).
async fn token_works(endpoint: &str, token: &str) -> Result<bool, String> {
    let options = ConnectOptions::parse(endpoint)
        .map_err(|e| e.to_string())?
        .token(token)
        .transport(TransportPreference::Native)
        .probe(false);
    let client = SpacesdClient::connect(options)
        .await
        .map_err(|e| e.to_string())?;
    match client
        .run(cua_spacesd_client::Command::new("true").timeout(Duration::from_secs(15)))
        .await
    {
        Ok(out) => Ok(out.success()),
        Err(e) => {
            let text = e.to_string();
            if text.to_lowercase().contains("unauthenticated")
                || text.contains("token")
                || text.contains("401")
            {
                Ok(false)
            } else {
                Err(text)
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = parse().map_err(|e| format!("space_identity: {e}"))?;
    // Embedded, not `Cua::connect`: through the daemon every Space answers
    // under the daemon's loopback token, which would hide the per-Space
    // token checks.
    let cua = Cua::embedded(CuaConfig::default())?;
    let mut failures: Vec<serde_json::Value> = Vec::new();
    let mut ids = Vec::new();
    for name in &a.names {
        match probe(&cua, name, a.net_url.as_deref()).await {
            Ok(id) => {
                println!(
                    "{}",
                    serde_json::json!({"space": id.name, "machine_id": id.machine_id,
                        "seal_key": id.seal_key, "hostname": id.hostname,
                        "random": id.random, "clock_error_ms": (id.clock_error_ms * 10.0).round() / 10.0,
                        "network": id.network})
                );
                if id.clock_error_ms > a.clock_ms {
                    failures.push(serde_json::json!({"check": "clock", "space": id.name,
                        "error_ms": id.clock_error_ms}));
                }
                if id.seal_key.is_empty() {
                    failures.push(serde_json::json!({"check": "seal_key", "space": id.name,
                        "error": "the Space reports no machine-seal key"}));
                }
                if id.machine_id.is_empty() {
                    failures.push(serde_json::json!({"check": "machine_id", "space": id.name,
                        "error": "no machine id"}));
                }
                if let Some(net) = &id.network {
                    let ok = net.parse::<u16>().is_ok_and(|c| (200..400).contains(&c));
                    if !ok {
                        failures.push(serde_json::json!({"check": "network", "space": id.name,
                            "result": net}));
                    }
                }
                ids.push(id);
            }
            Err(e) => failures.push(serde_json::json!({"check": "probe", "space": name,
                "error": e.to_string()})),
        }
    }
    for (i, x) in ids.iter().enumerate() {
        match token_works(&x.endpoint, &x.token).await {
            Ok(true) => {}
            other => failures.push(serde_json::json!({"check": "own_token", "space": x.name,
                "result": format!("{other:?}")})),
        }
        for y in &ids[i + 1..] {
            for (field, a_val, b_val) in [
                ("machine_id", &x.machine_id, &y.machine_id),
                ("seal_key", &x.seal_key, &y.seal_key),
                ("hostname", &x.hostname, &y.hostname),
                ("random", &x.random, &y.random),
                ("token", &x.token, &y.token),
            ] {
                if a_val == b_val {
                    failures.push(serde_json::json!({"check": format!("{field}_differs"),
                        "spaces": [x.name, y.name], "shared": if field == "token" { "<redacted>" } else { a_val.as_str() }}));
                }
            }
            for (host, guest_token, owner) in [(x, &y.token, &y.name), (y, &x.token, &x.name)] {
                match token_works(&host.endpoint, guest_token).await {
                    Ok(false) => {}
                    other => failures.push(serde_json::json!({"check": "foreign_token_refused",
                        "space": host.name, "token_of": owner, "result": format!("{other:?}")})),
                }
            }
        }
    }
    for f in &failures {
        println!("{f}");
    }
    let ok = failures.is_empty() && ids.len() == a.names.len();
    println!(
        "{}",
        serde_json::json!({"summary": true, "spaces": a.names, "checked": ids.len(),
            "failures": failures.len(), "ok": ok})
    );
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
