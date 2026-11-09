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
    if candidate.starts_with(&current) && candidate != current {
        return Err("The new location cannot be inside the current one.".into());
    }
    if fs::metadata(candidate).is_ok_and(|md| !md.is_dir()) {
        return Err("That is a file, not a folder.".into());
    }
    let existed = candidate.exists();
    let mut warnings = Vec::new();
    if existed
        && fs::read_dir(candidate)
            .map(|mut r| r.next().is_some())
            .unwrap_or(false)
    {
        warnings.push("The folder is not empty. Cua will add to it.".into());
    }
    let created = fs::create_dir_all(candidate);
    if let Err(e) = created {
        return Err(format!("Cua cannot create that folder: {e}"));
    }
    let probe = candidate.join(format!(".cua-write-test-{}", std::process::id()));
    let writable = fs::write(&probe, b"x");
    let _ = fs::remove_file(&probe);
    if !existed {
        let _ = fs::remove_dir(candidate);
    }
    writable.map_err(|e| format!("That folder is not writable: {e}"))?;
    let available = available_space(candidate);
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
}

/// Validates the move of `kind` to `to` and describes it.
pub fn plan_move(home: &Path, kind: Kind, to: &Path) -> Result<MovePlan, String> {
    let from = dir_in(home, kind, |k| std::env::var(k).ok());
    let bytes = disk_usage(&from);
    let v = validate(home, kind, to, bytes)?;
    Ok(MovePlan {
        kind,
        from,
        to: to.to_path_buf(),
        bytes,
        same_volume: !v.other_volume,
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
    let plan = plan_move(home, kind, to)?;
    if plan.from == plan.to {
        return Ok(plan);
    }
    crate::guard_write(&plan.to).map_err(|e| e.to_string())?;
    fs::create_dir_all(&plan.to).map_err(|e| e.to_string())?;
    if plan.from.exists() {
        let mut done = 0u64;
        copy_tree(&plan.from, &plan.to, &mut |n| {
            done += n;
            progress(done);
        })
        .map_err(|e| format!("Copy failed, nothing was changed: {e}"))?;
        let (a, b) = (disk_usage(&plan.from), disk_usage(&plan.to));
        // Allocated sizes differ a little between volumes; compare file counts and
        // logical sizes, which must match exactly.
        let (fa, fb) = (logical(&plan.from), logical(&plan.to));
        if fb.0 < fa.0 || fb.1 < fa.1 {
            return Err(format!(
                "The copy is incomplete ({} of {} files, {} of {} bytes); nothing was changed. (allocated {a} vs {b})",
                fb.0, fa.0, fb.1, fa.1
            ));
        }
    }
    set_override(home, kind, Some(&plan.to)).map_err(|e| e.to_string())?;
    if plan.from.exists() {
        let _ = fs::write(
            plan.from.join(MOVED_MARKER),
            plan.to.to_string_lossy().as_bytes(),
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

/// Deletes the old folder left by [`move_kind`]. Only call after the person
/// confirmed. Refuses anything that does not carry the marker, so it can
/// never remove a live location.
pub fn remove_old(old: &Path) -> io::Result<u64> {
    crate::guard_write(old)?;
    let marker = old.join(MOVED_MARKER);
    let target = fs::read_to_string(&marker)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "not an old Cua location"))?;
    if !Path::new(target.trim()).is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the new location is missing; keeping the old data",
        ));
    }
    let bytes = disk_usage(old);
    fs::remove_dir_all(old)?;
    Ok(bytes)
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
        assert_eq!(fs::read(to.join("top.txt")).unwrap(), b"hi");
        assert!(last >= 100_002);
        assert_eq!(dir_in(&h, Kind::Runtimes, no_env), to);
        // The old data is still there, with a marker.
        assert!(old.join("lume/1/disk.img").exists());
        assert!(old.join(MOVED_MARKER).exists());
        // Running it again changes nothing and loses nothing.
        let again = move_kind(&h, Kind::Runtimes, &to, |_| {}).unwrap();
        assert_eq!(again.from, again.to);
        // Removing the old copy needs the marker; a live place is refused.
        assert!(remove_old(&to).is_err());
        assert!(to.join("top.txt").exists());
        remove_old(&old).unwrap();
        assert!(!old.exists());
        assert!(to.join("lume/1/disk.img").exists());
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
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("a"), b"aaaa").unwrap();
        fs::write(old.join("b"), b"bbbb").unwrap();
        let to = d.path().join("new");
        fs::create_dir_all(&to).unwrap();
        fs::write(to.join("a"), b"aaaa").unwrap();
        fs::write(to.join(".b.part"), b"bb").unwrap();
        move_kind(&h, Kind::Images, &to, |_| {}).unwrap();
        assert_eq!(fs::read(to.join("b")).unwrap(), b"bbbb");
    }
}
