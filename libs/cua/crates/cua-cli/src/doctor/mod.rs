//! `cua doctor [REF]`: one report for a sandbox, merging three views.
//!
//! - **host**: can this machine run the sandbox's runtime (the runtime
//!   doctor's backends, gVisor, KVM/HVF, emulation, image cache space);
//! - **image**: what the registry says (resolved digest, variant, OS,
//!   `ai.cua.spacesd`), when the image is known;
//! - **guest**: `SystemService.Diagnose` through the SDK (every ref form:
//!   `local:`, `cloud:`, `direct:`, `relay:`, a bare name or a URL), with
//!   the caller's clock for the skew check; or, for images without
//!   cua-spacesd, the doctor shim against the image's own API.
//!
//! Cross checks compare the views (the registry's `ai.cua.spacesd` against a
//! spacesd answering, variant and OS against the guest's manifest, the
//! sandbox's runtime against the guest's detection). The output is the same
//! report schema (v1) `cua-spacesd doctor` writes, with `host.*`, `image.*`
//! and `cross.*` checks added. Read-only unless `--effects virtual`.

pub mod host;
pub mod identity;
pub mod parity;
pub mod shim;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use clap::{Args, ValueEnum};
use cua_sdk::{Cua, CuaError};
use cua_spacesd_client::diagnose::{Check, Report, Severity, Status};
use cua_spacesd_client::manifest::Loaded;

/// Output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Grouped `[ok]/[warn]/[fail]/[skip]` lines.
    Human,
    /// JSON (report schema version 1).
    Json,
    /// JUnit XML.
    Junit,
}

/// Which guest checks may act.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum EffectsArg {
    /// Read-only.
    None,
    /// Doctor-owned fixture windows inside the sandbox only.
    Virtual,
}

#[derive(Args, Debug, Clone)]
pub struct DoctorArgs {
    /// Sandbox to check: `local:<name>`, `cloud:<name>`, `direct:<host:port>`,
    /// a bare name, or a spacesd URL. Omit for the host checks only.
    pub target: Option<String>,
    /// Image to resolve for the image checks (default: the sandbox's).
    #[arg(long)]
    pub image: Option<String>,
    /// Architecture to resolve the image for (default: the sandbox's, else
    /// this host's).
    #[arg(long)]
    pub arch: Option<String>,
    /// Fail on warnings and on required checks that were skipped.
    #[arg(long)]
    pub strict: bool,
    /// Output format (`--json` selects json).
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
    /// Which guest checks may act on the guest.
    #[arg(long, value_enum, default_value_t = EffectsArg::None)]
    pub effects: EffectsArg,
    /// Guest check ids or groups to run (comma separated).
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Guest check ids or groups to skip.
    #[arg(long, value_delimiter = ',')]
    pub skip: Vec<String>,
    /// Guest budget in seconds.
    #[arg(long, default_value_t = 240)]
    pub timeout: u64,
    /// spacesd token for a URL target (default: CUA_ENV_TOKEN).
    #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
    /// Check an image without cua-spacesd through its own API instead:
    /// `computer-server=<url>`, `osworld=<url>` or `mcp=<url>`.
    #[arg(long, value_name = "KIND=URL")]
    pub shim: Option<String>,
    /// The image's claims (manifest JSON), for the shim or to override the
    /// guest's own.
    #[arg(long, value_name = "FILE")]
    pub expect_manifest: Option<PathBuf>,
    /// Also write the JSON report here.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Also write JUnit XML here.
    #[arg(long, value_name = "FILE")]
    pub junit: Option<PathBuf>,
    /// Skip the host checks.
    #[arg(long)]
    pub no_host: bool,
    /// Fail unless the build running in the guest is the expected one:
    /// COMPONENT=WANT with WANT `sha256:<hex>` (the executable), `git:<sha>`
    /// or a version (repeatable). Components: cua-spacesd, cua-driver, or
    /// an overlay name. `cua sb create --overlay` prints the sha256 to pass.
    #[arg(long = "expect", value_name = "COMPONENT=WANT")]
    pub expect: Vec<String>,
}

impl DoctorArgs {
    /// Whether resolving the target may need Fleet credentials.
    pub fn needs_fleet(&self) -> bool {
        self.target.as_deref().is_some_and(|t| {
            !(t.starts_with("http://")
                || t.starts_with("https://")
                || t.starts_with("direct:")
                || t.starts_with("local:"))
        })
    }
}

fn is_url(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

fn read_manifest(path: &Option<PathBuf>) -> Result<Option<Vec<u8>>, CuaError> {
    match path {
        None => Ok(None),
        Some(p) => std::fs::read(p)
            .map(Some)
            .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", p.display()))),
    }
}

fn timestamp(t: SystemTime) -> pbjson_types::Timestamp {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    pbjson_types::Timestamp {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

/// The guest part: Diagnose through the SDK (or the shim), plus what the
/// SDK knows about the sandbox.
struct Guest {
    report: Option<Report>,
    runtime_type: Option<String>,
    image: Option<String>,
    arch: Option<String>,
    unavailable: Option<String>,
}

async fn guest_part(
    cua: &Arc<Cua>,
    args: &DoctorArgs,
    manifest: &Option<Vec<u8>>,
) -> Result<Guest, CuaError> {
    let Some(target) = args.target.clone() else {
        return Ok(Guest {
            report: None,
            runtime_type: None,
            image: None,
            arch: None,
            unavailable: None,
        });
    };
    let mut guest = Guest {
        report: None,
        runtime_type: None,
        image: None,
        arch: None,
        unavailable: None,
    };
    let client = if is_url(&target) {
        cua.spacesd(target.clone(), args.token.clone()).await
    } else {
        // Refs resolve through the SDK (local/cloud/direct/relay, bare names).
        if let Ok(info) = cua.sandboxes().get(target.clone()).await {
            guest.runtime_type = Some(info.runtime_type.clone());
            guest.image = info.image.clone();
            guest.arch = info.image_info.as_ref().and_then(|i| i.arch.clone());
        }
        crate::sandbox::env_of(cua, &target).await
    };
    let client = match client {
        Ok(c) => c,
        Err(error) => {
            guest.unavailable = Some(error.to_string());
            return Ok(guest);
        }
    };
    warm_up(client.inner()).await;
    let sent = SystemTime::now();
    let options = cua_proto::env::v1::DiagnoseOptions {
        strict: args.strict,
        only: args.only.clone(),
        skip: args.skip.clone(),
        effects: match args.effects {
            EffectsArg::None => cua_proto::env::v1::DiagnoseEffects::None,
            EffectsArg::Virtual => cua_proto::env::v1::DiagnoseEffects::VirtualOnly,
        } as i32,
        expect_manifest: manifest.clone().unwrap_or_default(),
        timeout: Some(pbjson_types::Duration {
            seconds: args.timeout as i64,
            nanos: 0,
        }),
        host_time: Some(timestamp(sent)),
    };
    // The first answer bounds when the request arrived.
    let mut first: Option<SystemTime> = None;
    match client
        .inner()
        .diagnose_report(options, |_| {
            first.get_or_insert_with(SystemTime::now);
        })
        .await
    {
        Ok(mut report) => {
            let answered = first.unwrap_or_else(SystemTime::now);
            if let Some(arrived) = report.started_at.as_ref().and_then(system_time) {
                for check in report.checks.iter_mut().filter(|c| c.id == "time.skew") {
                    bound_skew(check, sent, arrived, answered);
                }
            }
            guest.report = Some(report)
        }
        Err(error) => guest.unavailable = Some(error.to_string()),
    }
    Ok(guest)
}

/// Skew that warns and that fails (as `cua-spacesd doctor` judges it).
const SKEW_WARN_MS: i64 = 100;
const SKEW_FAIL_MS: i64 = 500;

fn signed_ms(a: SystemTime, b: SystemTime) -> i64 {
    match a.duration_since(b) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

fn system_time(t: &pbjson_types::Timestamp) -> Option<SystemTime> {
    let secs = u64::try_from(t.seconds).ok()?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, t.nanos.max(0) as u32))
}

/// Judges the guest's `time.skew` from both ends of the request. The guest
/// compares `sent` (the caller's clock when it sent the request) with
/// `arrived` (its own clock when the request arrived), so the request's
/// transit counts as skew: right after a resume a busy guest takes a second
/// or more to pick a request up, and a guest within 20 ms of NTP read as up
/// to 5.7 s ahead. The arrival happened between `sent` and `answered` (the
/// caller's clock at the first answer), so the guest's offset lies in
/// `[arrived - answered, arrived - sent]`; the check fails only when every
/// offset in that range would.
fn bound_skew(check: &mut Check, sent: SystemTime, arrived: SystemTime, answered: SystemTime) {
    let (lo, hi) = (signed_ms(arrived, answered), signed_ms(arrived, sent));
    let least = if lo <= 0 && 0 <= hi {
        0
    } else {
        lo.abs().min(hi.abs())
    };
    let status = match least {
        m if m > SKEW_FAIL_MS => Status::Fail,
        m if m > SKEW_WARN_MS => Status::Warn,
        _ => Status::Pass,
    };
    if status == check.status || hi - lo <= 1 {
        return;
    }
    check.status = status;
    check.message = format!(
        "guest clock is {lo:+} to {hi:+} ms from the caller's (the request took {} ms; \
         warn > {SKEW_WARN_MS}, fail > {SKEW_FAIL_MS})",
        hi - lo
    );
    check.facts.insert("skew_min_ms".into(), lo.to_string());
    check.facts.insert("skew_max_ms".into(), hi.to_string());
    if status == Status::Pass {
        check.remediation.clear();
    }
}

/// Opens the connection and lets the guest answer a few cheap calls before
/// `host_time` is stamped. The guest's clock check compares that stamp with
/// the moment the request arrives, so a cold connection (the handshake, a
/// guest just resumed and still busy) counted as clock skew: a guest within
/// 20 ms of NTP read as up to 5.7 s ahead right after a resume. Bounded: at
/// most three calls of 5 s each, stopping at the first quick answer.
async fn warm_up(client: &cua_spacesd_client::SpacesdClient) {
    for _ in 0..3 {
        let t = Instant::now();
        let answered = matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), client.health()).await,
            Ok(Ok(_))
        );
        if answered && t.elapsed() < std::time::Duration::from_millis(50) {
            return;
        }
    }
}

/// The `guest.available` check when no guest report came back: an
/// informational skip, or under `--strict` a required skip (which
/// `Report::finalize` turns into a failure: nothing was verified).
fn guest_unavailable_check(reason: &str, strict: bool) -> Check {
    let (severity, why) = if strict {
        (Severity::Required, "guest_unreachable")
    } else {
        (Severity::Info, "not_applicable")
    };
    Check {
        severity,
        ..Check::new("guest.available", Status::Skip, format!("no guest report: {reason}"))
            .skip_reason(why)
            .fix("start the sandbox's cua-spacesd, or for images without it pass --shim computer-server=<url>|osworld=<url>|mcp=<url>")
    }
}

/// Runs `cua doctor`; returns the exit code (0 pass, 1 fail).
pub async fn run(
    cua: &Arc<Cua>,
    args: DoctorArgs,
    json: bool,
    out: &mut dyn std::io::Write,
) -> Result<i32, CuaError> {
    let started = Instant::now();
    let expectations =
        cua_spacesd_client::expect::parse_all(&args.expect).map_err(CuaError::InvalidArgument)?;
    let manifest_bytes = read_manifest(&args.expect_manifest)?;
    let loaded = manifest_bytes
        .as_deref()
        .map(|b| Loaded::parse(b, "--expect-manifest"))
        .unwrap_or_default();

    // Guest: the shim, or Diagnose.
    let (mut report, guest_info) = if let Some(spec) = &args.shim {
        let target = shim::Target::parse(spec)?;
        let report = shim::run(
            &target,
            &loaded,
            args.strict,
            args.effects == EffectsArg::Virtual,
            args.token.clone(),
        )
        .await;
        (report, None)
    } else {
        let guest = guest_part(cua, &args, &manifest_bytes).await?;
        let report = guest.report.clone().unwrap_or_else(|| Report {
            schema_version: cua_spacesd_client::diagnose::SCHEMA_VERSION,
            producer: "cua doctor".into(),
            ..Report::default()
        });
        (report, Some(guest))
    };
    // Guests whose cua-spacesd predates build identities: measure them here.
    if args.shim.is_none()
        && let Some(target) = &args.target
        && guest_info.as_ref().is_some_and(|g| g.report.is_some())
    {
        identity::fill(cua, target, &mut report).await;
    }
    report.producer = if report.producer.is_empty() {
        "cua doctor".into()
    } else {
        format!("cua doctor + {}", report.producer)
    };
    if report.started_at.is_none() {
        report.started_at = Some(timestamp(SystemTime::now()));
    }

    let mut extra: Vec<Check> = Vec::new();
    if let Some(guest) = &guest_info
        && let Some(reason) = &guest.unavailable
    {
        // A sandbox without cua-spacesd is not a failure of the sandbox:
        // guest checks are simply unavailable (use --shim to run them).
        // Under --strict an unchecked guest is a failure, not a pass.
        extra.push(guest_unavailable_check(reason, args.strict));
    }
    // Image facts and cross checks.
    let image_ref = args
        .image
        .clone()
        .or_else(|| guest_info.as_ref().and_then(|g| g.image.clone()));
    let arch = args
        .arch
        .clone()
        .or_else(|| guest_info.as_ref().and_then(|g| g.arch.clone()))
        .unwrap_or_else(|| host::host_arch().to_owned());
    let runtime_type = guest_info.as_ref().and_then(|g| g.runtime_type.clone());
    if let Some(image) = &image_ref {
        extra.extend(host::image_checks(image, &arch, runtime_type.as_deref(), &report).await);
    }
    if let Some(runtime) = &runtime_type {
        extra.extend(host::cross_runtime(runtime, &report));
    }
    // Host checks.
    if !args.no_host {
        extra.extend(host::host_checks(cua, runtime_type.as_deref(), &arch).await);
    }
    let mut checks = extra;
    checks.append(&mut report.checks);
    report.checks = checks;
    // Expected builds (a guest that could not be reached fails them too).
    cua_spacesd_client::expect::apply(&mut report, &expectations);
    report.finalize(args.strict, started.elapsed());

    let json_text = report.to_json();
    if let Some(path) = &args.out {
        std::fs::write(path, &json_text)
            .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", path.display())))?;
    }
    if let Some(path) = &args.junit {
        std::fs::write(path, report.to_junit())
            .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", path.display())))?;
    }
    let format = if json { Format::Json } else { args.format };
    let text = match format {
        Format::Human => report.to_human(),
        Format::Json => json_text + "\n",
        Format::Junit => report.to_junit(),
    };
    let _ = out.write_all(text.as_bytes());
    Ok(report.exit_code())
}

/// `pass` / `fail`.
pub fn verdict(ok: bool) -> Status {
    if ok { Status::Pass } else { Status::Fail }
}

/// `cua doctor [REF]` or `cua doctor parity A B`.
#[derive(Args, Debug, Clone)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DoctorCmd {
    #[command(subcommand)]
    pub sub: Option<DoctorSub>,
    #[command(flatten)]
    pub args: DoctorArgs,
}

/// Doctor subcommands.
#[derive(clap::Subcommand, Debug, Clone)]
pub enum DoctorSub {
    /// Eval parity between two variants of one image (rootfs vs
    /// containerDisk): doctor on both, fidelity diff, task outcomes.
    #[command(after_help = "Examples:
  cua doctor parity local:linux-rootfs local:linux-vm
  cua doctor parity local:linux-rootfs local:linux-vm --out parity.json")]
    Parity(parity::ParityArgs),
}

/// Runs `cua doctor ...`.
pub async fn dispatch(
    cua: &Arc<Cua>,
    cmd: DoctorCmd,
    json: bool,
    out: &mut dyn std::io::Write,
) -> Result<i32, CuaError> {
    match cmd.sub {
        Some(DoctorSub::Parity(args)) => {
            let (result, pass) = parity::run(cua, &args).await?;
            if let Some(path) = &args.out {
                std::fs::write(
                    path,
                    serde_json::to_string_pretty(&result).unwrap_or_default(),
                )
                .map_err(|e| CuaError::InvalidArgument(format!("{}: {e}", path.display())))?;
            }
            let text = if json {
                serde_json::to_string_pretty(&result).unwrap_or_default() + "\n"
            } else {
                parity::markdown(&result)
            };
            let _ = out.write_all(text.as_bytes());
            Ok(if pass { 0 } else { 1 })
        }
        None => run(cua, cmd.args, json, out).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finalized(strict: bool) -> Report {
        let mut report = Report {
            checks: vec![guest_unavailable_check("connection refused", strict)],
            ..Report::default()
        };
        report.finalize(strict, std::time::Duration::ZERO);
        report
    }

    #[test]
    fn an_expectation_fails_an_unreachable_guest_even_without_strict() {
        let mut report = Report {
            checks: vec![guest_unavailable_check("connection refused", false)],
            ..Report::default()
        };
        let want =
            cua_spacesd_client::expect::parse_all(&["cua-driver=git:abcdef1".into()]).unwrap();
        cua_spacesd_client::expect::apply(&mut report, &want);
        report.finalize(false, std::time::Duration::ZERO);
        assert_eq!(report.summary.status, Status::Fail);
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.id == "expect.cua-driver.git" && c.status == Status::Fail)
        );
    }

    #[test]
    fn an_unreachable_guest_fails_only_under_strict() {
        let lenient = finalized(false);
        assert_eq!(lenient.checks[0].status, Status::Skip);
        assert_ne!(lenient.summary.status, Status::Fail);
        let strict = finalized(true);
        assert_eq!(strict.checks[0].status, Status::Fail);
        assert_eq!(strict.summary.status, Status::Fail);
    }
}

#[cfg(test)]
mod skew_tests {
    use super::*;
    use std::time::Duration;

    fn check(status: Status, ms: i64) -> Check {
        Check::new(
            "time.skew",
            status,
            format!("guest clock is {ms:+} ms from the caller's"),
        )
    }

    /// Right after a resume: the guest picked the request up 1.2 s after it
    /// was sent, and its clock is right. The one-way reading (+1200 ms)
    /// failed; bounded by the answer, the offset may be 0: pass.
    #[test]
    fn transit_is_not_skew() {
        let sent = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let arrived = sent + Duration::from_millis(1200);
        let answered = sent + Duration::from_millis(1250);
        let mut c = check(Status::Fail, 1200);
        bound_skew(&mut c, sent, arrived, answered);
        assert_eq!(c.status, Status::Pass, "{}", c.message);
        assert_eq!(c.facts["skew_min_ms"], "-50");
        assert_eq!(c.facts["skew_max_ms"], "1200");
    }

    /// A clock really 3 s ahead fails whatever the transit.
    #[test]
    fn real_skew_still_fails() {
        let sent = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // Arrived 200 ms after sending, on a clock 3 s ahead.
        let arrived = sent + Duration::from_millis(3200);
        let answered = sent + Duration::from_millis(250);
        let mut c = check(Status::Fail, 3200);
        bound_skew(&mut c, sent, arrived, answered);
        assert_eq!(c.status, Status::Fail);
        // Behind by 800 ms, answered quickly: still a failure.
        let arrived = sent - Duration::from_millis(790);
        let mut c = check(Status::Fail, -790);
        bound_skew(&mut c, sent, arrived, sent + Duration::from_millis(20));
        assert_eq!(c.status, Status::Fail, "{}", c.message);
        // 300 ms off at best: a warning.
        let arrived = sent + Duration::from_millis(400);
        let mut c = check(Status::Warn, 400);
        bound_skew(&mut c, sent, arrived, sent + Duration::from_millis(100));
        assert_eq!(c.status, Status::Warn, "{}", c.message);
    }
}
