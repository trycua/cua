// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! X server `DISPLAY` / `XAUTHORITY` auto-discovery for SSH-driven sessions.
//!
//! When cua-driver is driven over SSH or from a TTY against a running
//! Wayland+Xwayland (or Xorg) desktop, `XAUTHORITY` is typically missing and
//! `DISPLAY` may be missing too. Xwayland authenticates X11 clients with a
//! per-session MIT cookie passed to it via its own `-auth <file>` argument,
//! which an X11 client cannot guess — so every X11 connect fails with
//! "Authorization required" (#1926) or "$DISPLAY variable not set" (#3047).
//!
//! [`ensure_xauthority_discovered`] reads the display and `-auth` file of the
//! running X server out of `/proc` and exports them before the first X11
//! connect. It fails closed: it adopts a pair only when exactly one X server
//! owned by this user matches, and only when its auth file is a private
//! regular file owned by this user. Values the caller set are never replaced.
//! Values it adopted itself are revalidated on each call, so a restarted
//! Xwayland's new cookie replaces a stale one.

use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::Mutex;

/// The variables this process exported itself, as `(DISPLAY, XAUTHORITY)`.
/// `DISPLAY` is `None` when the caller set it and only the cookie was adopted.
static ADOPTED: Mutex<Option<Adopted>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Adopted {
    display: Option<String>,
    xauthority: String,
}

/// One X server process's display and cookie, as read from its argv.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Candidate {
    /// The `:N` display argument. `None` when the server picks its own display
    /// (`-displayfd` with no `:N`, as GDM's Xorg does).
    display: Option<u32>,
    auth: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Keep,
    Set(Adopted),
    Clear,
}

/// Discover and export `DISPLAY` and `XAUTHORITY` from the running X server
/// when the caller left them unset. Safe to call eagerly at daemon startup;
/// each call revalidates what an earlier call adopted.
pub fn ensure_xauthority_discovered() {
    let mut adopted = ADOPTED.lock().unwrap_or_else(|e| e.into_inner());
    let display = std::env::var_os("DISPLAY");
    let xauthority = std::env::var_os("XAUTHORITY");
    let action = plan(
        display.as_deref(),
        xauthority.as_deref(),
        adopted.as_ref(),
        || scan_proc(Path::new("/proc"), current_euid()),
    );
    // `set_var`/`remove_var`: called from daemon startup, before any X11
    // connect or worker thread reads the environment.
    match action {
        Action::Keep => {}
        Action::Set(pair) => {
            if let Some(display) = &pair.display {
                std::env::set_var("DISPLAY", display);
            }
            std::env::set_var("XAUTHORITY", &pair.xauthority);
            tracing::info!(
                target: "cua-driver",
                "adopted the running X server's session: DISPLAY={} XAUTHORITY={} (#1926, #3047)",
                pair.display.as_deref().unwrap_or("(caller's)"),
                pair.xauthority,
            );
            *adopted = Some(pair);
        }
        Action::Clear => {
            if let Some(stale) = adopted.take() {
                if stale.display.is_some() {
                    std::env::remove_var("DISPLAY");
                }
                std::env::remove_var("XAUTHORITY");
                tracing::info!(
                    target: "cua-driver",
                    "the X server session adopted earlier is gone; unset the variables it supplied"
                );
            }
        }
    }
}

/// Decide what to export, given the current environment, what this process
/// adopted before, and a scan of the running X servers (run only when needed).
fn plan(
    display: Option<&OsStr>,
    xauthority: Option<&OsStr>,
    adopted: Option<&Adopted>,
    scan: impl FnOnce() -> Option<Vec<Candidate>>,
) -> Action {
    let ours = adopted.filter(|pair| {
        xauthority == Some(OsStr::new(&pair.xauthority))
            && pair
                .display
                .as_deref()
                .is_none_or(|ours| display == Some(OsStr::new(ours)))
    });
    let (display, adopt_display) = match (ours, display, xauthority) {
        // Revalidate an earlier adoption against the caller's original DISPLAY.
        (Some(pair), Some(display), _) => match &pair.display {
            Some(_) => (None, true),
            None => (Some(display), false),
        },
        (Some(_), None, _) => return Action::Keep,
        // The caller supplied the cookie: theirs to keep.
        (None, _, Some(_)) => return Action::Keep,
        (None, Some(display), None) => (Some(display), false),
        (None, None, None) => (None, true),
    };
    let wanted = match display {
        Some(display) => match local_display_number(display) {
            Some(number) => Some(number),
            // A remote or forwarded display (`localhost:10.0`) is not ours to
            // authenticate.
            None => return Action::Keep,
        },
        None => None,
    };
    let next = scan().and_then(|candidates| {
        let found = unique_match(&candidates, wanted)?;
        let display = match adopt_display {
            // A server that chose its own display cannot say which to export.
            true => Some(format!(":{}", found.display?)),
            false => None,
        };
        Some(Adopted {
            display,
            xauthority: found.auth.clone(),
        })
    });
    match (next, ours) {
        (Some(next), Some(pair)) if &next == pair => Action::Keep,
        (Some(next), _) => Action::Set(next),
        (None, Some(_)) => Action::Clear,
        (None, None) => Action::Keep,
    }
}

/// The one candidate serving `wanted`, or the only candidate at all when no
/// display is wanted; `None` for zero or several. A candidate that chose its
/// own display matches any wanted display.
fn unique_match(candidates: &[Candidate], wanted: Option<u32>) -> Option<&Candidate> {
    let mut matches = candidates
        .iter()
        .filter(|c| wanted.is_none_or(|number| c.display.is_none_or(|d| d == number)));
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

/// The display number of a local `DISPLAY` (`:0`, `:0.0`, `unix:1`,
/// `unix/:2`), or `None` for a remote, forwarded or malformed one.
fn local_display_number(display: &OsStr) -> Option<u32> {
    let display = display.to_str()?;
    let rest = ["unix/:", "unix:", ":"]
        .iter()
        .find_map(|prefix| display.strip_prefix(prefix))?;
    let (number, screen) = rest.split_once('.').unwrap_or((rest, "0"));
    parse_number(screen)?;
    parse_number(number)
}

fn parse_number(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Every valid X server candidate under `proc_root` owned by `uid`. `None`
/// when `/proc` itself cannot be read.
fn scan_proc(proc_root: &Path, uid: u32) -> Option<Vec<Candidate>> {
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(proc_root).ok()?.flatten() {
        let name = entry.file_name();
        if name.to_str().and_then(parse_number).is_none() {
            continue;
        }
        // One entry per process: two servers with the same arguments are
        // ambiguous, not one.
        candidates.extend(candidate_from_pid(&entry.path(), uid));
    }
    Some(candidates)
}

/// The candidate one `/proc/<pid>` describes, if it is an X server owned by
/// `uid` whose cookie is safe to adopt and which is still running once the
/// cookie has been checked.
fn candidate_from_pid(pid_dir: &Path, uid: u32) -> Option<Candidate> {
    // Everything below is read after this start time, so the closing check
    // catches a PID reused at any point in between.
    let started = start_time(&std::fs::read_to_string(pid_dir.join("stat")).ok()?)?;
    if std::fs::metadata(pid_dir).ok()?.uid() != uid {
        return None;
    }
    let comm = std::fs::read_to_string(pid_dir.join("comm")).ok()?;
    if !matches!(comm.trim_end_matches('\n'), "Xwayland" | "Xorg" | "X") {
        return None;
    }
    let cmdline = std::fs::read(pid_dir.join("cmdline")).ok()?;
    let candidate = parse_server_args(&cmdline)?;
    if !is_private_cookie(Path::new(&candidate.auth), uid) {
        return None;
    }
    // The process may have exited, or its PID been reused, while the cookie
    // was checked; only adopt what is still the same server.
    let still = std::fs::read_to_string(pid_dir.join("stat")).ok()?;
    if start_time(&still)? != started || std::fs::read(pid_dir.join("cmdline")).ok()? != cmdline {
        return None;
    }
    Some(candidate)
}

/// The start time (field 22) of a `/proc/<pid>/stat` line, which tells a
/// reused PID from the process that held it before. `comm` (field 2) may hold
/// spaces and parentheses, so fields are counted after its last `)`.
fn start_time(stat: &str) -> Option<u64> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_ascii_whitespace().nth(19)?.parse().ok()
}

/// Parse an X server's NUL-separated argv into its display and `-auth` file.
/// Rejects an argv without its closing NUL, a missing, empty, relative,
/// repeated or non-UTF-8 `-auth` value, and more than one `:N` display
/// argument.
fn parse_server_args(cmdline: &[u8]) -> Option<Candidate> {
    let cmdline = cmdline.strip_suffix(b"\0")?;
    let mut args = cmdline.split(|&b| b == 0).skip(1);
    let mut display = None;
    let mut auth = None;
    while let Some(arg) = args.next() {
        if arg == b"-auth" {
            let value = std::str::from_utf8(args.next()?).ok()?;
            if !value.starts_with('/') || auth.replace(value).is_some() {
                return None;
            }
        } else if let Some(number) = arg.strip_prefix(b":") {
            let number = parse_number(std::str::from_utf8(number).ok()?)?;
            if display.replace(number).is_some() {
                return None;
            }
        }
    }
    Some(Candidate {
        display,
        auth: auth?.to_owned(),
    })
}

/// Whether `path` is a regular file, not a symlink, owned by `uid`, readable,
/// and closed to group and others. Checked on the opened file so the answer
/// describes what was opened.
fn is_private_cookie(path: &Path, uid: u32) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    meta.file_type().is_file() && meta.uid() == uid && meta.mode() & 0o077 == 0
}

fn current_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    const MUTTER: &[u8] = b"/usr/bin/Xwayland\0:0\0-rootless\0-noreset\0-accessx\0-core\0\
-auth\0/run/user/1000/.mutter-Xwaylandauth.Q1W2E3\0-listenfd\x004\0-listenfd\x005\0\
-displayfd\x006\0-initfd\x007\0-byteswappedclients\0";
    const KWIN: &[u8] = b"/usr/bin/Xwayland\0:1\0-auth\0/run/user/1000/xauth_aBcDeF\0\
-listenfd\x0067\0-listenfd\x0068\0-displayfd\x0061\0-wm\x0069\0-rootless\0";
    const GDM_XORG: &[u8] = b"/usr/lib/xorg/Xorg\0vt2\0-displayfd\x003\0\
-auth\0/run/user/1000/gdm/Xauthority\0-nolisten\0tcp\0-background\0none\0\
-noreset\0-keeptty\0-novtswitch\0-verbose\x003\0";

    fn candidate(display: Option<u32>, auth: &str) -> Candidate {
        Candidate {
            display,
            auth: auth.into(),
        }
    }

    fn adopted(display: Option<&str>, auth: &str) -> Adopted {
        Adopted {
            display: display.map(Into::into),
            xauthority: auth.into(),
        }
    }

    fn scan_of(candidates: Vec<Candidate>) -> impl FnOnce() -> Option<Vec<Candidate>> {
        move || Some(candidates)
    }

    fn no_scan() -> Option<Vec<Candidate>> {
        panic!("explicit caller values must not trigger a scan")
    }

    #[test]
    fn parses_mutter_kwin_and_gdm_xorg_layouts() {
        assert_eq!(
            parse_server_args(MUTTER),
            Some(candidate(
                Some(0),
                "/run/user/1000/.mutter-Xwaylandauth.Q1W2E3"
            ))
        );
        assert_eq!(
            parse_server_args(KWIN),
            Some(candidate(Some(1), "/run/user/1000/xauth_aBcDeF"))
        );
        // GDM's Xorg picks its own display through -displayfd.
        assert_eq!(
            parse_server_args(GDM_XORG),
            Some(candidate(None, "/run/user/1000/gdm/Xauthority"))
        );
    }

    #[test]
    fn rejects_malformed_server_args() {
        for cmdline in [
            &b"Xwayland\0:0\0"[..],
            b"Xwayland\0:0\0-auth\0",
            b"Xwayland\0:0\0-auth\0\0",
            b"Xwayland\0:0\0-auth\0relative/cookie\0",
            b"Xwayland\0:0\0-auth\0/a\0-auth\0/b\0",
            b"Xwayland\0:0\0:1\0-auth\0/a\0",
            b"Xwayland\0:x\0-auth\0/a\0",
            b"Xwayland\0:0\0-auth\0/\xff\0",
            b"Xwayland\0:0\0-auth\0/a",
            b"",
        ] {
            assert_eq!(parse_server_args(cmdline), None, "{cmdline:?}");
        }
    }

    #[test]
    fn reads_only_local_display_numbers() {
        assert_eq!(local_display_number(OsStr::new(":0")), Some(0));
        assert_eq!(local_display_number(OsStr::new(":1.0")), Some(1));
        assert_eq!(local_display_number(OsStr::new("unix:2")), Some(2));
        assert_eq!(local_display_number(OsStr::new("unix/:3")), Some(3));
        assert_eq!(local_display_number(OsStr::new("localhost:10.0")), None);
        assert_eq!(local_display_number(OsStr::new(":")), None);
        assert_eq!(local_display_number(OsStr::new("unix/:3.bad")), None);
        assert_eq!(local_display_number(OsStr::new(":0.")), None);
        assert_eq!(local_display_number(OsStr::new("")), None);
    }

    #[test]
    fn explicit_values_are_kept_without_scanning() {
        let display = os(":0");
        let auth = os("/home/me/.Xauthority");
        assert_eq!(
            plan(Some(&display), Some(&auth), None, no_scan),
            Action::Keep
        );
        assert_eq!(plan(None, Some(&auth), None, no_scan), Action::Keep);
        let remote = os("localhost:10.0");
        assert_eq!(plan(Some(&remote), None, None, no_scan), Action::Keep);
    }

    #[test]
    fn both_absent_adopts_the_one_candidate() {
        let found = candidate(Some(0), "/run/user/1000/.mutter-Xwaylandauth.A");
        assert_eq!(
            plan(None, None, None, scan_of(vec![found])),
            Action::Set(adopted(Some(":0"), "/run/user/1000/.mutter-Xwaylandauth.A"))
        );
    }

    #[test]
    fn both_absent_fails_closed_on_none_several_or_unknown_display() {
        assert_eq!(plan(None, None, None, scan_of(vec![])), Action::Keep);
        assert_eq!(plan(None, None, None, || None), Action::Keep);
        let two = vec![candidate(Some(0), "/a"), candidate(Some(1), "/b")];
        assert_eq!(plan(None, None, None, scan_of(two)), Action::Keep);
        let conflicting = vec![candidate(Some(0), "/a"), candidate(Some(0), "/b")];
        assert_eq!(plan(None, None, None, scan_of(conflicting)), Action::Keep);
        // A server that chose its own display cannot tell us which DISPLAY to set.
        let unknown = vec![candidate(None, "/a")];
        assert_eq!(plan(None, None, None, scan_of(unknown)), Action::Keep);
    }

    #[test]
    fn caller_display_picks_its_own_server_cookie() {
        let display = os(":1");
        let servers = vec![candidate(Some(0), "/a"), candidate(Some(1), "/b")];
        assert_eq!(
            plan(Some(&display), None, None, scan_of(servers)),
            Action::Set(adopted(None, "/b"))
        );
        let self_chosen = vec![candidate(None, "/gdm")];
        assert_eq!(
            plan(Some(&display), None, None, scan_of(self_chosen)),
            Action::Set(adopted(None, "/gdm"))
        );
        let ambiguous = vec![candidate(None, "/gdm"), candidate(Some(1), "/b")];
        assert_eq!(
            plan(Some(&display), None, None, scan_of(ambiguous)),
            Action::Keep
        );
        let other = vec![candidate(Some(0), "/a")];
        assert_eq!(
            plan(Some(&display), None, None, scan_of(other)),
            Action::Keep
        );
    }

    #[test]
    fn adopted_values_follow_a_rotated_cookie() {
        let display = os(":0");
        let old = os("/run/user/1000/.mutter-Xwaylandauth.OLD");
        let before = adopted(Some(":0"), "/run/user/1000/.mutter-Xwaylandauth.OLD");
        let rotated = vec![candidate(
            Some(0),
            "/run/user/1000/.mutter-Xwaylandauth.NEW",
        )];
        assert_eq!(
            plan(Some(&display), Some(&old), Some(&before), scan_of(rotated)),
            Action::Set(adopted(
                Some(":0"),
                "/run/user/1000/.mutter-Xwaylandauth.NEW"
            ))
        );
        let same = vec![candidate(
            Some(0),
            "/run/user/1000/.mutter-Xwaylandauth.OLD",
        )];
        assert_eq!(
            plan(Some(&display), Some(&old), Some(&before), scan_of(same)),
            Action::Keep
        );
        assert_eq!(
            plan(Some(&display), Some(&old), Some(&before), scan_of(vec![])),
            Action::Clear
        );
        // A cookie adopted for the caller's DISPLAY is rechecked against it.
        let cookie_only = adopted(None, "/b");
        let b = os("/b");
        let rotated = vec![candidate(Some(0), "/c")];
        assert_eq!(
            plan(
                Some(&display),
                Some(&b),
                Some(&cookie_only),
                scan_of(rotated)
            ),
            Action::Set(adopted(None, "/c"))
        );
    }

    #[test]
    fn values_changed_since_adoption_belong_to_the_caller() {
        let display = os(":0");
        let theirs = os("/home/me/.Xauthority");
        let before = adopted(Some(":0"), "/run/user/1000/.mutter-Xwaylandauth.OLD");
        assert_eq!(
            plan(Some(&display), Some(&theirs), Some(&before), no_scan),
            Action::Keep
        );
    }

    struct FakeProc {
        root: tempfile::TempDir,
    }

    impl FakeProc {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().unwrap(),
            }
        }

        fn server(&self, pid: u32, comm: &str, display: &str, auth: &Path) {
            let dir = self.root.path().join(pid.to_string());
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            let stat = format!("{pid} ({comm}) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 4242 0 0\n");
            std::fs::write(dir.join("stat"), stat).unwrap();
            let cmdline = format!("/usr/bin/{comm}\0{display}\0-auth\0{}\0", auth.display());
            std::fs::write(dir.join("cmdline"), cmdline).unwrap();
        }

        fn cookie(&self, name: &str, mode: u32) -> std::path::PathBuf {
            let path = self.root.path().join(name);
            std::fs::write(&path, b"cookie").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }

        fn scan(&self) -> Vec<Candidate> {
            scan_proc(self.root.path(), current_euid()).unwrap()
        }
    }

    #[test]
    fn scan_finds_a_same_user_server_with_a_private_cookie() {
        let proc = FakeProc::new();
        let cookie = proc.cookie("cookie", 0o600);
        proc.server(100, "Xwayland", ":0", &cookie);
        assert_eq!(
            proc.scan(),
            vec![candidate(Some(0), cookie.to_str().unwrap())]
        );
        // Another user's server is never a candidate.
        assert_eq!(
            scan_proc(proc.root.path(), current_euid().wrapping_add(1)).unwrap(),
            vec![]
        );
    }

    #[test]
    fn two_servers_with_the_same_arguments_are_ambiguous() {
        let proc = FakeProc::new();
        let cookie = proc.cookie("cookie", 0o600);
        proc.server(100, "Xwayland", ":0", &cookie);
        proc.server(101, "Xwayland", ":0", &cookie);
        let candidates = proc.scan();
        assert_eq!(candidates.len(), 2);
        assert_eq!(plan(None, None, None, scan_of(candidates)), Action::Keep);
    }

    #[test]
    fn reads_start_time_after_the_last_parenthesis() {
        let stat = "7 (a) b (c)) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0\n";
        assert_eq!(start_time(stat), Some(98765));
        assert_eq!(start_time("7 (Xwayland) S 1"), None);
        assert_eq!(start_time(""), None);
    }

    #[test]
    fn scan_rejects_unsafe_or_missing_cookies_and_other_programs() {
        let proc = FakeProc::new();
        let shared = proc.cookie("shared", 0o640);
        proc.server(100, "Xwayland", ":0", &shared);
        let target = proc.cookie("target", 0o600);
        let link = proc.root.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        proc.server(101, "Xwayland", ":1", &link);
        let dir = proc.root.path().join("dir");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        proc.server(102, "Xorg", ":2", &dir);
        proc.server(103, "Xwayland", ":3", &proc.root.path().join("gone"));
        proc.server(104, "bash", ":4", &target);
        assert_eq!(proc.scan(), vec![]);
    }
}
