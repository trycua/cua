//! Small file helpers: atomic writes and owner-only permissions.

use std::io::Write;
use std::path::Path;

/// Writes `bytes` to `path` through a temporary file and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_through_rename(path, bytes, true)
}

/// Like [`write_atomic`] without flushing to disk first. For best-effort
/// files on the capture path, where a full sync (`F_FULLFSYNC` on macOS)
/// costs tens of milliseconds on the caller's thread.
pub fn write_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_through_rename(path, bytes, false)
}

fn write_through_rename(path: &Path, bytes: &[u8], sync: bool) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        if sync {
            f.sync_all().ok();
        }
    }
    restrict(&tmp);
    std::fs::rename(&tmp, path)
}

/// Creates `path` with `bytes` only if it does not exist. Returns the
/// content that is there afterwards (ours, or a concurrent writer's).
pub fn create_once(path: &Path, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => {
            f.write_all(bytes)?;
            f.sync_all().ok();
            restrict(path);
            Ok(bytes.to_vec())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // A concurrent creator may not have finished writing: retry a
            // few times for a complete value.
            for _ in 0..20 {
                let got = std::fs::read(path)?;
                if got.len() == bytes.len() {
                    return Ok(got);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            std::fs::read(path)
        }
        Err(e) => Err(e),
    }
}

/// Removes `path` if it exists.
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}
