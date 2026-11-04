//! Thin NSWorkspace launch helpers.
//!
//! Wraps the two AppKit launch entry points used by Swift's `AppLauncher.swift`:
//!
//! * `-[NSWorkspace openApplicationAtURL:configuration:completionHandler:]` —
//!   pure launch (no URL handoff).
//! * `-[NSWorkspace openURLs:withApplicationAtURL:configuration:completionHandler:]` —
//!   launch with one or more `application(_:open:)` payloads (used to open
//!   Safari to `about:blank`, Finder to a folder, etc).
//!
//! Both share an `OpenConfig` builder that mirrors the subset of
//! `NSWorkspaceOpenConfiguration` properties Swift sets:
//!   * `activates = false`         (background launch — no focus steal)
//!   * `addsToRecentItems = false` (don't pollute the Apple menu)
//!   * `createsNewApplicationInstance = …`
//!   * `arguments = …` / `environment = …`
//!   * `appleEvent = oapp(<bundleId>)` — synthetic `aevt/oapp` AppleEvent so
//!     LaunchServices reliably triggers window creation on cold launch /
//!     state-restored apps (see comments in Swift `AppLauncher.swift`).
//!
//! The Cocoa completion handler is bridged to a synchronous return value via
//! `tokio::sync::oneshot` + a 30s timeout. A wedged launch surfaces as an
//! error instead of hanging the caller's worker thread.
//!
//! The `oapp` AppleEvent descriptor uses the
//! `init(eventClass:eventID:targetDescriptor:returnID:transactionID:)`
//! selector which `objc2-foundation 0.2.2` does not bind natively — we
//! hand-roll the binding in [`apple_event`] via `extern_methods!`.

use std::collections::HashSet;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSAppleEventDescriptor, NSArray, NSDictionary, NSError, NSString, NSURL};

/// FourCharCode helper — packs a 4-byte ASCII tag into a `u32` the same way
/// Apple's CoreServices headers do (`kCoreEventClass = 'aevt'` etc).
const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

const K_CORE_EVENT_CLASS: u32 = fourcc(b"aevt"); // kCoreEventClass
const K_AE_OPEN_APPLICATION: u32 = fourcc(b"oapp"); // kAEOpenApplication
const K_AUTO_GENERATE_RETURN_ID: i16 = -1; // kAutoGenerateReturnID
const K_ANY_TRANSACTION_ID: i32 = 0; // kAnyTransactionID

/// Caller-friendly launch options. Mirrors the subset of
/// `NSWorkspaceOpenConfiguration` properties Swift `AppLauncher` sets.
///
/// Always sends `activates = false` + `addsToRecentItems = false`. The
/// optional fields are applied only when present so the builder doesn't
/// override an inherited default.
#[derive(Default, Debug, Clone)]
pub struct OpenConfig {
    /// `--args` for the launched process. Passed as argv entries (no shell
    /// expansion).
    pub arguments: Vec<String>,
    /// Additional env vars merged into the current process environment.
    pub environment: std::collections::HashMap<String, String>,
    /// Force a fresh application instance even if one is already running.
    pub creates_new_instance: bool,
    /// Attach a synthetic `aevt/oapp` AppleEvent addressed to this bundle id.
    /// Set this whenever the bundle id is known — see Swift `AppLauncher`
    /// comment for the reasoning (cold-launch window-creation reliability).
    pub apple_event_bundle_id: Option<String>,
}

/// One-shot timeout for the LaunchServices completion handler. A wedged
/// `open(...)` call returns `Err(LaunchError::Timeout)` instead of hanging
/// the calling thread forever.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);
const COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Errors returned by the NSWorkspace launch helpers.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("NSWorkspace launch failed: {0}")]
    Cocoa(String),
    #[error("NSWorkspace launch returned no NSRunningApplication and no NSError")]
    NoApp,
    #[error(
        "NSWorkspace launch callback did not complete within {:?}, and no matching running application was observed",
        COMPLETION_TIMEOUT
    )]
    Timeout,
    #[error("invalid url: {0}")]
    BadUrl(String),
}

/// Launch the application bundle at `app_url`, no URL handoff.
///
/// Equivalent to Swift's
/// ```swift
/// NSWorkspace.shared.open(appURL, configuration: cfg) { app, err in … }
/// ```
/// — the URL points to a `.app` bundle, NSWorkspace launches it,
/// `activates = false` keeps the prior frontmost app on top, and the
/// `oapp` AppleEvent attached to `cfg.appleEvent` triggers window creation
/// on cold launch.
/// `app_url` may be either:
///   * a bundle id (`com.apple.Safari`) — resolved via
///     `NSWorkspace.URLForApplicationWithBundleIdentifier`, preserving
///     the NSURL exactly as LaunchServices returns it (avoids
///     round-tripping through `path()` which can lose alias/cryptex
///     metadata — Safari lives under `/System/Cryptexes/App/...` and
///     can't be re-opened from its `path()` string),
///   * a filesystem path to an `.app` bundle (`/Applications/Foo.app`),
///   * a URL string with a scheme (`file:///...`, `http://...`).
pub fn open_application(
    app_url: &str,
    cfg: &OpenConfig,
) -> Result<Retained<NSRunningApplication>, LaunchError> {
    let ws = unsafe { NSWorkspace::sharedWorkspace() };
    let url = resolve_application_url(&ws, app_url)?;
    let config = build_configuration(cfg);
    let reconciliation = Reconciliation::new(cfg);

    let (tx, rx) = std::sync::mpsc::sync_channel::<CompletionResult>(1);
    // NSWorkspace may invoke its completion on another thread; retain atomic
    // callback ownership even though objc2 does not mark the retained app Send.
    #[allow(clippy::arc_with_non_send_sync)]
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

    let block = make_completion_block(tx);

    tracing::debug!(target: "platform_macos::apps::nsworkspace",
        ?app_url, "calling openApplicationAtURL");

    unsafe {
        ws.openApplicationAtURL_configuration_completionHandler(&url, &config, Some(&block));
    }

    wait_for_completion(rx, reconciliation)
}

/// Launch the application bundle at `app_url` and hand it `urls` via
/// `application(_:open:)`.
///
/// Equivalent to Swift's
/// ```swift
/// NSWorkspace.shared.open(urls, withApplicationAt: appURL,
///                         configuration: cfg) { app, err in … }
/// ```
pub fn open_urls_with_application(
    urls: &[String],
    app_url: &str,
    cfg: &OpenConfig,
) -> Result<Retained<NSRunningApplication>, LaunchError> {
    let ws = unsafe { NSWorkspace::sharedWorkspace() };
    let url = resolve_application_url(&ws, app_url)?;
    let config = build_configuration(cfg);
    let reconciliation = Reconciliation::new(cfg);

    let ns_urls: Vec<Retained<NSURL>> = urls
        .iter()
        .map(|u| file_or_app_url(u))
        .collect::<Result<_, _>>()?;
    let ns_array = NSArray::from_vec(ns_urls);

    let (tx, rx) = std::sync::mpsc::sync_channel::<CompletionResult>(1);
    // NSWorkspace may invoke its completion on another thread; retain atomic
    // callback ownership even though objc2 does not mark the retained app Send.
    #[allow(clippy::arc_with_non_send_sync)]
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

    let block = make_completion_block(tx);

    tracing::debug!(target: "platform_macos::apps::nsworkspace",
        ?app_url, urls_len = urls.len(), "calling openURLs:withApplicationAtURL");

    unsafe {
        ws.openURLs_withApplicationAtURL_configuration_completionHandler(
            &ns_array,
            &url,
            &config,
            Some(&block),
        );
    }

    wait_for_completion(rx, reconciliation)
}

// ── Internal helpers ─────────────────────────────────────────────────────────

/// Build an `NSWorkspaceOpenConfiguration` from `cfg`.
///
/// Always sets `activates = false` and `addsToRecentItems = false` to match
/// Swift's background-launch invariant.
fn build_configuration(cfg: &OpenConfig) -> Retained<NSWorkspaceOpenConfiguration> {
    let config = unsafe { NSWorkspaceOpenConfiguration::configuration() };
    unsafe {
        config.setActivates(false);
        config.setAddsToRecentItems(false);
        config.setCreatesNewApplicationInstance(cfg.creates_new_instance);

        if !cfg.arguments.is_empty() {
            let strs: Vec<Retained<NSString>> = cfg
                .arguments
                .iter()
                .map(|a| NSString::from_str(a))
                .collect();
            let arr = NSArray::from_vec(strs);
            config.setArguments(&arr);
        }

        if !cfg.environment.is_empty() {
            // Pass ONLY the caller's explicit overrides — never merge in
            // `std::env::vars()`. The launching process inherits its
            // environment from the user's shell (or systemd/launchd
            // service), which carries secrets, tokens, API keys, SSH
            // agent sockets, etc. Forwarding any of that to every
            // launched app is a real security leak.
            //
            // If `cfg.environment` is empty we don't set the field at
            // all (above guard) — LaunchServices then applies the
            // default app environment, same as a Finder double-click.
            let entries: Vec<(&String, &String)> = cfg.environment.iter().collect();
            let keys: Vec<Retained<NSString>> =
                entries.iter().map(|(k, _)| NSString::from_str(k)).collect();
            let vals: Vec<Retained<NSString>> =
                entries.iter().map(|(_, v)| NSString::from_str(v)).collect();
            let key_refs: Vec<&NSString> = keys.iter().map(|s| s.as_ref()).collect();
            let dict = NSDictionary::from_vec(&key_refs, vals);
            config.setEnvironment(&dict);
        }

        if let Some(bid) = &cfg.apple_event_bundle_id {
            if !bid.is_empty() {
                let event = apple_event::open_application_event(bid);
                config.setAppleEvent(Some(&event));
            }
        }
    }
    config
}

/// Resolve a caller-supplied app reference (bundle id, path, or URL
/// string) into the `NSURL` LaunchServices wants. Bundle ids preferred
/// when available — they preserve the NSURL exactly as LaunchServices
/// hands it back, which is the only thing that reliably launches
/// Cryptex-installed apps (Safari, etc).
fn resolve_application_url(
    ws: &NSWorkspace,
    app_ref: &str,
) -> Result<Retained<NSURL>, LaunchError> {
    // Heuristic: bundle ids contain at least one '.' and no '/' and no
    // '://'. Otherwise treat as a path / URL string.
    let looks_like_bundle_id =
        !app_ref.contains('/') && !app_ref.contains("://") && app_ref.contains('.');
    if looks_like_bundle_id {
        let ns = NSString::from_str(app_ref);
        if let Some(url) = unsafe { ws.URLForApplicationWithBundleIdentifier(&ns) } {
            return Ok(url);
        }
        // Fall through to the path/URL parse — caller might have passed
        // something odd that happens to look like a bundle id (e.g. an
        // app folder name).
    }
    file_or_app_url(app_ref)
}

/// Build an `NSURL` from a caller-supplied string.
///
/// * Strings containing `:` (any URL scheme — `http://`, `https://`,
///   `file://`, `about:blank`, custom schemes) go through `URLWithString:`.
///   The colon test catches `about:blank` (no `://`) which Swift's
///   `URL(string:)` also handles.
/// * Anything else is a bare filesystem path → `fileURLWithPath:`.
fn file_or_app_url(s: &str) -> Result<Retained<NSURL>, LaunchError> {
    if s.is_empty() {
        return Err(LaunchError::BadUrl("empty".into()));
    }
    unsafe {
        // A path can contain a colon (rare) but is far less likely to
        // start with `scheme:`. Use a slightly stricter test: must
        // contain a colon AND not start with `/` AND not start with `~`.
        let looks_like_url = s.contains(':') && !s.starts_with('/') && !s.starts_with('~');
        if looks_like_url {
            let ns = NSString::from_str(s);
            match NSURL::URLWithString(&ns) {
                Some(u) => Ok(u),
                None => Err(LaunchError::BadUrl(s.into())),
            }
        } else {
            let ns = NSString::from_str(s);
            Ok(NSURL::fileURLWithPath(&ns))
        }
    }
}

type CompletionResult = Result<Retained<NSRunningApplication>, LaunchError>;

/// Actual-process fallback for daemons where LaunchServices accepts the
/// request but never delivers the completion block. A bundle id is required
/// because it is the only stable identity shared by the request and
/// `NSRunningApplication`.
struct Reconciliation {
    bundle_id: Option<String>,
    pids_before_request: HashSet<i32>,
    require_new_pid: bool,
}

impl Reconciliation {
    fn new(cfg: &OpenConfig) -> Self {
        let bundle_id = cfg.apple_event_bundle_id.clone();
        let pids_before_request = bundle_id
            .as_deref()
            .map(running_pids_for_bundle)
            .unwrap_or_default();
        Self {
            bundle_id,
            pids_before_request,
            require_new_pid: cfg.creates_new_instance,
        }
    }

    fn running_application(&self) -> Option<Retained<NSRunningApplication>> {
        let bundle_id = self.bundle_id.as_deref()?;
        running_application_for_bundle(bundle_id, &self.pids_before_request, self.require_new_pid)
    }
}

fn running_pids_for_bundle(bundle_id: &str) -> HashSet<i32> {
    let bundle_id = NSString::from_str(bundle_id);
    let running =
        unsafe { NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle_id) };
    let mut pids = HashSet::new();
    unsafe {
        for index in 0..running.count() {
            let app = running.objectAtIndex(index);
            let pid = app.processIdentifier();
            if pid > 0 && !app.isTerminated() {
                pids.insert(pid);
            }
        }
    }
    pids
}

fn running_application_for_bundle(
    bundle_id: &str,
    pids_before_request: &HashSet<i32>,
    require_new_pid: bool,
) -> Option<Retained<NSRunningApplication>> {
    let current_pids = running_pids_for_bundle(bundle_id);
    reconciliation_pid(&current_pids, pids_before_request, require_new_pid).and_then(|pid| unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
    })
}

fn reconciliation_pid(
    current_pids: &HashSet<i32>,
    pids_before_request: &HashSet<i32>,
    require_new_pid: bool,
) -> Option<i32> {
    current_pids
        .iter()
        .copied()
        .find(|pid| !require_new_pid || !pids_before_request.contains(pid))
}

/// Build the `(NSRunningApplication?, NSError?) -> Void` completion block.
///
/// Sends exactly one result through `tx`. Late completions (after timeout)
/// land in `try_send` and are silently dropped — the channel is closed.
fn make_completion_block(
    tx: Arc<std::sync::Mutex<Option<std::sync::mpsc::SyncSender<CompletionResult>>>>,
) -> RcBlock<dyn Fn(*mut NSRunningApplication, *mut NSError)> {
    RcBlock::new(
        move |app_ptr: *mut NSRunningApplication, err_ptr: *mut NSError| {
            let result: CompletionResult = unsafe {
                if !err_ptr.is_null() {
                    let err = &*err_ptr;
                    let desc = err.localizedDescription();
                    Err(LaunchError::Cocoa(desc.to_string()))
                } else if let Some(app_ref) = NonNull::new(app_ptr) {
                    // `Retained::retain` bumps the refcount so we own it
                    // beyond the block scope.
                    match Retained::retain(app_ref.as_ptr()) {
                        Some(r) => Ok(r),
                        None => Err(LaunchError::NoApp),
                    }
                } else {
                    Err(LaunchError::NoApp)
                }
            };
            // Take the sender out so the channel closes after one send.
            let sender = tx.lock().ok().and_then(|mut guard| guard.take());
            if let Some(s) = sender {
                let _ = s.send(result);
            }
        },
    )
}

/// Wait for either the completion callback or actual process registration.
///
/// LaunchServices can accept an `openApplication...` request and register an
/// `NSRunningApplication` without delivering the callback to a background
/// daemon. Treating the callback as the sole success signal turns that state
/// into a false 30-second timeout. Poll the authoritative running-app state
/// while retaining the callback as the preferred source of Cocoa errors.
fn wait_for_completion(
    rx: std::sync::mpsc::Receiver<CompletionResult>,
    reconciliation: Reconciliation,
) -> Result<Retained<NSRunningApplication>, LaunchError> {
    wait_for_launch_signal(rx, COMPLETION_TIMEOUT, COMPLETION_POLL_INTERVAL, || {
        reconciliation.running_application()
    })
}

fn wait_for_launch_signal<T>(
    rx: std::sync::mpsc::Receiver<Result<T, LaunchError>>,
    timeout: Duration,
    poll_interval: Duration,
    mut reconcile: impl FnMut() -> Option<T>,
) -> Result<T, LaunchError> {
    let deadline = Instant::now() + timeout;
    let mut callback_disconnected = false;

    loop {
        if !callback_disconnected {
            match rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    callback_disconnected = true;
                }
            }
        }

        if let Some(running) = reconcile() {
            return Ok(running);
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(LaunchError::Timeout);
        }
        let wait = poll_interval.min(deadline.saturating_duration_since(now));

        if callback_disconnected {
            std::thread::sleep(wait);
            continue;
        }

        match rx.recv_timeout(wait) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                callback_disconnected = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{reconciliation_pid, wait_for_launch_signal, LaunchError};
    use std::collections::HashSet;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn missing_callback_reconciles_registered_process() {
        let (_tx, rx) = mpsc::sync_channel(1);
        let mut polls = 0;
        let result = wait_for_launch_signal(
            rx,
            Duration::from_millis(100),
            Duration::from_millis(1),
            || {
                polls += 1;
                (polls == 3).then_some(4242)
            },
        );

        assert_eq!(result.unwrap(), 4242);
    }

    #[test]
    fn queued_callback_error_wins_over_process_reconciliation() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(Err(LaunchError::Cocoa("denied".to_owned())))
            .unwrap();

        let result = wait_for_launch_signal(
            rx,
            Duration::from_millis(100),
            Duration::from_millis(1),
            || Some(4242),
        );

        assert!(matches!(result, Err(LaunchError::Cocoa(message)) if message == "denied"));
    }

    #[test]
    fn no_callback_or_process_returns_precise_bounded_timeout() {
        let (_tx, rx) = mpsc::sync_channel::<Result<i32, LaunchError>>(1);
        let result = wait_for_launch_signal(
            rx,
            Duration::from_millis(5),
            Duration::from_millis(1),
            || None,
        );

        assert!(matches!(result, Err(LaunchError::Timeout)));
    }

    #[test]
    fn new_instance_reconciliation_rejects_preexisting_pid() {
        let before = HashSet::from([100]);
        assert_eq!(
            reconciliation_pid(&HashSet::from([100]), &before, true),
            None
        );
        assert_eq!(
            reconciliation_pid(&HashSet::from([100, 200]), &before, true),
            Some(200)
        );
        assert_eq!(
            reconciliation_pid(&HashSet::from([100]), &before, false),
            Some(100)
        );
    }
}

/// Hand-rolled binding for
/// `-[NSAppleEventDescriptor initWithEventClass:eventID:targetDescriptor:returnID:transactionID:]`
/// (and the associated `OpenApplication` event constructor) — the selector
/// is not exposed by `objc2-foundation 0.2.2`.
mod apple_event {
    use super::{
        NSAppleEventDescriptor, NSString, Retained, K_AE_OPEN_APPLICATION, K_ANY_TRANSACTION_ID,
        K_AUTO_GENERATE_RETURN_ID, K_CORE_EVENT_CLASS,
    };
    use objc2::msg_send_id;
    use objc2::rc::Allocated;
    use objc2::ClassType;

    /// Build an `aevt/oapp` AppleEvent addressed to the bundle id `bid`.
    pub fn open_application_event(bid: &str) -> Retained<NSAppleEventDescriptor> {
        let target_string = NSString::from_str(bid);
        let target: Retained<NSAppleEventDescriptor> =
            unsafe { NSAppleEventDescriptor::descriptorWithBundleIdentifier(&target_string) };

        unsafe {
            let alloc: Allocated<NSAppleEventDescriptor> = NSAppleEventDescriptor::alloc();
            // initWithEventClass:eventID:targetDescriptor:returnID:transactionID:
            let event: Retained<NSAppleEventDescriptor> = msg_send_id![
                alloc,
                initWithEventClass: K_CORE_EVENT_CLASS,
                eventID: K_AE_OPEN_APPLICATION,
                targetDescriptor: &*target,
                returnID: K_AUTO_GENERATE_RETURN_ID,
                transactionID: K_ANY_TRANSACTION_ID,
            ];
            event
        }
    }
}
