//! Test helpers shared by the integration tests.
#![allow(dead_code)]

/// A stand-in cua-driver whose `mcp` answers one MCP `initialize`, made
/// once per test binary in a temporary folder. Set up writes the cua-driver
/// MCP server only for a driver that passes that check.
#[cfg(unix)]
pub fn answering_driver() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        use std::path::PathBuf;
        let dir: PathBuf = tempfile::tempdir().unwrap().keep();
        let p = dir.join("cua-driver");
        write_executable(
            &p,
            "#!/bin/sh\nread line\nprintf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"cua-driver\",\"version\":\"9.9.9\"}}}'\n",
        );
        p.display().to_string()
    })
}

/// Write an executable file without ever leaving a write handle open on the
/// path that gets exec'd. Writing in place and then running the file races
/// with any other test thread that forks: the child inherits the open write
/// fd until it execs, and executing the file meanwhile fails with ETXTBSY
/// ("Text file busy") on Linux. So: write a uniquely named sibling, close it,
/// set the mode, then rename it into place atomically.
#[cfg(unix)]
pub fn write_executable(path: &std::path::Path, contents: impl AsRef<[u8]>) {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let mut tmp = tempfile::Builder::new()
        .prefix(".write-exec-")
        .tempfile_in(dir)
        .unwrap();
    tmp.write_all(contents.as_ref()).unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    // Close the write handle before the file gets its final name.
    let tmp_path = tmp.into_temp_path();
    tmp_path.persist(path).unwrap();
}

/// Writes an executable shell script `name` into `dir` (a fake agent binary) and returns its path.
#[cfg(unix)]
pub fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    write_executable(&p, format!("#!/bin/sh\n{body}\n"));
    p
}
