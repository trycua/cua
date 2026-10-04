//! The cua home and the test guard.
//!
//! [`cua_home`] is `$CUA_HOME`, else `$HOME/.cua` (`%USERPROFILE%\.cua` on
//! Windows): the directory that holds the Spaces registry, sandbox state,
//! env tokens and credentials.
//!
//! [`guard_write`] refuses a write under the user's *real* `~/.cua` (the
//! passwd home, or on Windows the account's profile directory; never
//! `$HOME` or `USERPROFILE`) from a test process, so a test that forgets to
//! isolate `CUA_HOME` fails loudly instead of leaking state into the user's
//! Spaces app. A process is a test process when:
//!
//! - `CUA_TEST` is set to anything but `0`/`false`/empty (the Python, TS
//!   and Swift harnesses set it), or
//! - a test runner marks it: `cargo nextest` (`NEXTEST`), pytest
//!   (`PYTEST_CURRENT_TEST`, the Python binding) or `node --test`
//!   (`NODE_TEST_CONTEXT`, the TypeScript binding), or
//! - its executable is a cargo test binary (`target/<profile>/deps/<name>-<hash>`,
//!   every `cargo test` unit and integration test) or a Swift test bundle
//!   (`*.xctest`, `swift test`).
//!
//! Release binaries and `cargo run` are never test processes.

use std::io;
use std::path::{Path, PathBuf};

/// Environment variable that marks a test process.
pub const CUA_TEST_ENV: &str = "CUA_TEST";

/// The `cua` of the app bundle this process runs in
/// (`<Name>.app/Contents/MacOS/cua`, symlinks resolved), when there is one:
/// the `cua` an app ships and expects its daemon to run. `None` outside an
/// app bundle (a CLI on `PATH`, a Python host).
pub fn bundled_cua() -> Option<PathBuf> {
    bundled_cua_of(&std::env::current_exe().ok()?)
}

/// [`bundled_cua`] for the executable `exe`.
pub fn bundled_cua_of(exe: &Path) -> Option<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let bundle = exe
        .ancestors()
        .find(|d| d.extension().is_some_and(|e| e == "app"))?;
    let cua = bundle
        .join("Contents/MacOS")
        .join(if cfg!(windows) { "cua.exe" } else { "cua" });
    cua.canonicalize().ok().filter(|p| p.is_file())
}

/// The cua home: `$CUA_HOME`, else `~/.cua`.
pub fn cua_home() -> PathBuf {
    if let Some(h) = std::env::var_os("CUA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(h);
    }
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cua")
}

/// The user's real `~/.cua`, from the account database rather than `$HOME`
/// (a test that swaps `$HOME` still resolves the real one here).
pub fn real_cua_home() -> Option<PathBuf> {
    real_home().map(|h| h.join(".cua"))
}

#[cfg(unix)]
fn real_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: getpwuid_r writes into our buffers only; the result pointer is
    // either null or points at `pwd`, whose strings live in `buf`.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 16 * 1024];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        );
        if rc != 0 || out.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = CStr::from_ptr(pwd.pw_dir).to_bytes();
        if dir.is_empty() {
            return None;
        }
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir)))
    }
}

/// Windows: the account's profile directory from its token (the registry's
/// ProfileImagePath), never `USERPROFILE`: a test that points `USERPROFILE`
/// at a temp dir must not make that temp dir "the real home".
#[cfg(windows)]
fn real_home() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;
    // SAFETY: the token handle is ours and closed below; the buffer is
    // sized from the first call and the second writes at most `len` units.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetUserProfileDirectoryW(token, std::ptr::null_mut(), &mut len);
        let mut buf = vec![0u16; len as usize];
        let ok = len > 0 && GetUserProfileDirectoryW(token, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(token);
        if !ok {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (end > 0).then(|| PathBuf::from(OsString::from_wide(&buf[..end])))
    }
}

#[cfg(not(any(unix, windows)))]
fn real_home() -> Option<PathBuf> {
    None
}

fn truthy(v: &str) -> bool {
    let v = v.trim();
    !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
}

/// Whether this process is a test (see the crate docs).
pub fn is_test_process() -> bool {
    if std::env::var(CUA_TEST_ENV).is_ok_and(|v| truthy(&v)) {
        return true;
    }
    // cargo nextest, pytest (the Python binding) and `node --test` (the
    // TypeScript binding) mark their test processes.
    for marker in ["NEXTEST", "PYTEST_CURRENT_TEST", "NODE_TEST_CONTEXT"] {
        if std::env::var(marker).is_ok_and(|v| truthy(&v)) {
            return true;
        }
    }
    std::env::current_exe()
        .ok()
        .is_some_and(|exe| is_cargo_test_binary(&exe) || is_swift_test_binary(&exe))
}

/// An XCTest bundle or the SwiftPM test helper (`swift test`).
pub fn is_swift_test_binary(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.contains(".xctest/")
        || exe
            .file_name()
            .is_some_and(|n| n == "xctest" || n == "swiftpm-testing-helper")
}

/// `target/<profile>/deps/<name>-<16 hex>` (with `.exe` on Windows).
pub fn is_cargo_test_binary(exe: &Path) -> bool {
    let in_deps = exe
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == "deps");
    let stem = exe
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hashed = stem
        .rsplit_once('-')
        .is_some_and(|(_, h)| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    in_deps && hashed
}

/// `path` with its nearest existing ancestor canonicalized (symlinks such
/// as macOS `/var` -> `/private/var` resolved), the rest appended.
fn normalize(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(c) = cur.canonicalize() {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `path` is the real `~/.cua` or inside it.
pub fn is_under_real_home(path: &Path) -> bool {
    let Some(real) = real_cua_home() else {
        return false;
    };
    path.starts_with(&real) || normalize(path).starts_with(normalize(&real))
}

/// Refuses (`PermissionDenied`) a write at `path` when this is a test
/// process and `path` is under the user's real `~/.cua`. Call it before any
/// file system change to host state (registry, sandbox state, tokens,
/// credentials). The refusal is also printed to stderr so a swallowed error
/// still shows in the test output.
pub fn guard_write(path: &Path) -> io::Result<()> {
    guard_write_for(path, is_test_process())
}

/// [`guard_write`] with the test-process decision given.
pub fn guard_write_for(path: &Path, test_process: bool) -> io::Result<()> {
    if !test_process || !is_under_real_home(path) {
        return Ok(());
    }
    let msg = format!(
        "refusing to write {} from a test: it is the user's real ~/.cua. \
         Isolate the test with a temporary CUA_HOME (and HOME).",
        path.display()
    );
    eprintln!("cua-home guard: {msg}");
    Err(io::Error::new(io::ErrorKind::PermissionDenied, msg))
}

/// Writes `data` to `path` so it is readable by the owner only from the
/// moment it exists: a fresh, uniquely named temp file beside `path` is
/// created exclusively (never reusing or following an existing file or
/// symlink) with mode 0600 on Unix, synced, then renamed over `path`. Use it
/// for anything holding a token or a credential.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Bounded: each attempt uses a fresh name; a collision only means
    // another name is tried.
    let mut last = None;
    for _ in 0..16 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.{}.{nanos:x}.{n}.tmp", std::process::id()));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = match opts.open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e),
        };
        let written = f.write_all(data).and_then(|()| f.sync_all());
        drop(f);
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        return Ok(());
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free temp name")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn write_private_is_owner_only_and_ignores_planted_temp_files() {
        use std::os::unix::fs::PermissionsExt as _;
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("secret.json");
        // A pre-existing, world-readable file and a symlink where a naive
        // writer would put its temp file are neither reused nor followed.
        let decoy = d.path().join("decoy");
        std::fs::write(&decoy, b"untouched").unwrap();
        std::os::unix::fs::symlink(&decoy, d.path().join("secret.tmp")).unwrap();
        std::fs::write(
            d.path().join(format!("secret.tmp-{}", std::process::id())),
            b"x",
        )
        .unwrap();
        write_private(&target, b"token").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"token");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&decoy).unwrap(), b"untouched");
        // Overwriting an existing, looser file leaves it 0600.
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&target, b"rotated").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&target).unwrap(), b"rotated");
        // No temp files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().ends_with(".tmp")
                    && e.file_name().to_string_lossy().starts_with(".secret")
            })
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn this_unit_test_is_a_test_process() {
        assert!(is_test_process(), "cargo test binaries are detected");
    }

    #[test]
    fn detects_cargo_test_binaries_only() {
        assert!(is_cargo_test_binary(Path::new(
            "/w/target/debug/deps/topologies-0123456789abcdef"
        )));
        assert!(is_cargo_test_binary(Path::new(
            "C:/w/target/debug/deps/cua_sdk-0123456789abcdef.exe"
        )));
        assert!(!is_cargo_test_binary(Path::new("/w/target/debug/cua")));
        assert!(!is_cargo_test_binary(Path::new("/usr/local/bin/cua")));
        assert!(!is_cargo_test_binary(Path::new(
            "/w/target/debug/deps/libfoo.rlib"
        )));
        assert!(is_swift_test_binary(Path::new(
            "/w/.build/debug/CuaSpacesMacPackageTests.xctest/Contents/MacOS/CuaSpacesMacPackageTests"
        )));
        assert!(!is_swift_test_binary(Path::new(
            "/Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac"
        )));
    }

    #[test]
    fn truthy_values() {
        assert!(truthy("1") && truthy("yes") && truthy("true"));
        assert!(!truthy("") && !truthy("0") && !truthy("FALSE"));
    }

    /// The real home comes from the account, so the guard still knows it
    /// when a test has pointed `USERPROFILE` elsewhere.
    #[cfg(windows)]
    #[test]
    fn the_windows_real_home_is_the_account_profile() {
        let home = real_home().expect("the account has a profile directory");
        assert!(home.is_absolute() && home.is_dir(), "{}", home.display());
    }

    #[test]
    fn a_temp_home_is_allowed_and_the_real_one_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        guard_write_for(&tmp.path().join("spaces.json"), true).unwrap();
        let Some(real) = real_cua_home() else { return };
        let err = guard_write_for(&real.join("spaces.json"), true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("CUA_HOME"), "{err}");
        guard_write_for(&real, true).unwrap_err();
        guard_write_for(&real.join("sandboxes/x.json"), true).unwrap_err();
        // Outside a test, the real home is the point.
        guard_write_for(&real.join("spaces.json"), false).unwrap();
        // A sibling that merely shares the prefix is not inside it.
        let sibling = real.with_file_name(".cua-other").join("x");
        guard_write_for(&sibling, true).unwrap();
    }
}
