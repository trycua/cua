// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-spacesd doctor`: checks a running guest against what its image
//! claims (`/etc/cua-image/manifest.json`) and reports one machine-readable
//! verdict (`cua.env.v1.DiagnoseReport`, JSON schema version 1).
//!
//! The doctor is a *client* of cua-spacesd: every check goes through the
//! real `cua.env.v1` services (input through `ComputerService` and so
//! cua-driver, never xdotool), plus read-only probes of the guest's own
//! files for what no RPC exposes (token file modes, init units, compat
//! links). It runs either from the `cua-spacesd doctor` subcommand (against
//! the running service, or an in-process one when none is up) or inside the
//! server for `SystemService.Diagnose`.
//!
//! Safety: effectful checks run only with [`Effects::VirtualOnly`], only on
//! a sandboxed runtime (never a bare host), and only against fixture
//! windows the doctor launched itself (matched by pid and a per-run title).
//! Every check has a timeout and every loop a bound.

pub mod checks;
pub mod sys;

/// The image manifest model (shared with the host-side doctors).
pub use cua_spacesd_client::manifest;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use cua_spacesd_client::diagnose::{Check, Effects, Report, Severity, Status};
use cua_spacesd_client::{pb, SpacesdClient};
use tokio::sync::{mpsc, Mutex};

pub use manifest::{Loaded, Manifest};

/// Overall budget when the request sets none.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// Options for one run.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Warnings (and disallowed skips) fail.
    pub strict: bool,
    /// Check id prefixes to run (empty = all).
    pub only: Vec<String>,
    /// Check id prefixes to skip.
    pub skip: Vec<String>,
    /// Effectful checks allowed.
    pub effects: Effects,
    /// Manifest override (JSON bytes).
    pub expect_manifest: Option<Vec<u8>>,
    /// Overall budget.
    pub timeout: Option<Duration>,
    /// The caller's clock at request time.
    pub host_time: Option<SystemTime>,
    /// Directory for evidence files (screenshots, logs). `None` keeps them
    /// inline only.
    pub artifacts_dir: Option<std::path::PathBuf>,
    /// Manifest path when no override is given.
    pub manifest_path: Option<std::path::PathBuf>,
    /// Path of the cua-spacesd binary (for `--print-config` and compat
    /// probes); defaults to the current executable.
    pub spacesd_exe: Option<std::path::PathBuf>,
    /// Runtime the caller started the guest under ("container", "gvisor",
    /// "qemu", ...); the guest's own detection must agree.
    pub expect_runtime: Option<String>,
}

impl Options {
    /// From the RPC options.
    pub fn from_pb(options: &pb::DiagnoseOptions) -> Self {
        Options {
            strict: options.strict,
            only: options.only.clone(),
            skip: options.skip.clone(),
            effects: Effects::from_pb(options.effects),
            expect_manifest: (!options.expect_manifest.is_empty())
                .then(|| options.expect_manifest.clone()),
            timeout: options
                .timeout
                .as_ref()
                .map(|d| Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32))
                .filter(|d| !d.is_zero()),
            host_time: options.host_time.as_ref().map(|t| {
                std::time::UNIX_EPOCH
                    + Duration::new(t.seconds.max(0) as u64, t.nanos.max(0) as u32)
            }),
            ..Options::default()
        }
    }

    /// Whether check `id` is selected by `only` / `skip`.
    pub fn selects(&self, id: &str) -> bool {
        let hit = |prefix: &String| {
            id == prefix || id.starts_with(&format!("{prefix}.")) || prefix == "all"
        };
        (self.only.is_empty() || self.only.iter().any(hit)) && !self.skip.iter().any(hit)
    }
}

/// Where check events go (the RPC stream, or the CLI's live output).
pub type Events = mpsc::Sender<pb::DiagnoseResponse>;

/// State shared by every check of one run.
pub struct Ctx {
    /// Client of the spacesd under test.
    pub client: SpacesdClient,
    /// The token the client presents (never printed; used to look for
    /// leaks and to check that calls without it are refused).
    pub token: Option<String>,
    /// `cua-spacesd --print-config` output, once `meta.print_config` ran.
    pub print_config: std::sync::Mutex<Option<serde_json::Value>>,
    /// Linked cua-driver version, once `driver`/`mcp` learned it.
    pub driver_version: std::sync::Mutex<String>,
    /// `GetCapabilities` at the start of the run.
    pub caps: pb::GetCapabilitiesResponse,
    /// The image's claims.
    pub manifest: Loaded,
    /// Run options.
    pub options: Options,
    /// Unique per run (fixture titles, scratch paths).
    pub nonce: String,
    /// Scratch directory in the guest (created by the files check).
    pub scratch: String,
    /// Detected runtime name ("container", "gvisor", ...).
    pub runtime: String,
    /// Detected init system.
    pub init: String,
    /// Fixture windows this run launched (torn down at the end).
    pub fixtures: Mutex<checks::desktop::FixtureSet>,
    /// Fidelity facts gathered along the way.
    pub fidelity: Mutex<cua_spacesd_client::diagnose::Fidelity>,
    /// Hard deadline of the run.
    pub deadline: Instant,
    /// The guest clock when the run started (compared with the caller's
    /// `host_time`, which it sent at about the same moment).
    pub started_wall: SystemTime,
}

impl Ctx {
    /// Whether effectful checks may run here, and why not.
    pub fn effects_refusal(&self) -> Option<(&'static str, String)> {
        if self.options.effects != Effects::VirtualOnly {
            return Some((
                "effects_disabled",
                "effectful checks need --effects virtual".into(),
            ));
        }
        match self.runtime.as_str() {
            "container" | "gvisor" | "qemu" | "kubevirt" | "lume" | "hyperv" => None,
            other => Some((
                "bare_host_refused",
                format!(
                    "refusing effectful checks on runtime {other:?}: they only run inside a sandbox's virtual display"
                ),
            )),
        }
    }

    /// Whether the guest supports `feature`.
    pub fn supports(&self, feature: &str) -> bool {
        self.caps
            .features
            .iter()
            .any(|f| f.name == feature && f.supported)
    }

    /// The feature's limitation, if unsupported.
    pub fn limitation(&self, feature: &str) -> String {
        self.caps
            .features
            .iter()
            .find(|f| f.name == feature)
            .map(|f| {
                if f.supported {
                    String::new()
                } else if f.limitation.is_empty() {
                    "unsupported".into()
                } else {
                    f.limitation.clone()
                }
            })
            .unwrap_or_else(|| "not reported by GetCapabilities".into())
    }

    /// Time left in the run.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Guest OS family ("linux", "macos", "windows").
    pub fn os(&self) -> &'static str {
        match self
            .caps
            .os
            .as_ref()
            .and_then(|o| pb::OsFamily::try_from(o.family).ok())
        {
            Some(pb::OsFamily::Macos) => "macos",
            Some(pb::OsFamily::Windows) => "windows",
            _ => "linux",
        }
    }

    /// Whether the doctor runs on the guest itself (local file probes are
    /// guest facts). Always true: the doctor lives in the guest.
    pub fn local(&self) -> bool {
        true
    }
}

/// Collects results, applies selection, timeouts and severity, and streams
/// events.
pub struct Recorder<'a> {
    ctx: &'a Ctx,
    events: Option<&'a Events>,
    checks: Vec<Check>,
}

impl<'a> Recorder<'a> {
    fn new(ctx: &'a Ctx, events: Option<&'a Events>) -> Self {
        Self {
            ctx,
            events,
            checks: Vec::new(),
        }
    }

    /// Whether `id` is selected.
    pub fn wants(&self, id: &str) -> bool {
        self.ctx.options.selects(id)
    }

    /// Whether any check under `prefix` is selected.
    pub fn wants_group(&self, prefix: &str) -> bool {
        let o = &self.ctx.options;
        let skipped = o.skip.iter().any(|s| s == prefix);
        !skipped
            && (o.only.is_empty()
                || o.only.iter().any(|p| {
                    p == prefix
                        || p.starts_with(&format!("{prefix}."))
                        || prefix.starts_with(&format!("{p}."))
                        || p == "all"
                }))
    }

    async fn send(&self, event: pb::diagnose_response::Event) {
        if let Some(events) = self.events {
            // A closed receiver never stops the run (fixtures must still be
            // torn down); a slow one gets a bounded wait.
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                events.send(pb::DiagnoseResponse { event: Some(event) }),
            )
            .await;
        }
    }

    /// Runs one check with `timeout` (capped by the run's deadline). `claims`
    /// set the severity (see [`Loaded::severity`]).
    pub async fn run<F>(&mut self, id: &str, claims: &[&str], timeout: Duration, check: F)
    where
        F: std::future::Future<Output = Check>,
    {
        if !self.wants(id) {
            return;
        }
        self.send(pb::diagnose_response::Event::Started(
            pb::DiagnoseCheckStarted {
                id: id.to_owned(),
                group: id.split('.').next().unwrap_or_default().to_owned(),
            },
        ))
        .await;
        let started = Instant::now();
        let budget = scaled(timeout).min(self.ctx.remaining());
        let mut result = match tokio::time::timeout(budget, check).await {
            Ok(check) => check,
            Err(_) => Check::new(
                id,
                Status::Fail,
                format!("timed out after {} ms", budget.as_millis()),
            )
            .fix("the guest is too slow or a service hangs; see the spacesd log"),
        };
        result.id = id.to_owned();
        result.group = id.split('.').next().unwrap_or_default().to_owned();
        result.duration_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.finish(result, claims).await;
    }

    /// Records a check computed without I/O.
    pub async fn push(&mut self, check: Check, claims: &[&str]) {
        if !self.wants(&check.id) {
            return;
        }
        self.finish(check, claims).await;
    }

    /// Records `id` as skipped.
    pub async fn skip(&mut self, id: &str, claims: &[&str], reason: &str, message: String) {
        self.push(
            Check::new(id, Status::Skip, message).skip_reason(reason),
            claims,
        )
        .await;
    }

    /// Runs `check` unless effects are refused here (then records a skip).
    pub async fn run_effectful<F>(&mut self, id: &str, claims: &[&str], timeout: Duration, check: F)
    where
        F: std::future::Future<Output = Check>,
    {
        match self.ctx.effects_refusal() {
            Some((reason, message)) => self.skip(id, claims, reason, message).await,
            None => self.run(id, claims, timeout, check).await,
        }
    }

    async fn finish(&mut self, mut check: Check, claims: &[&str]) {
        let severity = self.ctx.manifest.severity(claims);
        check.severity = severity;
        check.claimed_by = claims
            .iter()
            .filter(|c| self.ctx.manifest.severity(&[c]) != Severity::Info || **c == "core")
            .map(|c| (*c).to_owned())
            .collect();
        if check.group.is_empty() {
            check.group = check.id.split('.').next().unwrap_or_default().to_owned();
        }
        self.send(pb::diagnose_response::Event::Check(check.to_pb()))
            .await;
        self.checks.push(check);
    }

    /// Checks recorded so far.
    pub fn checks(&self) -> &[Check] {
        &self.checks
    }
}

/// Connects to a running spacesd and runs the doctor against it.
pub async fn diagnose(
    url: &str,
    token: Option<String>,
    options: Options,
    events: Option<Events>,
) -> Report {
    let started = Instant::now();
    let started_wall = SystemTime::now();
    let mut connect = match cua_spacesd_client::ConnectOptions::parse(url) {
        Ok(o) => o,
        Err(error) => return connect_failure(url, error.to_string(), &options, started),
    };
    let token_for_ctx = token.clone();
    connect.token = token;
    connect = connect.probe_timeout(Duration::from_secs(10));
    let client = match SpacesdClient::connect(connect).await {
        Ok(client) => client,
        Err(error) => return connect_failure(url, error.to_string(), &options, started),
    };
    let mut report = run(client, token_for_ctx, options, events.as_ref()).await;
    report.started_at = Some(timestamp(started_wall));
    report
}

/// Stretch factor for check budgets and fixture waits, from
/// `CUA_DOCTOR_TIMEOUT_SCALE` (1 to 20, default 1). For software-emulated
/// guests (QEMU TCG), where every step runs several times slower; the run's
/// overall `--timeout` still applies.
pub fn timeout_scale() -> f64 {
    static SCALE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        parse_timeout_scale(std::env::var("CUA_DOCTOR_TIMEOUT_SCALE").ok().as_deref())
    })
}

fn parse_timeout_scale(value: Option<&str>) -> f64 {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map_or(1.0, |v| v.clamp(1.0, 20.0))
}

/// `timeout` stretched by [`timeout_scale`].
pub fn scaled(timeout: Duration) -> Duration {
    timeout.mul_f64(timeout_scale())
}

fn timestamp(t: SystemTime) -> pbjson_types::Timestamp {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    pbjson_types::Timestamp {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

fn connect_failure(url: &str, error: String, options: &Options, started: Instant) -> Report {
    let mut report = Report {
        schema_version: cua_spacesd_client::diagnose::SCHEMA_VERSION,
        producer: "cua-spacesd".into(),
        started_at: Some(timestamp(SystemTime::now())),
        checks: vec![Check {
            severity: Severity::Required,
            claimed_by: vec!["core".into()],
            ..Check::new(
                "meta.spacesd.reachable",
                Status::Fail,
                format!("cannot reach cua-spacesd at {url}: {error}"),
            )
            .fix("start the service (supervisorctl start cua-spacesd / systemctl start cua-spacesd) or pass --target and --token-file")
        }],
        ..Report::default()
    };
    report.finalize(options.strict, started.elapsed());
    report
}

/// Runs every selected check with `client` and returns the finished report.
pub async fn run(
    client: SpacesdClient,
    token: Option<String>,
    options: Options,
    events: Option<&Events>,
) -> Report {
    let started = Instant::now();
    let started_wall = SystemTime::now();
    let budget = options.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let caps = match client.refresh_capabilities().await {
        Ok(caps) => caps,
        Err(error) => {
            return connect_failure(
                &client.endpoint().to_string(),
                error.to_string(),
                &options,
                started,
            );
        }
    };
    let manifest = match &options.expect_manifest {
        Some(bytes) => Loaded::parse(bytes, "request"),
        None => Loaded::read(
            options
                .manifest_path
                .as_deref()
                .unwrap_or(std::path::Path::new(manifest::local_path())),
        ),
    };
    let runtime = runtime_name(&caps);
    let init = sys::detect_init();
    let nonce = format!(
        "{:x}{:04x}",
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        std::process::id() & 0xffff
    );
    let scratch = sys::scratch_dir(&nonce);
    let ctx = Arc::new(Ctx {
        client,
        token,
        print_config: std::sync::Mutex::new(None),
        driver_version: std::sync::Mutex::new(String::new()),
        caps,
        manifest,
        options,
        nonce,
        scratch,
        runtime,
        init,
        fixtures: Mutex::new(Default::default()),
        fidelity: Mutex::new(Default::default()),
        deadline: started + budget,
        started_wall,
    });
    let mut recorder = Recorder::new(&ctx, events);
    checks::run_all(&ctx, &mut recorder).await;
    // Always tear down what this run started, even after a timeout.
    checks::desktop::teardown(&ctx).await;
    let checks = recorder.checks;
    let fidelity = checks::fidelity::collect(&ctx).await;
    // The running daemon's revision (from its own health_report), else
    // this doctor's.
    let running_git_sha = checks
        .iter()
        .find(|c| c.id == cua_spacesd_client::expect::IDENTITY_CHECK)
        .and_then(|c| c.facts.get("cua-spacesd.git_sha"))
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| crate::checks::meta::build_git_sha(&ctx));
    let mut report = Report {
        schema_version: cua_spacesd_client::diagnose::SCHEMA_VERSION,
        producer: "cua-spacesd".into(),
        started_at: None,
        spacesd: Some(cua_spacesd_client::diagnose::Spacesd {
            version: ctx.caps.version.clone(),
            protocol_revision: ctx.caps.protocol_revision,
            git_sha: running_git_sha,
            cua_driver_version: checks::driver::linked_version(&ctx).await,
        }),
        image: cua_spacesd_client::diagnose::Image {
            name: ctx.manifest.manifest.name.clone(),
            reference: ctx.manifest.manifest.reference.clone(),
            variant: ctx.manifest.manifest.variant.clone(),
            os: if ctx.manifest.present() {
                ctx.manifest.manifest.os.clone()
            } else {
                ctx.os().to_owned()
            },
            manifest_sha256: ctx.manifest.sha256.clone(),
            manifest_source: ctx.manifest.source.clone(),
        },
        environment: cua_spacesd_client::diagnose::Environment {
            runtime: ctx.runtime.clone(),
            runtime_detail: ctx.caps.runtime_detail.clone(),
            arch: arch_name(&ctx.caps),
            init: ctx.init.clone(),
            display_server: display_name(&ctx.caps),
            os: ctx
                .caps
                .os
                .as_ref()
                .map(|o| format!("{} {}", o.name, o.version).trim().to_owned())
                .unwrap_or_default(),
        },
        summary: Default::default(),
        checks,
        fidelity,
    };
    report.finalize(ctx.options.strict, started.elapsed());
    report
}

/// Lowercase runtime name of the capabilities.
pub fn runtime_name(caps: &pb::GetCapabilitiesResponse) -> String {
    match pb::Runtime::try_from(caps.runtime).unwrap_or_default() {
        pb::Runtime::Kubevirt => "kubevirt",
        pb::Runtime::Gvisor => "gvisor",
        pb::Runtime::Lume => "lume",
        pb::Runtime::Qemu => "qemu",
        pb::Runtime::Container => "container",
        pb::Runtime::Bare => "bare",
        pb::Runtime::Hyperv => "hyperv",
        pb::Runtime::Unknown | pb::Runtime::Unspecified => "unknown",
    }
    .to_owned()
}

pub(crate) fn arch_name(caps: &pb::GetCapabilitiesResponse) -> String {
    match pb::Architecture::try_from(caps.arch).unwrap_or_default() {
        pb::Architecture::X8664 => "x86_64",
        pb::Architecture::Arm64 => "arm64",
        pb::Architecture::Unspecified => "",
    }
    .to_owned()
}

fn display_name(caps: &pb::GetCapabilitiesResponse) -> String {
    match pb::DisplayServer::try_from(caps.display_server).unwrap_or_default() {
        pb::DisplayServer::X11 => "x11",
        pb::DisplayServer::Wayland => "wayland",
        pb::DisplayServer::Quartz => "quartz",
        pb::DisplayServer::Win32 => "win32",
        pb::DisplayServer::None => "none",
        pb::DisplayServer::Unspecified => "",
    }
    .to_owned()
}

/// The `Diagnoser` linked into cua-spacesd's server for
/// `SystemService.Diagnose`.
pub struct ServerDiagnoser {
    /// Manifest location (default [`manifest::local_path`]).
    pub manifest_path: Option<std::path::PathBuf>,
    /// Evidence directory.
    pub artifacts_dir: Option<std::path::PathBuf>,
}

#[tonic::async_trait]
impl cua_spacesd_server::services::diagnose::Diagnoser for ServerDiagnoser {
    async fn diagnose(
        &self,
        target: cua_spacesd_server::services::diagnose::DiagnoseTarget,
        options: pb::DiagnoseOptions,
        events: mpsc::Sender<pb::DiagnoseResponse>,
    ) -> pb::DiagnoseReport {
        let mut options = Options::from_pb(&options);
        options.manifest_path = self.manifest_path.clone();
        options.artifacts_dir = self.artifacts_dir.clone();
        diagnose(&target.url, target.token, options, Some(events))
            .await
            .to_pb()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn timeout_scale_is_bounded_and_defaults_to_one() {
        use super::parse_timeout_scale;
        assert_eq!(parse_timeout_scale(None), 1.0);
        assert_eq!(parse_timeout_scale(Some("4")), 4.0);
        assert_eq!(parse_timeout_scale(Some(" 2.5 ")), 2.5);
        assert_eq!(parse_timeout_scale(Some("0.1")), 1.0);
        assert_eq!(parse_timeout_scale(Some("1000")), 20.0);
        assert_eq!(parse_timeout_scale(Some("inf")), 1.0);
        assert_eq!(parse_timeout_scale(Some("fast")), 1.0);
    }
    use super::*;

    #[test]
    fn selection_matches_ids_and_prefixes() {
        let mut o = Options::default();
        assert!(o.selects("stream.encode.openh264"));
        o.only = vec!["stream".into()];
        assert!(o.selects("stream.encode.openh264"));
        assert!(!o.selects("streaming.x"));
        assert!(!o.selects("auth.mode"));
        o.skip = vec!["stream.encode".into()];
        assert!(!o.selects("stream.encode.openh264"));
        assert!(o.selects("stream.media.desktop"));
    }

    #[test]
    fn options_from_rpc() {
        let o = Options::from_pb(&pb::DiagnoseOptions {
            strict: true,
            effects: pb::DiagnoseEffects::VirtualOnly as i32,
            timeout: Some(pbjson_types::Duration {
                seconds: 30,
                nanos: 0,
            }),
            host_time: Some(pbjson_types::Timestamp {
                seconds: 10,
                nanos: 0,
            }),
            expect_manifest: b"{}".to_vec(),
            ..Default::default()
        });
        assert!(o.strict);
        assert_eq!(o.effects, Effects::VirtualOnly);
        assert_eq!(o.timeout, Some(Duration::from_secs(30)));
        assert_eq!(
            o.host_time,
            Some(std::time::UNIX_EPOCH + Duration::from_secs(10))
        );
        assert_eq!(o.expect_manifest.as_deref(), Some(&b"{}"[..]));
        assert_eq!(Options::from_pb(&Default::default()).effects, Effects::None);
    }
}
