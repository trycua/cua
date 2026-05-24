//! Startup "new version available" banner.
//!
//! On the interactive entry points (`mcp`, `serve`, `doctor`) we check the
//! GitHub releases API for a newer `cua-driver-rs-v*` tag, cache the answer
//! on disk for ~20 hours, and print a small two-line banner to **stderr**
//! if a strictly-newer release exists and the user hasn't dismissed it.
//!
//! Design constraints:
//!
//! - **Non-blocking.** [`maybe_announce_update`] returns instantly. The
//!   actual HTTP fetch runs on a background task (`tokio::task::spawn` when
//!   a runtime is already live, else a short-lived OS thread). If the
//!   network is slow, the daemon starts up regardless and the banner
//!   either lands on a later stderr line or appears next launch.
//! - **Silent on failure.** HTTP timeouts, 4xx/5xx, JSON-parse errors,
//!   refusal-to-write-cache — everything is `tracing::debug!` only. Never
//!   pollute the user's stderr with "couldn't check for updates" noise;
//!   the next launch just retries.
//! - **Cache-first.** A 20-hour-old cache short-circuits the network call
//!   entirely. This bounds outbound requests to roughly one per machine
//!   per day even on a hot reload loop.
//! - **Opt-out at three layers.** Env var `CUA_DRIVER_RS_UPDATE_CHECK=false`
//!   (single invocation), persisted config `update_check_enabled = false`
//!   in `~/.cua-driver/config.json` (permanent), and an automatic skip when
//!   `CARGO_PKG_VERSION` carries any pre-release suffix (source / dev
//!   builds — there is no matching published release to recommend).
//! - **Skip in machine-readable contexts.** Only the long-running
//!   entry points call [`maybe_announce_update`]. `--version`,
//!   `list-tools`, `describe`, `call`, `dump-docs`, etc. are routinely
//!   piped through `jq` from scripts; a banner would corrupt parseable
//!   output. The decision lives at the call sites in `main.rs`, not here.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Disk-resident cache file (sibling of the telemetry artifacts under
/// `~/.cua-driver-rs/`). Holds the last-seen latest version plus the list
/// of versions the user has actively dismissed.
const CACHE_FILE_NAME: &str = "version_check.json";

/// `~/.cua-driver-rs/` — same subdirectory the telemetry client uses,
/// kept separate from the `~/.cua-driver/` config tree that the Swift
/// reference owns.
const HOME_SUBDIRECTORY: &str = ".cua-driver-rs";

/// Single-invocation opt-out env var. Recognised values mirror the
/// telemetry opt-out: `0|false|no|off` disables the check, everything
/// else (including unset) leaves it on.
const ENV_UPDATE_CHECK: &str = "CUA_DRIVER_RS_UPDATE_CHECK";

/// Refresh threshold for the on-disk cache. A fresh cache short-circuits
/// the network call so we hit the GitHub API at most ~1× per machine per
/// day even when the daemon restarts repeatedly.
const CACHE_REFRESH_SECONDS: u64 = 20 * 60 * 60; // 20 hours

/// HTTP timeout for the releases API fetch. Kept tight so the background
/// task can't linger past the daemon's normal startup window.
const HTTP_TIMEOUT_SECONDS: u64 = 4;

/// Tag-name prefix used by the Rust-port releases on the trycua/cua repo.
/// Releases tagged with anything else (e.g. the Swift port's
/// `cua-driver-v*`) are filtered out so we never recommend the wrong binary.
pub(crate) const RELEASE_TAG_PREFIX: &str = "cua-driver-rs-v";

/// GitHub releases API endpoint. Paginates newest-first; 40 entries is
/// plenty of headroom past the most recent stable release even when
/// pre-releases are sprinkled in between.
const RELEASES_URL: &str =
    "https://api.github.com/repos/trycua/cua/releases?per_page=40";

// ── Public API ───────────────────────────────────────────────────────────

/// Kick off a background "is there a newer release?" check.
///
/// **Returns immediately.** The actual network round-trip happens on a
/// `tokio::task::spawn` task (when a runtime is live) or a short-lived
/// OS thread (sync entry points). The two-line banner is printed to
/// **stderr** if/when the check completes and finds a strictly-newer
/// non-dismissed version.
///
/// No-op when:
/// - `CUA_DRIVER_RS_UPDATE_CHECK` env var is set to a falsy value
/// - The persisted config flag `update_check_enabled` is `false`
/// - `CARGO_PKG_VERSION` has a pre-release suffix (source / dev build —
///   nothing on GitHub will be "newer" in a meaningful way)
pub fn maybe_announce_update() {
    if !is_enabled() {
        tracing::debug!(target: "cua_driver::version_check",
                        "update check skipped (opt-out / pre-release build)");
        return;
    }

    let current = env!("CARGO_PKG_VERSION").to_owned();

    let task = move || {
        run_check_and_announce(&current, fetch_latest_version, std::io::stderr());
    };

    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(task);
    } else {
        std::thread::Builder::new()
            .name("cua-version-check".into())
            .spawn(task)
            .ok();
    }
}

/// Mark `version` as dismissed so the banner stops nagging the user about
/// this specific release. They will see the next banner the moment a
/// strictly-newer tag ships.
///
/// Idempotent. Failures (no HOME, IO error) are logged via
/// `tracing::debug!` and silently dropped — dismissal is a UX nicety, not
/// a correctness boundary.
///
/// Exposed publicly so a future interactive prompt (TUI, GUI helper) can
/// wire it in without re-implementing the persistence layer. No call site
/// in the current binary — the banner today is informational only.
#[allow(dead_code)]
pub fn dismiss_version(version: &str) {
    let mut cache = read_cache().unwrap_or_default();
    if !cache.dismissed_versions.iter().any(|v| v == version) {
        cache.dismissed_versions.push(version.to_owned());
    }
    if let Err(e) = write_cache(&cache) {
        tracing::debug!(target: "cua_driver::version_check",
                        "failed to persist dismissal: {e}");
    }
}

// ── Core logic (testable seam) ───────────────────────────────────────────

/// Inner routine wired up by [`maybe_announce_update`].
///
/// Split out from the public entry point so tests can drive the check
/// against a stubbed fetcher and capture the banner into an in-memory
/// buffer. Errors here never propagate — the function consumes them
/// and converts to `tracing::debug!` lines.
fn run_check_and_announce<F, W>(current: &str, fetch: F, mut writer: W)
where
    F: FnOnce() -> Result<String, String>,
    W: std::io::Write,
{
    let now = unix_now();

    // Decide whether the cache is still fresh enough to skip the network.
    let cached = read_cache().unwrap_or_default();
    let needs_refresh = cached
        .last_checked_unix
        .map(|t| now.saturating_sub(t) >= CACHE_REFRESH_SECONDS)
        .unwrap_or(true);

    let latest = if needs_refresh {
        match fetch() {
            Ok(v) => {
                // Persist on success so the next launch re-uses the answer.
                let new_cache = VersionCache {
                    last_checked_unix: Some(now),
                    last_checked_at: Some(iso8601(now)),
                    latest_version: Some(v.clone()),
                    dismissed_versions: cached.dismissed_versions.clone(),
                };
                if let Err(e) = write_cache(&new_cache) {
                    tracing::debug!(target: "cua_driver::version_check",
                                    "failed to write cache: {e}");
                }
                v
            }
            Err(e) => {
                tracing::debug!(target: "cua_driver::version_check",
                                "fetch failed: {e}");
                // Fall back to the cached value if any — better an old
                // banner than none on a brief network blip.
                match cached.latest_version.clone() {
                    Some(v) => v,
                    None => return,
                }
            }
        }
    } else {
        match cached.latest_version.clone() {
            Some(v) => v,
            None => return,
        }
    };

    // Re-read dismissals: dismiss_version may have run between our cache
    // load and now (e.g. on a separately-spawned task in the same process).
    let dismissed = read_cache()
        .map(|c| c.dismissed_versions)
        .unwrap_or(cached.dismissed_versions);

    if !is_newer(&latest, current) {
        return;
    }
    if dismissed.iter().any(|v| v == &latest) {
        tracing::debug!(target: "cua_driver::version_check",
                        "newer version {latest} dismissed; skipping banner");
        return;
    }

    let banner = format_banner(&latest, current);
    if let Err(e) = writer.write_all(banner.as_bytes()) {
        tracing::debug!(target: "cua_driver::version_check",
                        "failed to print banner: {e}");
    }
}

/// Format the two-line banner. Plain text, no ANSI colours — terminals
/// without UTF-8 still see the `✨` byte sequence but the text is
/// readable either way.
fn format_banner(latest: &str, current: &str) -> String {
    format!(
        "\n\u{2728} cua-driver v{latest} is available (you have v{current}).\n   \
         Update with: cua-driver update\n   \
         Release notes: https://github.com/trycua/cua/releases/tag/{prefix}{latest}\n\n",
        prefix = RELEASE_TAG_PREFIX,
    )
}

// ── Enable / disable logic ───────────────────────────────────────────────

/// True when the update check should run on this invocation.
///
/// Order of precedence (any one being "off" wins):
/// 1. Env var `CUA_DRIVER_RS_UPDATE_CHECK` (falsy → off)
/// 2. Persisted config `update_check_enabled` (false → off)
/// 3. Built `CARGO_PKG_VERSION` is a pre-release (`-dev`, `-rc.1`, …) → off
fn is_enabled() -> bool {
    if let Some(false) = parse_env_bool(ENV_UPDATE_CHECK) {
        return false;
    }
    if let Some(false) = read_config_flag() {
        return false;
    }
    if is_prerelease(env!("CARGO_PKG_VERSION")) {
        return false;
    }
    true
}

/// Read the `update_check_enabled` flag out of the same JSON config file
/// the `cua-driver config set` subcommand writes to. Returns `None` when
/// the file is missing, unreadable, or doesn't have the key.
fn read_config_flag() -> Option<bool> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = PathBuf::from(home).join(".cua-driver").join("config.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("update_check_enabled").and_then(|v| v.as_bool())
}

fn parse_env_bool(var: &str) -> Option<bool> {
    let raw = std::env::var(var).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => Some(false),
        "1" | "true" | "yes" | "on" => Some(true),
        _ => None,
    }
}

/// True when `version` carries any semver pre-release suffix.
///
/// Returns `true` (skip the check) for unparseable strings too — better
/// to silently skip than to print a banner against a malformed version.
pub(crate) fn is_prerelease(version: &str) -> bool {
    match semver::Version::parse(version) {
        Ok(v) => !v.pre.is_empty(),
        Err(_) => true,
    }
}

// ── Semver comparison ────────────────────────────────────────────────────

/// True when `latest` is strictly newer than `current` under semver.
///
/// Unparseable inputs return `false` so a broken release tag or
/// `CARGO_PKG_VERSION` quirk can never produce a spurious "update
/// available" banner. We'd rather miss a real update than nag the user
/// about a phantom one.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let Ok(l) = semver::Version::parse(latest) else { return false; };
    let Ok(c) = semver::Version::parse(current) else { return false; };
    l > c
}

// ── On-disk cache ────────────────────────────────────────────────────────

/// Serialised shape of `~/.cua-driver-rs/version_check.json`.
///
/// `last_checked_unix` is the source of truth for cache-age decisions;
/// `last_checked_at` is the same instant rendered as ISO-8601 for human
/// inspection of the file. Both fields are optional so we can write
/// partial state (e.g. only a dismissed list) without forcing a sentinel.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct VersionCache {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub dismissed_versions: Vec<String>,
}

/// Read the cache file. Returns `None` when missing / unreadable / not
/// valid JSON — callers fall back to the default (empty) shape.
fn read_cache() -> Option<VersionCache> {
    let path = cache_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write the cache file. Creates the parent directory if missing.
/// IO errors propagate so the caller can decide whether to log; the
/// fire-and-forget callers in this module always swallow them.
fn write_cache(cache: &VersionCache) -> std::io::Result<()> {
    let path = cache_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no HOME / USERPROFILE — cannot resolve version_check cache path",
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)
}

fn cache_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(HOME_SUBDIRECTORY).join(CACHE_FILE_NAME))
}

// ── HTTP fetch (shared with the `update` subcommand) ─────────────────────

/// Fetch the highest `cua-driver-rs-v*` release tag from GitHub.
///
/// Uses `ureq` (already a dep via telemetry) so we don't shell out to
/// `curl` from a background task and stay cross-platform. Filters out:
///
/// - tags that don't start with `cua-driver-rs-v` (Swift port releases)
/// - draft releases (`"draft": true`)
/// - pre-releases (`"prerelease": true`)
///
/// Returns the bare version string (e.g. `"0.1.4"`) on success, or a
/// human-readable error string on failure. The caller is expected to
/// downgrade errors to `tracing::debug!`.
pub(crate) fn fetch_latest_version() -> Result<String, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(HTTP_TIMEOUT_SECONDS)))
        .build()
        .new_agent();

    let response = agent
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("cua-driver-rs/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| format!("HTTP error: {e}"))?;

    let body: serde_json::Value = response
        .into_body()
        .read_json()
        .map_err(|e| format!("JSON parse error: {e}"))?;

    pick_latest_release(&body)
        .ok_or_else(|| "no matching cua-driver-rs-v* release in response".to_owned())
}

/// Pull the highest non-draft non-prerelease `cua-driver-rs-v*` tag out
/// of the parsed releases response. Split out so unit tests can feed in
/// canned JSON without hitting the network.
pub(crate) fn pick_latest_release(body: &serde_json::Value) -> Option<String> {
    let releases = body.as_array()?;
    let mut versions: Vec<semver::Version> = releases
        .iter()
        .filter_map(|r| {
            let tag = r.get("tag_name")?.as_str()?;
            let bare = tag.strip_prefix(RELEASE_TAG_PREFIX)?;
            if r.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) {
                return None;
            }
            if r.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false) {
                return None;
            }
            semver::Version::parse(bare).ok()
        })
        .collect();
    versions.sort();
    versions.last().map(|v| v.to_string())
}

// ── Time helpers ─────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Render a unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`. Mirrors the
/// telemetry module's formatter so the on-disk cache file stays
/// human-readable without dragging in `chrono`.
fn iso8601(unix_secs: u64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix(unix_secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_unix(unix_secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = (unix_secs % 86_400) as u32;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    // Howard Hinnant's civil_from_days, shifted to 0000-03-01 era epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (year, m, d, hour, minute, second)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// All env-mutating tests serialise on this lock — `std::env::set_var`
    /// is process-global, parallel tests would race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Redirect `HOME` / `USERPROFILE` to a fresh temp dir for the body
    /// of `f`, then restore. Ensures the cache file lives in an isolated
    /// directory and never touches the developer's real
    /// `~/.cua-driver-rs/version_check.json`.
    fn with_isolated_home<R>(f: impl FnOnce(&std::path::Path) -> R) -> R {
        let tmp = tempfile::tempdir().expect("tempdir");
        let saved_home = std::env::var_os("HOME");
        let saved_userprofile = std::env::var_os("USERPROFILE");
        unsafe { std::env::set_var("HOME", tmp.path()); }
        unsafe { std::env::set_var("USERPROFILE", tmp.path()); }

        let result = f(tmp.path());

        match saved_home {
            Some(s) => unsafe { std::env::set_var("HOME", s); },
            None => unsafe { std::env::remove_var("HOME"); },
        }
        match saved_userprofile {
            Some(s) => unsafe { std::env::set_var("USERPROFILE", s); },
            None => unsafe { std::env::remove_var("USERPROFILE"); },
        }
        result
    }

    // ── is_newer ────────────────────────────────────────────────────────

    #[test]
    fn is_newer_recognises_strict_patch_bump() {
        assert!(is_newer("0.1.4", "0.1.3"));
    }

    #[test]
    fn is_newer_recognises_minor_bump_past_double_digit() {
        // 0.2.0 > 0.1.99 — naive lexicographic compare would fail here.
        assert!(is_newer("0.2.0", "0.1.99"));
    }

    #[test]
    fn is_newer_treats_pre_release_as_lower_than_release() {
        // 0.1.3 > 0.1.3-dev — semver rule: release > pre-release of same triple.
        assert!(is_newer("0.1.3", "0.1.3-dev"));
    }

    #[test]
    fn is_newer_returns_false_for_equal_versions() {
        assert!(!is_newer("0.1.3", "0.1.3"));
    }

    #[test]
    fn is_newer_returns_false_for_older_candidate() {
        assert!(!is_newer("0.1.2", "0.1.3"));
        assert!(!is_newer("0.0.99", "0.1.0"));
    }

    #[test]
    fn is_newer_returns_false_for_unparseable_input() {
        // Garbage in → false out. Better to miss a real update than nag
        // about a phantom one.
        assert!(!is_newer("not-a-version", "0.1.3"));
        assert!(!is_newer("0.1.4", "broken"));
    }

    // ── Pre-release detection ───────────────────────────────────────────

    #[test]
    fn is_prerelease_flags_dev_and_rc_suffixes() {
        assert!(is_prerelease("0.1.4-dev"));
        assert!(is_prerelease("0.1.4-rc.1"));
        assert!(is_prerelease("0.1.4-beta"));
    }

    #[test]
    fn is_prerelease_passes_plain_release() {
        assert!(!is_prerelease("0.1.4"));
        assert!(!is_prerelease("1.0.0"));
    }

    #[test]
    fn is_prerelease_returns_true_for_garbage() {
        // Unparseable strings count as "pre-release" so the update check
        // skips them rather than blowing up.
        assert!(is_prerelease("not-a-version"));
        assert!(is_prerelease(""));
    }

    // ── Cache round-trip ────────────────────────────────────────────────

    #[test]
    fn cache_round_trips_through_tempdir() {
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            let original = VersionCache {
                last_checked_unix: Some(1_700_000_000),
                last_checked_at: Some("2023-11-14T22:13:20Z".into()),
                latest_version: Some("0.1.4".into()),
                dismissed_versions: vec!["0.1.3".into()],
            };
            write_cache(&original).expect("write_cache");
            let read_back = read_cache().expect("read_cache");
            assert_eq!(read_back.last_checked_unix, Some(1_700_000_000));
            assert_eq!(read_back.latest_version.as_deref(), Some("0.1.4"));
            assert_eq!(read_back.dismissed_versions, vec!["0.1.3".to_owned()]);
        });
    }

    #[test]
    fn dismissed_versions_persist_across_writes() {
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            // First dismissal.
            dismiss_version("0.1.4");
            let after_first = read_cache().expect("cache after first");
            assert_eq!(after_first.dismissed_versions, vec!["0.1.4".to_owned()]);

            // Second dismissal of a different version appends, doesn't replace.
            dismiss_version("0.1.5");
            let after_second = read_cache().expect("cache after second");
            assert_eq!(
                after_second.dismissed_versions,
                vec!["0.1.4".to_owned(), "0.1.5".to_owned()],
            );

            // Re-dismissing an already-dismissed version is idempotent.
            dismiss_version("0.1.4");
            let after_dup = read_cache().expect("cache after dup");
            assert_eq!(
                after_dup.dismissed_versions,
                vec!["0.1.4".to_owned(), "0.1.5".to_owned()],
            );
        });
    }

    // ── 20h refresh threshold ───────────────────────────────────────────

    #[test]
    fn cache_older_than_threshold_triggers_refresh() {
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            // Seed the cache 21 hours in the past.
            let now = unix_now();
            let stale = VersionCache {
                last_checked_unix: Some(now.saturating_sub(21 * 60 * 60)),
                last_checked_at: Some(iso8601(now.saturating_sub(21 * 60 * 60))),
                latest_version: Some("0.1.3".into()), // stale data
                dismissed_versions: vec![],
            };
            write_cache(&stale).unwrap();

            let mut buf: Vec<u8> = Vec::new();
            let fetch_calls = std::sync::atomic::AtomicUsize::new(0);
            run_check_and_announce(
                "0.1.3",
                || {
                    fetch_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok("0.1.4".to_owned()) // newer version returned by network
                },
                &mut buf,
            );

            assert_eq!(
                fetch_calls.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "stale cache must trigger a fetch",
            );
            let banner = String::from_utf8(buf).unwrap();
            assert!(banner.contains("v0.1.4 is available"), "got: {banner:?}");
            assert!(banner.contains("you have v0.1.3"), "got: {banner:?}");
        });
    }

    #[test]
    fn cache_younger_than_threshold_skips_network() {
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            // Seed the cache 1 hour in the past with a known-newer version.
            let now = unix_now();
            let fresh = VersionCache {
                last_checked_unix: Some(now.saturating_sub(60 * 60)),
                last_checked_at: Some(iso8601(now.saturating_sub(60 * 60))),
                latest_version: Some("0.1.4".into()),
                dismissed_versions: vec![],
            };
            write_cache(&fresh).unwrap();

            let mut buf: Vec<u8> = Vec::new();
            let fetch_calls = std::sync::atomic::AtomicUsize::new(0);
            run_check_and_announce(
                "0.1.3",
                || {
                    fetch_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok("0.1.5".to_owned())
                },
                &mut buf,
            );

            assert_eq!(
                fetch_calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "fresh cache must NOT trigger a fetch",
            );
            let banner = String::from_utf8(buf).unwrap();
            // Banner uses the cached value (0.1.4), not the would-be network value (0.1.5).
            assert!(banner.contains("v0.1.4 is available"), "got: {banner:?}");
        });
    }

    #[test]
    fn dismissed_latest_suppresses_banner() {
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            // Cache is fresh and reports a newer version, but the user
            // dismissed exactly that version — banner must stay silent.
            let now = unix_now();
            let cache = VersionCache {
                last_checked_unix: Some(now),
                last_checked_at: Some(iso8601(now)),
                latest_version: Some("0.1.4".into()),
                dismissed_versions: vec!["0.1.4".into()],
            };
            write_cache(&cache).unwrap();

            let mut buf: Vec<u8> = Vec::new();
            run_check_and_announce("0.1.3", || Ok("0.1.4".to_owned()), &mut buf);
            assert!(buf.is_empty(), "dismissed version must suppress banner");
        });
    }

    // ── Opt-out paths ───────────────────────────────────────────────────

    #[test]
    fn env_opt_out_short_circuits_enabled_check() {
        let _g = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os(ENV_UPDATE_CHECK);

        unsafe { std::env::set_var(ENV_UPDATE_CHECK, "false"); }
        // Use a separately-redirected HOME so any config file present on
        // the developer's machine can't influence the result.
        with_isolated_home(|_| {
            assert!(!is_enabled(), "explicit false env var must disable");
        });

        unsafe { std::env::set_var(ENV_UPDATE_CHECK, "0"); }
        with_isolated_home(|_| {
            assert!(!is_enabled(), "0 must disable");
        });

        unsafe { std::env::set_var(ENV_UPDATE_CHECK, "off"); }
        with_isolated_home(|_| {
            assert!(!is_enabled(), "off must disable");
        });

        // Restore.
        match saved {
            Some(s) => unsafe { std::env::set_var(ENV_UPDATE_CHECK, s); },
            None => unsafe { std::env::remove_var(ENV_UPDATE_CHECK); },
        }
    }

    #[test]
    fn config_flag_disables_check() {
        let _g = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os(ENV_UPDATE_CHECK);
        unsafe { std::env::remove_var(ENV_UPDATE_CHECK); }

        with_isolated_home(|home| {
            // Write a config that disables the check.
            let cfg_dir = home.join(".cua-driver");
            std::fs::create_dir_all(&cfg_dir).unwrap();
            std::fs::write(
                cfg_dir.join("config.json"),
                r#"{"update_check_enabled": false}"#,
            ).unwrap();

            // Build version is a normal release (this crate's pkg version
            // ships as "0.1.3", a stable release — see workspace Cargo.toml).
            // The env var is unset, so the config flag is the only signal.
            assert!(!is_enabled(),
                "persisted update_check_enabled=false must disable the check");
        });

        match saved {
            Some(s) => unsafe { std::env::set_var(ENV_UPDATE_CHECK, s); },
            None => unsafe { std::env::remove_var(ENV_UPDATE_CHECK); },
        }
    }

    #[test]
    fn config_flag_true_or_missing_leaves_check_on() {
        let _g = ENV_LOCK.lock().unwrap();
        let saved = std::env::var_os(ENV_UPDATE_CHECK);
        unsafe { std::env::remove_var(ENV_UPDATE_CHECK); }

        with_isolated_home(|home| {
            // Case A: no config file at all → enabled.
            // CARGO_PKG_VERSION is "0.1.3" (stable), env unset, no config file.
            assert!(is_enabled(),
                "no config file + stable version + no env opt-out → enabled");

            // Case B: config file present but flag = true → still enabled.
            let cfg_dir = home.join(".cua-driver");
            std::fs::create_dir_all(&cfg_dir).unwrap();
            std::fs::write(
                cfg_dir.join("config.json"),
                r#"{"update_check_enabled": true, "other_key": 42}"#,
            ).unwrap();
            assert!(is_enabled(),
                "explicit update_check_enabled=true must leave check on");
        });

        match saved {
            Some(s) => unsafe { std::env::set_var(ENV_UPDATE_CHECK, s); },
            None => unsafe { std::env::remove_var(ENV_UPDATE_CHECK); },
        }
    }

    // ── Banner formatting ───────────────────────────────────────────────

    #[test]
    fn banner_contains_required_lines() {
        let banner = format_banner("0.1.4", "0.1.3");
        // Headline.
        assert!(banner.contains("cua-driver v0.1.4 is available"), "got: {banner:?}");
        assert!(banner.contains("you have v0.1.3"), "got: {banner:?}");
        // Update instruction.
        assert!(banner.contains("Update with: cua-driver update"), "got: {banner:?}");
        // Release notes URL uses the correct tag prefix.
        assert!(
            banner.contains(
                "https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.1.4"
            ),
            "got: {banner:?}",
        );
    }

    // ── pick_latest_release ─────────────────────────────────────────────

    #[test]
    fn pick_latest_release_filters_swift_port_tags() {
        // Releases JSON contains both Swift-port tags and Rust-port tags;
        // only the Rust-port (`cua-driver-rs-v*`) tags must be considered.
        let body = serde_json::json!([
            {"tag_name": "cua-driver-v0.9.0", "draft": false, "prerelease": false},
            {"tag_name": "cua-driver-rs-v0.1.4", "draft": false, "prerelease": false},
            {"tag_name": "cua-driver-rs-v0.1.3", "draft": false, "prerelease": false},
            {"tag_name": "cua-driver-v9.9.9", "draft": false, "prerelease": false},
        ]);
        assert_eq!(pick_latest_release(&body).as_deref(), Some("0.1.4"));
    }

    #[test]
    fn pick_latest_release_skips_drafts_and_prereleases() {
        let body = serde_json::json!([
            {"tag_name": "cua-driver-rs-v0.2.0", "draft": true,  "prerelease": false},
            {"tag_name": "cua-driver-rs-v0.1.5", "draft": false, "prerelease": true},
            {"tag_name": "cua-driver-rs-v0.1.4", "draft": false, "prerelease": false},
        ]);
        assert_eq!(pick_latest_release(&body).as_deref(), Some("0.1.4"));
    }

    #[test]
    fn pick_latest_release_returns_none_for_empty_array() {
        assert_eq!(pick_latest_release(&serde_json::json!([])), None);
    }

    #[test]
    fn iso8601_round_trips_against_known_unix_timestamp() {
        // 2023-11-14T22:13:20Z = 1_700_000_000 unix seconds.
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    }
}
