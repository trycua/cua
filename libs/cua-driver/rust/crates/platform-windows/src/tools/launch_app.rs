use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    window_target::z_index_from_front_to_back,
};
use serde_json::{json, Value};

use super::impl_::{
    is_chromium_browser_target, restore_foreground_polling_best_effort, split_launchable_target,
};

/// Index of the first URL not already consumed as the primary shell target.
/// An explicit app target consumes no URL; a URLs-only launch consumes index 0.
fn first_unopened_shell_url_index(has_app_target: bool) -> usize {
    usize::from(!has_app_target)
}

/// Anti-throttling flags injected on hidden Chromium launches (#1620).
///
/// `launch_app` uses `SW_SHOWNOACTIVATE` so the launched window is
/// non-foreground from birth. Chromium's `CalculateNativeWinOcclusion`
/// feature treats that as occluded and suspends the renderer process for
/// the tab's entire lifetime — UIA tree exposes only browser chrome, no
/// page DOM, and `PrintWindow` returns a blank body.
///
/// `CalculateNativeWinOcclusion` is the root cause; the two
/// `--disable-backgrounding-*` flags backstop the same effect through the
/// process-priority and renderer-throttling layers (Chromium suspends
/// renderers on multiple signals).
const CHROMIUM_ANTI_THROTTLING_FLAGS: &[&str] = &[
    "--disable-features=CalculateNativeWinOcclusion",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
];

/// Prepend the Chromium anti-throttling flags to `extra_args` unless the
/// caller already has each one. Merges into an existing `--disable-features=`
/// list rather than appending a second `--disable-features=` entry (Chromium
/// has subtle merging rules across duplicate flags).
fn inject_chromium_anti_throttling_flags(extra_args: &mut Vec<String>) {
    const OCCLUSION_FEATURE: &str = "CalculateNativeWinOcclusion";

    // Merge `CalculateNativeWinOcclusion` into any existing
    // `--disable-features=` entry, or insert a fresh one at the front.
    let has_occlusion = extra_args.iter().any(|a| {
        a.strip_prefix("--disable-features=")
            .map(|v| v.split(',').any(|f| f.trim() == OCCLUSION_FEATURE))
            .unwrap_or(false)
    });
    if !has_occlusion {
        if let Some(idx) = extra_args
            .iter()
            .position(|a| a.starts_with("--disable-features="))
        {
            extra_args[idx] = format!("{},{OCCLUSION_FEATURE}", extra_args[idx]);
        } else {
            extra_args.insert(0, format!("--disable-features={OCCLUSION_FEATURE}"));
        }
    }

    // Boolean toggles — insert at front only when not already present.
    for flag in &[
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
    ] {
        if !extra_args.iter().any(|a| a == flag) {
            extra_args.insert(0, (*flag).to_string());
        }
    }
    let _ = CHROMIUM_ANTI_THROTTLING_FLAGS; // referenced for docs alignment
}

// ── launch_app ───────────────────────────────────────────────────────────────

/// Shape of the resolved launch-target params, used to decide whether the
/// best-effort foreground-restore guard should fire.
///
/// The decision is intentionally a pure function of the params (not of any
/// HWND state) so it can be unit-tested. See
/// [`should_restore_foreground_after_launch`].
#[derive(Clone, Copy, Debug)]
struct LaunchTargetShape {
    has_aumid: bool,
    has_bundle_id: bool,
    has_name: bool,
    has_path: bool,
    has_launch_path: bool,
    has_urls: bool,
}

/// True iff the launch is "open an app in the background" (so we want to
/// restore the user's foreground after the spawn activates the new window)
/// rather than "open a URL in the default browser" (where the user
/// explicitly asked for that page to come up).
///
/// Rule, matching the audit spec:
/// - URLs-only call (no app-identifying field set) → SKIP restore. The
///   user asked for `https://…` to be openable in their default browser;
///   a browser instance freshly launched for that URL is the legitimate
///   foreground.
/// - Any app-identifying field present (`aumid` / `bundle_id` / `name` /
///   `path` / `launch_path`) → RESTORE. The intent is hidden-launch +
///   background drive; the no-foreground contract applies.
fn should_restore_foreground_after_launch(shape: LaunchTargetShape) -> bool {
    let has_app_identifier = shape.has_aumid
        || shape.has_bundle_id
        || shape.has_name
        || shape.has_path
        || shape.has_launch_path;
    // URLs-only carve-out: when the caller passed urls AND no
    // app-identifying field, skip the restore.
    if shape.has_urls && !has_app_identifier {
        return false;
    }
    has_app_identifier
}

/// Bound on one standard-user shell launch helper. It stays inside the 15 s
/// launch deadline so a helper stuck on a shell dialog is terminated rather
/// than left behind.
const STANDARD_USER_SHELL_LAUNCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

pub struct LaunchAppTool;
static LAUNCH_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        LAUNCH_DEF.get_or_init(|| ToolDef {
            name: "launch_app".into(),
            // Description ported from Swift `LaunchAppTool.swift` with
            // Windows-specific notes. `bundle_id` is accepted as an alias
            // for `name` on Windows, with one extra behavior: if the
            // string looks like an AUMID (App User Model ID — contains
            // `!`, e.g. `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`) we
            // route through `IApplicationActivationManager` so packaged
            // (Microsoft Store / UWP / MSIX) apps return the real
            // process pid instead of a stub-redirect pid.
            description: "Launch a Windows app hidden — the driver never brings the target to \
                the foreground, the target's window is launched with SW_SHOWNOACTIVATE so it \
                does not steal focus from whatever is currently frontmost.\n\n\
                Provide either `bundle_id` / `name` / `aumid` (resolved as below) or `path` \
                (full path to an executable). If both `name` and `path` are given, `path` wins. \
                `urls` opens each URL in the default browser without activating it.\n\n\
                Routing order: explicit `aumid` (or a `bundle_id` containing `!`) activates the \
                packaged app via `IApplicationActivationManager::ActivateApplication`, returning \
                the real packaged-process pid — required on Win11 for built-in apps like \
                Notepad, Calculator, and Paint, which now ship as Microsoft Store packages \
                where the legacy .exe in System32 is a ~7 KB stub that exits immediately. A \
                plain `name` first attempts a `shell:AppsFolder` lookup; packaged matches use \
                the activation manager while desktop registrations are launched through their \
                shell parsing path. A miss falls back to ShellExecuteEx's PATH search.\n\n\
                Returns the launched app's pid, name, active flag, AND a `windows` array — \
                same per-window shape `list_windows` returns. When the launch settles but no \
                window has materialized yet (transient; rare), `windows` comes back empty — \
                call `list_windows(pid)` explicitly a moment later. `bundle_id` in the \
                response is set to the AUMID actually used for packaged-app launches and \
                `null` for ShellExecuteEx launches.\n\n\
                Windows-only field: `path` (Swift uses `bundle_id` since macOS apps resolve via \
                LaunchServices; Windows has no LaunchServices, so `path` is the canonical form). \
                The macOS-specific `webkit_inspector_port`, \
                `creates_new_application_instance`, and `additional_arguments` fields are \
                accepted; `additional_arguments` is honored (forwarded as ShellExecuteEx \
                parameters or as the AUMID activation arguments string). \
                The remaining macOS-only fields currently no-op on Windows.".into(),
            input_schema: json!({"type":"object","properties":{
                "bundle_id":{"type":"string","description":"App User Model ID (AUMID) for a packaged app — pattern `{PackageFamilyName}!{ApplicationId}`, e.g. `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`. Falls back to a `name` alias if no `!` is present. Either bundle_id, name, aumid, path, or launch_path must be provided."},
                "aumid":{"type":"string","description":"Explicit AUMID for a packaged app. Cleaner alternative to overloading `bundle_id`. Takes precedence over `bundle_id` / `name` when present."},
                "name":{"type":"string","description":"App display name. Tried against the `shell:AppsFolder` index first for packaged-app lookup; on a miss, passed to ShellExecuteEx's PATH search."},
                "path":{"type":"string","description":"Full path to executable. Windows-only; takes precedence over name/bundle_id/aumid (but not over `launch_path`)."},
                "launch_path":{"type":"string","description":"Round-trip the `launch_path` returned by `list_apps`. Highest precedence — when set, this exact string is handed to ShellExecuteEx unchanged. For Windows desktop apps it's the full `.exe` commandline (path + arguments preserved from the source shortcut); for UWP apps it's `shell:appsFolder\\{PackageFamilyName}!{AppId}`. For precise UWP pid capture, prefer `aumid` over `launch_path`."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open in the default browser via ShellExecuteEx (no activation)."},
                "additional_arguments":{"type":"array","items":{"type":"string"},"description":"Extra command-line arguments passed to the launched process (or activation arguments for packaged apps)."},
                "webkit_inspector_port":{"type":"integer","description":"Accepted for cross-platform parity; no-op on Windows."},
                "creates_new_application_instance":{"type":"boolean","description":"Accepted for parity; no-op on Windows (ShellExecuteEx always creates a new process)."},
                "start_minimized":{"type":"boolean","description":"When true, launch the app's window minimized to the taskbar instead of restored-but-not-activated. Use this when the agent wants to drive the app entirely in the background — the user's previously-frontmost window (e.g. terminal) stays visually on top. Desktop launches hold the foreground lock through startup and use SW_SHOWMINNOACTIVE; packaged-app activation remains broker-controlled and receives a best-effort SW_SHOWMINNOACTIVE post-pass. UIA / background dispatch still work on a minimized window; only `screenshot` and `delivery_mode:\"foreground\"` need it restored."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let launch_path_opt = args
            .get("launch_path")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let path_opt = args.get("path").and_then(|v| v.as_str()).map(str::to_owned);
        let name_opt = args.get("name").and_then(|v| v.as_str()).map(str::to_owned);
        let bundle_id_opt = args
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let aumid_opt = args
            .get("aumid")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let urls: Vec<String> = args
            .get("urls")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let start_minimized = args
            .get("start_minimized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // `nShow` for the ShellExecuteEx-based launches. SW_SHOWNOACTIVATE
        // keeps the new window from grabbing focus but still renders it on
        // top of the previous frontmost window. SW_SHOWMINNOACTIVE both
        // skips activation AND drops the window to the taskbar, which is
        // what an agent doing background work wants — the user's terminal
        // stays visually on top. UIA / PostMessage / set_value all keep
        // working against a minimized window; only screenshot and
        // delivery_mode:"foreground" pay a cost.
        let n_show: i32 = if start_minimized {
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE.0
        } else {
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE.0
        };
        // Snapshot the set of currently-running pids BEFORE we launch. The
        // detached start_minimized polling task uses this to detect "any
        // process that came into existence as a result of our launch", so
        // it can minimize their windows too — covers the launcher-stub
        // case (LibreOffice swriter.exe → soffice.bin, GIMP gimp-3.exe →
        // gimp-3.2.exe, etc.) where the launched pid exits and a child
        // process with a different name owns the actual window. Captured
        // here (before the launch) so the new soffice.bin pid won't be
        // in the snapshot.
        let pre_launch_pids: std::sync::Arc<std::collections::HashSet<u32>> = if start_minimized {
            let pids: std::collections::HashSet<u32> = crate::win32::list_processes()
                .into_iter()
                .map(|p| p.pid)
                .collect();
            std::sync::Arc::new(pids)
        } else {
            std::sync::Arc::new(std::collections::HashSet::new())
        };

        // Capture the foreground window BEFORE any launch path runs so the
        // post-spawn polling restore (below) has a target HWND to flip back
        // to. Mirrors the macOS `FocusRestoreGuard` behavior. Best-effort —
        // see `restore_foreground_polling_best_effort` for the actual
        // restoration semantics.
        //
        // The UWP/AUMID path already restores foreground synchronously
        // (see `launch_uwp::restore_foreground_best_effort`), so this only
        // gates the legacy ShellExecuteExW path. We capture early so the
        // value is consistent regardless of which sub-path runs.
        //
        // Stored as `usize` (not `HWND`) so this `invoke` future stays
        // `Send` — `HWND` wraps `*mut c_void` and would taint the future
        // for the multi-threaded tokio runtime if held across `.await`.
        let foreground_before_addr: usize =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize };

        // Pure-function decision on whether to fire the post-launch
        // restore. Shape mirrors the resolved params — see
        // `should_restore_foreground_after_launch` for the rule.
        let launch_shape = LaunchTargetShape {
            has_aumid: aumid_opt.is_some(),
            has_bundle_id: bundle_id_opt.is_some(),
            has_name: name_opt.is_some(),
            has_path: path_opt.is_some(),
            has_launch_path: launch_path_opt.is_some(),
            has_urls: !urls.is_empty(),
        };
        let restore_foreground = should_restore_foreground_after_launch(launch_shape);
        let mut extra_args: Vec<String> = args
            .get("additional_arguments")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if args.get("cdp_debugging_port").is_some() {
            return ToolResult::error(
                "cdp_debugging_port moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }
        if launch_path_opt
            .as_deref()
            .into_iter()
            .chain(path_opt.as_deref())
            .chain(name_opt.as_deref())
            .chain(extra_args.iter().map(String::as_str))
            .any(cua_driver_core::launch_guard::contains_remote_debugging_flag)
        {
            return ToolResult::error(
                cua_driver_core::launch_guard::REMOTE_DEBUGGING_LAUNCH_REFUSAL,
            );
        }

        // Resolve target — launch_path > path > aumid > name > bundle_id
        // (alias of name on Windows). launch_path is highest precedence for
        // round-trip with list_apps; the value is handed to ShellExecuteEx
        // unchanged (UWP routing below is bypassed when launch_path is set).
        let target = launch_path_opt
            .clone()
            .or(path_opt.clone())
            .or(aumid_opt.clone())
            .or(name_opt.clone())
            .or(bundle_id_opt.clone());

        // #1620: auto-inject Chromium anti-throttling flags when the resolved
        // target names a Chromium-based browser (Edge / Chrome / Brave /
        // Vivaldi / Opera / Chromium / Arc / Thorium / Iridium / Yandex).
        // launch_app hides the window via SW_SHOWNOACTIVATE; Chromium's
        // occlusion logic suspends the renderer in that case, leaving the
        // UIA tree empty and PrintWindow capturing a blank body. The three
        // flags below defeat that for the lifetime of the launched process
        // without affecting normal interactive use.
        //
        // Skipped when `aumid` or `bundle_id` route through the UWP path
        // (packaged Edge is the exception; the desktop Edge channel is what
        // ships now and that goes through the ShellExecuteEx path below).
        if launch_path_opt.is_some() || path_opt.is_some() || name_opt.is_some() {
            if let Some(t) = target.as_deref() {
                if is_chromium_browser_target(t) {
                    inject_chromium_anti_throttling_flags(&mut extra_args);
                }
            }
        }

        if target.is_none() && urls.is_empty() {
            // Error message lists every field the resolver actually accepts.
            // The Swift-original "bundle_id or name" message predated the Windows
            // additions (aumid, path, launch_path, urls) and made #1635 look like
            // an aumid-specific bug. The actual cause of #1635 was the upstream
            // PS argv quote-stripping bug fixed in #1637; this message just stops
            // misleading anyone who hits the error for unrelated reasons.
            return ToolResult::error(
                "Provide one of: bundle_id, name, aumid, path, launch_path, or urls to identify the app to launch.",
            );
        }

        // ── Packaged-app (UWP / MSIX) routing decision ──────────────────────
        //
        // Routing precedence — most explicit signal wins:
        //   1. `launch_path` set: skip UWP routing entirely — round-trip
        //      semantics demand the value be handed to ShellExecuteEx
        //      unchanged. Callers wanting precise UWP pid capture should
        //      pass `aumid` instead.
        //   2. `aumid` parameter (any value): packaged path, AUMID = value.
        //   3. `bundle_id` containing `!`: packaged path, AUMID = value
        //      (Win32 PATH lookups never produce a `!` in the executable
        //      name, so this is a safe marker of caller intent).
        //   4. `name` with no `path`: try `shell:AppsFolder` lookup. A
        //      packaged hit uses the resolved AUMID; a desktop registration
        //      uses `shell:AppsFolder\<AppUserModelID>` via ShellExecuteExW.
        //      On miss, fall through to the existing Win32 path.
        //   5. Anything else: existing ShellExecuteExW path.
        //
        // `path` is intentionally excluded from packaged routing — when
        // the caller has a concrete executable in mind they want
        // CreateProcess semantics, not Start Menu activation.
        let mut name_lookup_missed = false;
        let mut shell_target_from_name: Option<String> = None;
        let aumid_for_uwp: Option<String> = if launch_path_opt.is_some() || path_opt.is_some() {
            None
        } else if let Some(a) = aumid_opt.clone() {
            Some(a)
        } else if let Some(b) = bundle_id_opt
            .clone()
            .filter(|s| crate::launch_uwp::is_aumid(s))
        {
            Some(b)
        } else if let Some(n) = name_opt.clone() {
            // The cold AppsFolder COM walk can stop making progress when its
            // interactive shell broker is unhealthy. Bound it before the
            // separate ShellExecuteEx deadline below; the resolver keeps a
            // timed-out worker single-flight until COM really returns.
            match crate::launch_uwp::resolve_apps_folder_target_by_name_bounded(n.clone()).await {
                Ok(Some(crate::launch_uwp::AppsFolderLaunchTarget::PackagedAumid(aumid))) => {
                    Some(aumid)
                }
                Ok(Some(crate::launch_uwp::AppsFolderLaunchTarget::ShellLaunchPath(
                    launch_path,
                ))) => {
                    shell_target_from_name = Some(launch_path);
                    None
                }
                Ok(None) => {
                    name_lookup_missed = true;
                    None
                }
                Err(crate::launch_uwp::AppsFolderLookupError::Timeout) => {
                    return ToolResult::error(format!(
                        "App name lookup for {n:?} is temporarily unavailable: Windows did not respond to the shell:AppsFolder query within 4s. No app was launched; retry after the shell recovers, or pass an explicit path or aumid."
                    ))
                }
                Err(crate::launch_uwp::AppsFolderLookupError::Busy) => {
                    return ToolResult::error(format!(
                        "App name lookup for {n:?} is temporarily unavailable because a prior Windows shell lookup is still running or cooling down. No app was launched; retry later, or pass an explicit path or aumid."
                    ))
                }
                Err(crate::launch_uwp::AppsFolderLookupError::Unavailable) => {
                    return ToolResult::error(format!(
                        "App name lookup for {n:?} is unavailable because the Windows shell lookup worker failed. No app was launched; pass an explicit path or aumid."
                    ))
                }
            }
        } else {
            None
        };
        let resolved_target = shell_target_from_name.or(target);

        // Single launch path. `display` is the human-readable identifier
        // we report in the success header; for the packaged path it's
        // the AUMID, which is more informative than the display name.
        let display = aumid_for_uwp
            .clone()
            .or_else(|| resolved_target.clone())
            .unwrap_or_else(|| urls.join(", "));

        // When the resolved target came from `launch_path` it may carry
        // shortcut arguments (e.g. `"C:\Path\chrome.exe" --profile-directory="Profile 2"`).
        // Split it into a file token + arguments tail so ShellExecuteEx
        // sees them separately — passing the whole string as lpFile would
        // fail because the shell only resolves a single filesystem path
        // there. No-op for AUMID tokens / plain paths without args / UWP
        // `shell:appsFolder\..!..` tokens.
        let (target_file_opt, extra_joined) = {
            let base_extra = extra_args.join(" ");
            if launch_path_opt.is_some() {
                if let Some(t) = resolved_target.as_deref() {
                    let (file, args_tail) = split_launchable_target(t);
                    let combined = if args_tail.is_empty() {
                        base_extra
                    } else if base_extra.is_empty() {
                        args_tail
                    } else {
                        format!("{args_tail} {base_extra}")
                    };
                    let file_opt = if file.is_empty() { None } else { Some(file) };
                    (file_opt, combined)
                } else {
                    (None, base_extra)
                }
            } else {
                (resolved_target.clone(), base_extra)
            }
        };

        // Strict no-activation is available only for the desktop launch path.
        // UWP activation is broker-controlled and retains its existing
        // restore-after-activation behavior.
        let mut foreground_lock = if start_minimized && aumid_for_uwp.is_none() {
            Some(crate::input::ForegroundLockGuard::acquire())
        } else {
            None
        };
        if foreground_lock
            .as_ref()
            .is_some_and(|guard| !guard.acquired())
        {
            return ToolResult::error(
                "Background minimized launch is unavailable because Windows did not grant the \
                 foreground lock required to prevent the new process from activating. No process \
                 was started. Launch without start_minimized only when a foreground change is \
                 acceptable.",
            )
            .with_structured(json!({
                "code": "background_unavailable",
                "delivery_mode": "background",
                "event_kind": "app_launch",
            }));
        }

        // Branch: AUMID activation if we resolved one; else legacy
        // ShellExecuteExW. Both branches still need to handle `urls`
        // (additional URLs always go through ShellExecuteExW since the
        // Start Menu doesn't activate by URL).
        let pid = if let Some(aumid) = aumid_for_uwp.clone() {
            let aumid_clone = aumid.clone();
            let args_clone = extra_joined.clone();
            let activation = tokio::task::spawn_blocking(move || {
                crate::launch_uwp::launch_uwp(&aumid_clone, &args_clone)
            })
            .await;
            match activation {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    return ToolResult::error(format!(
                        "Failed to activate packaged app {aumid:?}: {e}"
                    ))
                }
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Legacy ShellExecuteExW path for plain Win32 apps and for
            // callers passing an explicit `path` / `launch_path`. An elevated
            // Driver runs these launches with its standard-user token so the
            // app does not inherit administrator rights (#3607).
            let launch_token = match crate::standard_user_launch::app_launch_token() {
                Ok(token) => token,
                Err(message) => return ToolResult::error(message),
            };
            let urls_clone = urls.clone();
            let target_for_shell = target_file_opt.clone();
            let extra_for_shell = extra_joined.clone();
            let n_show_for_shell = n_show;
            let direct_minimized_exe = start_minimized
                && target_for_shell.as_deref().is_some_and(|target| {
                    std::path::Path::new(target).is_file()
                        && std::path::Path::new(target)
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                });
            // Bound the shell launch with a timeout. An unregistered protocol
            // or file association makes `ShellExecuteExW` block on a modal shell
            // dialog ("you'll need a new app to open this …") on the *session*
            // desktop — which a Session-0/headless daemon can neither see nor
            // dismiss — wedging the launch (and that request) indefinitely.
            // `SEE_MASK_FLAG_NO_UI` (below) suppresses the no-association error
            // UI so that case fails fast with an error code; the timeout is the
            // backstop for any *other* blocking broker dialog (SmartScreen, an
            // elevation/consent surface) so a bad target can't hang the daemon.
            let launch = tokio::task::spawn_blocking(move || -> anyhow::Result<u32> {
                use windows::core::{PCWSTR, PWSTR};
                use windows::Win32::Foundation::CloseHandle;
                use windows::Win32::System::Threading::{
                    CreateProcessW, GetProcessId, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                    STARTF_USESHOWWINDOW, STARTUPINFOW,
                };
                use windows::Win32::UI::Shell::{
                    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS,
                    SHELLEXECUTEINFOW,
                };

                fn to_wide(s: &str) -> Vec<u16> {
                    s.encode_utf16().chain(std::iter::once(0)).collect()
                }
                let op_w = to_wide("open");
                let file_w = to_wide(if let Some(t) = target_for_shell.as_deref() {
                    t
                } else {
                    urls_clone.first().map(|s| s.as_str()).unwrap_or("")
                });
                let args_w = to_wide(&extra_for_shell);

                use crate::standard_user_launch::AppLaunchToken;
                let pid = if direct_minimized_exe {
                    let target = target_for_shell
                        .as_deref()
                        .expect("checked executable path");
                    let command_line_text = if extra_for_shell.is_empty() {
                        format!(r#""{target}""#)
                    } else {
                        format!(r#""{target}" {extra_for_shell}"#)
                    };
                    if let AppLaunchToken::StandardUser(token) = &launch_token {
                        crate::standard_user_launch::create_process(
                            token,
                            target,
                            &command_line_text,
                            n_show_for_shell,
                        )
                        .map_err(anyhow::Error::msg)?
                    } else {
                        let mut command_line = to_wide(&command_line_text);
                        let startup = STARTUPINFOW {
                            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                            dwFlags: STARTF_USESHOWWINDOW,
                            wShowWindow: n_show_for_shell as u16,
                            ..Default::default()
                        };
                        let mut process = PROCESS_INFORMATION::default();
                        unsafe {
                            CreateProcessW(
                                PCWSTR(file_w.as_ptr()),
                                PWSTR(command_line.as_mut_ptr()),
                                None,
                                None,
                                false,
                                PROCESS_CREATION_FLAGS(0),
                                None,
                                PCWSTR::null(),
                                &startup,
                                &mut process,
                            )?;
                            let _ = CloseHandle(process.hThread);
                            let _ = CloseHandle(process.hProcess);
                        }
                        process.dwProcessId
                    }
                } else if let AppLaunchToken::StandardUser(token) = &launch_token {
                    let file = target_for_shell
                        .as_deref()
                        .or_else(|| urls_clone.first().map(String::as_str))
                        .unwrap_or("");
                    crate::standard_user_launch::shell_execute(
                        token,
                        file,
                        &extra_for_shell,
                        n_show_for_shell,
                        STANDARD_USER_SHELL_LAUNCH_TIMEOUT,
                    )
                    .map_err(anyhow::Error::msg)?
                } else {
                    let mut info = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
                        lpVerb: PCWSTR(op_w.as_ptr()),
                        lpFile: PCWSTR(file_w.as_ptr()),
                        lpParameters: if extra_for_shell.is_empty() {
                            PCWSTR::null()
                        } else {
                            PCWSTR(args_w.as_ptr())
                        },
                        nShow: n_show_for_shell,
                        ..Default::default()
                    };
                    unsafe {
                        ShellExecuteExW(&mut info)?;
                    }
                    if !info.hProcess.is_invalid() {
                        let p = unsafe { GetProcessId(info.hProcess) };
                        unsafe {
                            let _ = CloseHandle(info.hProcess);
                        }
                        p
                    } else {
                        0
                    }
                };

                // Open any additional URLs in the default browser (no focus
                // steal, no pid capture for these — Swift's NSWorkspace flow
                // behaves the same way for secondary URLs).
                // When an app target was launched above, none of the URLs has
                // been consumed yet. URLs-only launches use the first URL as
                // `lpFile`, so only their remaining URLs belong here.
                let first_unopened_url = first_unopened_shell_url_index(target_for_shell.is_some());
                for url in &urls_clone[first_unopened_url.min(urls_clone.len())..] {
                    if let AppLaunchToken::StandardUser(token) = &launch_token {
                        let _ = crate::standard_user_launch::shell_execute(
                            token,
                            url,
                            "",
                            n_show_for_shell,
                            STANDARD_USER_SHELL_LAUNCH_TIMEOUT,
                        );
                        continue;
                    }
                    let file = to_wide(url);
                    let mut url_info = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        fMask: SEE_MASK_FLAG_NO_UI,
                        lpVerb: PCWSTR(op_w.as_ptr()),
                        lpFile: PCWSTR(file.as_ptr()),
                        nShow: n_show_for_shell,
                        ..Default::default()
                    };
                    unsafe {
                        let _ = ShellExecuteExW(&mut url_info);
                    }
                }

                Ok(pid)
            });
            // 15s is generous: ShellExecuteExW returns once activation starts
            // (process created), not when the window appears, so a healthy
            // launch resolves in well under a second. On timeout we abandon the
            // blocking task (a spawn_blocking thread can't be cancelled — it
            // unblocks if/when the modal is dismissed) and return an error so
            // the daemon stays responsive instead of wedging on the request.
            let result = tokio::time::timeout(std::time::Duration::from_secs(15), launch).await;

            match result {
                Ok(Ok(Ok(p))) => p,
                Ok(Ok(Err(e))) if name_lookup_missed => {
                    return ToolResult::error(format!(
                        "App {:?} was not found in shell:AppsFolder or by Windows PATH/association lookup: {e}",
                        name_opt.as_deref().unwrap_or("")
                    ))
                }
                Ok(Ok(Err(e))) => return ToolResult::error(format!("Failed to launch: {e}")),
                Ok(Err(e)) => return ToolResult::error(format!("Task error: {e}")),
                Err(_elapsed) => {
                    return ToolResult::error(format!(
                        "Launch of {:?} timed out after 15s — the target likely has no \
                     registered handler and a blocking shell dialog appeared on the \
                     session desktop; aborted to keep the daemon responsive.",
                        target_file_opt
                            .as_deref()
                            .or_else(|| urls.first().map(|s| s.as_str()))
                            .unwrap_or("")
                    ))
                }
            }
        };

        // Best-effort foreground-restore for the legacy ShellExecuteExW
        // path. The UWP/AUMID branch already restores foreground
        // synchronously inside `launch_uwp::launch_uwp`, so we only spawn
        // the polling task when we went through the ShellExecuteExW
        // branch. URLs-only invocations are excluded via `restore_foreground`
        // (see `should_restore_foreground_after_launch`).
        //
        // `HWND` wraps a raw `*mut c_void` which is `!Send`, so we marshal
        // the foreground handle across the spawn boundary as a `usize` and
        // reconstruct the `HWND` only inside the synchronous probes (see
        // `restore_foreground_polling_best_effort`). Mirrors how
        // `launch_uwp.rs` carries its restore target.
        if aumid_for_uwp.is_none() && restore_foreground {
            let spawned_pid_for_restore = pid;
            let target_foreground_addr = foreground_before_addr;
            tokio::spawn(async move {
                restore_foreground_polling_best_effort(
                    target_foreground_addr,
                    spawned_pid_for_restore,
                )
                .await;
            });
        }

        // When the packaged path was taken we still need to fan out any
        // extra URLs to the default browser (ShellExecuteEx — no pid
        // capture, mirroring Swift's NSWorkspace secondary-URL flow).
        if aumid_for_uwp.is_some() && !urls.is_empty() {
            let urls_clone = urls.clone();
            let n_show_for_urls = n_show;
            let _ = tokio::task::spawn_blocking(move || {
                use windows::core::PCWSTR;
                use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
                fn to_wide(s: &str) -> Vec<u16> {
                    s.encode_utf16().chain(std::iter::once(0)).collect()
                }
                let op_w = to_wide("open");
                for url in &urls_clone {
                    let file = to_wide(url);
                    let mut url_info = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        lpVerb: PCWSTR(op_w.as_ptr()),
                        lpFile: PCWSTR(file.as_ptr()),
                        nShow: n_show_for_urls,
                        ..Default::default()
                    };
                    unsafe {
                        let _ = ShellExecuteExW(&mut url_info);
                    }
                }
            })
            .await;
        }

        // Resolve the pid's windows so the caller can skip a list_windows
        // round-trip — same approach as Swift's `resolveWindows`. Retry
        // 5×200ms; Win32 window registration can lag the launch.
        //
        // Launcher-stub fallback: when the launched binary is a wrapper that
        // re-execs and exits (GIMP's `gimp-3.exe` → `gimp-3.2.exe`; LO's
        // `swriter.exe` → `soffice.bin`), the launched pid never gets a
        // window — list_windows(Some(pid)) stays empty forever. After the
        // primary retry budget we fall back to scanning the launched pid's
        // descendant + name-related processes and pick the first one with a
        // window. The resolved pid is reflected in the response's `pid` field
        // so callers can target it with subsequent calls. See #1615.
        let mut windows_json: Vec<serde_json::Value> = Vec::new();
        let mut resolved_pid: u32 = pid;

        // UWP / packaged-app host-window resolution. `launch_uwp` returns the
        // real packaged-app pid (e.g. CalculatorApp.exe), but a UWP app's
        // top-level window — the HWND a caller must actually drive — is owned
        // by `ApplicationFrameHost.exe`, not by that process. The app process
        // only owns a `CoreWindow` reparented as a child of the AFH frame, so
        // `list_windows(Some(app_pid))` is empty and the generic retry loop
        // below would either spin out to a stub-fallback miss or return no
        // windows at all (the original "launch_app of a UWP app returns empty
        // windows[] / a pid you can't drive" bug). Map app_pid → its AFH host
        // frame and report `(frame_hwnd, afh_pid)` so the returned handles
        // resolve in `get_window_state` and element actions land. Retry: the
        // frame can lag the activation by a few hundred ms.
        if aumid_for_uwp.is_some() {
            for _ in 0..10 {
                let host =
                    tokio::task::spawn_blocking(move || crate::win32::resolve_uwp_host_window(pid))
                        .await
                        .unwrap_or(None);
                if let Some(w) = host {
                    windows_json = vec![json!({
                        "window_id": w.hwnd, "title": w.title,
                        "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                        "layer": 0,
                        "z_index": 0,
                        "is_on_screen": true,
                    })];
                    // Report the AFH pid: that's the pid that owns the frame
                    // HWND, so `get_window_state(resolved_pid, window_id)`
                    // validates and `list_windows(Some(resolved_pid))` lists it.
                    resolved_pid = w.pid;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }

        for _ in 0..5 {
            if !windows_json.is_empty() {
                break;
            }
            let wins =
                tokio::task::spawn_blocking(move || crate::win32::list_windows_win32_first(pid))
                    .await
                    .unwrap_or_default();
            if !wins.is_empty() {
                let window_count = wins.len();
                windows_json = wins.iter().enumerate().map(|(position, w)| json!({
                    "window_id": w.hwnd, "title": w.title,
                    "bounds":    { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                    "layer":     0,
                    "z_index":   z_index_from_front_to_back(window_count, position),
                    "is_on_screen": w.is_on_screen,
                })).collect();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        if windows_json.is_empty() {
            // Launcher-stub fallback. Compute the exe basename from the
            // launchable target so name-based matching has something to work
            // with (e.g. "gimp-3.exe" → prefix "gimp" matches "gimp-3.2.exe").
            let basename_for_match = target_file_opt
                .as_deref()
                .and_then(|t| t.rsplit(|c: char| c == '\\' || c == '/').next())
                .unwrap_or("")
                .to_owned();
            // Known-slow launchers get an extended retry budget. GIMP 3.x in
            // particular spends 10-20s on its first launch (font cache rebuild,
            // plugin scan, etc.) before the main window registers. We don't
            // want to wait 20s for every launcher — gate on basename prefix
            // matching known-slow apps. Add to this list as encountered.
            let bn_lower = basename_for_match.to_ascii_lowercase();
            let is_slow_launcher = bn_lower.starts_with("gimp")
                || bn_lower.starts_with("blender")          // OpenGL init can stall
                || bn_lower.starts_with("inkscape")         // similar GTK pattern
                || bn_lower.starts_with("krita")
                || bn_lower.starts_with("freecad");
            let max_candidate_attempts: usize = if is_slow_launcher { 30 } else { 3 };

            let basename_clone = basename_for_match.clone();
            let candidates_initial = tokio::task::spawn_blocking(move || {
                crate::win32::related_processes(pid, &basename_clone)
            })
            .await
            .unwrap_or_default();

            // For slow launchers we may also need to RE-SCAN candidates over
            // time, because the wrapper may not have spawned its child yet
            // when we first scanned. Cap total wait at ~12s (slow) / 0.6s (fast).
            let mut tried: std::collections::HashSet<u32> = std::collections::HashSet::new();
            tried.insert(pid); // already tried in the primary loop
            let mut candidate_queue: Vec<u32> = candidates_initial
                .into_iter()
                .filter(|p| tried.insert(*p))
                .collect();
            let mut total_attempts: usize = 0;
            'outer: loop {
                while let Some(candidate_pid) = candidate_queue.pop() {
                    for _ in 0..max_candidate_attempts {
                        total_attempts += 1;
                        let wins = tokio::task::spawn_blocking(move || {
                            crate::win32::list_windows_win32_first(candidate_pid)
                        })
                        .await
                        .unwrap_or_default();
                        if !wins.is_empty() {
                            let window_count = wins.len();
                            windows_json = wins.iter().enumerate().map(|(position, w)| json!({
                                "window_id": w.hwnd, "title": w.title,
                                "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                                "layer": 0,
                                "z_index": z_index_from_front_to_back(window_count, position),
                                "is_on_screen": w.is_on_screen,
                            })).collect();
                            resolved_pid = candidate_pid;
                            break 'outer;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
                // For slow launchers, keep re-scanning descendants — the
                // wrapper may not have spawned its child yet. Cap total
                // wait at ~12s (60 × 200ms) for the slow path.
                if !is_slow_launcher || total_attempts > 60 {
                    break;
                }
                // Give the wrapper a moment to spawn before re-scanning.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                total_attempts += 3; // count the 500ms wait as 3 attempts
                let basename_rescan = basename_for_match.clone();
                let fresh = tokio::task::spawn_blocking(move || {
                    crate::win32::related_processes(pid, &basename_rescan)
                })
                .await
                .unwrap_or_default();
                let new_ones: Vec<u32> = fresh.into_iter().filter(|p| tried.insert(*p)).collect();
                candidate_queue = new_ones;
                // Continue the outer loop regardless — even with empty
                // new_ones we want another iteration that hits the
                // total_attempts cap. The loop body handles the empty queue
                // by falling through to the re-scan again.
            }
        }
        let pid = resolved_pid;

        // start_minimized post-pass: drop every resolved window to the
        // taskbar. The desktop ShellExecuteEx path already minimizes via
        // nShow=SW_SHOWMINNOACTIVE, but three cases still need this
        // fallback:
        //   1. AUMID / UWP activations come up un-minimized —
        //      ActivateApplication has no nShow.
        //   2. Apps that ignore the nShow hint and decide their own geometry
        //      (notable offenders: LibreOffice, some Electron apps).
        //   3. Launcher-stub flows (LO swriter.exe → soffice.bin, GIMP, etc.)
        //      where the launched pid exits before the real window appears,
        //      so `windows_json` is empty at this point.
        //
        // To cover (3) we spawn a background polling task that keeps
        // looking for windows under the launched pid family for up to 5 s
        // and minimizes anything that materializes. The task runs detached
        // so the launch_app response isn't held up by it.
        //
        // SW_SHOWMINNOACTIVE preserves the foreground while minimizing. Using
        // SW_MINIMIZE here would itself activate the next z-order window and
        // force a visible restore-after-steal cycle.
        if start_minimized {
            // First, minimize anything already resolved (covers the common
            // single-process path where windows_json was populated).
            let immediate_hwnds: Vec<u64> = windows_json
                .iter()
                .filter_map(|w| w["window_id"].as_u64())
                .collect();
            // Capture pid + basename for the post-launch polling task to
            // pick up launcher-stub child processes.
            let stub_basename = target_file_opt
                .as_deref()
                .and_then(|t| t.rsplit(|c: char| c == '\\' || c == '/').next())
                .unwrap_or("")
                .to_owned();
            // parent_pid is the launched pid family root — used below to
            // constrain the post-launch minimize sweep to descendants /
            // name-relatives of the launched app, so we don't accidentally
            // minimize a user's unrelated app that started during the 5 s
            // poll window.
            let parent_pid = pid;
            let immediate_hwnds_for_poll = immediate_hwnds.clone();
            let _ = tokio::task::spawn_blocking(move || {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWMINNOACTIVE};
                for h in immediate_hwnds {
                    unsafe {
                        let _ = ShowWindow(HWND(h as *mut _), SW_SHOWMINNOACTIVE);
                    }
                }
            })
            .await;
            // Poll launcher-stub late-window cases before returning. The
            // start_minimized contract is observable at response time; a
            // detached task allowed callers to see a transient restored
            // window immediately after a successful launch.
            // Strategy: every 200 ms for 5 s, find pids that
            //   (a) weren't in the pre-launch snapshot, AND
            //   (b) are part of the launched app's family — either a
            //       descendant of the launched root pid, OR a process
            //       sharing the launched binary's name prefix (covers
            //       renamed-binary chains like swriter.exe → soffice.bin
            //       where the child's name doesn't match but it's a
            //       direct descendant).
            // Minimize THEIR visible windows only. Without the family
            // constraint, anything the user opens during the 5 s window
            // (Chrome, terminal, IDE) would also get minimized — that's
            // the regression CodeRabbit flagged.
            //
            // Loop ends early once we've minimized the first set of
            // windows AND they remain minimized for three ticks — the typical
            // app has its main window up within ~2 s of launch.
            let pre_pids = pre_launch_pids.clone();
            let basename_for_poll = stub_basename.clone();
            let launch_foreground_lock = foreground_lock.take();
            (async move {
                use std::collections::HashSet;
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{
                    IsIconic, ShowWindow, SW_SHOWMINNOACTIVE,
                };
                let _foreground_lock = launch_foreground_lock;
                let mut minimized: HashSet<u64> = immediate_hwnds_for_poll.into_iter().collect();
                let mut idle_ticks_after_any_hit: u8 = 0;
                let mut hit_count_total = minimized.len();
                for _ in 0..25 {
                    let pre_pids_clone = pre_pids.clone();
                    let basename_clone = basename_for_poll.clone();
                    // Build the family pid set fresh each tick — both
                    // descendant graph and name-relatives can grow as
                    // launcher-stub chains spawn deeper children.
                    let family_new_pids: Vec<u32> = tokio::task::spawn_blocking(move || {
                        let related: std::collections::HashSet<u32> =
                            crate::win32::related_processes(parent_pid, &basename_clone)
                                .into_iter()
                                .chain(std::iter::once(parent_pid))
                                .collect();
                        crate::win32::list_processes()
                            .into_iter()
                            .filter(|p| {
                                !pre_pids_clone.contains(&p.pid) && related.contains(&p.pid)
                            })
                            .map(|p| p.pid)
                            .collect()
                    })
                    .await
                    .unwrap_or_default();
                    let mut tick_hits: usize = 0;
                    for cpid in family_new_pids {
                        let wins = tokio::task::spawn_blocking(move || {
                            crate::win32::list_windows(Some(cpid))
                        })
                        .await
                        .unwrap_or_default();
                        for w in wins {
                            let is_new = minimized.insert(w.hwnd);
                            if is_new {
                                hit_count_total += 1;
                            }
                            let hwnd_iso = w.hwnd as usize;
                            let restored = tokio::task::spawn_blocking(move || unsafe {
                                let hwnd = HWND(hwnd_iso as *mut _);
                                if IsIconic(hwnd).as_bool() {
                                    false
                                } else {
                                    let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                                    true
                                }
                            })
                            .await
                            .unwrap_or(false);
                            if is_new || restored {
                                tick_hits += 1;
                            }
                        }
                    }
                    if tick_hits == 0 {
                        idle_ticks_after_any_hit += 1;
                        if hit_count_total > 0 && idle_ticks_after_any_hit >= 3 {
                            // Stable: known windows remained minimized and no
                            // new window appeared for 600 ms.
                            break;
                        }
                    } else {
                        idle_ticks_after_any_hit = 0;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            })
            .await;
        }

        // Match Swift text format 1:1.
        let mut summary = format!("✅ Launched {display} (pid {pid}) in background.");
        if !windows_json.is_empty() {
            summary.push_str("\n\nWindows:");
            for w in &windows_json {
                let title = w["title"].as_str().unwrap_or("");
                let title_disp = if title.is_empty() {
                    "(no title)".to_owned()
                } else {
                    format!("\"{title}\"")
                };
                let wid = w["window_id"].as_u64().unwrap_or(0);
                summary.push_str(&format!("\n- {title_disp} [window_id: {wid}]"));
            }
            summary.push_str(&format!(
                "\n→ Call get_window_state(pid: {pid}, window_id) to inspect."
            ));
        }
        // `bundle_id` is the AUMID when we went through the packaged-app
        // path (so the caller can round-trip the same value to relaunch),
        // and `null` for plain Win32 launches (which truly have no bundle
        // identifier concept on Windows).
        let bundle_id_response = match aumid_for_uwp.as_ref() {
            Some(a) => serde_json::Value::String(a.clone()),
            None => serde_json::Value::Null,
        };
        let structured = json!({
            "pid":       pid,
            "bundle_id": bundle_id_response,
            "name":      display,
            "running":   true,
            "active":    false,  // SW_SHOWNOACTIVATE — Swift's background-launch invariant
            "windows":   windows_json,
        });
        ToolResult::text(summary).with_structured(structured)
    }
}

#[cfg(test)]
mod launch_focus_restore_decision_tests {
    use super::{
        first_unopened_shell_url_index, should_restore_foreground_after_launch, LaunchTargetShape,
    };

    #[test]
    fn only_app_identifying_launches_restore_foreground() {
        for (fields, restore) in [
            // A urls-only launch explicitly asks for navigation in the default
            // browser; that browser is the legitimate foreground.
            (&["urls"][..], false),
            (&["name"], true),
            (&["path"], true),
            // The AUMID path restores synchronously in launch_uwp.rs; the
            // caller gates the polling restore on `aumid_for_uwp.is_none()`.
            (&["aumid"], true),
            (&["bundle_id"], true),
            (&["launch_path"], true),
            // An app-identifying field alongside urls opens them in that app
            // in the background, so the prior foreground is restored.
            (&["name", "urls"], true),
            (&["path", "urls"], true),
            (&["aumid", "urls"], true),
            // Nothing was launched.
            (&[], false),
        ] {
            let shape = LaunchTargetShape {
                has_aumid: fields.contains(&"aumid"),
                has_bundle_id: fields.contains(&"bundle_id"),
                has_name: fields.contains(&"name"),
                has_path: fields.contains(&"path"),
                has_launch_path: fields.contains(&"launch_path"),
                has_urls: fields.contains(&"urls"),
            };
            assert_eq!(
                should_restore_foreground_after_launch(shape),
                restore,
                "{fields:?}"
            );
        }
    }

    #[test]
    fn named_app_launch_forwards_every_url() {
        assert_eq!(first_unopened_shell_url_index(true), 0);
    }

    #[test]
    fn urls_only_launch_does_not_reopen_primary_url() {
        assert_eq!(first_unopened_shell_url_index(false), 1);
    }
}

#[cfg(test)]
mod chromium_flag_injection_tests {
    use super::{inject_chromium_anti_throttling_flags, is_chromium_browser_target};

    #[test]
    fn detects_bare_browser_names() {
        for name in [
            "msedge", "chrome", "brave", "opera", "vivaldi", "chromium", "thorium", "iridium",
            "browser", "arc",
        ] {
            assert!(is_chromium_browser_target(name), "{name} should match");
            assert!(
                is_chromium_browser_target(&format!("{name}.exe")),
                "{name}.exe should match"
            );
            // Case-insensitive.
            assert!(
                is_chromium_browser_target(&name.to_uppercase()),
                "uppercase {name} should match"
            );
        }
    }

    #[test]
    fn detects_full_paths() {
        assert!(is_chromium_browser_target(
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
        ));
        assert!(is_chromium_browser_target(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe"
        ));
        // Forward slashes too (some shells write paths that way).
        assert!(is_chromium_browser_target(
            r"C:/Program Files/Google/Chrome/Application/chrome.exe"
        ));
    }

    #[test]
    fn detects_launch_path_with_trailing_args() {
        // Round-tripped launch_path from list_apps with shortcut arguments.
        assert!(is_chromium_browser_target(
            r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --profile-directory="Profile 2""#
        ));
    }

    #[test]
    fn does_not_match_non_chromium_apps() {
        for name in ["firefox", "notepad", "explorer", "code", "soffice"] {
            assert!(!is_chromium_browser_target(name), "{name} should NOT match");
            assert!(
                !is_chromium_browser_target(&format!("{name}.exe")),
                "{name}.exe should NOT match"
            );
        }
        // Empty target.
        assert!(!is_chromium_browser_target(""));
    }

    #[test]
    fn injects_three_flags_into_empty_args() {
        let mut args: Vec<String> = vec![];
        inject_chromium_anti_throttling_flags(&mut args);
        assert!(args.contains(&"--disable-features=CalculateNativeWinOcclusion".to_string()));
        assert!(args.contains(&"--disable-backgrounding-occluded-windows".to_string()));
        assert!(args.contains(&"--disable-renderer-backgrounding".to_string()));
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn merges_into_existing_disable_features_list() {
        let mut args = vec!["--disable-features=Foo,Bar".to_string()];
        inject_chromium_anti_throttling_flags(&mut args);
        // Should NOT have two --disable-features= entries.
        let dfe: Vec<_> = args
            .iter()
            .filter(|a| a.starts_with("--disable-features="))
            .collect();
        assert_eq!(dfe.len(), 1);
        assert!(dfe[0].contains("CalculateNativeWinOcclusion"));
        assert!(dfe[0].contains("Foo"));
        assert!(dfe[0].contains("Bar"));
    }

    #[test]
    fn idempotent_when_all_flags_already_present() {
        let mut args = vec![
            "--disable-features=CalculateNativeWinOcclusion".to_string(),
            "--disable-backgrounding-occluded-windows".to_string(),
            "--disable-renderer-backgrounding".to_string(),
        ];
        let before = args.clone();
        inject_chromium_anti_throttling_flags(&mut args);
        assert_eq!(args, before, "must not duplicate flags");
    }

    #[test]
    fn preserves_user_url_argument_after_flags() {
        let mut args = vec!["file:///C:/test_page.html".to_string()];
        inject_chromium_anti_throttling_flags(&mut args);
        // URL must still be present.
        assert!(args.iter().any(|a| a == "file:///C:/test_page.html"));
        // All three flags now in args.
        assert!(args
            .iter()
            .any(|a| a == "--disable-features=CalculateNativeWinOcclusion"));
        assert!(args
            .iter()
            .any(|a| a == "--disable-backgrounding-occluded-windows"));
        assert!(args.iter().any(|a| a == "--disable-renderer-backgrounding"));
    }
}
