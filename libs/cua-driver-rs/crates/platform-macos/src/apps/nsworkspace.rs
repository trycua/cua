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

use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{
    NSAppleEventDescriptor, NSArray, NSDictionary, NSError, NSString, NSURL,
};

/// FourCharCode helper — packs a 4-byte ASCII tag into a `u32` the same way
/// Apple's CoreServices headers do (`kCoreEventClass = 'aevt'` etc).
const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24)
        | ((s[1] as u32) << 16)
        | ((s[2] as u32) << 8)
        | (s[3] as u32)
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

/// Errors returned by the NSWorkspace launch helpers.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("NSWorkspace launch failed: {0}")]
    Cocoa(String),
    #[error("NSWorkspace launch returned no NSRunningApplication and no NSError")]
    NoApp,
    #[error("NSWorkspace launch did not complete within {:?}", COMPLETION_TIMEOUT)]
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

    let (tx, rx) = std::sync::mpsc::sync_channel::<CompletionResult>(1);
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

    let block = make_completion_block(tx);

    tracing::debug!(target: "platform_macos::apps::nsworkspace",
        ?app_url, "calling openApplicationAtURL");

    unsafe {
        ws.openApplicationAtURL_configuration_completionHandler(
            &url,
            &config,
            Some(&block),
        );
    }

    wait_for_completion(rx)
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

    let ns_urls: Vec<Retained<NSURL>> = urls
        .iter()
        .map(|u| file_or_app_url(u))
        .collect::<Result<_, _>>()?;
    let ns_array = NSArray::from_vec(ns_urls);

    let (tx, rx) = std::sync::mpsc::sync_channel::<CompletionResult>(1);
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

    wait_for_completion(rx)
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
            let sender = tx.lock().ok().and_then(|mut g| g.take());
            if let Some(s) = sender {
                let _ = s.send(result);
            }
        },
    )
}

/// Block on `rx` for up to `COMPLETION_TIMEOUT`. Returns `Err(Timeout)` if
/// the completion handler never fires (wedged LaunchServices).
fn wait_for_completion(
    rx: std::sync::mpsc::Receiver<CompletionResult>,
) -> Result<Retained<NSRunningApplication>, LaunchError> {
    match rx.recv_timeout(COMPLETION_TIMEOUT) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(LaunchError::Timeout),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(LaunchError::NoApp),
    }
}

/// Hand-rolled binding for
/// `-[NSAppleEventDescriptor initWithEventClass:eventID:targetDescriptor:returnID:transactionID:]`
/// (and the associated `OpenApplication` event constructor) — the selector
/// is not exposed by `objc2-foundation 0.2.2`.
mod apple_event {
    use super::{
        NSAppleEventDescriptor, NSString, Retained, K_AE_OPEN_APPLICATION,
        K_ANY_TRANSACTION_ID, K_AUTO_GENERATE_RETURN_ID, K_CORE_EVENT_CLASS,
    };
    use objc2::msg_send_id;
    use objc2::rc::Allocated;
    use objc2::ClassType;

    /// Build an `aevt/oapp` AppleEvent addressed to the bundle id `bid`.
    pub fn open_application_event(bid: &str) -> Retained<NSAppleEventDescriptor> {
        let target_string = NSString::from_str(bid);
        let target: Retained<NSAppleEventDescriptor> = unsafe {
            NSAppleEventDescriptor::descriptorWithBundleIdentifier(&target_string)
        };

        unsafe {
            let alloc: Allocated<NSAppleEventDescriptor> =
                NSAppleEventDescriptor::alloc();
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

