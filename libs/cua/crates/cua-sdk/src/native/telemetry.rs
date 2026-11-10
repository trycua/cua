//! Anonymous usage telemetry for every language (cua-telemetry). The SDK records its own events (sandbox
//! create and delete, teleport, feature use); these calls let a binding
//! name its surface, let apps record their own fixed-vocabulary events,
//! and give users the switches.
//!
//! What is sent, what never is, and how to turn it off:
//! <https://cua.ai/docs/cua-sdk/concepts/telemetry>.

use std::time::{Duration, Instant};

use cua_telemetry::Captured;
use cua_telemetry::events::{self, Outcome};

use crate::{CuaError, Result};

/// Telemetry state.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TelemetryStatus {
    /// Usage events may be sent.
    pub enabled: bool,
    /// Why (`env CUA_TELEMETRY`, `env DO_NOT_TRACK`, `config <path>`, `CI
    /// environment ...`, `default`).
    pub source: String,
    /// `do_not_track`, `env`, `legacy_env`, `config`, `ci` or `default`.
    pub source_kind: String,
    /// Running in CI.
    pub is_ci: bool,
    /// The first 8 characters of the anonymous install id, if one exists.
    pub install_id: Option<String>,
    /// The first-run notice was shown on this machine.
    pub notice_shown: bool,
    /// Where batches go.
    pub endpoint: String,
    /// Which Cua program events are attributed to.
    pub product: String,
    /// The exact properties every event carries, as JSON.
    pub envelope_json: String,
}

fn io(e: impl std::fmt::Display) -> CuaError {
    CuaError::Internal(e.to_string())
}

fn t() -> &'static cua_telemetry::Telemetry {
    cua_telemetry::global()
}

/// Telemetry state, and exactly what every event carries.
#[uniffi::export]
pub fn telemetry_status() -> TelemetryStatus {
    let s = t().status();
    TelemetryStatus {
        enabled: s.enabled,
        source: s.source,
        source_kind: s.source_kind,
        is_ci: s.is_ci,
        install_id: s.install_id,
        notice_shown: s.notice_shown,
        endpoint: s.endpoint,
        product: s.product,
        envelope_json: serde_json::Value::Object(t().envelope_preview()).to_string(),
    }
}

/// Turns usage telemetry on or off for this machine (`$CUA_HOME/config.toml`,
/// the same switch as `cua telemetry off` and the Spaces app setting). The
/// environment (`DO_NOT_TRACK`, `CUA_TELEMETRY`) still wins.
#[uniffi::export]
pub fn telemetry_set_enabled(enabled: bool) -> Result<TelemetryStatus> {
    t().set_enabled(enabled).map_err(io)?;
    Ok(telemetry_status())
}

/// Deletes the anonymous install id and its salt.
#[uniffi::export]
pub fn telemetry_reset_id() -> Result<()> {
    t().reset_id().map(|_| ()).map_err(io)
}

/// The last `limit` events queued or sent from this machine, exactly as
/// sent, as JSON (`[{status, payload}]`).
#[uniffi::export]
pub fn telemetry_show_last(limit: u32) -> String {
    serde_json::Value::Array(t().show_last(limit as usize)).to_string()
}

/// Every event and property that may be sent, as JSON.
#[uniffi::export]
pub fn telemetry_schema_json() -> String {
    cua_telemetry::schema::to_json().to_string()
}

/// Names the SDK surface events are attributed to. Bindings call it at
/// import: `sdk_python`, `sdk_typescript`, `sdk_swift`, `sdk_kotlin`;
/// apps pass their own product (`spaces_app`). Unknown names are ignored.
#[uniffi::export]
pub fn telemetry_set_surface(surface: String, version: String) {
    t().set_product(&surface, &version);
}

/// The first-run notice text (an app shows it in its own UI, then calls
/// [`telemetry_acknowledge_notice`]).
#[uniffi::export]
pub fn telemetry_notice_text() -> String {
    cua_telemetry::notice::TEXT.to_string()
}

/// Records that the first-run notice was shown in the app's UI (nothing is
/// sent from a machine before that). Apps that show the notice themselves
/// call [`telemetry_use_app_notice`] first so the SDK does not print it.
#[uniffi::export]
pub fn telemetry_acknowledge_notice() {
    t().acknowledge_notice();
}

/// The host app shows the first-run notice itself (no stderr notice).
#[uniffi::export]
pub fn telemetry_use_app_notice() {
    t().set_notice_mode(cua_telemetry::NoticeMode::External);
}

// ---- Linking usage data to the signed-in account (CUA-1881) ------------------

/// The one-time notice for machines that saw the older "anonymous" wording,
/// shown before anything is linked to the account. Both buttons close it;
/// the second also turns usage data off.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TelemetryLinkNotice {
    pub title: String,
    pub body: String,
    /// Keeps usage data on ("Got it").
    pub ok_label: String,
    /// Turns usage data off ("Turn off usage data").
    pub turn_off_label: String,
    /// The link to what is collected.
    pub details_label: String,
    pub docs_url: String,
}

/// The one-time linking notice's words.
#[uniffi::export]
pub fn telemetry_link_notice() -> TelemetryLinkNotice {
    use cua_telemetry::link;
    TelemetryLinkNotice {
        title: link::NOTICE_TITLE.into(),
        body: link::NOTICE_BODY.into(),
        ok_label: link::NOTICE_OK.into(),
        turn_off_label: link::NOTICE_TURN_OFF.into(),
        details_label: link::NOTICE_DETAILS.into(),
        docs_url: cua_telemetry::notice::DOCS_URL.into(),
    }
}

/// Whether the app should show the one-time linking notice now: usage data
/// is on, the account offered linking, and this machine has not shown it.
#[uniffi::export]
pub fn telemetry_link_notice_due() -> bool {
    t().link_notice_due()
}

/// The person closed the linking notice (either button). Linking starts
/// with the app's next launch, never in the process that showed it.
#[uniffi::export]
pub fn telemetry_acknowledge_link_notice() {
    t().acknowledge_link_notice();
}

/// Where linking stands: `off`, `not_offered`, `notice_due` or `linked`.
#[uniffi::export]
pub fn telemetry_link_state() -> String {
    t().link_state().as_str().into()
}

/// Asks the Spaces API for the signed-in account's opaque id and hands it
/// to telemetry (none unlinks). Call at launch and after a sign-in; the
/// SDK's own sign-in already does. Returns the link state. Best effort:
/// offline or signed out, nothing changes.
#[cfg(feature = "host")]
#[uniffi::export]
pub async fn telemetry_sync_account() -> String {
    let _ = crate::native::run(async {
        sync_account(&cua_auth::Session::from_env()).await;
        Ok(())
    })
    .await;
    telemetry_link_state()
}

/// [`telemetry_sync_account`] for `session`.
#[cfg(feature = "host")]
pub(crate) async fn sync_account(session: &cua_auth::Session) {
    sync_account_with(t(), || analytics_id(session)).await;
}

/// How often a signed-in SDK process asks the Spaces API for the account
/// id again ([`sync_account_if_due`]).
#[cfg(any(feature = "host", test))]
pub(crate) const ACCOUNT_CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

/// A process that was signed in before the server offered linking links
/// too: at most every [`ACCOUNT_CHECK_EVERY`] per machine (any Cua program
/// asking counts), and only while signed in. Best effort, never fails.
#[cfg(feature = "host")]
pub(crate) async fn sync_account_if_due(session: &cua_auth::Session) {
    if t().account_check_due(ACCOUNT_CHECK_EVERY) && matches!(session.credentials(), Ok(Some(_))) {
        sync_account(session).await;
    }
}

/// The signed-in account's `analytics_id` from `GET /v1/me`: `Some(None)`
/// from a server that does not offer linking (an older one, or linking off
/// there); none when it could not be asked (signed out, offline, slow).
#[cfg(feature = "host")]
async fn analytics_id(session: &cua_auth::Session) -> Option<Option<String>> {
    let budget = Duration::from_secs(10);
    let token = tokio::time::timeout(budget, session.access_token(false))
        .await
        .ok()?
        .ok()?;
    let api = cua_host::spaces_api::SpacesApiClient::current().ok()?;
    let me = tokio::time::timeout(budget, api.me(&token))
        .await
        .ok()?
        .ok()?;
    Some(me.analytics_id)
}

/// Asks `fetch` for the account id and hands the answer to `t`, only while
/// usage data is on (off: nothing is asked or written). Returns the link
/// state.
#[cfg(any(test, feature = "host"))]
pub(crate) async fn sync_account_with<F, Fut>(
    t: &cua_telemetry::Telemetry,
    fetch: F,
) -> cua_telemetry::LinkState
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<Option<String>>>,
{
    if !t.is_enabled() {
        return cua_telemetry::LinkState::Off;
    }
    t.mark_account_checked();
    if let Some(id) = fetch().await {
        t.offer_account(id.as_deref());
    }
    t.link_state()
}

/// Records use of a Spaces app feature (fixed names: `space_create_local`,
/// `teleport_drop`, ... see the docs). Returns whether it was queued;
/// unknown names are dropped.
#[uniffi::export]
pub fn telemetry_record_feature(feature: String) -> bool {
    events::spaces_feature_used(&feature).is_some_and(|e| t().capture(e) == Captured::Queued)
}

/// Records an install or onboarding funnel step (fixed names:
/// `onboarding_shown`, `signed_in`, `first_space_created`, ...; `first_*`
/// steps count once per install). Returns whether it was queued.
#[uniffi::export]
pub fn telemetry_record_onboarding_step(step: String, ok: bool) -> bool {
    t().capture_step(&step, if ok { Outcome::Ok } else { Outcome::Error }) == Captured::Queued
}

/// One stream session's aggregate stats (sampled).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StreamStatsReport {
    /// `quic`, `webrtc`, `websocket` or `grpc`.
    pub transport: String,
    /// `h264`, `hevc`, `av1`, `vp8`, `vp9`, `jpeg` or `png`.
    pub codec: String,
    /// Hardware decode (None: not decoded here).
    pub hw_decode: Option<bool>,
    pub avg_fps: f64,
    /// p95 input-to-present or control round trip, ms.
    pub p95_latency_ms: Option<f64>,
    /// Stream height in pixels.
    pub height: u32,
    pub duration_ms: u64,
}

/// Records one stream session's aggregate stats (bucketed and sampled).
#[uniffi::export]
pub fn telemetry_record_stream_stats(stats: StreamStatsReport) -> bool {
    let e = events::stream_stats(&events::StreamStats {
        transport: &stats.transport,
        codec: &stats.codec,
        hw_decode: stats.hw_decode,
        avg_fps: stats.avg_fps,
        p95_latency_ms: stats.p95_latency_ms,
        height: stats.height,
        duration: Duration::from_millis(stats.duration_ms),
    });
    t().capture(e) == Captured::Queued
}

/// One media session as the SDK saw it (raw; bucketed into
/// `cua_stream_stats` by cua-telemetry).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StreamSummary {
    /// `websocket`, `quic`, ...
    pub transport: &'static str,
    /// The negotiated video codec (`h264`, `png`, ...).
    pub codec: String,
    /// Hardware decode (None: not decoded here).
    pub hw_decode: Option<bool>,
    /// Video frames received.
    pub frames: u64,
    /// Largest frame height seen, pixels.
    pub height: u32,
    pub duration: Duration,
}

/// The `cua_stream_stats` event of a finished stream; None when it carried
/// no video (nothing to measure).
pub(crate) fn stream_stats_event(s: &StreamSummary) -> Option<events::Event> {
    events::stream_summary(
        s.transport,
        &s.codec,
        s.hw_decode,
        s.frames,
        s.height,
        None,
        s.duration,
    )
}

/// Records a finished stream on `t` (the process-wide client in
/// [`stream_ended`]; a test client in tests).
pub(crate) fn record_stream_on(
    t: &cua_telemetry::Telemetry,
    s: &StreamSummary,
) -> Option<Captured> {
    stream_stats_event(s).map(|e| t.capture(e))
}

/// A media session ended: coarse, bucketed stats (sampled; off with every
/// telemetry switch).
pub(crate) fn stream_ended(s: &StreamSummary) {
    let _ = record_stream_on(t(), s);
}

/// Records a finished cua-bench run: the taskset (catalog name, else
/// `custom`), the aggregate success rate (0 to 1, bucketed), the task count
/// (bucketed) and the outcome (`ok`, `error`, `cancelled`).
#[uniffi::export]
pub fn telemetry_record_bench_run(
    taskset: String,
    success_rate: Option<f64>,
    task_count: u64,
    outcome: String,
) -> bool {
    let o = match outcome.as_str() {
        "ok" => Outcome::Ok,
        "cancelled" => Outcome::Cancelled,
        _ => Outcome::Error,
    };
    t().capture(events::bench_run_completed(
        &taskset,
        success_rate,
        task_count,
        o,
    )) == Captured::Queued
}

/// Sends queued events now, waiting at most `timeout_ms`; anything left
/// is kept on disk for a later process.
#[uniffi::export]
pub fn telemetry_flush(timeout_ms: u32) {
    t().shutdown(Duration::from_millis(u64::from(timeout_ms)));
}

// ---------------------------------------------------------------------------
// SDK-internal recording
// ---------------------------------------------------------------------------

/// The error variant of a result, for `error_kind` (never the message).
pub(crate) fn variant<T>(r: &Result<T>) -> Option<&'static str> {
    r.as_ref().err().map(CuaError::variant)
}

/// A create's error kind: the cause the error's text names
/// (`image_pull_not_found`, `runtime_not_running`, `vm_boot_timeout`, ...;
/// [`cua_create_failure::create_failure_kind`]), else the variant. Only the
/// bounded word leaves the process, never the message. A cloud create's
/// "X is not available" is its provider, not a local runtime.
fn create_error_kind<T>(on: &str, r: &Result<T>) -> Option<&'static str> {
    let e = r.as_ref().err()?;
    let local = on == "local";
    let variant = match e {
        // A cloud account that is not set up stays provider_not_configured.
        CuaError::ProviderNotConfigured(_) if !local => return Some(e.variant()),
        CuaError::ProviderNotConfigured(_) => "Runtime",
        _ => e.variant(),
    };
    Some(cua_create_failure::create_failure_kind(
        &e.to_string(),
        "",
        variant,
        local,
    ))
}

fn outcome<T>(r: &Result<T>) -> Outcome {
    if r.is_ok() {
        Outcome::Ok
    } else {
        Outcome::Error
    }
}

/// Records `cua_api_used` for `api` (a fixed name).
pub(crate) fn api<T>(api: &str, started: Instant, r: &Result<T>) {
    if let Some(e) = events::api_used(api, outcome(r), variant(r), started.elapsed()) {
        t().capture(e);
    }
}

/// What a sandbox create asked for, classified before the call.
pub(crate) struct CreateFacts {
    on: String,
    kind: String,
    runtime: String,
    image: String,
    guest_os: String,
    overlays: bool,
    build: bool,
    started: Instant,
}

impl CreateFacts {
    pub(crate) fn of(o: &super::SandboxCreateOptions) -> Self {
        let (on, kind, runtime) = match o.placement() {
            Ok(p) => (
                p.on.location().to_string(),
                p.kind.as_str().to_string(),
                p.runtime.to_string(),
            ),
            Err(_) => (
                o.on.clone().unwrap_or_default(),
                o.kind.clone().unwrap_or_default(),
                o.runtime.clone().unwrap_or_default(),
            ),
        };
        // Aliases (`linux`, `macos:tahoe`) name catalog images.
        let image = cua_image::canonical::alias(&o.image).unwrap_or_else(|| o.image.clone());
        let guest_os = o.os.clone().unwrap_or_else(|| {
            let i = o.image.to_ascii_lowercase();
            ["linux", "windows", "macos", "android"]
                .into_iter()
                .find(|os| i.contains(os))
                .unwrap_or("")
                .to_string()
        });
        Self {
            on,
            kind,
            runtime,
            image,
            guest_os,
            overlays: !o.overlays.is_empty(),
            build: o.build.is_some(),
            started: Instant::now(),
        }
    }

    /// Whether the sandbox runs on this machine.
    pub(crate) fn is_local(&self) -> bool {
        self.on == "local"
    }

    pub(crate) fn record<T>(self, r: &Result<T>) {
        let e = events::sandbox_created(
            &events::SandboxCreate {
                on: &self.on,
                kind: &self.kind,
                runtime: &self.runtime,
                image: &self.image,
                guest_os: &self.guest_os,
                with_overlays: self.overlays,
                with_build: self.build,
            },
            outcome(r),
            create_error_kind(&self.on, r),
            self.started.elapsed(),
        );
        t().capture(e);
        if r.is_ok() && self.on != "direct" {
            t().capture_step("first_sandbox_created", Outcome::Ok);
        }
    }
}

/// Records `cua_sandbox_deleted`.
pub(crate) fn sandbox_deleted<T>(location: &str, r: &Result<T>) {
    t().capture(events::sandbox_deleted(location, outcome(r), variant(r)));
}

/// Records a teleport from this process (no broker verified the caller:
/// `caller_kind` is self-reported from the SDK surface).
pub(crate) fn teleport<T>(
    app: &str,
    capability: &str,
    move_kind: &str,
    started: Instant,
    r: &Result<T>,
    items: u64,
) {
    let product = t().product();
    let info = events::TeleportInfo::new(
        app,
        capability,
        move_kind,
        events::CallerKind::self_reported(product),
        events::CallerKindSource::SelfReported,
        "sdk",
    );
    t().capture(events::teleport_attempted(&info));
    let o = match r {
        Ok(_) => events::TeleportOutcome::Ok,
        Err(e) => teleport_outcome(e),
    };
    t().capture(events::teleport_completed(
        &info,
        o,
        started.elapsed(),
        items,
    ));
    if r.is_ok() {
        t().capture_step("first_teleport", Outcome::Ok);
    }
}

/// The typed teleport outcome of an SDK error. `RequiresCuaApp` reaches the
/// SDK as `HostCapabilityMissing` whose message names the Cua app.
pub(crate) fn teleport_outcome(e: &CuaError) -> events::TeleportOutcome {
    use events::TeleportOutcome as T;
    match e {
        CuaError::TeleportRefused(_) => T::ConsentDenied,
        CuaError::HostCapabilityMissing(m) if requires_cua_app(m) => T::RequiresCuaApp,
        CuaError::PermissionDenied(_) | CuaError::Unauthenticated(_) => T::Forbidden,
        CuaError::NotFound(_) => T::NotFound,
        CuaError::InvalidArgument(_) => T::Invalid,
        _ => T::Error,
    }
}

/// Whether an error message is the Keyvault's `RequiresCuaApp` (its
/// install link is fixed, and its open link is the app's own: `cua://` for
/// the released app, `cua-local://` for Cua Spaces Local).
pub(crate) fn requires_cua_app(message: &str) -> bool {
    message.contains(cua_keyvault_links::INSTALL_URL)
        || cua_home::channel::CuaChannel::ALL
            .iter()
            .any(|c| message.contains(&c.keyvault_url()))
}

/// The fixed link `RequiresCuaApp` carries (cua-keyvault `embedded`).
mod cua_keyvault_links {
    pub const INSTALL_URL: &str = "https://cua.ai/download";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_client(
        home: &std::path::Path,
        off: bool,
    ) -> (
        cua_telemetry::Telemetry,
        std::sync::Arc<cua_telemetry::sink::MemorySink>,
    ) {
        let sink = std::sync::Arc::new(cua_telemetry::sink::MemorySink::new());
        let t = cua_telemetry::Telemetry::builder()
            .env(move |k| match k {
                "CUA_TELEMETRY_FORBID_NETWORK" => Some("1".into()),
                "DO_NOT_TRACK" if off => Some("1".into()),
                _ => None,
            })
            .home(home)
            .sink(sink.clone())
            .product("spaces_app", "0.1.0")
            .notice_mode(cua_telemetry::NoticeMode::External)
            .foreground()
            .build();
        t.acknowledge_notice();
        (t, sink)
    }

    const ACCT: &str = "acct_0123456789abcdef0123456789abcdef";

    /// CUA-1881, the apps' and SDK's linking: off, nothing is asked,
    /// written or identified; an older server without `analytics_id` links
    /// nothing; a server that offers it links (no email, ever).
    #[tokio::test]
    async fn account_sync_respects_the_switch_and_older_servers() {
        use cua_telemetry::LinkState;
        let flush = |t: &cua_telemetry::Telemetry| {
            t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
            t.flush(Duration::from_secs(1));
        };
        let identifies = |s: &cua_telemetry::sink::MemorySink| {
            s.events()
                .iter()
                .filter(|e| e["event"] == "$identify")
                .count()
        };

        let off = tempfile::tempdir().unwrap();
        let (t, sink) = link_client(off.path(), true);
        let state =
            sync_account_with(&t, || async { panic!("asked while usage data is off") }).await;
        assert_eq!(state, LinkState::Off);
        flush(&t);
        assert!(sink.events().is_empty());
        assert!(
            !off.path()
                .join("telemetry")
                .join(cua_telemetry::link::ACCOUNT_FILE)
                .exists()
        );

        let old = tempfile::tempdir().unwrap();
        let (t, sink) = link_client(old.path(), false);
        assert_eq!(
            sync_account_with(&t, || async { Some(None) }).await,
            LinkState::NotOffered
        );
        flush(&t);
        assert_eq!(identifies(&sink), 0);
        assert!(
            !t.account_check_due(ACCOUNT_CHECK_EVERY),
            "asked: not again for a while"
        );

        let new = tempfile::tempdir().unwrap();
        let (t, sink) = link_client(new.path(), false);
        assert_eq!(
            sync_account_with(&t, || async { Some(Some(ACCT.to_string())) }).await,
            LinkState::Linked
        );
        flush(&t);
        assert_eq!(identifies(&sink), 1);
        assert!(!sink.events().iter().any(|e| e.to_string().contains('@')));
        // Offline: nothing changes.
        assert_eq!(
            sync_account_with(&t, || async { None }).await,
            LinkState::Linked
        );
    }

    #[test]
    fn create_error_kind_splits_local_runtime_failures() {
        let err = |e: CuaError| -> Result<()> { Err(e) };
        let kind = |on: &str, e: CuaError| {
            create_error_kind(on, &err(e)).map(cua_telemetry::events::error_kind)
        };
        let no_engine = "local runtime: [local] a container engine is not available: no socket";
        assert_eq!(
            kind("local", CuaError::Runtime(no_engine.into())),
            Some("runtime_missing")
        );
        assert_eq!(
            kind("local", CuaError::ProviderNotConfigured(no_engine.into())),
            Some("runtime_missing")
        );
        assert_eq!(
            kind(
                "local",
                CuaError::Runtime("permission denied on unix:///x: add".into())
            ),
            Some("runtime_permission")
        );
        assert_eq!(
            kind("local", CuaError::Runtime("boom".into())),
            Some("runtime")
        );
        // The rest of `runtime`, by cause (each a schema word, not `other`).
        for (message, want) in [
            (
                "lume API 409: VM start rejected: host limit of 2 running macOS guests reached",
                "vm_limit",
            ),
            (
                "Local Network access is not available: allow it",
                "local_network",
            ),
            (
                "QEMU is not available: download QEMU for Windows",
                "vm_runtime_missing",
            ),
            (
                "container 'a' exited right after start (status Stopped)",
                "container_exited",
            ),
            ("lume API 500: boom", "vm_error"),
            ("container engine: create: conflict", "engine_error"),
        ] {
            let m = format!("local runtime: [local] {message}");
            assert_eq!(kind("local", CuaError::Runtime(m)), Some(want), "{message}");
        }
        // A cloud without an account stays provider_not_configured.
        assert_eq!(
            kind(
                "cloud",
                CuaError::ProviderNotConfigured("x is not available: y".into())
            ),
            Some("provider_not_configured")
        );
        assert_eq!(
            kind(
                "local",
                CuaError::Runtime("container engine: pull ghcr.io/x/y:1: manifest unknown".into())
            ),
            Some("image_pull_not_found")
        );
        assert_eq!(
            kind(
                "local",
                CuaError::Runtime("Docker is not running: start Docker Desktop".into())
            ),
            Some("runtime_not_running")
        );
        assert_eq!(
            kind(
                "cloud",
                CuaError::Runtime("sandbox quota exceeded for this account".into())
            ),
            Some("quota_exceeded")
        );
        assert_eq!(create_error_kind::<()>("local", &Ok(())), None);
    }

    #[test]
    fn requires_cua_app_maps_to_its_own_outcome() {
        let m = "Teleport requires the Cua app, which keeps your sessions in its Keyvault and asks you before anything moves. Install it from https://cua.ai/download.";
        assert_eq!(
            teleport_outcome(&CuaError::HostCapabilityMissing(m.into())),
            events::TeleportOutcome::RequiresCuaApp
        );
        assert_eq!(
            teleport_outcome(&CuaError::HostCapabilityMissing("no display".into())),
            events::TeleportOutcome::Error
        );
        assert_eq!(
            teleport_outcome(&CuaError::TeleportRefused("no".into())),
            events::TeleportOutcome::ConsentDenied
        );
    }

    #[test]
    fn requires_cua_app_knows_each_channels_open_link() {
        // "Installed, not running" carries the app's open link: the released
        // app's `cua://`, or Cua Spaces Local's `cua-local://`.
        for link in ["cua://keyvault", "cua-local://keyvault"] {
            let m = format!("Teleport needs the Cua app running: open Cua ({link}) and try again.");
            assert!(requires_cua_app(&m), "{link}");
        }
        assert!(!requires_cua_app("no display"));
        assert!(!requires_cua_app("open cua-other://keyvault"));
    }

    fn summary() -> StreamSummary {
        StreamSummary {
            transport: "websocket",
            codec: "h264".into(),
            hw_decode: Some(true),
            frames: 900,
            height: 1080,
            duration: Duration::from_secs(30),
        }
    }

    fn client(
        home: &std::path::Path,
        env: &[(&str, &str)],
    ) -> (
        cua_telemetry::Telemetry,
        std::sync::Arc<cua_telemetry::sink::MemorySink>,
    ) {
        let m: std::collections::HashMap<String, String> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let sink = std::sync::Arc::new(cua_telemetry::sink::MemorySink::new());
        let t = cua_telemetry::Telemetry::builder()
            .env(move |k| m.get(k).cloned())
            .home(home)
            .sink(sink.clone())
            .product("sdk_rust", "1.2.3")
            .foreground()
            .build();
        t.acknowledge_notice();
        (t, sink)
    }

    #[test]
    fn a_finished_stream_sends_only_bucketed_schema_properties() {
        let home = tempfile::tempdir().unwrap();
        let (t, sink) = client(
            home.path(),
            &[
                ("CUA_TELEMETRY", "1"),
                ("CUA_TELEMETRY_FORBID_NETWORK", "1"),
            ],
        );
        // 50% sampled: retry a bounded number of times.
        let queued = (0..64).any(|_| record_stream_on(&t, &summary()) == Some(Captured::Queued));
        assert!(queued, "never queued in 64 tries");
        t.flush(Duration::from_secs(1));
        let events = sink.events();
        let e = events
            .iter()
            .find(|e| e["event"] == "cua_stream_stats")
            .expect("a cua_stream_stats event");
        let props = e["properties"].as_object().unwrap();
        // Exactly the schema's own properties plus the common envelope.
        let spec = cua_telemetry::schema::spec("cua_stream_stats").unwrap();
        let own: Vec<&str> = spec.props.iter().map(|p| p.name).collect();
        for k in &own {
            assert!(props.contains_key(*k), "missing {k}");
        }
        let preview = t.preview(&stream_stats_event(&summary()).unwrap()).unwrap();
        let allowed: std::collections::BTreeSet<&String> =
            preview["properties"].as_object().unwrap().keys().collect();
        for k in props.keys() {
            assert!(allowed.contains(k), "unexpected property {k}");
        }
        assert_eq!(props["transport"], "websocket");
        assert_eq!(props["codec"], "h264");
        assert_eq!(props["decode"], "hw");
        assert_eq!(props["fps_bucket"], "30_49");
        assert_eq!(props["latency_bucket"], "unknown");
        // Coarse buckets only: no raw frame counts, sizes or seconds.
        for (k, v) in &own.iter().map(|k| (*k, &props[*k])).collect::<Vec<_>>() {
            assert!(v.is_string(), "{k} is not a bucket: {v}");
        }
        // No raw summary number anywhere in the properties (the envelope's
        // random ids and timestamps may contain the digits, so compare
        // values, not the serialised event).
        let raw = [
            serde_json::json!(900),
            serde_json::json!(1080),
            serde_json::json!(30),
            serde_json::json!(30.0),
        ];
        for (k, v) in props {
            assert!(!raw.contains(v), "{k} carries a raw value: {v}");
            if let Some(s) = v.as_str() {
                assert!(!["900", "1080", "30"].contains(&s), "{k} = {s}");
            }
        }
    }

    #[test]
    fn no_stream_event_without_video() {
        let mut s = summary();
        s.frames = 0;
        assert!(stream_stats_event(&s).is_none());
        let mut s = summary();
        s.duration = Duration::ZERO;
        assert!(stream_stats_event(&s).is_none());
    }

    #[test]
    fn nothing_is_sent_for_a_stream_under_any_opt_out() {
        let cases: &[&[(&str, &str)]] = &[
            &[("DO_NOT_TRACK", "1")],
            &[("DO_NOT_TRACK", "1"), ("CUA_TELEMETRY", "1")],
            &[("CUA_TELEMETRY", "0")],
            &[("CUA_TELEMETRY_DISABLED", "1")],
            &[("CI", "true")],
        ];
        for env in cases {
            let home = tempfile::tempdir().unwrap();
            let (t, sink) = client(home.path(), env);
            for _ in 0..16 {
                assert_eq!(
                    record_stream_on(&t, &summary()),
                    Some(Captured::Disabled),
                    "{env:?}"
                );
            }
            t.flush(Duration::from_millis(200));
            assert!(sink.events().is_empty(), "{env:?}");
            assert!(t.show_last(10).is_empty(), "{env:?}");
        }
        // The config switch (`cua telemetry off`, the Spaces app setting).
        let home = tempfile::tempdir().unwrap();
        let (t, sink) = client(home.path(), &[("CUA_TELEMETRY_FORBID_NETWORK", "1")]);
        t.set_enabled(false).unwrap();
        t.refresh();
        assert_eq!(record_stream_on(&t, &summary()), Some(Captured::Disabled));
        t.flush(Duration::from_millis(200));
        assert!(sink.events().is_empty());
    }

    #[test]
    fn exports_work_while_telemetry_is_off_in_tests() {
        // libs/cua/.cargo/config.toml turns telemetry off for cargo runs.
        let s = telemetry_status();
        assert!(!s.enabled);
        assert!(serde_json::from_str::<serde_json::Value>(&s.envelope_json).is_ok());
        assert!(!telemetry_record_feature("teleport_drop".into()));
        assert!(!telemetry_record_feature("/Users/alice".into()));
        assert!(telemetry_schema_json().contains("cua_sandbox_created"));
    }
}
