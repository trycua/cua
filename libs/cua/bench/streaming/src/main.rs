// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! cua-bench-streaming: benchmark driver for the spacesd media plane.
//!
//! See README.md. `run` executes the matrix (runtime × transport × decoder ×
//! scenario, plus language lanes) against local Spaces containers and writes
//! JSON + markdown; `check` gates a results file against budgets.json.

mod audio;
mod link;
mod report;
mod run;
mod shim;
mod space;
mod video;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::link::Transport;
use crate::report::{Results, Run, Skip};
use crate::run::{RunConfig, Scenario};
use crate::space::{Launcher, Space};

#[derive(Parser)]
#[command(
    name = "cua-bench-streaming",
    about = "Media-plane benchmarks for cua-spacesd"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the benchmark matrix.
    Run(Box<RunArgs>),
    /// Check a results file against budgets; non-zero exit on regressions.
    Check {
        #[arg(long)]
        results: PathBuf,
        #[arg(long, default_value_os_t = default_budgets())]
        budgets: PathBuf,
    },
    /// Print the markdown summary of a results file.
    Report {
        #[arg(long)]
        results: PathBuf,
    },
}

#[derive(clap::Args)]
struct RunArgs {
    /// Container runtimes to start the Space under, one at a time.
    #[arg(long, value_delimiter = ',', default_value = "runc")]
    runtimes: Vec<String>,
    /// Use an already running container instead (token from CUA_ENV_TOKEN).
    #[arg(long)]
    attach: Option<String>,
    /// Spaces image (default: space.sh's local linux tag).
    #[arg(long)]
    image: Option<String>,
    #[arg(long, default_value_t = 33200)]
    port_base: u16,
    #[arg(long, value_delimiter = ',', default_value = "ws,quic")]
    transports: Vec<Transport>,
    /// Client decoder for the main lanes (default: videotoolbox on macOS,
    /// openh264 elsewhere).
    #[arg(long)]
    decoder: Option<String>,
    /// Extra decoders compared on the timecode and video scenarios (ws).
    #[arg(long, value_delimiter = ',')]
    alt_decoders: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    scenarios: Vec<String>,
    /// Language lanes (timecode scenario over WebSocket, first runtime):
    /// rust is the native lane; node, python, browser use shims/<name>.sh.
    #[arg(long, value_delimiter = ',', default_value = "rust")]
    clients: Vec<String>,
    #[arg(long, default_value_t = 20)]
    seconds: u64,
    #[arg(long, default_value_t = 30)]
    max_fps: u32,
    /// QUIC video datagram loss injected in the `loss` scenario.
    #[arg(long, default_value_t = 5.0)]
    loss_percent: f64,
    /// Input policy for input→photon probes: `activate` (XTest, may raise the
    /// window) or `background` (XSendEvent to the window, no focus change).
    #[arg(long, default_value = "activate")]
    input_policy: String,
    /// Attach to any spacesd instead of a local container (token from
    /// CUA_ENV_TOKEN); needs --quic-addr and --time-addr.
    #[arg(long)]
    env_url: Option<String>,
    #[arg(long)]
    quic_addr: Option<std::net::SocketAddr>,
    /// The bench fixture's time server (TCP 18081 in the guest).
    #[arg(long)]
    time_addr: Option<std::net::SocketAddr>,
    /// Runtime label for run ids with --env-url.
    #[arg(long)]
    runtime_label: Option<String>,
    /// Local container name (host-side container CPU) with --env-url.
    #[arg(long)]
    docker_name: Option<String>,
    /// Client label in run ids for the native lanes.
    #[arg(long, default_value = "rust")]
    client_label: String,
    /// Also run the native lanes from a Linux sidecar container sharing the
    /// Space's network (OpenH264 decode, WS + QUIC); needs
    /// scripts/build-linux.sh.
    #[arg(long)]
    sidecar: bool,
    #[arg(long, default_value = "rust:1-bookworm")]
    sidecar_image: String,
    #[arg(long)]
    sidecar_bin: Option<PathBuf>,
    /// Run QUIC lanes from this host (false when the host cannot reach the
    /// container's UDP ports; the sidecar covers QUIC then).
    #[arg(long)]
    host_quic: Option<bool>,
    #[arg(long, default_value = "results/latest.json")]
    out: PathBuf,
    /// Also write the markdown summary here.
    #[arg(long)]
    markdown: Option<PathBuf>,
    /// Gate the results against these budgets after the run.
    #[arg(long)]
    check: bool,
    #[arg(long, default_value_os_t = default_budgets())]
    budgets: PathBuf,
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn default_budgets() -> PathBuf {
    crate_dir().join("budgets.json")
}

fn default_decoder() -> &'static str {
    if cfg!(target_os = "macos") {
        "videotoolbox"
    } else {
        "openh264"
    }
}

fn git_sha() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(crate_dir())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn deferred_skips() -> Vec<Skip> {
    let mut skips: Vec<Skip> = ["nvenc", "vaapi", "qsv", "amf", "mediafoundation"]
        .iter()
        .map(|e| Skip {
            lane: format!("encoder/{e}"),
            reason: "deferred until GPU access (plan §8.6): probe-only (DetectedOnly), skipped by selection; tests gated behind CUA_CODEC_TEST_*".into(),
        })
        .collect();
    skips.push(Skip {
        lane: "encoder/videotoolbox".into(),
        reason: "unavailable in this matrix: the server runs in a Linux container (OpenH264); VideoToolbox encode applies to macOS guests".into(),
    });
    skips.push(Skip {
        lane: "runtime/qemu".into(),
        reason: "optional arm64 VM lane not run by this harness yet (docker runc/runsc only)"
            .into(),
    });
    skips
}

fn host_info() -> std::collections::BTreeMap<String, String> {
    let mut host = std::collections::BTreeMap::new();
    host.insert("os".into(), std::env::consts::OS.into());
    host.insert("arch".into(), std::env::consts::ARCH.into());
    host.insert(
        "cpus".into(),
        std::thread::available_parallelism()
            .map(|n| n.get().to_string())
            .unwrap_or_default(),
    );
    // Other containers share the docker VM's CPUs; record the load.
    if let Ok(out) = std::process::Command::new("docker")
        .args(["ps", "-q"])
        .output()
    {
        let count = String::from_utf8_lossy(&out.stdout).lines().count();
        host.insert(
            "docker_containers_running_at_start".into(),
            count.to_string(),
        );
    }
    if let Ok(ci) = std::env::var("GITHUB_ACTIONS") {
        host.insert(
            "ci".into(),
            if ci == "true" {
                "github-actions".into()
            } else {
                ci
            },
        );
    }
    host
}

fn write_results(results: &Results, out: &Path, markdown: Option<&Path>) -> std::io::Result<()> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, serde_json::to_string_pretty(results)? + "\n")?;
    if let Some(md) = markdown {
        std::fs::write(md, report::markdown(results))?;
    }
    Ok(())
}

fn transport_name(t: Transport) -> &'static str {
    match t {
        Transport::Ws => "ws",
        Transport::Quic => "quic",
    }
}

/// A container name unique to this run and runtime, so concurrent or
/// leftover bench runs never share (and so never remove) each other's
/// containers. CUA_BENCH_CONTAINER overrides the prefix (CI uses a run-scoped
/// one it can clean up after a crash).
fn run_container_name(runtime: &str) -> String {
    let prefix = std::env::var("CUA_BENCH_CONTAINER")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("cua-e2e-bench-space-{}-{secs}", std::process::id())
        });
    format!("{prefix}-{runtime}")
}

/// The Space for one runtime: started here, a named local container, or any
/// spacesd URL (sidecar / remote).
async fn open_space(args: &RunArgs, runtime: &str, launcher: &Launcher) -> Result<Space, String> {
    let token_env =
        || std::env::var("CUA_ENV_TOKEN").map_err(|_| "CUA_ENV_TOKEN is required".to_string());
    if let Some(url) = &args.env_url {
        let quic = args.quic_addr.ok_or("--env-url needs --quic-addr")?;
        let time = args.time_addr.ok_or("--env-url needs --time-addr")?;
        return Space::connect(
            url.clone(),
            token_env()?,
            quic,
            time,
            runtime.into(),
            args.docker_name.clone(),
        )
        .await
        .map_err(|e| e.to_string());
    }
    let token = match &args.attach {
        Some(_) => token_env()?,
        None => launcher
            .start(runtime, args.image.as_deref())
            .map_err(|e| e.to_string())?,
    };
    let (url, quic, time) = launcher.addrs();
    let runtime = match &args.attach {
        Some(name) => std::process::Command::new("docker")
            .args(["inspect", "-f", "{{.HostConfig.Runtime}}", name])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_else(|_| "attached".into()),
        None => runtime.to_owned(),
    };
    Space::connect(url, token, quic, time, runtime, Some(launcher.name.clone()))
        .await
        .map_err(|e| e.to_string())
}

/// Run the Linux build of this harness in a container that shares the
/// Space's network namespace (for QUIC where the host cannot reach the
/// container's UDP ports, e.g. Docker Desktop / colima on macOS).
fn run_sidecar(args: &RunArgs, space: &Space, results: &mut Results) {
    let Some(name) = &space.docker_name else {
        return;
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };
    let bin = args
        .sidecar_bin
        .clone()
        .unwrap_or_else(|| crate_dir().join(format!("target/linux-{arch}/cua-bench-streaming")));
    let lane = format!("{}/*/openh264/rust-sidecar/*", space.runtime);
    let Ok(rel) = bin.strip_prefix(crate_dir()) else {
        results.skipped.push(Skip {
            lane,
            reason: format!(
                "sidecar binary {} must live under {}",
                bin.display(),
                crate_dir().display()
            ),
        });
        return;
    };
    if !bin.exists() {
        results.skipped.push(Skip {
            lane,
            reason: format!(
                "sidecar binary {} not built (scripts/build-linux.sh)",
                bin.display()
            ),
        });
        return;
    }
    // Under the crate (the user's home), which Docker Desktop / colima share with the VM.
    let out_dir = crate_dir().join(format!("target/sidecar-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&out_dir);
    let scenarios = if args.scenarios.is_empty() {
        String::new()
    } else {
        args.scenarios.join(",")
    };
    let transports: Vec<&str> = args.transports.iter().map(|t| transport_name(*t)).collect();
    // Reach the Space by its bridge address: sharing its network namespace
    // does not work under runsc (gVisor has its own netstack).
    let ip = std::process::Command::new("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}",
            name,
        ])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .map(str::to_owned)
        });
    let Some(ip) = ip else {
        results.skipped.push(Skip {
            lane,
            reason: format!("no bridge address for {name}"),
        });
        return;
    };
    let mut cmd = std::process::Command::new("docker");
    cmd.args(["run", "--rm", "--name", &format!("{name}-sidecar")])
        .args(["--memory=4g", "--memory-swap=4g"])
        .arg("-v")
        .arg(format!("{}:/bench:ro", crate_dir().display()))
        .arg("-v")
        .arg(format!("{}:/out", out_dir.display()))
        .args(["-e", "CUA_ENV_TOKEN"])
        .env("CUA_ENV_TOKEN", &space.token)
        .arg(&args.sidecar_image)
        .arg(format!("/bench/{}", rel.display()))
        .args([
            "run",
            "--env-url",
            &format!("http://{ip}:3211"),
            "--quic-addr",
            &format!("{ip}:3212"),
            "--time-addr",
            &format!("{ip}:18081"),
        ])
        .args([
            "--runtime-label",
            &space.runtime,
            "--client-label",
            "rust-sidecar",
            "--decoder",
            "openh264",
        ])
        .args([
            "--transports",
            &transports.join(","),
            "--seconds",
            &args.seconds.to_string(),
        ])
        .args([
            "--loss-percent",
            &args.loss_percent.to_string(),
            "--input-policy",
            &args.input_policy,
        ])
        .args(["--out", "/out/sidecar.json"]);
    if !scenarios.is_empty() {
        cmd.args(["--scenarios", &scenarios]);
    }
    eprintln!("-- sidecar lanes in {}", args.sidecar_image);
    let status = cmd.status();
    match std::fs::read_to_string(out_dir.join("sidecar.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Results>(&t).ok())
    {
        Some(sidecar) => {
            results.runs.extend(sidecar.runs);
            results.skipped.extend(sidecar.skipped);
        }
        None => results.skipped.push(Skip {
            lane,
            reason: format!("sidecar produced no results ({status:?})"),
        }),
    }
    let _ = std::fs::remove_dir_all(&out_dir);
}

async fn run_matrix(args: RunArgs) -> Result<Results, String> {
    let shims = crate_dir().join("shims");
    let decoder = args
        .decoder
        .clone()
        .unwrap_or_else(|| default_decoder().into());
    let scenarios: Vec<Scenario> = if args.scenarios.is_empty() {
        Scenario::all().to_vec()
    } else {
        args.scenarios
            .iter()
            .map(|s| Scenario::parse(s))
            .collect::<Result<_, _>>()?
    };
    let mut results = Results {
        schema: report::SCHEMA,
        created_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        git_sha: git_sha(),
        host: host_info(),
        image: args
            .image
            .clone()
            .unwrap_or_else(|| "space.sh default (cua-e2e-local/linux:docker-local-<arch>)".into()),
        seconds_per_run: args.seconds,
        runs: Vec::new(),
        skipped: if args.env_url.is_some() {
            Vec::new()
        } else {
            deferred_skips()
        },
    };
    if let Ok(driver) = std::env::var("CUA_BENCH_DRIVER_BIN") {
        results.host.insert("driver_override".into(), driver);
    }
    let runtimes: Vec<String> = match (&args.env_url, &args.attach) {
        (Some(_), _) => vec![args
            .runtime_label
            .clone()
            .unwrap_or_else(|| "remote".into())],
        (None, Some(_)) => vec!["attached".into()],
        (None, None) => args.runtimes.clone(),
    };
    for (runtime_index, runtime) in runtimes.iter().enumerate() {
        eprintln!("== runtime {runtime}");
        let launcher = Launcher {
            script: crate_dir().join("scripts/space.sh"),
            name: args
                .attach
                .clone()
                .unwrap_or_else(|| run_container_name(runtime)),
            port_base: args.port_base,
        };
        let space = match open_space(&args, runtime, &launcher).await {
            Ok(space) => space,
            Err(error) => {
                results.skipped.push(Skip {
                    lane: format!("runtime/{runtime}"),
                    reason: format!("Space unavailable: {error}"),
                });
                if args.attach.is_none() && args.env_url.is_none() {
                    launcher.stop();
                }
                continue;
            }
        };
        let runtime_name = space.runtime.clone();
        if args.env_url.is_none() {
            // Let the desktop settle after boot.
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        let mut lanes: Vec<(Transport, String, Scenario)> = Vec::new();
        for transport in &args.transports {
            if *transport == Transport::Quic && args.host_quic == Some(false) {
                continue;
            }
            for scenario in &scenarios {
                if scenario.applies(*transport) {
                    lanes.push((*transport, decoder.clone(), *scenario));
                }
            }
        }
        for alt in &args.alt_decoders {
            for scenario in [Scenario::Timecode, Scenario::Video] {
                if scenarios.contains(&scenario) && *alt != decoder {
                    lanes.push((Transport::Ws, alt.clone(), scenario));
                }
            }
        }
        if args.clients.iter().any(|c| c == "rust") {
            for (transport, decoder, scenario) in lanes {
                let id = format!(
                    "{runtime_name}/{}/{decoder}/{}/{}",
                    transport_name(transport),
                    args.client_label,
                    scenario.name()
                );
                eprintln!("-- {id}");
                let config = RunConfig {
                    scenario,
                    transport,
                    decoder: decoder.clone(),
                    seconds: args.seconds,
                    max_fps: args.max_fps,
                    loss_percent: args.loss_percent,
                    activate: args.input_policy != "background",
                };
                let mut run = Run {
                    id: id.clone(),
                    runtime: runtime_name.clone(),
                    transport: transport_name(transport).into(),
                    decoder: decoder.clone(),
                    client: args.client_label.clone(),
                    scenario: scenario.name().into(),
                    ..Default::default()
                };
                match run::run_native(&space, &config).await {
                    Ok(outcome) => {
                        run.ok = true;
                        run.metrics = outcome.metrics;
                        run.encoder = outcome.encoder;
                        run.target = outcome.target;
                        eprintln!(
                            "   fps {:?} g2g p50 {:?} ttff {:?} bytes/s {:?}",
                            run.metrics.get("fps"),
                            run.metrics.get("g2g_ms_p50"),
                            run.metrics.get("ttff_ms"),
                            run.metrics.get("video_bytes_per_s")
                        );
                    }
                    Err(error) => {
                        eprintln!("   FAILED: {error}");
                        run.error = Some(error.to_string());
                    }
                }
                results.runs.push(run);
                write_results(&results, &args.out, args.markdown.as_deref())
                    .map_err(|e| e.to_string())?;
            }
        }
        if args.sidecar {
            run_sidecar(&args, &space, &mut results);
            write_results(&results, &args.out, args.markdown.as_deref())
                .map_err(|e| e.to_string())?;
        }
        // Language lanes: timecode over WebSocket, first runtime only.
        if runtime_index == 0 {
            for client in args.clients.iter().filter(|c| *c != "rust" && *c != "none") {
                let id = format!("{runtime_name}/ws/sdk/{client}/timecode");
                eprintln!("-- {id}");
                let outcome =
                    shim::run_shim(&shims, client, &space, Scenario::Timecode, args.seconds).await;
                let base = Run {
                    id: id.clone(),
                    runtime: runtime_name.clone(),
                    transport: "ws".into(),
                    decoder: "sdk".into(),
                    client: client.clone(),
                    scenario: "timecode".into(),
                    target: "window:CUA Bench Timecode".into(),
                    ..Default::default()
                };
                match outcome {
                    Ok(shim::ShimOutcome::Ran(metrics)) => {
                        eprintln!(
                            "   fps {:?} g2g p50 {:?} ttff {:?}",
                            metrics.get("fps"),
                            metrics.get("g2g_ms_p50"),
                            metrics.get("ttff_ms")
                        );
                        results.runs.push(Run {
                            ok: true,
                            metrics,
                            ..base
                        });
                    }
                    Ok(shim::ShimOutcome::Skipped(reason)) => {
                        eprintln!("   skipped: {reason}");
                        results.skipped.push(Skip { lane: id, reason });
                    }
                    Err(error) => {
                        eprintln!("   FAILED: {error}");
                        results.runs.push(Run {
                            error: Some(error),
                            ..base
                        });
                    }
                }
                write_results(&results, &args.out, args.markdown.as_deref())
                    .map_err(|e| e.to_string())?;
            }
        }
        if args.attach.is_none() && args.env_url.is_none() {
            launcher.stop();
        }
    }
    Ok(results)
}

fn load<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn gate(results: &Results, budgets_path: &Path) -> ExitCode {
    let budgets: report::Budgets = match load(budgets_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("budgets: {e}");
            return ExitCode::from(2);
        }
    };
    let (checked, failures) = report::check(results, &budgets);
    if failures.is_empty() {
        println!("budget check: {checked} checks passed");
        ExitCode::SUCCESS
    } else {
        println!(
            "budget check: {} failure(s) ({checked} checks passed):",
            failures.len()
        );
        for failure in failures {
            println!("  FAIL {failure}");
        }
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { results, budgets } => match load::<Results>(&results) {
            Ok(results) => gate(&results, &budgets),
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        Command::Report { results } => match load::<Results>(&results) {
            Ok(results) => {
                print!("{}", report::markdown(&results));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::from(2)
            }
        },
        Command::Run(args) => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("tokio runtime");
            let check = args.check;
            let budgets = args.budgets.clone();
            let out = args.out.clone();
            let markdown = args.markdown.clone();
            match runtime.block_on(run_matrix(*args)) {
                Ok(results) => {
                    if let Err(e) = write_results(&results, &out, markdown.as_deref()) {
                        eprintln!("write results: {e}");
                        return ExitCode::from(2);
                    }
                    print!("{}", report::markdown(&results));
                    if check {
                        gate(&results, &budgets)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(error) => {
                    eprintln!("run failed: {error}");
                    ExitCode::from(2)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run_container_name;

    #[test]
    fn container_names_are_unique_per_run_and_runtime() {
        // Not the old fixed name another run (or a user) could own.
        let runc = run_container_name("runc");
        assert!(runc.starts_with("cua-e2e-bench-space-"));
        assert_ne!(runc, "cua-e2e-bench-space");
        assert!(runc.contains(&std::process::id().to_string()));
        assert!(runc.ends_with("-runc"));
        assert_ne!(runc, run_container_name("runsc"));
    }
}
