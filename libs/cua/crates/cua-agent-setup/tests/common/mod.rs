//! Test helpers shared by the integration tests.
#![allow(dead_code)]

/// A stand-in cua-driver whose `mcp` answers one MCP `initialize`, made
/// once per test binary in a temporary folder. Set up writes the cua-driver
/// MCP server only for a driver that passes that check.
#[cfg(unix)]
pub fn answering_driver() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        let dir: PathBuf = tempfile::tempdir().unwrap().keep();
        let p = dir.join("cua-driver");
        std::fs::write(
            &p,
            "#!/bin/sh\nread line\nprintf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"cua-driver\",\"version\":\"9.9.9\"}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    })
}
