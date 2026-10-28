//! Crash reporting for the Cua Spaces app's Rust core and daemon.
//!
//! The Spaces shells (Electron, SwiftUI) pass a Sentry DSN down through
//! `CUA_CRASH_*` environment variables. When the DSN is set and the user
//! allows it, panics in this process go to Sentry as events with a stack
//! trace and nothing else: no messages, paths, emails, host names, IPs,
//! breadcrumbs or sessions.
//!
//! Consent is the "Share usage data" decision of [`cua_telemetry`]: reports
//! are allowed only when telemetry is enabled and the first-run notice has
//! been shown. It is checked at start and again for every event.
//!
//! Without `CUA_CRASH_DSN` nothing is initialised, so the `cua` CLI and
//! every other user of these crates are unaffected.

mod scrub;

pub use scrub::{scrub_event, scrub_text, scrub_text_with_home, scrub_url};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Environment variables the shell sets before the native code starts.
pub const ENV_DSN: &str = "CUA_CRASH_DSN";
pub const ENV_RELEASE: &str = "CUA_CRASH_RELEASE";
pub const ENV_ENVIRONMENT: &str = "CUA_CRASH_ENVIRONMENT";
pub const ENV_SHELL: &str = "CUA_CRASH_SHELL";
/// Local builds only: the named component panics once, shortly after init.
pub const ENV_TEST_PANIC: &str = "CUA_CRASH_TEST_PANIC";

/// How long a panic (and shutdown) waits for the event to leave.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// Everything the SDK needs; [`Config::from_env`] reads it from the shell.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub dsn: String,
    pub release: Option<String>,
    pub environment: String,
    pub shell: String,
    pub component: String,
}

impl Config {
    /// Reads the `CUA_CRASH_*` variables. `None` when no DSN is set.
    pub fn from_env(component: &str) -> Option<Self> {
        Self::from_lookup(component, |k| std::env::var(k).ok())
    }

    fn from_lookup(component: &str, get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let non_empty = |k| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Some(Self {
            dsn: non_empty(ENV_DSN)?,
            release: non_empty(ENV_RELEASE),
            environment: non_empty(ENV_ENVIRONMENT).unwrap_or_else(|| "stable".into()),
            shell: non_empty(ENV_SHELL).unwrap_or_else(|| "unknown".into()),
            component: component.to_string(),
        })
    }
}

/// Whether reports are allowed right now. Called for every event.
pub type Consent = Arc<dyn Fn() -> bool + Send + Sync>;

/// The "Share usage data" rule: telemetry enabled and the notice shown.
pub fn telemetry_consent() -> bool {
    let s = cua_telemetry::global().status();
    s.enabled && s.notice_shown
}

/// Keeps the SDK alive; dropping it flushes what is queued and closes the
/// client.
pub struct Guard {
    _inner: sentry::ClientInitGuard,
}

impl Guard {
    /// Whether this guard's client is enabled (a valid DSN was given).
    pub fn is_enabled(&self) -> bool {
        self._inner.is_enabled()
    }
}

static STARTED: AtomicBool = AtomicBool::new(false);
static PROCESS_GUARD: OnceLock<Guard> = OnceLock::new();

/// Starts crash reporting for this process from the `CUA_CRASH_*` variables.
///
/// Returns `None` when no DSN is set, when the user has not allowed reports
/// (telemetry off or the notice not yet shown; a switch turned on later
/// takes effect at the next launch), or when it already started in this
/// process. Keep the guard for the life of the process.
pub fn init_from_env(component: &str) -> Option<Guard> {
    let cfg = Config::from_env(component)?;
    if !telemetry_consent() {
        return None;
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        return None;
    }
    let guard = init_with(cfg, Arc::new(telemetry_consent));
    if guard.is_none() {
        STARTED.store(false, Ordering::SeqCst);
    }
    guard
}

/// [`init_from_env`] for callers with nowhere to keep the guard (the FFI
/// library): it lives for the process. Returns whether reporting started.
pub fn start_for_process(component: &str) -> bool {
    match init_from_env(component) {
        Some(g) => PROCESS_GUARD.set(g).is_ok(),
        None => false,
    }
}

/// Closes the client when consent was withdrawn while running. Cheap; call
/// it after the "Share usage data" switch changes.
pub fn refresh_consent() {
    if !telemetry_consent()
        && let Some(client) = sentry::Hub::main().client()
    {
        client.close(Some(Duration::from_millis(200)));
    }
}

/// Starts the SDK with an explicit config and consent check (the shared
/// path of [`init_from_env`]; tests use it with a loopback DSN). Not
/// idempotent on its own.
pub fn init_with(cfg: Config, consent: Consent) -> Option<Guard> {
    let dsn: sentry::types::Dsn = cfg.dsn.parse().ok()?;
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    let before_send = {
        let consent = consent.clone();
        move |event: sentry::protocol::Event<'static>| {
            if !consent() {
                return None;
            }
            Some(scrub_event(event, home.as_deref()))
        }
    };
    // `ClientOptions` is non-exhaustive: start from the default and set.
    // Traces sampling defaults to off and stays there; sessions need the
    // `release-health` feature, which is not enabled.
    let mut options = sentry::ClientOptions::default();
    options.dsn = Some(dsn);
    options.release = cfg.release.clone().map(Into::into);
    options.environment = Some(cfg.environment.clone().into());
    options.send_default_pii = false;
    options.attach_stacktrace = true;
    options.server_name = None;
    options.max_breadcrumbs = 0;
    options.shutdown_timeout = FLUSH_TIMEOUT;
    options.before_send = Some(Arc::new(before_send));
    options.before_breadcrumb = Some(Arc::new(|_| None));
    let guard = sentry::init(options);
    if !guard.is_enabled() {
        return None;
    }
    sentry::configure_scope(|scope| {
        scope.set_tag("shell", &cfg.shell);
        scope.set_tag("component", &cfg.component);
    });
    if cfg.environment == "local" && std::env::var(ENV_TEST_PANIC).is_ok_and(|c| c == cfg.component)
    {
        spawn_test_panic(&cfg.component);
    }
    Some(Guard { _inner: guard })
}

/// Local builds only: panics once, a moment after init, so the pipeline
/// can be exercised end to end.
fn spawn_test_panic(component: &str) {
    static FIRED: AtomicBool = AtomicBool::new(false);
    let component = component.to_string();
    let _ = std::thread::Builder::new()
        .name("cua-crash-test-panic".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            if !FIRED.swap(true, Ordering::SeqCst) {
                panic!("cua-crash test panic ({component})");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn no_dsn_means_off() {
        assert!(Config::from_lookup("core", lookup(&[])).is_none());
        assert!(Config::from_lookup("core", lookup(&[(ENV_DSN, "  ")])).is_none());
    }

    #[test]
    fn reads_the_contract_variables() {
        let c = Config::from_lookup(
            "daemon",
            lookup(&[
                (ENV_DSN, "https://k@o1.ingest.sentry.io/2"),
                (ENV_RELEASE, "cua-spaces-desktop@0.7.2-beta.1"),
                (ENV_ENVIRONMENT, "beta"),
                (ENV_SHELL, "swift"),
            ]),
        )
        .unwrap();
        assert_eq!(
            c.release.as_deref(),
            Some("cua-spaces-desktop@0.7.2-beta.1")
        );
        assert_eq!(
            (c.environment.as_str(), c.shell.as_str()),
            ("beta", "swift")
        );
        assert_eq!(c.component, "daemon");
    }
}
