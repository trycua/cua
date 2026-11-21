// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The operating-system differences in the `cua-driver` entry point.
//!
//! `main` and its command dispatch are shared. Each OS implements
//! [`PlatformMainHooks`] on [`Platform`] for the few steps it does
//! differently. The SDK embedder owns its own lifecycle, so these hooks stay
//! in the binary.
//!
//! The agent-cursor overlay is the one difference without a hook of its own.
//! AppKit must own the macOS main thread, so [`PlatformMainHooks::run_long_lived`]
//! runs it there. The Windows STA pump and the Linux overlay worker start
//! their own threads when the platform tool registry is built, for SDK hosts
//! and this binary alike.

use std::sync::Arc;

pub(crate) struct Platform;

pub(crate) trait PlatformMainHooks {
    /// Exit code for a `serve` authorization or configuration error at
    /// startup. Each OS keeps the code it has always used.
    const STARTUP_AUTHORIZATION_EXIT: i32;

    /// Run a helper launch of this executable that must do no other Driver
    /// work, and return its exit code.
    fn early_exit() -> Option<i32>;

    /// Serve-only platform setup that runs before the desktop runtime exists.
    fn serve_preflight(no_permissions_gate: bool, no_automation_prompts: bool) -> ServeGate;

    /// Run `work` for the rest of the process and return the exit code.
    ///
    /// On macOS `work` runs on its own thread and the process exits with its
    /// code, while the main thread runs `on_main` and then, when `on_main`
    /// returns `true`, the AppKit overlay. Elsewhere `work` runs on the
    /// calling thread and `on_main` never runs.
    fn run_long_lived(
        work: impl FnOnce() -> i32 + Send + 'static,
        on_main: impl FnOnce() -> bool,
    ) -> i32;
}

#[cfg(target_os = "macos")]
impl PlatformMainHooks for Platform {
    const STARTUP_AUTHORIZATION_EXIT: i32 = 64;

    fn early_exit() -> Option<i32> {
        if let Some(code) = platform_macos::permissions::gate::run_permission_probe_if_requested() {
            return Some(code);
        }
        // macOS's "Quit & Reopen" after the Screen Recording switch restarts
        // the app with no arguments. Mid-guide, come back as the same service
        // so the guide can show that it finished.
        if let Some(args) = platform_macos::permissions::guide::resume_serve_args() {
            use std::os::unix::process::CommandExt as _;
            if let Ok(exe) = std::env::current_exe() {
                let error = std::process::Command::new(exe).args(&args).exec();
                eprintln!("cua-driver: could not reopen after the permission guide: {error}");
            }
        }
        crate::cli::run_permissions_host_request_if_requested()
    }

    fn serve_preflight(no_permissions_gate: bool, no_automation_prompts: bool) -> ServeGate {
        platform_macos::apple_events::refuse_ungranted_automation(no_automation_prompts);
        let opts = platform_macos::permissions::GateOpts::from_env_and_flag(no_permissions_gate);
        if let Some((progress, context)) =
            platform_macos::permissions::gate::prepare_telemetry_context(opts.opt_out)
        {
            if progress == platform_macos::permissions::GateProgress::Started {
                crate::telemetry::capture_permissions_gate_started(
                    context.missing_accessibility,
                    context.missing_screen_recording,
                );
            }
        }
        ServeGate { opts }
    }

    fn run_long_lived(
        work: impl FnOnce() -> i32 + Send + 'static,
        on_main: impl FnOnce() -> bool,
    ) -> i32 {
        // AppKit's event loop is process-long, so `work` finishing is what
        // ends the process.
        let worker = std::thread::Builder::new()
            .name("cua-serve".into())
            .spawn(move || std::process::exit(work()))
            .expect("spawn long-lived worker thread");
        if on_main() {
            platform_macos::cursor::overlay::run_on_main_thread();
        }
        let _ = worker.join();
        // The worker exits the process itself, so reaching here means it
        // panicked.
        101
    }
}

#[cfg(target_os = "macos")]
pub(crate) struct ServeGate {
    opts: platform_macos::permissions::GateOpts,
}

#[cfg(target_os = "macos")]
impl ServeGate {
    /// Fail closed until a fresh helper-process probe completes. This also
    /// covers a probe launch failure without letting the serving process
    /// perform and cache its own negative TCC preflight. Call it before the
    /// serve socket binds.
    pub(crate) fn hold(&self, driver: &cua_driver_sdk::CuaDriver) {
        crate::serve::set_permission_gate_pending(driver, !self.opts.opt_out);
    }

    /// Run the first-launch permissions gate on the main thread while serve
    /// is already reachable (#1761). It is a no-op when both grants are
    /// active, and honors `--no-permissions-gate` and
    /// `CUA_DRIVER_RS_PERMISSIONS_GATE=0`. The gate's NSPanel needs the main
    /// thread; short-lived helper processes own the prompt and status APIs.
    pub(crate) fn settle(self, driver: Arc<cua_driver_sdk::CuaDriver>) {
        use crate::telemetry;
        use platform_macos::permissions::{gate, GateProgress};

        let slow_opts = self.opts.after_timeout();
        let gate_result = platform_macos::permissions::run_if_needed_with_observer(
            self.opts,
            |progress, context| match progress {
                GateProgress::Started => {
                    telemetry::capture_permissions_gate_started(
                        context.missing_accessibility,
                        context.missing_screen_recording,
                    );
                }
                GateProgress::Dismissed => {
                    telemetry::capture_permissions_gate_dismissed(
                        context.missing_accessibility,
                        context.missing_screen_recording,
                        context.elapsed,
                    );
                }
            },
        );
        crate::serve::settle_permission_gate(driver, gate_result.is_ok(), move || {
            gate::wait_for_grants(&slow_opts)
        });
        let gate_context = gate::telemetry_context();
        if gate_context.engaged {
            telemetry::capture_permissions_gate_completed(
                gate_context.missing_accessibility,
                gate_context.missing_screen_recording,
                gate_context.panel_shown,
                gate_context.dismissed,
                telemetry::permissions_gate_resolution(
                    gate_result.is_err(),
                    gate_context.dismissed,
                ),
                gate_context.elapsed,
            );
        }
        if let Err(e) = gate_result {
            eprintln!("[cua-driver] permissions gate: {e}");
            eprintln!(
                "[cua-driver] desktop tool calls remain gated until Accessibility and \
                 Screen Recording are granted; still checking in the background."
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl PlatformMainHooks for Platform {
    const STARTUP_AUTHORIZATION_EXIT: i32 = 1;

    fn early_exit() -> Option<i32> {
        // An elevated Driver runs shell launches through this executable with
        // a standard-user token (#3607).
        #[cfg(target_os = "windows")]
        if let Some(code) =
            platform_windows::standard_user_launch::run_shell_launch_helper_if_requested()
        {
            return Some(code);
        }
        None
    }

    fn serve_preflight(no_permissions_gate: bool, no_automation_prompts: bool) -> ServeGate {
        // The permissions gate and Apple Events refusal are macOS concepts.
        // Windows and Linux accept the flags for CLI uniformity.
        let _ = (no_permissions_gate, no_automation_prompts);
        ServeGate
    }

    fn run_long_lived(
        work: impl FnOnce() -> i32 + Send + 'static,
        _on_main: impl FnOnce() -> bool,
    ) -> i32 {
        work()
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct ServeGate;

#[cfg(not(target_os = "macos"))]
impl ServeGate {
    pub(crate) fn hold(&self, _driver: &cua_driver_sdk::CuaDriver) {}

    pub(crate) fn settle(self, _driver: Arc<cua_driver_sdk::CuaDriver>) {}
}
