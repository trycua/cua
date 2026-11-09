//! Where Cua keeps each kind of user state.
//!
//! Every place that reads or writes user state asks [`dir`] for a directory
//! by [`Kind`], instead of joining a path under the cua home itself. The
//! answer is, in order:
//!
//! 1. the kind's environment variable (`CUA_RUNTIMES_DIR`, ...),
//! 2. the `[paths]` table of `$CUA_HOME/storage.toml` (`runtimes = "/Volumes/X/cua"`),
//! 3. the default below the cua home (today's layout).
//!
//! Only the large kinds ([`Kind::movable`]) can be relocated. The settings
//! file always lives in the default cua home, so it is found before any
//! override is applied.
//!
//! [`validate`] checks a candidate location, [`plan_move`] and [`move_kind`]
//! copy existing data to it without deleting anything, and [`remove_old`]
//! deletes the old copy once the caller has asked the person.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// File name of the storage settings under the cua home.
pub const SETTINGS_FILE: &str = "storage.toml";
/// Written in a relocated folder: whether Cua created it (`owned=1`), so an
/// uninstall may remove the whole folder, or the person pointed Cua at a
/// folder that already had files (`owned=0`).
pub const OWNED_MARKER: &str = ".cua-storage";
/// Marker written in an old location after a move, naming the new one.
pub const MOVED_MARKER: &str = ".cua-moved-to";

/// A kind of user state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Settings (`config.toml`, `cache.json`, `storage.toml`).
    Config,
    /// Registry and small state files (`spaces.json`, `sandboxes/`, ...).
    Data,
    /// Downloads, icons, thumbnails, install caches.
    Cache,
    /// Logs (`daemon.log`, ...).
    Logs,
    /// Built-in runtimes and VM disks (`runtimes/`).
    Runtimes,
    /// The Space image cache.
    Images,
    /// `cua do` trajectories.
    Trajectories,
    /// Credential metadata and key files (secrets live in the OS keystore).
    Keys,
}

impl Kind {
    /// Every kind.
    pub const ALL: [Kind; 8] = [
        Kind::Config,
        Kind::Data,
        Kind::Cache,
        Kind::Logs,
        Kind::Runtimes,
        Kind::Images,
        Kind::Trajectories,
        Kind::Keys,
    ];

    /// The name used in `storage.toml` and by `cua storage`.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Config => "config",
            Kind::Data => "data",
            Kind::Cache => "cache",
            Kind::Logs => "logs",
            Kind::Runtimes => "runtimes",
            Kind::Images => "images",
            Kind::Trajectories => "trajectories",
            Kind::Keys => "keys",
        }
    }

    /// Parses [`Kind::name`].
    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s.trim())
    }

    /// The environment variable that overrides this kind, for kinds that
    /// can be relocated.
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Kind::Cache => Some("CUA_CACHE_DIR"),
            Kind::Logs => Some("CUA_LOGS_DIR"),
            Kind::Runtimes => Some("CUA_RUNTIMES_DIR"),
            Kind::Images => Some("CUA_IMAGES_DIR"),
            Kind::Trajectories => Some("CUA_TRAJECTORIES_DIR"),
            _ => None,
        }
    }

    /// Whether the location can be changed (the large kinds).
    pub fn movable(self) -> bool {
        self.env_var().is_some()
    }

    /// The default location below the cua home `home`.
    pub fn default_in(self, home: &Path) -> PathBuf {
        match self {
            Kind::Config | Kind::Data | Kind::Keys => home.to_path_buf(),
            Kind::Cache => home.join("cache"),
            Kind::Logs => home.join("logs"),
            Kind::Runtimes => home.join("runtimes"),
            Kind::Images => home.join("images"),
            Kind::Trajectories => home.join("trajectories"),
        }
    }
}

/// The directory for `kind`: environment, then `storage.toml`, then default.
pub fn dir(kind: Kind) -> PathBuf {
    dir_in(&crate::cua_home(), kind, |k| std::env::var(k).ok())
}

/// [`dir`] for the cua home `home` with an environment lookup given.
pub fn dir_in(home: &Path, kind: Kind, env: impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(var) = kind.env_var()
        && let Some(v) = env(var).filter(|v| !v.trim().is_empty())
    {
        return PathBuf::from(v);
    }
    if kind.movable()
        && let Some(p) = read_settings(home).remove(&kind)
    {
        return p;
    }
    kind.default_in(home)
}

/// Where a kind comes from, for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The default below the cua home.
    Default,
    /// `storage.toml`.
    Settings,
    /// The environment variable (cannot be changed from the app).
    Env,
}

/// [`Source`] of `kind` in `home`.
pub fn source_in(home: &Path, kind: Kind, env: impl Fn(&str) -> Option<String>) -> Source {
    if kind
        .env_var()
        .and_then(&env)
        .is_some_and(|v| !v.trim().is_empty())
    {
        Source::Env
    } else if read_settings(home).contains_key(&kind) {
        Source::Settings
    } else {
        Source::Default
    }
}

/// The settings file in `home`.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(SETTINGS_FILE)
}

/// The `[paths]` of `storage.toml` (a missing or unreadable file is empty;
/// unknown or non-movable kinds are ignored, so an older or newer file
/// never breaks a build).
pub fn read_settings(home: &Path) -> BTreeMap<Kind, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(text) = fs::read_to_string(settings_path(home)) else {
        return out;
    };
    let mut in_paths = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_paths = line == "[paths]";
            continue;
        }
        if !in_paths {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        let Some(v) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
            continue;
        };
        if let Some(kind) = Kind::parse(k)
            && kind.movable()
            && !v.is_empty()
        {
            out.insert(kind, PathBuf::from(unescape(v)));
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some(n) => out.push(n),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Sets (`Some`) or clears (`None`) the override of `kind` in `home`'s
/// `storage.toml`. The file is replaced atomically.
pub fn set_override(home: &Path, kind: Kind, path: Option<&Path>) -> io::Result<()> {
    if !kind.movable() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} cannot be relocated", kind.name()),
        ));
    }
    let file = settings_path(home);
    crate::guard_write(&file)?;
    let mut paths = read_settings(home);
    match path {
        Some(p) => {
            paths.insert(kind, p.to_path_buf());
        }
        None => {
            paths.remove(&kind);
        }
    }
    // The folder that becomes live is no "old location" any more.
    let now_live = match path {
        Some(p) => p.to_path_buf(),
        None => kind.default_in(home),
    };
    let _ = fs::remove_file(now_live.join(MOVED_MARKER));
    if paths.is_empty() && !file.exists() {
        return Ok(());
    }
    let mut text = String::from(
        "# Where Cua keeps its large folders. Edit here or in Settings, Storage.\n[paths]\n",
    );
    for (k, v) in &paths {
        text.push_str(&format!(
            "{} = \"{}\"\n",
            k.name(),
            escape(&v.to_string_lossy())
        ));
    }
    crate::write_private(&file, text.as_bytes())
}

// ---------------------------------------------------------------- sizes

/// Bytes allocated on disk under `path` (sparse files count what they use).
/// Symlinks are not followed. A missing path is 0.
pub fn disk_usage(path: &Path) -> u64 {
    let Ok(md) = fs::symlink_metadata(path) else {
        return 0;
    };
    if md.is_dir() {
        let mut total = 0;
        if let Ok(rd) = fs::read_dir(path) {
            for e in rd.flatten() {
                total += disk_usage(&e.path());
            }
        }
        total
    } else {
        allocated(&md)
    }
}

#[cfg(unix)]
fn allocated(md: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.blocks() * 512
}

#[cfg(not(unix))]
fn allocated(md: &fs::Metadata) -> u64 {
    md.len()
}

/// Bytes available to this user on the volume holding `path` (the nearest
/// existing ancestor). `None` when unknown.
pub fn available_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut p = path.to_path_buf();
        while !p.exists() {
            if !p.pop() {
                return None;
            }
        }
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` is NUL-terminated and `st` is plain data filled in by the call.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

// ----------------------------------------------------------- validation

/// What [`validate`] found out about a candidate location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    /// The location exists already (an existing folder is fine; it must be empty
    /// or be a folder Cua can add to).
    pub exists: bool,
    /// Bytes free on its volume, when known.
    pub available: Option<u64>,
    /// Whether it is on another volume than the current location (a move copies).
    pub other_volume: bool,
    /// Non-blocking notes to show.
    pub warnings: Vec<String>,
}

/// Checks that `candidate` can hold `kind`: absolute, not inside another
/// kind's folder (or the reverse), writable (it is created when missing),
/// and has `needed` bytes free. Creates nothing that is left behind: a
/// folder this call created is removed again.
pub fn validate(
    home: &Path,
    kind: Kind,
    candidate: &Path,
    needed: u64,
) -> Result<Validation, String> {
    if !kind.movable() {
        return Err(format!("{} cannot be relocated", kind.name()));
    }
    if !candidate.is_absolute() {
        return Err("The location must be a full path.".into());
    }
    if candidate.components().any(|c| c.as_os_str() == "..") {
        return Err("The location must not contain \"..\".".into());
    }
    let current = dir_in(home, kind, |k| std::env::var(k).ok());
    for other in Kind::ALL {
        if other == kind || other == Kind::Config || other == Kind::Data || other == Kind::Keys {
            continue;
        }
        let o = dir_in(home, other, |k| std::env::var(k).ok());
        if candidate.starts_with(&o) || o.starts_with(candidate) {
            return Err(format!("That folder overlaps the {} folder.", other.name()));
        }
    }
    let (cur_n, cand_n) = (normalize(&current), normalize(candidate));
    if cand_n.starts_with(&cur_n) && cand_n != cur_n {
        return Err("The new location cannot be inside the current one.".into());
    }
    if cur_n.starts_with(&cand_n) && cand_n != cur_n {
        return Err("The new location cannot contain the current one: the old files could never be removed.".into());
    }
    if fs::metadata(candidate).is_ok_and(|md| !md.is_dir()) {
        return Err("That is a file, not a folder.".into());
    }
    let existed = candidate.exists();
    // The topmost folder this check would create, to remove again.
    let first_missing = candidate
        .ancestors()
        .take_while(|a| !a.exists())
        .last()
        .map(Path::to_path_buf);
    let mut warnings = Vec::new();
    if existed
        && fs::read_dir(candidate)
            .map(|mut r| r.next().is_some())
            .unwrap_or(false)
    {
        warnings.push("The folder is not empty. Cua will add to it.".into());
        let others = other_entries(kind, candidate);
        if !others.is_empty() {
            warnings.push(format!(
                "The new folder holds files Cua does not recognise ({}). They stay as they are.",
                list_names(&others)
            ));
        }
    }
    let src_others = other_entries(kind, &current);
    if !src_others.is_empty() {
        warnings.push(format!(
            "The current folder holds files Cua does not recognise ({}). They are not moved and not deleted.",
            list_names(&src_others)
        ));
    }
    let created = fs::create_dir_all(candidate);
    if let Err(e) = created {
        return Err(format!("Cua cannot create that folder: {e}"));
    }
    let probe = candidate.join(format!(".cua-write-test-{}", std::process::id()));
    let writable = fs::write(&probe, b"x");
    let _ = fs::remove_file(&probe);
    // Take back every empty folder this check created, from the leaf up.
    if let Some(top) = &first_missing {
        let mut cur = Some(candidate);
        while let Some(c) = cur {
            if fs::remove_dir(c).is_err() || c == top.as_path() {
                break;
            }
            cur = c.parent();
        }
    }
    writable.map_err(|e| format!("That folder is not writable: {e}"))?;
    let available = available_space(candidate);
    if let Some(fs_name) = fs_type(candidate)
        && matches!(fs_name.as_str(), "msdos" | "vfat")
        && largest_file(&dir_in(home, kind, |k| std::env::var(k).ok())) >= (4 << 30) - 1
    {
        return Err(
            "That drive is formatted as FAT32, which cannot hold files of 4 GB or more. Choose a drive formatted as APFS, Mac OS Extended, exFAT or NTFS."
                .into(),
        );
    }
    if let Some(a) = available
        && a < needed.saturating_add(5 << 30)
    {
        return Err(format!(
            "Not enough free space: {} needed plus 5 GiB spare, {} free.",
            human(needed),
            human(a)
        ));
    }
    if cfg!(target_os = "macos") && candidate.starts_with("/Volumes") {
        warnings.push(
            "This is an external drive. Computers will not start while it is unplugged.".into(),
        );
    }
    Ok(Validation {
        exists: existed,
        available,
        other_volume: !same_volume(&current, candidate),
        warnings,
    })
}

/// The size `kind`'s data takes when copied to `candidate`: allocated blocks
/// when the target keeps files sparse and clones them (APFS), else the logical
/// size, because a sparse 40 GB disk becomes 40 GB on exFAT, NTFS or a network
/// share.
pub fn needed_bytes(kind: Kind, from: &Path, candidate: &Path) -> u64 {
    let allocated = known_usage(kind, from);
    if fs_type(candidate).is_some_and(|t| t == "apfs") {
        allocated
    } else {
        allocated.max(known_logical(kind, from).1)
    }
}

/// The largest single file under `dir`.
fn largest_file(dir: &Path) -> u64 {
    let Ok(md) = fs::symlink_metadata(dir) else {
        return 0;
    };
    if md.is_dir() {
        fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| largest_file(&e.path()))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    } else {
        md.len()
    }
}

/// The file system type name of the volume holding `path` (`apfs`, `exfat`,
/// `msdos`, `ntfs`, `smbfs`, ...), when known (macOS).
pub fn fs_type(path: &Path) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut p = path.to_path_buf();
        while !p.exists() {
            if !p.pop() {
                return None;
            }
        }
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` is NUL-terminated and `st` is plain data filled in by the call.
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        let name: Vec<u8> = st
            .f_fstypename
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as u8)
            .collect();
        Some(String::from_utf8_lossy(&name).to_lowercase())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        None
    }
}

fn list_names(names: &[String]) -> String {
    let mut shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
    if names.len() > 3 {
        shown.push("...");
    }
    shown.join(", ")
}

fn human(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

#[cfg(unix)]
fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let nearest = |p: &Path| {
        let mut p = p.to_path_buf();
        while !p.exists() {
            if !p.pop() {
                return None;
            }
        }
        fs::metadata(p).ok().map(|m| m.dev())
    };
    matches!((nearest(a), nearest(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(not(unix))]
fn same_volume(_: &Path, _: &Path) -> bool {
    false
}

// -------------------------------------------------------- what Cua owns

/// Whether a top-level entry of `dir` is one Cua itself writes for `kind`. A move
/// copies, and a removal deletes, only such entries: anything else in the folder
/// (a person's own files, another program's) is left alone.
///
/// - runtimes: `linux`, `lume`
/// - images: `blobs`, `disks`, `rootfs`, `refs`
/// - cache: `icons`, `installables`, `thumbnails`
/// - logs: files ending `.log` or `.log.N`
/// - trajectories: a machine folder whose entries are all `YYYYMMDD-HHMMSS` sessions
///
/// Cua's own bookkeeping files (`.cua-storage`, `.cua-moved-to`, `.last-used`,
/// `.DS_Store`, `*.part` leftovers of an interrupted copy) are never "other".
pub fn is_known_entry(kind: Kind, path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return false;
    };
    let is_dir = fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
    match kind {
        Kind::Runtimes => is_dir && matches!(name.as_str(), "linux" | "lume"),
        Kind::Images => is_dir && matches!(name.as_str(), "blobs" | "disks" | "rootfs" | "refs"),
        Kind::Cache => is_dir && matches!(name.as_str(), "icons" | "installables" | "thumbnails"),
        Kind::Logs => !is_dir && (name.ends_with(".log") || name.contains(".log.")),
        Kind::Trajectories => {
            is_dir
                && fs::read_dir(path).is_ok_and(|rd| {
                    rd.flatten().all(|e| {
                        let n = e.file_name().to_string_lossy().into_owned();
                        n.len() == 15
                            && n.as_bytes()[8] == b'-'
                            && n.bytes()
                                .enumerate()
                                .all(|(i, b)| i == 8 || b.is_ascii_digit())
                    })
                })
        }
        _ => false,
    }
}

fn is_bookkeeping(name: &str) -> bool {
    matches!(
        name,
        OWNED_MARKER | MOVED_MARKER | ".last-used" | ".DS_Store"
    ) || (name.starts_with('.') && name.ends_with(".part"))
}

/// The top-level entries of `dir`: (Cua's, others). Bookkeeping files are in neither.
pub fn entries(kind: Kind, dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut known, mut other) = (Vec::new(), Vec::new());
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_bookkeeping(&name) {
                continue;
            }
            if is_known_entry(kind, &e.path()) {
                known.push(e.path());
            } else {
                other.push(e.path());
            }
        }
    }
    known.sort();
    other.sort();
    (known, other)
}

/// Short names of the entries Cua does not recognise in `dir`.
pub fn other_entries(kind: Kind, dir: &Path) -> Vec<String> {
    entries(kind, dir)
        .1
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

/// Bytes allocated by Cua's own entries of `dir`.
pub fn known_usage(kind: Kind, dir: &Path) -> u64 {
    entries(kind, dir).0.iter().map(|p| disk_usage(p)).sum()
}

fn known_logical(kind: Kind, dir: &Path) -> (u64, u64) {
    entries(kind, dir)
        .0
        .iter()
        .map(|p| logical(p))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
}

/// What a removal did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoveReport {
    /// Bytes freed.
    pub freed: u64,
    /// Names of entries that were not Cua's and stay (the folder stays too).
    pub left: Vec<String>,
}

/// Removes Cua's own entries of `kind` from `dir`, plus its bookkeeping files, and
/// then the folder itself only if nothing else is in it.
pub fn remove_known(kind: Kind, dir: &Path) -> io::Result<RemoveReport> {
    crate::guard_write(dir)?;
    let (known, other) = entries(kind, dir);
    let mut freed = 0;
    for p in &known {
        freed += disk_usage(p);
        if fs::symlink_metadata(p)?.is_dir() {
            fs::remove_dir_all(p)?;
        } else {
            fs::remove_file(p)?;
        }
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if is_bookkeeping(&e.file_name().to_string_lossy()) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let left: Vec<String> = other
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if left.is_empty() {
        let _ = fs::remove_dir(dir);
    }
    Ok(RemoveReport { freed, left })
}

// ----------------------------------------------------------------- move

/// What a move would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovePlan {
    /// The kind.
    pub kind: Kind,
    /// Current folder.
    pub from: PathBuf,
    /// New folder.
    pub to: PathBuf,
    /// Bytes to copy.
    pub bytes: u64,
    /// Same volume: the move is a rename and needs no extra space.
    pub same_volume: bool,
    /// Notes to show (files Cua does not recognise in either folder, an external drive).
    pub warnings: Vec<String>,
}

/// Validates the move of `kind` to `to` and describes it.
pub fn plan_move(home: &Path, kind: Kind, to: &Path) -> Result<MovePlan, String> {
    let from = dir_in(home, kind, |k| std::env::var(k).ok());
    let bytes = needed_bytes(kind, &from, to);
    let v = validate(home, kind, to, bytes)?;
    Ok(MovePlan {
        kind,
        from,
        to: to.to_path_buf(),
        bytes,
        same_volume: !v.other_volume,
        warnings: v.warnings,
    })
}

/// Whether the daemon of `home` is running (a move must not run then).
pub fn daemon_running(home: &Path) -> bool {
    let Ok(text) = fs::read_to_string(home.join("daemon.json")) else {
        return false;
    };
    let Some(pid) = text.split("\"pid\"").nth(1).and_then(|r| {
        r.trim_start_matches([' ', ':', '\t'])
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse::<i32>().ok())
    }) else {
        return false;
    };
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Copies the data of `kind` to `to`, checks the copy, then writes the
/// setting so the new place is used. The old folder is left as it was (plus
/// a marker file) until [`remove_old`]. A failure at any step leaves the
/// setting alone, so the old place keeps being used.
///
/// Refuses while the daemon is running: stop the computers first.
/// `progress` is called with the bytes copied so far.
pub fn move_kind(
    home: &Path,
    kind: Kind,
    to: &Path,
    mut progress: impl FnMut(u64),
) -> Result<MovePlan, String> {
    if daemon_running(home) {
        return Err("Stop Cua's computers before moving their files.".into());
    }
    let current = dir_in(home, kind, |k| std::env::var(k).ok());
    let busy = processes_using(&current);
    if !busy.is_empty() {
        return Err(format!(
            "A computer is still running from that folder ({}). Stop it first.",
            busy.join(", ")
        ));
    }
    let plan = plan_move(home, kind, to)?;
    if plan.from == plan.to {
        return Ok(plan);
    }
    crate::guard_write(&plan.to).map_err(|e| e.to_string())?;
    // Cua owns the folder only if it was new or empty: uninstall removes such a
    // folder, and never anything else on a drive the person shares with other files.
    let was_empty = !plan.to.exists()
        || fs::read_dir(&plan.to)
            .map(|mut r| r.all(|e| e.is_ok_and(|e| e.file_name() == OWNED_MARKER)))
            .unwrap_or(false);
    fs::create_dir_all(&plan.to).map_err(|e| e.to_string())?;
    if plan.from.exists() {
        let mut done = 0u64;
        // Only Cua's own entries are copied; anything else in the folder stays where it is.
        for src in entries(kind, &plan.from).0 {
            let dst = plan.to.join(src.file_name().unwrap_or_default());
            copy_tree(&src, &dst, &mut |n| {
                done += n;
                progress(done);
            })
            .map_err(|e| format!("Copy failed, nothing was changed: {e}"))?;
        }
        let (a, b) = (known_usage(kind, &plan.from), known_usage(kind, &plan.to));
        // Allocated sizes differ a little between volumes; compare file counts and
        // logical sizes, which must match exactly.
        let (fa, fb) = (
            known_logical(kind, &plan.from),
            known_logical(kind, &plan.to),
        );
        if fb.0 < fa.0 || fb.1 < fa.1 {
            return Err(format!(
                "The copy is incomplete ({} of {} files, {} of {} bytes); nothing was changed. (allocated {a} vs {b})",
                fb.0, fa.0, fb.1, fa.1
            ));
        }
    }
    set_override(home, kind, Some(&plan.to)).map_err(|e| e.to_string())?;
    let _ = fs::write(
        plan.to.join(OWNED_MARKER),
        format!("kind={}\nowned={}\n", kind.name(), u8::from(was_empty)),
    );
    if plan.from.exists() {
        let _ = fs::write(
            plan.from.join(MOVED_MARKER),
            format!("{}\nkind={}\n", plan.to.to_string_lossy(), kind.name()),
        );
    }
    Ok(plan)
}

fn logical(p: &Path) -> (u64, u64) {
    let Ok(md) = fs::symlink_metadata(p) else {
        return (0, 0);
    };
    if md.is_dir() {
        let mut t = (0, 0);
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                if e.file_name() == MOVED_MARKER {
                    continue;
                }
                let (a, b) = logical(&e.path());
                t = (t.0 + a, t.1 + b);
            }
        }
        t
    } else {
        (1, md.len())
    }
}

/// Recursive copy that preserves symlinks and permissions, skips files
/// already present with the same size (so an interrupted move resumes), and
/// copies to a `.part` name first so a half-written file is never taken for
/// a finished one.
fn copy_tree(from: &Path, to: &Path, count: &mut dyn FnMut(u64)) -> io::Result<()> {
    if !fs::symlink_metadata(from)?.is_dir() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        let len = fs::metadata(from)?.len();
        if fs::metadata(to).is_ok_and(|m| m.len() == len) {
            count(len);
            return Ok(());
        }
        let part = to.with_file_name(format!(
            ".{}.part",
            to.file_name().unwrap_or_default().to_string_lossy()
        ));
        let n = fs::copy(from, &part)?;
        fs::rename(&part, to)?;
        count(n);
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        if name == MOVED_MARKER {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_tree(&src, &dst, count)?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(&src)?;
                if fs::symlink_metadata(&dst).is_ok() {
                    continue;
                }
                std::os::unix::fs::symlink(target, &dst)?;
            }
        } else {
            let len = e.metadata()?.len();
            if fs::metadata(&dst).is_ok_and(|m| m.len() == len) {
                count(len);
                continue;
            }
            let part = dst.with_file_name(format!(".{}.part", name.to_string_lossy()));
            let n = fs::copy(&src, &part)?;
            fs::rename(&part, &dst)?;
            count(n);
        }
    }
    Ok(())
}

/// Whether `path` is, contains or lies inside a folder Cua is using now (the
/// relocatable kinds), or contains the cua home itself. Symlinks are resolved,
/// so a link to a live folder counts as that folder.
pub fn is_in_use(home: &Path, path: &Path) -> bool {
    let canon = |p: &Path| normalize(p);
    let me = canon(path);
    if canon(home).starts_with(&me) {
        return true;
    }
    Kind::ALL.into_iter().filter(|k| k.movable()).any(|k| {
        let live = canon(&dir_in(home, k, |v| std::env::var(v).ok()));
        live.starts_with(&me) || me.starts_with(&live)
    })
}

/// `path` with its nearest existing ancestor resolved through symlinks.
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

/// Removes Cua's own entries from the old folder left by [`move_kind`]. Only call
/// after the person confirmed. Refuses anything that does not carry the marker,
/// and anything that is, contains or lies inside a folder in use. Entries Cua
/// does not recognise (a person's own files that were in the folder) are never
/// touched: the folder stays and the report names them.
pub fn remove_old(old: &Path) -> io::Result<RemoveReport> {
    crate::guard_write(old)?;
    let denied = |m: &str| io::Error::new(io::ErrorKind::PermissionDenied, m.to_string());
    if is_in_use(&crate::cua_home(), old) {
        return Err(denied("that folder is in use"));
    }
    let marker = fs::read_to_string(old.join(MOVED_MARKER))
        .map_err(|_| denied("not an old Cua location"))?;
    let mut lines = marker.lines();
    let new_dir = PathBuf::from(lines.next().unwrap_or("").trim());
    // The kind the marker names, else the one the folder is called (images, runtimes, ...).
    let kind = lines
        .find_map(|l| l.strip_prefix("kind=").and_then(Kind::parse))
        .or_else(|| {
            old.file_name()
                .and_then(|n| n.to_str())
                .and_then(Kind::parse)
        })
        .filter(|k| k.movable())
        .ok_or_else(|| denied("cannot tell what this folder held; keeping it"))?;
    // The data must really be in the new place before the old copy goes.
    if !new_dir.is_dir() {
        return Err(denied("the new location is missing; keeping the old data"));
    }
    if known_logical(kind, &new_dir).0 < known_logical(kind, old).0 {
        return Err(denied(
            "the new location has fewer files than the old one; keeping the old data",
        ));
    }
    if !processes_using(old).is_empty() {
        return Err(denied("a computer is still using that folder"));
    }
    remove_known(kind, old)
}

/// Processes (other than this one) whose command line names a path inside
/// `dir`: a running VM, its host agent, or anything else holding the
/// folder's files. Empty when the list cannot be read or on Windows.
pub fn processes_using(dir: &Path) -> Vec<String> {
    #[cfg(unix)]
    {
        let needle = normalize(dir).to_string_lossy().into_owned();
        let raw = dir.to_string_lossy().into_owned();
        let Ok(out) = std::process::Command::new("ps")
            .args(["-axww", "-o", "pid=,command="])
            .output()
        else {
            return vec![];
        };
        let me = std::process::id().to_string();
        let mut found = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let line = line.trim();
            let Some((pid, cmd)) = line.split_once(' ') else {
                continue;
            };
            if pid == me {
                continue;
            }
            let hit = |n: &str| {
                cmd.match_indices(n).any(|(i, _)| {
                    let after = cmd[i + n.len()..].chars().next();
                    after.is_none_or(|c| c == '/' || c.is_whitespace() || c == '"' || c == ':')
                })
            };
            if hit(&needle) || hit(&raw) {
                found.push(format!(
                    "{} ({})",
                    cmd.split_whitespace().next().unwrap_or(""),
                    pid
                ));
            }
        }
        found
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        vec![]
    }
}

/// Reads a whole file into a string, for callers that only want a quick
/// look; kept here so every kind of state goes through this module.
pub fn read_to_string(kind: Kind, rel: &str) -> io::Result<String> {
    let mut s = String::new();
    fs::File::open(dir(kind).join(rel))?.read_to_string(&mut s)?;
    Ok(s)
}

/// Writes a small file below the directory of `kind`.
pub fn write(kind: Kind, rel: &str, data: &[u8]) -> io::Result<()> {
    let p = dir(kind).join(rel);
    crate::guard_write(&p)?;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    fs::File::create(p)?.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_are_todays_layout() {
        let h = Path::new("/h/.cua");
        assert_eq!(dir_in(h, Kind::Runtimes, no_env), h.join("runtimes"));
        assert_eq!(dir_in(h, Kind::Images, no_env), h.join("images"));
        assert_eq!(dir_in(h, Kind::Cache, no_env), h.join("cache"));
        assert_eq!(dir_in(h, Kind::Logs, no_env), h.join("logs"));
        assert_eq!(
            dir_in(h, Kind::Trajectories, no_env),
            h.join("trajectories")
        );
        assert_eq!(dir_in(h, Kind::Config, no_env), h);
        assert_eq!(dir_in(h, Kind::Keys, no_env), h);
    }

    #[test]
    fn env_beats_settings_beats_default() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        set_override(h, Kind::Runtimes, Some(Path::new("/Volumes/X/rt"))).unwrap();
        assert_eq!(
            dir_in(h, Kind::Runtimes, no_env),
            Path::new("/Volumes/X/rt")
        );
        assert_eq!(source_in(h, Kind::Runtimes, no_env), Source::Settings);
        let env = |k: &str| (k == "CUA_RUNTIMES_DIR").then(|| "/e/rt".to_string());
        assert_eq!(dir_in(h, Kind::Runtimes, env), Path::new("/e/rt"));
        assert_eq!(source_in(h, Kind::Runtimes, env), Source::Env);
        set_override(h, Kind::Runtimes, None).unwrap();
        assert_eq!(dir_in(h, Kind::Runtimes, no_env), h.join("runtimes"));
    }

    #[test]
    fn settings_file_tolerates_junk_and_unknown_kinds() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            settings_path(d.path()),
            "[other]\nruntimes = \"/no\"\n[paths]\nbogus = \"/x\"\nconfig = \"/c\"\nimages = /bare\nlogs = \"/l\\\\x\"\n",
        )
        .unwrap();
        let s = read_settings(d.path());
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(s[&Kind::Logs], Path::new("/l\\x"));
    }

    #[test]
    fn config_cannot_be_relocated() {
        let d = tempfile::tempdir().unwrap();
        assert!(set_override(d.path(), Kind::Config, Some(Path::new("/x"))).is_err());
    }

    #[test]
    fn validate_rejects_bad_locations() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        fs::create_dir_all(&h).unwrap();
        assert!(validate(&h, Kind::Runtimes, Path::new("rel"), 0).is_err());
        assert!(validate(&h, Kind::Runtimes, &d.path().join("a/../b"), 0).is_err());
        assert!(validate(&h, Kind::Runtimes, &h.join("images/x"), 0).is_err());
        assert!(validate(&h, Kind::Runtimes, &h.join("runtimes/sub"), 0).is_err());
        let f = d.path().join("file");
        fs::write(&f, b"x").unwrap();
        assert!(validate(&h, Kind::Runtimes, &f, 0).is_err());
        let ok = d.path().join("new/rt");
        let v = validate(&h, Kind::Runtimes, &ok, 0).unwrap();
        assert!(!v.exists);
        assert!(!ok.exists(), "validate leaves nothing behind");
    }

    #[test]
    fn move_copies_verifies_switches_and_keeps_the_old_data() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        let old = h.join("runtimes");
        fs::create_dir_all(old.join("lume/1")).unwrap();
        fs::write(old.join("lume/1/disk.img"), vec![7u8; 100_000]).unwrap();
        fs::write(old.join("top.txt"), b"hi").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("top.txt", old.join("link")).unwrap();
        let to = d.path().join("ext/rt");
        let mut last = 0;
        let plan = move_kind(&h, Kind::Runtimes, &to, |n| last = n).unwrap();
        assert_eq!(plan.from, old);
        assert_eq!(fs::read(to.join("lume/1/disk.img")).unwrap().len(), 100_000);
        assert!(last >= 100_000);
        assert_eq!(dir_in(&h, Kind::Runtimes, no_env), to);
        // The old data is still there, with a marker.
        assert!(old.join("lume/1/disk.img").exists());
        assert!(old.join(MOVED_MARKER).exists());
        // Running it again changes nothing and loses nothing.
        let again = move_kind(&h, Kind::Runtimes, &to, |_| {}).unwrap();
        assert_eq!(again.from, again.to);
        // Removing the old copy needs the marker; a live place is refused.
        assert!(remove_old(&to).is_err());
        assert!(to.join("lume/1/disk.img").exists());
        let rep = remove_old(&old).unwrap();
        // Cua's own entry goes; the person's top.txt and link were never Cua's, so they stay.
        assert!(!old.join("lume").exists());
        assert_eq!(rep.left, vec!["link".to_string(), "top.txt".to_string()]);
        assert!(old.join("top.txt").exists());
        assert!(
            !to.join("top.txt").exists(),
            "files Cua does not know are not copied"
        );
        assert!(to.join("lume/1/disk.img").exists());
    }

    #[test]
    fn reset_makes_the_old_place_live_again_and_it_cannot_be_removed_as_old() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        let old = h.join("images");
        fs::create_dir_all(old.join("blobs")).unwrap();
        fs::write(old.join("blobs/a"), b"1").unwrap();
        let to = d.path().join("new");
        move_kind(&h, Kind::Images, &to, |_| {}).unwrap();
        assert!(old.join(MOVED_MARKER).exists());
        set_override(&h, Kind::Images, None).unwrap();
        assert!(
            !old.join(MOVED_MARKER).exists(),
            "the marker goes when it is live again"
        );
        assert!(remove_old(&old).is_err());
        assert!(old.join("blobs/a").exists());
    }

    #[test]
    fn remove_old_refuses_without_the_new_place() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("old");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("a"), b"1").unwrap();
        fs::write(
            old.join(MOVED_MARKER),
            d.path().join("gone").to_string_lossy().as_bytes(),
        )
        .unwrap();
        assert!(remove_old(&old).is_err());
        assert!(old.join("a").exists());
    }

    #[test]
    fn a_move_resumes_after_an_interruption() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        let old = h.join("images");
        fs::create_dir_all(old.join("blobs")).unwrap();
        fs::write(old.join("blobs/a"), b"aaaa").unwrap();
        fs::write(old.join("blobs/b"), b"bbbb").unwrap();
        let to = d.path().join("new");
        fs::create_dir_all(to.join("blobs")).unwrap();
        fs::write(to.join("blobs/a"), b"aaaa").unwrap();
        fs::write(to.join("blobs/.b.part"), b"bb").unwrap();
        move_kind(&h, Kind::Images, &to, |_| {}).unwrap();
        assert_eq!(fs::read(to.join("blobs/b")).unwrap(), b"bbbb");
    }

    fn set_cua_home_for_test(h: &Path) {
        // remove_old/is_in_use read the live folders through the global cua home.
        // SAFETY: tests in this module that touch the environment run one at a time (#[serial-like] via the lock).
        unsafe { std::env::set_var("CUA_HOME", h) };
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The audit's data-loss repro: move away, reset, then remove the default folder.
    #[test]
    fn remove_can_never_delete_the_live_folder_after_reset_or_a_move_back() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        set_cua_home_for_test(&h);
        let live = h.join("images");
        fs::create_dir_all(live.join("blobs")).unwrap();
        fs::write(live.join("blobs/a"), b"live").unwrap();
        let b = d.path().join("B");
        move_kind(&h, Kind::Images, &b, |_| {}).unwrap();
        set_override(&h, Kind::Images, None).unwrap();
        assert!(
            remove_old(&live).is_err(),
            "reset: the default is live again"
        );
        assert!(live.join("blobs/a").exists());
        // Move back (B becomes the old copy, the default is live).
        move_kind(&h, Kind::Images, &b, |_| {}).unwrap();
        move_kind(&h, Kind::Images, &live, |_| {}).unwrap();
        assert!(remove_old(&live).is_err());
        assert!(live.join("blobs/a").exists());
        // Even with a stale marker planted in the live folder.
        fs::write(live.join(MOVED_MARKER), b.to_string_lossy().as_bytes()).unwrap();
        assert!(remove_old(&live).is_err());
        assert!(live.join("blobs/a").exists());
        // The old copy in B may go.
        assert!(remove_old(&b).is_ok());
        assert!(live.join("blobs/a").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_refuses_parents_children_and_symlinks_of_live_folders() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        set_cua_home_for_test(&h);
        let ext = d.path().join("ext");
        let live = ext.join("rt");
        fs::create_dir_all(&live).unwrap();
        set_override(&h, Kind::Runtimes, Some(&live)).unwrap();
        for p in [&ext, &live, &live.join("sub")] {
            fs::create_dir_all(p).unwrap();
            fs::write(p.join(MOVED_MARKER), live.to_string_lossy().as_bytes()).unwrap();
            assert!(remove_old(p).is_err(), "{}", p.display());
        }
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&live, &link).unwrap();
        assert!(remove_old(&link).is_err());
        assert!(live.exists());
        // The cua home is never "old".
        fs::write(h.join(MOVED_MARKER), live.to_string_lossy().as_bytes()).unwrap();
        assert!(remove_old(&h).is_err());
    }

    #[test]
    fn check_leaves_no_empty_parents_behind() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        fs::create_dir_all(&h).unwrap();
        let deep = d.path().join("a/b/c");
        validate(&h, Kind::Runtimes, &deep, 0).unwrap();
        assert!(!d.path().join("a").exists());
        // An existing parent stays.
        fs::create_dir_all(d.path().join("keep")).unwrap();
        validate(&h, Kind::Runtimes, &d.path().join("keep/x/y"), 0).unwrap();
        assert!(d.path().join("keep").exists() && !d.path().join("keep/x").exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_size_to_copy_counts_logical_bytes_off_apfs() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("rt");
        fs::create_dir_all(src.join("linux")).unwrap();
        let f = fs::File::create(src.join("linux/datadisk")).unwrap();
        f.set_len(40 << 30).unwrap(); // a sparse 40 GB disk
        assert!(disk_usage(&src) < (1 << 20));
        let target = d.path().join("other");
        // The temp dir is not APFS on CI Linux; on APFS the clone stays cheap.
        let need = needed_bytes(Kind::Runtimes, &src, &target);
        if fs_type(&target).as_deref() == Some("apfs") {
            assert!(need < (1 << 20));
        } else {
            assert!(need >= 40 << 30, "{need}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_move_is_refused_while_a_process_runs_from_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        let old = h.join("runtimes");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("disk"), b"x").unwrap();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 20; true", &old.join("vm").to_string_lossy()])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let r = move_kind(&h, Kind::Runtimes, &d.path().join("new"), |_| {});
        let _ = child.kill();
        let _ = child.wait();
        let err = r.unwrap_err();
        assert!(err.contains("still running"), "{err}");
        assert!(!d.path().join("new").exists(), "nothing was copied");
    }

    /// N1/N2: a person's own file in a Cua folder is never copied, moved or deleted.
    #[test]
    fn users_own_files_in_either_folder_are_never_touched() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        set_cua_home_for_test(&h);
        let old = h.join("trajectories");
        fs::create_dir_all(old.join("mac/20260101-120000")).unwrap();
        fs::write(old.join("mac/20260101-120000/turn_001.json"), b"{}").unwrap();
        fs::write(old.join("my-vacation-video.mov"), b"precious").unwrap();
        // The target already holds the person's files too.
        let to = d.path().join("ext");
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("tax.pdf"), b"also precious").unwrap();
        let plan = move_kind(&h, Kind::Trajectories, &to, |_| {}).unwrap();
        assert!(
            plan.warnings
                .iter()
                .any(|w| w.contains("my-vacation-video.mov")),
            "{:?}",
            plan.warnings
        );
        assert!(
            plan.warnings.iter().any(|w| w.contains("tax.pdf")),
            "{:?}",
            plan.warnings
        );
        assert!(to.join("mac/20260101-120000/turn_001.json").exists());
        assert!(!to.join("my-vacation-video.mov").exists(), "not copied");
        let rep = remove_old(&old).unwrap();
        assert_eq!(rep.left, vec!["my-vacation-video.mov".to_string()]);
        assert_eq!(
            fs::read(old.join("my-vacation-video.mov")).unwrap(),
            b"precious"
        );
        assert!(!old.join("mac").exists());
        assert!(!old.join(MOVED_MARKER).exists());
        assert_eq!(fs::read(to.join("tax.pdf")).unwrap(), b"also precious");
        // remove_known on the new folder (what uninstall does) keeps tax.pdf and its folder.
        let rep = remove_known(Kind::Trajectories, &to).unwrap();
        assert_eq!(rep.left, vec!["tax.pdf".to_string()]);
        assert!(to.join("tax.pdf").exists() && !to.join("mac").exists());
    }

    #[test]
    fn a_folder_with_only_cuas_entries_goes_whole() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("lg");
        fs::create_dir_all(&f).unwrap();
        fs::write(f.join("daemon.log"), b"x").unwrap();
        fs::write(f.join("daemon.log.1"), b"x").unwrap();
        fs::write(f.join(OWNED_MARKER), b"owned=1").unwrap();
        let rep = remove_known(Kind::Logs, &f).unwrap();
        assert!(rep.left.is_empty());
        assert!(!f.exists());
    }

    /// N3: the target may not be a parent of the live folder.
    #[test]
    fn a_target_that_contains_the_current_folder_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().join("home");
        fs::create_dir_all(h.join("images")).unwrap();
        let e = validate(&h, Kind::Images, &h, 0).unwrap_err();
        assert!(e.contains("overlap") || e.contains("contain"), "{e}");
        let ext = d.path().join("ext");
        fs::create_dir_all(ext.join("img")).unwrap();
        set_override(&h, Kind::Images, Some(&ext.join("img"))).unwrap();
        assert!(validate(&h, Kind::Images, &ext, 0).is_err());
    }
}
