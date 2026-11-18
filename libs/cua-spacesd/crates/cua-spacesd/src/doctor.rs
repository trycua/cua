// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-spacesd doctor`: runs the image self-test and prints one report.
//!
//! Targets the running service on loopback (the token from `CUA_ENV_TOKEN`
//! or the token files the image uses), or `--target`. When nothing listens
//! and no target was given, it starts an in-process server on an ephemeral
//! loopback port with a throwaway token, so the libraries are checked even
//! when the service is down (the report says so in `meta.target`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, ValueEnum};
use cua_spacesd_client::diagnose::{Check, Effects, Report, Severity, Status};
use cua_spacesd_server::config::ServerConfig;
use cua_spacesd_server::{ServerBuilder, ServerContext};

/// Output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// `[ok] / [warn] / [fail] / [skip]` lines.
    Human,
    /// The report, JSON schema version 1.
    Json,
    /// JUnit XML (one testsuite per group).
    Junit,
}

/// Which checks may act on the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum EffectsArg {
    /// Read-only.
    None,
    /// Effects only on fixture windows the doctor launches, inside the
    /// sandbox's virtual display (refused on a bare host).
    Virtual,
}

#[derive(Args, Debug, Clone)]
pub struct DoctorArgs {
    /// Fail on warnings too (and on skipped required checks, except skips
    /// the report marks as allowed, such as deferred hardware encoders).
    #[arg(long)]
    pub strict: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
    /// Same as `--format json`.
    #[arg(long, conflicts_with = "format")]
    json: bool,
    /// Which checks may act on the guest.
    #[arg(long, value_enum, default_value_t = EffectsArg::None)]
    effects: EffectsArg,
    /// Check against this manifest instead of /etc/cua-image/manifest.json.
    #[arg(long, value_name = "FILE")]
    expect_manifest: Option<PathBuf>,
    /// Run only these check ids or groups (comma separated or repeated).
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Skip these check ids or groups.
    #[arg(long, value_delimiter = ',')]
    pub skip: Vec<String>,
    /// Overall budget in seconds.
    #[arg(long, default_value_t = 180)]
    pub timeout: u64,
    /// cua-spacesd to check (default: the local service on CUA_ENV_PORT).
    #[arg(long, env = "CUA_DOCTOR_TARGET")]
    target: Option<String>,
    /// Token file for --target (default: CUA_ENV_TOKEN, then the image's
    /// token files).
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// Also write the JSON report here.
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Also write JUnit XML here.
    #[arg(long, value_name = "FILE")]
    junit: Option<PathBuf>,
    /// Write evidence (screenshots) into this directory.
    #[arg(long, value_name = "DIR")]
    artifacts: Option<PathBuf>,
    /// Check an in-process server instead of the running service.
    #[arg(long)]
    in_process: bool,
    /// Print each check as it finishes (stderr).
    #[arg(long)]
    progress: bool,
    /// The runtime the guest was started under (container, gvisor, qemu,
    /// kubevirt, lume); the guest's own detection must agree.
    #[arg(long)]
    expect_runtime: Option<String>,
    /// Fail unless the running build of COMPONENT is WANT:
    /// `cua-driver=sha256:<hex>`, `cua-spacesd=git:<sha>`, `cua-driver=0.3.1`
    /// (repeatable). Components: cua-spacesd, cua-driver, or an overlay name.
    #[arg(long = "expect", value_name = "COMPONENT=WANT")]
    pub expect: Vec<String>,
}

impl DoctorArgs {
    /// Effective output format.
    pub fn format(&self) -> Format {
        if self.json {
            Format::Json
        } else {
            self.format
        }
    }
}

/// Token files the image uses, most specific first.
#[cfg(not(windows))]
const TOKEN_FILES: &[&str] = &[
    "/run/cua-env/env-token",
    "/run/cua/env-token",
    "/etc/cua/env-token",
];

/// Token file of the Windows image (libs/images/windows-2022): the first
/// Init persists the token here.
#[cfg(windows)]
const TOKEN_FILES: &[&str] = &[r"C:\ProgramData\cua\spacesd\token"];

fn read_token(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

fn local_token(explicit: Option<&PathBuf>) -> Option<String> {
    if let Some(path) = explicit {
        return read_token(path);
    }
    if let Some(token) = std::env::var("CUA_ENV_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
    {
        return Some(token.trim().to_owned());
    }
    TOKEN_FILES
        .iter()
        .find_map(|f| read_token(std::path::Path::new(f)))
        .or_else(|| {
            // The macOS image keeps the token in the desktop user's home
            // (libs/images/macos/files/start-spacesd.sh).
            if !cfg!(target_os = "macos") {
                return None;
            }
            let home = std::env::var_os("HOME")?;
            read_token(&PathBuf::from(home).join(".cua/spacesd/token"))
        })
}

fn local_target() -> String {
    let port = std::env::var("CUA_ENV_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(cua_proto::SPACESD_DEFAULT_PORT);
    format!("http://127.0.0.1:{port}")
}

/// Starts a full server (desktop services when available) on an ephemeral
/// loopback port; returns its URL and token.
async fn in_process_server() -> Result<(String, String), String> {
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let data_dir = std::env::temp_dir().join(format!("cua-doctor-inproc-{}", std::process::id()));
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        token_source: "generated (doctor)".into(),
        data_dir,
        media_quic_port: 0,
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some(token.clone()));
    let mut builder = ServerBuilder::new(ctx.clone());
    match cua_spacesd_desktop::grpc::DesktopServiceProvider::for_host(ctx.clone()) {
        Ok(provider) => builder = builder.provider(Arc::new(provider)),
        Err(error) => {
            tracing::warn!(%error, "desktop services unavailable in-process");
            if let Some(tools) = crate::platform_tools() {
                builder = builder.tools(tools);
            }
        }
    }
    let server = builder.build();
    let addr = cua_spacesd_server::spawn_local(server)
        .await
        .map_err(|e| format!("in-process server: {e}"))?;
    Ok((format!("http://{addr}"), token))
}

/// Whether nothing answers at `url` (connection refused), as opposed to
/// something answering badly.
async fn nothing_listens(url: &str) -> bool {
    let host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = host.split('/').next().unwrap_or(host);
    tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(host))
        .await
        .map(|r| r.is_err())
        .unwrap_or(true)
}

/// Runs the doctor; returns the process exit code.
pub async fn run(args: DoctorArgs) -> i32 {
    init_quiet_tracing();
    let expectations = match cua_spacesd_client::expect::parse_all(&args.expect) {
        Ok(e) => e,
        Err(error) => {
            eprintln!("cua-spacesd doctor: {error}");
            return 2;
        }
    };
    let expect_manifest = match &args.expect_manifest {
        None => None,
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                eprintln!("cua-spacesd doctor: reading {}: {error}", path.display());
                return 2;
            }
        },
    };
    let options = cua_spacesd_doctor::Options {
        strict: args.strict,
        only: args.only.clone(),
        skip: args.skip.clone(),
        effects: match args.effects {
            EffectsArg::None => Effects::None,
            EffectsArg::Virtual => Effects::VirtualOnly,
        },
        expect_manifest,
        timeout: Some(Duration::from_secs(args.timeout.max(10))),
        host_time: None,
        artifacts_dir: args.artifacts.clone(),
        manifest_path: None,
        spacesd_exe: std::env::current_exe().ok(),
        expect_runtime: args.expect_runtime.clone(),
    };
    let (url, token, target_kind) = if args.in_process {
        match in_process_server().await {
            Ok((url, token)) => (url, Some(token), "in-process"),
            Err(error) => {
                eprintln!("cua-spacesd doctor: {error}");
                return 2;
            }
        }
    } else if let Some(target) = &args.target {
        (
            target.clone(),
            local_token(args.token_file.as_ref()),
            "target",
        )
    } else {
        let url = local_target();
        if nothing_listens(&url).await {
            match in_process_server().await {
                Ok((url, token)) => (url, Some(token), "in-process (the service is not running)"),
                Err(error) => {
                    eprintln!("cua-spacesd doctor: nothing listens on {url} and {error}");
                    return 2;
                }
            }
        } else {
            (
                url,
                local_token(args.token_file.as_ref()),
                "running service",
            )
        }
    };

    let (sender, mut receiver) =
        tokio::sync::mpsc::channel::<cua_spacesd_client::pb::DiagnoseResponse>(64);
    let progress = args.progress;
    let printer = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            if !progress {
                continue;
            }
            if let Some(cua_spacesd_client::pb::diagnose_response::Event::Check(check)) =
                event.event
            {
                eprintln!(
                    "  {:<6} {}",
                    match cua_spacesd_client::pb::CheckStatus::try_from(check.status)
                        .unwrap_or_default()
                    {
                        cua_spacesd_client::pb::CheckStatus::Pass => "ok",
                        cua_spacesd_client::pb::CheckStatus::Warn => "warn",
                        cua_spacesd_client::pb::CheckStatus::Fail => "fail",
                        _ => "skip",
                    },
                    check.id
                );
            }
        }
    });
    let target = url.clone();
    let run = tokio::spawn(async move {
        cua_spacesd_doctor::diagnose(&target, token, options, Some(sender)).await
    });
    let mut report = match run.await {
        Ok(report) => report,
        Err(panic) => {
            eprintln!("cua-spacesd doctor: crashed: {panic}");
            return 2;
        }
    };
    let _ = printer.await;
    if !expectations.is_empty() {
        cua_spacesd_client::expect::apply(&mut report, &expectations);
        let took = Duration::from_millis(report.summary.duration_ms as u64);
        report.finalize(args.strict, took);
    }
    report.checks.insert(
        0,
        Check {
            severity: Severity::Info,
            ..Check::new(
                "meta.target",
                Status::Pass,
                format!("checked {target_kind} at {url}"),
            )
            .fact("kind", target_kind)
        },
    );
    report.summary.pass += 1;
    emit(&args, &report)
}

fn emit(args: &DoctorArgs, report: &Report) -> i32 {
    let json = report.to_json();
    if let Some(path) = &args.out {
        if let Err(error) = std::fs::write(path, &json) {
            eprintln!("cua-spacesd doctor: writing {}: {error}", path.display());
            return 2;
        }
    }
    if let Some(path) = &args.junit {
        if let Err(error) = std::fs::write(path, report.to_junit()) {
            eprintln!("cua-spacesd doctor: writing {}: {error}", path.display());
            return 2;
        }
    }
    match args.format() {
        Format::Human => print!("{}", report.to_human()),
        Format::Json => println!("{json}"),
        Format::Junit => print!("{}", report.to_junit()),
    }
    report.exit_code()
}

fn init_quiet_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CUA_ENV_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_lookup_prefers_the_explicit_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t");
        std::fs::write(&file, "  abc \n").unwrap();
        assert_eq!(local_token(Some(&file)).as_deref(), Some("abc"));
        std::fs::write(&file, "\n").unwrap();
        assert_eq!(local_token(Some(&file)), None);
    }

    #[test]
    fn default_target_is_loopback() {
        assert!(local_target().starts_with("http://127.0.0.1:"));
    }
}
