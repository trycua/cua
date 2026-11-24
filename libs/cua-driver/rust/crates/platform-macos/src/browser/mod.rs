// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

pub mod ax_page_reader;
pub mod browser_js;
pub mod cdp_client;
mod consent_ui;
pub mod electron_js;
mod managed_policy;
pub mod platform;
mod setup_ui;
pub mod wk_web_view;

pub use ax_page_reader::AXPageReader;
pub use browser_js::BrowserJs;
pub use cdp_client::{CdpClient, CdpSessionCache};
pub use electron_js::ElectronJs;
pub use platform::MacOsBrowserPlatform;
pub use wk_web_view::is_wk_web_view_app;

const MACOS_LSOF_PATH: &str = "/usr/sbin/lsof";

fn resolve_lsof_program_with(exists: impl FnOnce(&std::path::Path) -> bool) -> &'static str {
    if exists(std::path::Path::new(MACOS_LSOF_PATH)) {
        MACOS_LSOF_PATH
    } else {
        "lsof"
    }
}

/// Resolve macOS's `/usr/sbin/lsof` directly so minimal caller `PATH`
/// environments that omit `/usr/sbin` still find it, falling back to `lsof`
/// on `PATH` when the standard system binary is absent.
pub(super) fn lsof_program() -> &'static str {
    resolve_lsof_program_with(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_program_prefers_system_usr_sbin_path_when_present() {
        assert_eq!(
            resolve_lsof_program_with(|path| path == std::path::Path::new(MACOS_LSOF_PATH)),
            MACOS_LSOF_PATH
        );
    }

    #[test]
    fn lsof_program_falls_back_to_path_lookup_when_system_path_absent() {
        assert_eq!(resolve_lsof_program_with(|_| false), "lsof");
    }

    #[tokio::test]
    async fn lsof_program_executes_without_usr_sbin_on_path() {
        if !std::path::Path::new(MACOS_LSOF_PATH).is_file() {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let port = listener.local_addr().expect("listener addr").port();
        let pid = std::process::id().to_string();

        let output = tokio::process::Command::new(lsof_program())
            .env("PATH", "/usr/bin:/bin")
            .args(["-a", "-n", "-p", &pid, "-iTCP", "-sTCP:LISTEN", "-Fn", "-P"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .await
            .expect("lsof should spawn without /usr/sbin on PATH");

        assert!(
            output.status.success(),
            "lsof exited with {}",
            output.status
        );
        let ports = platform::parse_loopback_lsof_ports(&String::from_utf8_lossy(&output.stdout));
        assert!(
            ports.contains(&port),
            "expected bound loopback port {port} in {ports:?}"
        );
    }
}
