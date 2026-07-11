//! BrowserJS: run JavaScript in Chrome/Brave/Edge/Safari through the browsers'
//! Apple Event suites.

use anyhow::Context;
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::windows::WindowBounds;

pub struct BrowserJs;

const CHROME_APP_BUNDLE_PREFIX: &str = "com.google.Chrome.app.";

#[derive(Clone, Debug)]
struct NativeWindowTarget {
    title: String,
    bounds: WindowBounds,
    same_bounds_ordinal: usize,
}

fn app_name_for_bundle(bundle_id: &str) -> Option<&'static str> {
    match bundle_id {
        "com.google.Chrome" => Some("Google Chrome"),
        "com.brave.Browser" => Some("Brave Browser"),
        "com.microsoft.edgemac" => Some("Microsoft Edge"),
        "com.apple.Safari" => Some("Safari"),
        _ if bundle_id.starts_with(CHROME_APP_BUNDLE_PREFIX) => Some("Google Chrome"),
        _ => None,
    }
}

impl BrowserJs {
    /// Returns true if this bundle ID is a supported browser.
    pub fn supports(bundle_id: &str) -> bool {
        app_name_for_bundle(bundle_id).is_some()
    }

    /// Execute JavaScript in the browser window identified by window_id.
    pub async fn execute(
        javascript: &str,
        bundle_id: &str,
        window_id: u32,
    ) -> anyhow::Result<String> {
        if !Self::supports(bundle_id) {
            anyhow::bail!("Unsupported browser bundle: {bundle_id}");
        }
        let javascript = javascript.to_owned();
        let bundle_id = bundle_id.to_owned();
        let script = tokio::task::spawn_blocking(move || {
            build_browser_script(&javascript, &bundle_id, window_id)
        })
        .await
        .context("Preparing native browser Apple Event failed")??;

        execute_script_on_main_queue(script).await
    }

    /// Patch the browser Preferences JSON to enable Allow JavaScript from Apple Events,
    /// then relaunch the browser.
    pub async fn enable_javascript_apple_events(bundle_id: &str) -> anyhow::Result<()> {
        let app_name = app_name_for_bundle(bundle_id)
            .ok_or_else(|| anyhow::anyhow!("Unsupported browser bundle: {bundle_id}"))?;

        // Quit through NSRunningApplication and wait for the corresponding
        // NSWorkspace termination notifications before touching Preferences.
        // This avoids racing the browser's own shutdown-time preferences write.
        let bundle_id_for_quit = bundle_id.to_owned();
        tokio::task::spawn_blocking(move || {
            terminate_running_bundle(&bundle_id_for_quit, Duration::from_secs(15))
        })
        .await
        .context("Native browser termination task failed")??;

        // Find profile directory.
        let home = std::env::var("HOME").unwrap_or_default();
        let profiles_dir = match bundle_id {
            "com.google.Chrome" => format!("{home}/Library/Application Support/Google/Chrome"),
            _ if bundle_id.starts_with(CHROME_APP_BUNDLE_PREFIX) => {
                format!("{home}/Library/Application Support/Google/Chrome")
            }
            "com.brave.Browser" => {
                format!("{home}/Library/Application Support/BraveSoftware/Brave-Browser")
            }
            "com.microsoft.edgemac" => format!("{home}/Library/Application Support/Microsoft Edge"),
            _ => anyhow::bail!("No profiles directory for {bundle_id}"),
        };

        // Find all Preferences files.
        let prefs_files = find_preferences_files(&profiles_dir);

        for path in prefs_files {
            if let Err(e) = patch_preferences_file(&path) {
                tracing::warn!("Failed to patch {path}: {e}");
            }
        }

        // Relaunch through NSWorkspace rather than the `open` executable.
        crate::apps::launch_app(bundle_id).with_context(|| format!("Relaunching {app_name}"))?;

        Ok(())
    }
}

fn build_browser_script(
    javascript: &str,
    bundle_id: &str,
    window_id: u32,
) -> anyhow::Result<String> {
    let app_name = app_name_for_bundle(bundle_id)
        .ok_or_else(|| anyhow::anyhow!("Unsupported browser bundle: {bundle_id}"))?;

    // Resolve the WindowServer CGWindowID to stable properties that can be
    // matched against the browser's scripting window model. This work can be
    // slow, so it stays off the AppKit main thread.
    let target = native_window_target(window_id)?;
    let escaped_js = escape_js_for_applescript(javascript);

    if bundle_id == "com.apple.Safari" {
        let escaped_title = escape_for_applescript_string(&target.title);
        Ok(format!(
            r#"tell application "Safari"
  set matchedDoc to missing value
  repeat with d in documents
    if name of d contains "{escaped_title}" then
      set matchedDoc to d
      exit repeat
    end if
  end repeat
  if matchedDoc is missing value then
    set matchedDoc to document 1
  end if
  do JavaScript {escaped_js} in matchedDoc
end tell"#
        ))
    } else {
        Ok(chromium_window_script(
            app_name,
            &target,
            &escaped_js,
            window_id,
        ))
    }
}

struct MainThreadScript {
    script: String,
    reply: tokio::sync::oneshot::Sender<anyhow::Result<String>>,
    state: Arc<AtomicU8>,
}

const SCRIPT_QUEUED: u8 = 0;
const SCRIPT_RUNNING: u8 = 1;
const SCRIPT_CANCELLED: u8 = 2;
const SCRIPT_FINISHED: u8 = 3;

fn cancel_queued_script(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            SCRIPT_QUEUED,
            SCRIPT_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn claim_queued_script(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            SCRIPT_QUEUED,
            SCRIPT_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

struct QueuedScriptCancellation {
    state: Arc<AtomicU8>,
}

impl Drop for QueuedScriptCancellation {
    fn drop(&mut self) {
        let _ = cancel_queued_script(&self.state);
    }
}

#[link(name = "dispatch", kind = "dylib")]
extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

/// Execute NSAppleScript on the AppKit main thread, as required by Apple's
/// documented threading contract. The binary entry points keep the AppKit run
/// loop alive even when the cursor overlay is disabled, so this dispatch is
/// serviced in MCP, daemon, and one-shot CLI modes.
async fn execute_script_on_main_queue(script: String) -> anyhow::Result<String> {
    if !crate::session::has_graphic_access() {
        anyhow::bail!("Browser automation requires an active macOS graphic session");
    }
    if let Some(main_thread) = objc2_foundation::MainThreadMarker::new() {
        return execute_script_on_main_thread(main_thread, &script);
    }
    if !crate::pip::appkit_main_loop_available() {
        anyhow::bail!(
            "Browser automation requires a registered AppKit main loop; the current host is not servicing main-queue work"
        );
    }

    let (reply, response) = tokio::sync::oneshot::channel();
    let state = Arc::new(AtomicU8::new(SCRIPT_QUEUED));
    let _cancel_on_drop = QueuedScriptCancellation {
        state: state.clone(),
    };
    let request = Box::new(MainThreadScript {
        script,
        reply,
        state: state.clone(),
    });
    unsafe {
        let main_queue = &raw const _dispatch_main_q as *const c_void;
        dispatch_async_f(
            main_queue,
            Box::into_raw(request) as *mut c_void,
            execute_script_main_cb,
        );
    }

    let mut response = response;
    match tokio::time::timeout(Duration::from_secs(20), &mut response).await {
        Ok(reply) => reply.context("AppKit main-thread browser automation task was cancelled")?,
        Err(_) => {
            if cancel_queued_script(&state) {
                anyhow::bail!(
                    "Timed out before the AppKit main thread began browser automation; the queued script was cancelled"
                );
            }

            // Execution won the race with the queue timeout. NSAppleScript's
            // own 15-second Apple Event timeout is now authoritative; retain a
            // short grace window so we do not report failure while the script
            // is already performing the requested side effect.
            tokio::time::timeout(Duration::from_secs(16), &mut response)
                .await
                .context(
                    "Timed out while browser automation was running on the AppKit main thread",
                )?
                .context("AppKit main-thread browser automation task was cancelled")?
        }
    }
}

unsafe extern "C" fn execute_script_main_cb(context: *mut c_void) {
    let request = unsafe { Box::from_raw(context as *mut MainThreadScript) };
    if !claim_queued_script(&request.state) {
        let _ = request.reply.send(Err(anyhow::anyhow!(
            "Queued browser automation was cancelled"
        )));
        return;
    }
    // A panic must never cross libdispatch's C callback boundary. Convert it
    // into the same Result channel used for ordinary script failures.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute_script_if_main_thread(&request.script)
    }))
    .unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "Browser automation panicked on the AppKit main thread"
        ))
    });
    request.state.store(SCRIPT_FINISHED, Ordering::Release);
    let _ = request.reply.send(result);
}

fn execute_script_if_main_thread(script: &str) -> anyhow::Result<String> {
    let main_thread = objc2_foundation::MainThreadMarker::new()
        .context("NSAppleScript must execute on the AppKit main thread")?;
    execute_script_on_main_thread(main_thread, script)
}

/// Run a script with Foundation's in-process OSA runtime. This preserves the
/// browser scripting suites and their Automation/TCC attribution without a
/// shell process, a temporary file, or a command-line script interpreter.
fn execute_script_on_main_thread(
    _main_thread: objc2_foundation::MainThreadMarker,
    script: &str,
) -> anyhow::Result<String> {
    // Bound the Apple Event wait in the script itself. This tells OSA to
    // cancel the pending event transaction if the browser stops responding.
    let source = format!("with timeout of 15 seconds\n{script}\nend timeout");
    let source = std::ffi::CString::new(source)
        .context("Browser script source contained an interior NUL byte")?;
    let mut result_ptr = std::ptr::null_mut();
    let mut error_ptr = std::ptr::null_mut();
    let succeeded =
        unsafe { cua_browser_execute_script(source.as_ptr(), &mut result_ptr, &mut error_ptr) };
    let result = unsafe { take_browser_shim_string(result_ptr) };
    let error = unsafe { take_browser_shim_string(error_ptr) };

    if succeeded {
        return result.context("Browser Apple Event returned no UTF-8 result");
    }
    let message = error.unwrap_or_else(|| "Unknown browser Apple Event failure".to_owned());
    if message.contains("turned off") || message.contains("AppleScript is turned off") {
        anyhow::bail!(
            "JavaScript from Apple Events is disabled. Use action=enable_javascript_apple_events \
             to enable it (requires browser restart)."
        );
    }
    anyhow::bail!("Browser Apple Event failed: {message}")
}

#[link(name = "cua_browser_exception_shim", kind = "static")]
extern "C" {
    fn cua_browser_execute_script(
        source_utf8: *const std::ffi::c_char,
        result_out: *mut *mut std::ffi::c_char,
        error_out: *mut *mut std::ffi::c_char,
    ) -> bool;
    #[cfg(test)]
    fn cua_browser_exception_boundary_probe(error_out: *mut *mut std::ffi::c_char) -> bool;
    fn cua_browser_free_string(value: *mut std::ffi::c_char);
}

unsafe fn take_browser_shim_string(value: *mut std::ffi::c_char) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let output = unsafe { std::ffi::CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    unsafe { cua_browser_free_string(value) };
    Some(output)
}

/// Cooperatively terminate every running instance for `bundle_id`, then wait
/// for NSWorkspace lifecycle notifications. No polling delay or shell process
/// is involved.
fn terminate_running_bundle(bundle_id: &str, timeout: Duration) -> anyhow::Result<()> {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::Allocated;
    use objc2::runtime::AnyObject;
    use objc2::ClassType;
    use objc2_app_kit::{
        NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
        NSWorkspaceDidTerminateApplicationNotification,
    };
    use objc2_foundation::{NSNotification, NSOperationQueue, NSString};
    use std::ptr::NonNull;

    let bundle_id = NSString::from_str(bundle_id);
    let applications =
        unsafe { NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle_id) };
    let mut pending = (0..applications.len())
        .filter_map(|index| applications.get(index))
        .filter(|app| !unsafe { app.isTerminated() })
        .map(|app| unsafe { app.processIdentifier() })
        .collect::<HashSet<_>>();
    if pending.is_empty() {
        return Ok(());
    }

    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let center = unsafe { workspace.notificationCenter() };
    let allocated: Allocated<NSOperationQueue> = NSOperationQueue::alloc();
    let queue = unsafe { NSOperationQueue::init(allocated) };
    unsafe { queue.setMaxConcurrentOperationCount(1) };
    let (tx, rx) = std::sync::mpsc::channel();
    let block = RcBlock::new(move |note_ptr: NonNull<NSNotification>| {
        let note = unsafe { note_ptr.as_ref() };
        let Some(info) = (unsafe { note.userInfo() }) else {
            return;
        };
        let app_ptr: *mut AnyObject =
            unsafe { msg_send![&*info, objectForKey: NSWorkspaceApplicationKey] };
        if app_ptr.is_null() {
            return;
        }
        let pid: libc::pid_t = unsafe { msg_send![app_ptr, processIdentifier] };
        let _ = tx.send(pid);
    });
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidTerminateApplicationNotification),
            None,
            Some(&queue),
            &block,
        )
    };

    for index in 0..applications.len() {
        let Some(app) = applications.get(index) else {
            continue;
        };
        let pid = unsafe { app.processIdentifier() };
        if pending.contains(&pid) && !unsafe { app.terminate() } {
            unsafe { center.removeObserver(&observer) };
            anyhow::bail!("Browser pid {pid} refused the cooperative terminate request");
        }
    }

    // Close the notification race where an app exits between `terminate()`
    // and the first receive.
    for index in 0..applications.len() {
        if let Some(app) = applications.get(index) {
            if unsafe { app.isTerminated() } {
                pending.remove(&unsafe { app.processIdentifier() });
            }
        }
    }

    let deadline = Instant::now() + timeout;
    while !pending.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            unsafe { center.removeObserver(&observer) };
            anyhow::bail!("Timed out waiting for browser pids {pending:?} to terminate");
        }
        match rx.recv_timeout(remaining) {
            Ok(pid) => {
                pending.remove(&pid);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                unsafe { center.removeObserver(&observer) };
                anyhow::bail!("Timed out waiting for browser pids {pending:?} to terminate");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                unsafe { center.removeObserver(&observer) };
                anyhow::bail!("Browser termination observer disconnected");
            }
        }
    }

    unsafe { center.removeObserver(&observer) };
    Ok(())
}

fn native_window_target(window_id: u32) -> anyhow::Result<NativeWindowTarget> {
    let windows = crate::windows::all_windows();
    let target = windows
        .iter()
        .find(|w| w.window_id == window_id)
        .ok_or_else(|| anyhow::anyhow!("No macOS window found for window_id {window_id}"))?;
    let mut same_bounds = windows
        .iter()
        .filter(|window| window.pid == target.pid && same_bounds(&window.bounds, &target.bounds))
        .collect::<Vec<_>>();
    same_bounds.sort_by(|a, b| b.z_index.cmp(&a.z_index));
    let same_bounds_ordinal = same_bounds
        .iter()
        .position(|window| window.window_id == target.window_id)
        .unwrap_or(0);

    Ok(NativeWindowTarget {
        title: target.title.clone(),
        bounds: target.bounds.clone(),
        same_bounds_ordinal,
    })
}

fn chromium_window_script(
    app_name: &str,
    target: &NativeWindowTarget,
    escaped_js: &str,
    window_id: u32,
) -> String {
    let left = rounded_i64(target.bounds.x);
    let top = rounded_i64(target.bounds.y);
    let right = rounded_i64(target.bounds.x + target.bounds.width);
    let bottom = rounded_i64(target.bounds.y + target.bounds.height);
    let ordinal = target.same_bounds_ordinal;
    let title_fallback = if target.title.trim().is_empty() {
        String::new()
    } else {
        let escaped_title = escape_for_applescript_string(&target.title);
        format!(
            r#"  if matchedWindow is missing value then
    repeat with w in windows
      if name of w contains "{escaped_title}" then
        set matchedWindow to w
        exit repeat
      end if
    end repeat
  end if
"#
        )
    };

    format!(
        r#"tell application "{app_name}"
  set matchedWindow to missing value
  set matchingBoundsSeen to 0
  repeat with w in windows
    set candidateBounds to bounds of w
    if (item 1 of candidateBounds is {left}) and (item 2 of candidateBounds is {top}) and (item 3 of candidateBounds is {right}) and (item 4 of candidateBounds is {bottom}) then
      if matchingBoundsSeen is {ordinal} then
        set matchedWindow to w
        exit repeat
      end if
      set matchingBoundsSeen to matchingBoundsSeen + 1
    end if
  end repeat
{title_fallback}  if matchedWindow is missing value then
    error "No {app_name} AppleScript window matched native window_id {window_id}"
  end if
  tell active tab of matchedWindow
    execute javascript {escaped_js}
  end tell
end tell"#
    )
}

fn same_bounds(left: &WindowBounds, right: &WindowBounds) -> bool {
    rounded_i64(left.x) == rounded_i64(right.x)
        && rounded_i64(left.y) == rounded_i64(right.y)
        && rounded_i64(left.width) == rounded_i64(right.width)
        && rounded_i64(left.height) == rounded_i64(right.height)
}

fn rounded_i64(value: f64) -> i64 {
    value.round() as i64
}

fn find_preferences_files(profiles_dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else {
        return vec![];
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let prefs = path.join("Preferences");
        if prefs.exists() {
            if let Some(s) = prefs.to_str() {
                result.push(s.to_owned());
            }
        }
    }
    result
}

fn patch_preferences_file(path: &str) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path).with_context(|| format!("Reading {path}"))?;
    let mut json: serde_json::Value =
        serde_json::from_str(&content).with_context(|| format!("Parsing {path}"))?;

    // Set browser.allow_javascript_apple_events = true.
    if let Some(obj) = json.as_object_mut() {
        let browser = obj
            .entry("browser")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(b) = browser.as_object_mut() {
            b.insert(
                "allow_javascript_apple_events".to_owned(),
                serde_json::Value::Bool(true),
            );
        }
        // Also set account_values.browser.allow_javascript_apple_events.
        let account_values = obj
            .entry("account_values")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(av) = account_values.as_object_mut() {
            let av_browser = av
                .entry("browser")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let Some(avb) = av_browser.as_object_mut() {
                avb.insert(
                    "allow_javascript_apple_events".to_owned(),
                    serde_json::Value::Bool(true),
                );
            }
        }
    }

    let new_content = serde_json::to_string(&json)?;
    std::fs::write(path, new_content).with_context(|| format!("Writing {path}"))?;
    Ok(())
}

/// Escape a plain string value for embedding inside an AppleScript double-quoted string.
fn escape_for_applescript_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Escape JS for embedding in an AppleScript string literal.
/// Multi-line JS is split by newline and concatenated with `& (ASCII character 10) &`.
fn escape_js_for_applescript(js: &str) -> String {
    let lines: Vec<String> = js
        .lines()
        .map(|l| {
            let escaped = l.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect();
    if lines.is_empty() {
        return "\"\"".to_owned();
    }
    if lines.len() == 1 {
        return lines[0].clone();
    }
    lines.join(" & (ASCII character 10) & ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(title: &str, same_bounds_ordinal: usize) -> NativeWindowTarget {
        NativeWindowTarget {
            title: title.to_owned(),
            bounds: WindowBounds {
                x: 22.0,
                y: 55.0,
                width: 1200.0,
                height: 829.0,
            },
            same_bounds_ordinal,
        }
    }

    #[test]
    fn chromium_script_matches_window_bounds_and_same_bounds_ordinal() {
        let script = chromium_window_script(
            "Google Chrome",
            &target("", 2),
            "\"window.location.href\"",
            27655,
        );

        assert!(script.contains("item 1 of candidateBounds is 22"));
        assert!(script.contains("item 2 of candidateBounds is 55"));
        assert!(script.contains("item 3 of candidateBounds is 1222"));
        assert!(script.contains("item 4 of candidateBounds is 884"));
        assert!(script.contains("if matchingBoundsSeen is 2"));
        assert!(
            script.contains("No Google Chrome AppleScript window matched native window_id 27655")
        );
    }

    #[test]
    fn chromium_script_does_not_fall_back_to_front_window_or_empty_title_match() {
        let script =
            chromium_window_script("Google Chrome", &target("", 0), "\"document.title\"", 27655);

        assert!(!script.contains("front window"));
        assert!(!script.contains("contains \"\""));
        assert!(script.contains(
            "error \"No Google Chrome AppleScript window matched native window_id 27655\""
        ));
    }

    #[test]
    fn chromium_script_uses_non_empty_title_as_secondary_fallback() {
        let script = chromium_window_script(
            "Google Chrome",
            &target("Example \"quoted\" Domain", 0),
            "\"document.title\"",
            27655,
        );

        assert!(script.contains("if name of w contains \"Example \\\"quoted\\\" Domain\""));
    }

    #[test]
    fn same_bounds_compares_rounded_window_geometry() {
        assert!(same_bounds(
            &WindowBounds {
                x: 22.2,
                y: 55.4,
                width: 1199.8,
                height: 829.1,
            },
            &WindowBounds {
                x: 22.0,
                y: 55.0,
                width: 1200.0,
                height: 829.0,
            }
        ));
    }

    #[test]
    fn native_script_entrypoint_rejects_worker_threads_before_touching_osa() {
        let error = std::thread::spawn(|| execute_script_if_main_thread("return 42"))
            .join()
            .expect("worker should not panic")
            .expect_err("NSAppleScript must reject a worker thread");
        assert!(error.to_string().contains("AppKit main thread"));
    }

    #[test]
    fn queue_timeout_cancels_before_callback_can_claim_script() {
        let state = AtomicU8::new(SCRIPT_QUEUED);
        assert!(cancel_queued_script(&state));
        assert!(!claim_queued_script(&state));

        let state = AtomicU8::new(SCRIPT_QUEUED);
        assert!(claim_queued_script(&state));
        assert!(!cancel_queued_script(&state));
    }

    #[test]
    fn dropping_waiter_cancels_script_that_is_still_queued() {
        let state = Arc::new(AtomicU8::new(SCRIPT_QUEUED));
        {
            let _guard = QueuedScriptCancellation {
                state: state.clone(),
            };
        }
        assert_eq!(state.load(Ordering::Acquire), SCRIPT_CANCELLED);
        assert!(!claim_queued_script(&state));
    }

    #[tokio::test]
    async fn worker_dispatch_fails_before_queueing_without_registered_main_loop() {
        if !crate::session::has_graphic_access() {
            return;
        }
        crate::pip::prepare_appkit_main_loop();
        let error = execute_script_on_main_queue("return 1".to_owned())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("registered AppKit main loop"));
    }

    #[test]
    fn objective_c_exception_is_mapped_before_the_ffi_callback_boundary() {
        let mut error_ptr = std::ptr::null_mut();
        assert!(!unsafe { cua_browser_exception_boundary_probe(&mut error_ptr) });
        let error = unsafe { take_browser_shim_string(error_ptr) }.unwrap();
        assert!(error.contains("beyond bounds"));
    }
}
