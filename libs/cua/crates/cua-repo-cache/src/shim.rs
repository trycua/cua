//! What a Space sees: the helper, the system git config that routes a
//! mirrored repository's URL to it, the script that installs both, and the
//! log the helper keeps.

use std::path::Path;

use crate::GUEST_ROOT;
use crate::store::{self, Entry};

/// The helper a Space runs for a mirrored repository.
pub const HELPER: &str = include_str!("../assets/git-remote-cuarepo");

/// The helper's file name (`git remote-<name>` finds it).
pub const HELPER_NAME: &str = "git-remote-cuarepo";

/// Where the helper keeps its log in the Space.
pub const GUEST_LOG: &str = "/tmp/.cua-repo-cache.log";

/// Writes the helper into `<root>/bin`.
pub fn write_helper(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = root.join("bin");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(HELPER_NAME);
    if std::fs::read_to_string(&p).is_ok_and(|t| t == HELPER) {
        return Ok(());
    }
    store::write_atomic(&p, HELPER.as_bytes())?;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))
}

/// The system git config a Space includes: one rewrite per verified mirror.
/// `<url>` (and `<url>.git`, which it is a prefix of) goes to the helper,
/// which checks everything again and falls back to plain `https`.
pub fn config_text(entries: &[Entry]) -> String {
    let mut out =
        String::from("# Written by the Cua daemon from its repo cache. Read-only in a Space.\n");
    for e in entries {
        // Keys and URLs are normalized (hex and a restricted alphabet).
        out.push_str(&format!(
            "[url \"cuarepo::{key}::{url}\"]\n\tinsteadOf = {url}\n",
            key = e.key,
            url = e.url
        ));
    }
    out
}

/// Rewrites `<root>/gitconfig` for the mirrors there now.
pub fn write_config(root: &Path) -> std::io::Result<()> {
    store::write_atomic(
        &root.join("gitconfig"),
        config_text(&store::entries(root)).as_bytes(),
    )
}

/// The script that makes a Space use the cache, run in the Space by the
/// daemon once it is up (as root, through the Space's own daemon): it
/// installs the helper where git finds it and includes the cache's git
/// config in the system config. It does nothing, quietly, in a Space
/// without the mount, without git, or without the rights.
pub fn install_script() -> String {
    format!(
        r#"root={GUEST_ROOT}
[ -f "$root/gitconfig" ] && [ -f "$root/bin/{HELPER_NAME}" ] || exit 0
command -v git >/dev/null 2>&1 || exit 0
for dir in "$(git --exec-path 2>/dev/null)" /usr/local/bin; do
  [ -n "$dir" ] && [ -d "$dir" ] && [ -w "$dir" ] || continue
  if cmp -s "$root/bin/{HELPER_NAME}" "$dir/{HELPER_NAME}" 2>/dev/null; then ok=1; break; fi
  if cp "$root/bin/{HELPER_NAME}" "$dir/{HELPER_NAME}.tmp$$" 2>/dev/null \
    && chmod 0755 "$dir/{HELPER_NAME}.tmp$$" \
    && mv "$dir/{HELPER_NAME}.tmp$$" "$dir/{HELPER_NAME}"; then ok=1; break; fi
  rm -f "$dir/{HELPER_NAME}.tmp$$"
done
[ "${{ok:-0}}" = 1 ] || exit 0
git config --system --get-all include.path 2>/dev/null | grep -qx "$root/gitconfig" \
  || git config --system --add include.path "$root/gitconfig" 2>/dev/null
exit 0
"#
    )
}

/// The command that hands the helper's log to the daemon and empties it.
pub fn take_log_command() -> String {
    format!(
        "f={GUEST_LOG}; [ -s \"$f\" ] && mv \"$f\" \"$f.read\" 2>/dev/null && head -c 65536 \"$f.read\"; rm -f \"$f.read\"; exit 0"
    )
}

/// One line the helper logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub kind: LogKind,
    pub key: String,
    pub secs: u32,
}

/// What the helper did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    /// The clone came from the mirror within its lease: no network.
    HitLease,
    /// The remote was asked, the mirror supplied what it already had.
    HitChecked,
    /// A mirror failed a check; the clone went the normal way.
    Skipped,
    /// The clone failed.
    Failed,
    /// The copy into the clone failed: it keeps the mirror as an alternate.
    Kept,
}

/// Parses what a Space wrote (never trusted: unknown or malformed lines are
/// ignored, and only the first 1000 count).
pub fn parse_log(text: &str) -> Vec<LogLine> {
    text.lines()
        .take(1000)
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let _epoch = w.next()?.parse::<u64>().ok()?;
            let kind = match w.next()? {
                "hit_lease" => LogKind::HitLease,
                "hit_checked" => LogKind::HitChecked,
                "skipped" => LogKind::Skipped,
                "failed" => LogKind::Failed,
                "kept" => LogKind::Kept,
                _ => return None,
            };
            let key = w.next()?;
            if key.len() != 16
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return None;
            }
            let secs = w.next()?.parse::<u32>().ok()?.min(3600);
            w.next().is_none().then(|| LogLine {
                kind,
                key: key.to_string(),
                secs,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_log_is_parsed_strictly() {
        let l = parse_log(
            "1700000000 hit_lease 0123456789abcdef 0\n\
             1700000001 hit_checked 0123456789abcdef 1\n\
             1700000002 failed 0123456789abcdef 0\n\
             garbage\n\
             1700000003 hit_lease ../../etc/passwd 0\n\
             1700000004 hit_lease 0123456789abcdef 0 extra\n\
             1700000005 evil 0123456789abcdef 0\n",
        );
        assert_eq!(
            l.iter().map(|x| x.kind).collect::<Vec<_>>(),
            [LogKind::HitLease, LogKind::HitChecked, LogKind::Failed]
        );
    }

    #[test]
    fn the_config_rewrites_each_mirror_to_the_helper() {
        let e = Entry {
            url: "https://github.com/o/r".into(),
            key: "0123456789abcdef".into(),
            filled_ms: 0,
            refreshed_ms: 0,
            used_ms: 0,
            bytes: 0,
            fill_secs: 0.0,
            head: String::new(),
            packs: vec![],
        };
        let t = config_text(&[e]);
        assert!(t.contains("[url \"cuarepo::0123456789abcdef::https://github.com/o/r\"]"));
        assert!(t.contains("insteadOf = https://github.com/o/r\n"));
    }

    #[test]
    fn the_install_script_does_nothing_without_the_mount() {
        // Syntax check with the shell, in a place with no mount.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(install_script())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }
}
