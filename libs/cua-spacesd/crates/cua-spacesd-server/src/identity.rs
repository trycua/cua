// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A guest that just became a new machine (`InitRequest.new_machine`): a
//! Space claimed from a warm pool, or restored or forked from a memory
//! snapshot other Spaces share.
//!
//! Everything secret this driver holds comes from the OS (`getrandom`), never
//! from a userspace PRNG whose state a memory snapshot would copy into every
//! clone. The OS pool itself can be a snapshot's too, until the kernel
//! reseeds, so each secret regenerated here is also mixed with the caller's
//! `entropy` ([`fresh_secret`]): two clones given different entropy never
//! derive the same key even when their kernels hand out the same bytes.
//!
//! The guest-wide part ([`apply`]): the caller's entropy goes into the
//! kernel pool (always written to `/dev/urandom`; credited with
//! `RNDADDENTROPY` and followed by `RNDRESEEDCRNG` with the privilege), the
//! clock is stepped, and `/etc/machine-id` (plus a separate dbus copy) is
//! rewritten. A driver that runs as an unprivileged user (the Linux images
//! run it as the desktop user) does what it may itself and hands the rest
//! to `sudo -n cua-spacesd new-machine` ([`apply_privileged`]), the same
//! route the volume mount takes.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

/// The machine-id files a Linux guest keeps: systemd's and the dbus copy
/// (often a symlink to the first; rewritten only when it is a file).
pub const MACHINE_ID_PATHS: &[&str] = &["/etc/machine-id", "/var/lib/dbus/machine-id"];

/// Fewest `InitRequest.entropy` bytes `new_machine` accepts.
pub const MIN_ENTROPY_BYTES: usize = 32;
/// Most `InitRequest.entropy` bytes accepted.
pub const MAX_ENTROPY_BYTES: usize = 4096;

/// The privileged helper's subcommand (`cua-spacesd new-machine`).
pub const HELPER_SUBCOMMAND: &str = "new-machine";

/// How long the privileged helper may take.
const HELPER_TIMEOUT: Duration = Duration::from_secs(5);

/// 32 secret bytes: SHA-256 over a label, 32 bytes of OS randomness and
/// `extra` (the caller's entropy, or nothing).
pub fn fresh_secret(label: &[u8], extra: &[u8]) -> [u8; 32] {
    let mut os = [0u8; 32];
    getrandom(&mut os);
    let mut hasher = Sha256::new();
    hasher.update(b"cua-spacesd/identity/v1\0");
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label);
    hasher.update(os);
    hasher.update((extra.len() as u64).to_be_bytes());
    hasher.update(extra);
    hasher.finalize().into()
}

/// Fills `buf` from the OS CSPRNG (`getrandom(2)` / the platform
/// equivalent), never a userspace generator.
pub fn getrandom(buf: &mut [u8]) {
    use rand::RngCore as _;
    rand::rngs::OsRng.fill_bytes(buf);
}

/// A new systemd machine id (32 lowercase hex digits, a random v4 UUID
/// without dashes, as `systemd-machine-id-setup` writes) derived with
/// [`fresh_secret`].
pub fn new_machine_id(extra: &[u8]) -> String {
    let mut id: [u8; 16] = fresh_secret(b"machine-id", extra)[..16]
        .try_into()
        .expect("16 bytes");
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    hex::encode(id)
}

/// What [`apply`] should do to the guest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GuestRenewal {
    /// The caller's entropy, hex.
    pub entropy: String,
    /// Step the wall clock to this (Unix nanoseconds), when more than a
    /// second off.
    #[serde(default)]
    pub now_unix_nanos: Option<u128>,
    /// The machine id to write.
    pub machine_id: String,
    /// The machine-id files to rewrite ([`MACHINE_ID_PATHS`] in a guest).
    pub machine_id_paths: Vec<PathBuf>,
    /// The guest runs on its host's kernel (a runc container) or on one
    /// whose random pool and clock its runtime keeps (gVisor): the kernel
    /// pool is not this guest's to reseed and the clock not its to step, so
    /// neither is attempted (and neither calls for the privileged helper).
    #[serde(default)]
    pub shares_host_kernel: bool,
}

/// What [`apply`] did. A step reports `false` when it did not apply (no
/// privilege, a read-only file, not Linux); the error names why.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuestRenewalReport {
    /// The entropy reached the kernel pool (`/dev/urandom` write).
    pub entropy_mixed: bool,
    /// The entropy was credited and the kernel CRNG reseeded at once
    /// (`RNDADDENTROPY` + `RNDRESEEDCRNG`, root only).
    pub rng_reseeded: bool,
    /// The clock was stepped.
    pub clock_adjusted: bool,
    /// The clock was off by more than a second and could not be stepped.
    pub clock_denied: bool,
    /// Every existing machine-id file now holds the new id.
    pub machine_id_written: bool,
    /// Steps that failed for lack of privilege (the unprivileged pass hands
    /// these to the helper).
    pub denied: Vec<String>,
    /// Other failures, for the log.
    pub errors: Vec<String>,
}

impl GuestRenewalReport {
    fn merge(&mut self, other: GuestRenewalReport) {
        self.entropy_mixed |= other.entropy_mixed;
        self.rng_reseeded |= other.rng_reseeded;
        self.clock_adjusted |= other.clock_adjusted;
        if other.clock_adjusted {
            self.clock_denied = false;
        }
        self.machine_id_written |= other.machine_id_written;
        self.denied = other.denied;
        self.errors.extend(other.errors);
    }
}

/// Applies `renewal` to the guest with this process's privileges.
pub fn apply(renewal: &GuestRenewal) -> GuestRenewalReport {
    let mut report = GuestRenewalReport::default();
    let entropy = match hex::decode(&renewal.entropy) {
        Ok(e) => e,
        Err(e) => {
            report.errors.push(format!("entropy: {e}"));
            Vec::new()
        }
    };
    if !entropy.is_empty() {
        mix_entropy(&entropy, !renewal.shares_host_kernel, &mut report);
    }
    if let Some(now) = renewal
        .now_unix_nanos
        .filter(|_| !renewal.shares_host_kernel)
    {
        let now = UNIX_EPOCH + Duration::from_nanos(now.min(u64::MAX as u128) as u64);
        match step_clock(now) {
            ClockStep::InSync => {}
            ClockStep::Stepped => report.clock_adjusted = true,
            ClockStep::Denied => {
                report.clock_denied = true;
                report.denied.push("clock".into());
            }
            ClockStep::Failed(e) => {
                report.clock_denied = true;
                report.errors.push(format!("clock: {e}"));
            }
        }
    }
    write_machine_ids(&renewal.machine_id, &renewal.machine_id_paths, &mut report);
    report
}

/// [`apply`], then the steps it could not take for lack of privilege
/// through `sudo -n <this binary> new-machine` (Linux, not root, the
/// running binary is cua-spacesd). Blocking; call it off the async
/// runtime.
pub fn apply_privileged(renewal: &GuestRenewal) -> GuestRenewalReport {
    let mut report = apply(renewal);
    if report.denied.is_empty() || is_root() {
        return report;
    }
    let Some(exe) = helper_executable() else {
        return report;
    };
    match run_helper(&exe, renewal) {
        Ok(helper) => report.merge(helper),
        Err(e) => report.errors.push(format!("privileged helper: {e}")),
    }
    report
}

/// The binary to re-run as root: this one, when it is cua-spacesd (a test
/// harness or an embedding binary never re-runs itself under sudo).
fn helper_executable() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_str()?;
    if !matches!(name, "cua-spacesd" | "cua-env-driver" | "cua-guestd") {
        return None;
    }
    let has_sudo = std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join("sudo").is_file()));
    has_sudo.then_some(exe)
}

fn run_helper(exe: &Path, renewal: &GuestRenewal) -> Result<GuestRenewalReport, String> {
    use std::process::{Command, Stdio};
    let input = serde_json::to_vec(renewal).map_err(|e| e.to_string())?;
    // The entropy goes over stdin, never argv (world-readable in /proc).
    let mut child = Command::new("sudo")
        .arg("-n")
        .arg(exe)
        .arg(HELPER_SUBCOMMAND)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("sudo: {e}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(&input)
        .map_err(|e| format!("writing to the helper: {e}"))?;
    let deadline = std::time::Instant::now() + HELPER_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("helper report: {e}"))
}

/// The `cua-spacesd new-machine` helper: reads a [`GuestRenewal`] (JSON)
/// from stdin, applies it, prints the [`GuestRenewalReport`] (JSON).
/// Only the machine-id files [`MACHINE_ID_PATHS`] names may be written
/// through it, whatever the input says.
pub fn run_helper_main() -> Result<(), String> {
    use std::io::Read as _;
    let mut input = Vec::new();
    std::io::stdin()
        .take(64 * 1024)
        .read_to_end(&mut input)
        .map_err(|e| e.to_string())?;
    let mut renewal: GuestRenewal =
        serde_json::from_slice(&input).map_err(|e| format!("input: {e}"))?;
    if !valid_machine_id(&renewal.machine_id) {
        return Err("input: machine_id must be 32 lowercase hex digits".into());
    }
    renewal.machine_id_paths.retain(|p| {
        MACHINE_ID_PATHS
            .iter()
            .any(|allowed| Path::new(allowed) == p)
    });
    let report = apply(&renewal);
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// 32 lowercase hex digits.
pub fn valid_machine_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn permission_denied(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::PermissionDenied)
        || matches!(e.raw_os_error(), Some(code) if code == libc::EPERM || code == libc::EACCES)
}

/// Mixes `entropy` into the kernel pool: written to `/dev/urandom` (any
/// user; mixed, not credited), then, with the privilege, credited with
/// `RNDADDENTROPY` and the CRNG reseeded with `RNDRESEEDCRNG` so every
/// later `getrandom` in the guest (other processes' TLS keys included)
/// differs from the snapshot's at once rather than at the kernel's next
/// scheduled reseed.
fn mix_entropy(entropy: &[u8], reseed: bool, report: &mut GuestRenewalReport) {
    #[cfg(target_os = "linux")]
    {
        match std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/urandom")
            .and_then(|mut f| f.write_all(entropy))
        {
            Ok(()) => report.entropy_mixed = true,
            Err(e) => report.errors.push(format!("/dev/urandom: {e}")),
        }
        if reseed {
            match reseed_crng(entropy) {
                Ok(()) => report.rng_reseeded = true,
                Err(e) if permission_denied(&e) => report.denied.push("rng".into()),
                Err(e) => report.errors.push(format!("rng reseed: {e}")),
            }
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let _ = reseed;
        // macOS: /dev/random accepts writes from root and mixes them in.
        match std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/random")
            .and_then(|mut f| f.write_all(entropy))
        {
            Ok(()) => report.entropy_mixed = true,
            Err(e) => report.errors.push(format!("/dev/random: {e}")),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (entropy, reseed, report);
    }
}

/// `RNDADDENTROPY` (credit every bit: the bytes come from the caller's
/// CSPRNG) then `RNDRESEEDCRNG`. Both need `CAP_SYS_ADMIN`.
#[cfg(target_os = "linux")]
fn reseed_crng(entropy: &[u8]) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;
    // _IOW('R', 0x03, int[2]) and _IO('R', 0x07).
    const RNDADDENTROPY: u64 = 0x4008_5203;
    const RNDRESEEDCRNG: u64 = 0x5207;
    let bytes = &entropy[..entropy.len().min(512)];
    // struct rand_pool_info { int entropy_count; int buf_size; __u32 buf[]; }
    let mut info = Vec::with_capacity(8 + bytes.len().next_multiple_of(4));
    info.extend_from_slice(&((bytes.len() * 8) as i32).to_ne_bytes());
    info.extend_from_slice(&(bytes.len() as i32).to_ne_bytes());
    info.extend_from_slice(bytes);
    info.resize(8 + bytes.len().next_multiple_of(4), 0);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/random")?;
    // SAFETY: `info` is a valid rand_pool_info of `bytes.len()` bytes; the
    // fd is open for the duration of both calls.
    unsafe {
        if libc::ioctl(file.as_raw_fd(), RNDADDENTROPY as _, info.as_ptr()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::ioctl(file.as_raw_fd(), RNDRESEEDCRNG as _) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Result of [`step_clock`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockStep {
    /// Within a second already.
    InSync,
    /// Stepped.
    Stepped,
    /// Off by more than a second; this process may not set the clock.
    Denied,
    /// Off by more than a second; setting it failed otherwise.
    Failed(String),
}

/// Steps the wall clock to `now` when it is more than a second off (Linux;
/// elsewhere the hypervisor or the OS keeps guest time).
pub fn step_clock(now: SystemTime) -> ClockStep {
    let skew = match now.duration_since(SystemTime::now()) {
        Ok(ahead) => ahead,
        Err(behind) => behind.duration(),
    };
    if skew <= Duration::from_secs(1) {
        return ClockStep::InSync;
    }
    #[cfg(target_os = "linux")]
    {
        let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
        let ts = libc::timespec {
            tv_sec: since.as_secs() as _,
            tv_nsec: since.subsec_nanos() as _,
        };
        // SAFETY: valid timespec.
        if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) } == 0 {
            return ClockStep::Stepped;
        }
        let e = std::io::Error::last_os_error();
        if permission_denied(&e) {
            return ClockStep::Denied;
        }
        ClockStep::Failed(e.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        ClockStep::Failed("this driver steps the clock on Linux only".into())
    }
}

/// Rewrites each existing machine-id file in `paths` with `id` in place
/// (so a bind mount stays a bind mount), skipping a symlink (it follows
/// the file it points at) and a missing file. Linux only.
fn write_machine_ids(id: &str, paths: &[PathBuf], report: &mut GuestRenewalReport) {
    if !cfg!(target_os = "linux") || paths.is_empty() {
        return;
    }
    let mut all = true;
    let mut any = false;
    for path in paths {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => continue,
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                report.errors.push(format!("{}: {e}", path.display()));
                all = false;
                continue;
            }
        }
        any = true;
        let written = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(path)
            .and_then(|mut f| {
                f.write_all(format!("{id}\n").as_bytes())?;
                f.sync_all()
            });
        match written {
            Ok(()) => {}
            Err(e) if permission_denied(&e) => {
                all = false;
                if !report.denied.iter().any(|d| d == "machine-id") {
                    report.denied.push("machine-id".into());
                }
            }
            Err(e) => {
                all = false;
                report.errors.push(format!("{}: {e}", path.display()));
            }
        }
    }
    report.machine_id_written = any && all;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_secrets_differ_and_depend_on_label() {
        assert_ne!(fresh_secret(b"a", b"x"), fresh_secret(b"a", b"x"));
        assert_ne!(fresh_secret(b"a", b""), fresh_secret(b"b", b""));
    }

    #[test]
    fn machine_ids_are_systemd_shaped_and_unique() {
        let a = new_machine_id(b"e");
        let b = new_machine_id(b"e");
        assert!(valid_machine_id(&a), "{a}");
        assert_ne!(a, b);
        assert_eq!(&a[12..13], "4", "v4 UUID version nibble");
        assert!(!valid_machine_id("ABCDEF0123456789abcdef0123456789"));
        assert!(!valid_machine_id("abc"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rewrites_files_in_place_and_leaves_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("machine-id");
        std::fs::write(&etc, "0e499f8038b24ee29a5185340af7be5e\n").unwrap();
        let dbus = dir.path().join("dbus-machine-id");
        std::os::unix::fs::symlink(&etc, &dbus).unwrap();
        let missing = dir.path().join("missing");
        let id = new_machine_id(b"e");
        let report = apply(&GuestRenewal {
            entropy: hex::encode([7u8; 32]),
            now_unix_nanos: None,
            machine_id: id.clone(),
            machine_id_paths: vec![etc.clone(), dbus.clone(), missing.clone()],
            shares_host_kernel: false,
        });
        assert!(report.machine_id_written, "{report:?}");
        assert!(report.entropy_mixed, "{report:?}");
        assert_eq!(std::fs::read_to_string(&etc).unwrap(), format!("{id}\n"));
        assert!(std::fs::symlink_metadata(&dbus)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!missing.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unwritable_machine_id_is_reported_denied() {
        use std::os::unix::fs::PermissionsExt as _;
        if is_root() {
            return; // root writes a 0444 file anyway
        }
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("machine-id");
        std::fs::write(&etc, "0e499f8038b24ee29a5185340af7be5e\n").unwrap();
        std::fs::set_permissions(&etc, std::fs::Permissions::from_mode(0o444)).unwrap();
        let report = apply(&GuestRenewal {
            entropy: String::new(),
            now_unix_nanos: None,
            machine_id: new_machine_id(b""),
            machine_id_paths: vec![etc.clone()],
            shares_host_kernel: false,
        });
        assert!(!report.machine_id_written);
        assert_eq!(report.denied, vec!["machine-id".to_string()]);
        // Unprivileged: the CRNG reseed and the clock are root's.
        assert!(!report.rng_reseeded);
    }

    #[test]
    fn a_clock_within_a_second_is_left_alone() {
        assert_eq!(step_clock(SystemTime::now()), ClockStep::InSync);
    }

    #[test]
    fn the_helper_is_never_this_test_binary() {
        assert_eq!(helper_executable(), None);
    }
}
