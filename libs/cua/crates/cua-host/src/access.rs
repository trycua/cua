//! Reads the host's access log: who reached this machine, through what, and
//! when.
//!
//! `cua-spacesd` appends one hash-chained JSON line per access to
//! `<data-dir>/access.log` (its `access_log` module). This reader verifies
//! the chain before trusting anything in it: a log that was edited or had
//! lines removed is reported as such rather than shown as clean.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Accesses shown by default.
pub const RECENT_LIMIT: usize = 20;
/// Accesses a status reads (the app classifies, collapses and pages them;
/// background probes make most of the lines).
pub const STATUS_LIMIT: usize = 1000;
/// A caller whose session started with a connection this recent stays
/// connected while it keeps calling (background calls included, such as
/// presence while a stream is open), at most this long after it.
pub const SESSION_WINDOW_MS: u64 = 10 * 60_000;
/// A caller with an access this recent counts as connected now. The driver
/// writes at most one line per caller and service per minute while the
/// caller keeps using it, so two minutes covers an active session.
pub const ACTIVE_WINDOW_MS: u64 = 120_000;
/// Lines read at most (the driver rotates at 1 MiB, so this is generous).
const MAX_LINES: usize = 200_000;

/// The hashed part of a line (field order is the canonical form; keep in
/// sync with `cua-spacesd-server`'s `AccessBody`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Body {
    seq: u64,
    ts_ms: u64,
    via: String,
    who: String,
    what: String,
    prev: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Line {
    #[serde(flatten)]
    body: Body,
    hash: String,
}

fn entry_hash(body: &Body) -> String {
    let json = serde_json::to_string(body).expect("serializable");
    let mut h = Sha256::new();
    h.update(body.prev.as_bytes());
    h.update(b"\n");
    h.update(json.as_bytes());
    hex::encode(h.finalize())
}

/// One access, for display.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRecord {
    /// Unix milliseconds.
    pub at_ms: u64,
    /// How the caller authenticated: `relay`, `viewer` or `token`.
    pub via: String,
    /// Who (a relay-verified account, a viewer, or the token holder).
    pub who: String,
    /// What they used (`ProcessService`, `DesktopService`, `MCP`, ...).
    pub what: String,
}

impl AccessRecord {
    /// What it was ([`classify`]).
    pub fn kind(&self) -> AccessKind {
        classify(&self.who, &self.what)
    }
}

/// What an access was, as the host classified it when it wrote the line
/// (`cua-spacesd-server`'s `auth::call_what`; keep in sync). Never from
/// anything the caller said about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessKind {
    /// Someone used the machine: a stream, input, a shell, files, an
    /// agent, teleport, a share, the volume. Counts as a connection.
    Connection,
    /// A read-only probe clients make on their own (capabilities, health,
    /// status, presence, listings). Never a connection.
    Background,
    /// A thumbnail-sized screenshot by the machine's owner.
    OwnerThumbnail,
    /// A thumbnail-sized screenshot by anyone else: shown, not a
    /// connection.
    Thumbnail,
    /// Refused (the desktop is not shared, a view-only share, a bad
    /// assertion). Shown, never a connection.
    Refused,
}

impl AccessKind {
    /// Whether it makes the caller "connected now".
    pub fn is_connection(self) -> bool {
        self == Self::Connection
    }
}

/// The suffix the driver gives a background call.
const BACKGROUND_TAG: &str = " (background)";

/// Classifies an access line. Lines from drivers that predate the
/// classification name only the service: their `SystemService` and
/// `PresenceService` lines are background, the rest connections.
pub fn classify(who: &str, what: &str) -> AccessKind {
    if who == "refused" || what.starts_with("refused ") {
        AccessKind::Refused
    } else if what.ends_with(BACKGROUND_TAG) {
        AccessKind::Background
    } else if what.ends_with("(thumbnail, not owner)") {
        AccessKind::Thumbnail
    } else if what.ends_with("(thumbnail)") {
        AccessKind::OwnerThumbnail
    } else if matches!(what, "SystemService" | "PresenceService") {
        AccessKind::Background
    } else {
        AccessKind::Connection
    }
}

/// The log's recent entries and whether its chain verified.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessReport {
    /// Newest first, at most the requested limit.
    pub recent: Vec<AccessRecord>,
    /// Set when the chain does not verify (edited or truncated log).
    pub error: Option<String>,
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

/// Reads and verifies `path` (after its rotated `<path>.1`), keeping the
/// newest `limit` accesses. A missing log is an empty report.
pub fn read(path: &Path, limit: usize) -> AccessReport {
    let mut report = AccessReport::default();
    let mut all: std::collections::VecDeque<AccessRecord> = Default::default();
    let mut prev: Option<String> = None;
    let mut n = 0usize;
    'files: for p in [rotated(path), path.to_path_buf()] {
        let Ok(f) = std::fs::File::open(&p) else {
            continue;
        };
        for (i, line) in BufReader::new(f).lines().enumerate() {
            n += 1;
            if n > MAX_LINES {
                report.error = Some("access log too long to verify".into());
                break 'files;
            }
            let Ok(line) = line else {
                report.error = Some(format!("{} line {}: unreadable", p.display(), i + 1));
                break 'files;
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(e) = serde_json::from_str::<Line>(&line) else {
                report.error = Some(format!("{} line {}: malformed", p.display(), i + 1));
                break 'files;
            };
            if prev.as_ref().is_some_and(|prev| &e.body.prev != prev) {
                report.error = Some(format!("{} line {}: chain broken", p.display(), i + 1));
                break 'files;
            }
            if entry_hash(&e.body) != e.hash {
                report.error = Some(format!("{} line {}: altered", p.display(), i + 1));
                break 'files;
            }
            prev = Some(e.hash);
            all.push_back(AccessRecord {
                at_ms: e.body.ts_ms,
                via: e.body.via,
                who: e.body.who,
                what: e.body.what,
            });
            if all.len() > limit.max(1) * 8 {
                all.pop_front();
            }
        }
    }
    report.recent = all.into_iter().rev().take(limit).collect();
    report
}

/// The distinct callers connected at `now_ms` (newest first, each as its
/// latest connection): a connection ([`AccessKind::Connection`]) in the
/// last [`ACTIVE_WINDOW_MS`], or one in the last [`SESSION_WINDOW_MS`]
/// followed by calls of any kind with no gap longer than
/// [`ACTIVE_WINDOW_MS`] up to now (a stream kept open, with presence).
/// Background probes, thumbnails and refusals alone never count.
pub fn active(recent: &[AccessRecord], now_ms: u64) -> Vec<&AccessRecord> {
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for r in recent {
        let key = (r.via.as_str(), r.who.as_str());
        if !r.kind().is_connection() || seen.contains(&key) {
            continue;
        }
        let age = now_ms.saturating_sub(r.at_ms);
        if age > SESSION_WINDOW_MS {
            continue;
        }
        let live = age <= ACTIVE_WINDOW_MS || {
            // The caller's calls since then (newest first, refusals
            // aside) must leave no gap longer than the active window.
            let mut last = now_ms;
            let mut ok = true;
            for c in recent.iter().filter(|c| {
                (c.via.as_str(), c.who.as_str()) == key
                    && c.at_ms >= r.at_ms
                    && c.kind() != AccessKind::Refused
            }) {
                if last.saturating_sub(c.at_ms) > ACTIVE_WINDOW_MS {
                    ok = false;
                    break;
                }
                last = c.at_ms;
            }
            ok && last.saturating_sub(r.at_ms) <= ACTIVE_WINDOW_MS
        };
        if live {
            seen.insert(key);
            out.push(r);
        }
    }
    out
}

/// Whether a relay-reported client (account `id`, its presence since
/// `since_ms`) made a connection on this machine: one since its presence
/// began, or in the last [`ACTIVE_WINDOW_MS`]. `None` when the log has
/// nothing from that account at all (no log, or a driver that does not
/// write one): the relay's word stands then.
pub fn relay_client_connected(
    recent: &[AccessRecord],
    id: &str,
    since_ms: u64,
    now_ms: u64,
) -> Option<bool> {
    let suffix = format!("({id})");
    let mut theirs = recent
        .iter()
        .filter(|r| r.via == "relay" && (r.who == id || r.who.ends_with(&suffix)))
        .peekable();
    theirs.peek()?;
    Some(theirs.any(|r| {
        r.kind().is_connection()
            && (r.at_ms + 5_000 >= since_ms || now_ms.saturating_sub(r.at_ms) <= ACTIVE_WINDOW_MS)
    }))
}

#[cfg(test)]
const TEST_GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Writes a valid chained log of `(ts_ms, via, who, what)` rows (tests).
#[cfg(test)]
pub(crate) fn write_log(path: &Path, rows: &[(u64, &str, &str, &str)]) {
    let mut prev = TEST_GENESIS.to_string();
    use std::io::Write as _;
    let mut f = std::fs::File::create(path).unwrap();
    for (i, (ts, via, who, what)) in rows.iter().enumerate() {
        let body = Body {
            seq: i as u64 + 1,
            ts_ms: *ts,
            via: (*via).into(),
            who: (*who).into(),
            what: (*what).into(),
            prev: prev.clone(),
        };
        let hash = entry_hash(&body);
        let mut v = serde_json::to_value(&body).unwrap();
        v["hash"] = hash.clone().into();
        writeln!(f, "{v}").unwrap();
        prev = hash;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// The same fixture `cua-spacesd-server`'s writer pins.
    #[test]
    fn canonical_hash_matches_the_driver() {
        let body = Body {
            seq: 1,
            ts_ms: 1_700_000_000_000,
            via: "relay".into(),
            who: "ada@example.test".into(),
            what: "ProcessService".into(),
            prev: GENESIS.into(),
        };
        assert_eq!(
            entry_hash(&body),
            "3fa4bad8455fa8b9fd268f97865102bb7c638b1b8a6f8a20c75d8742e9793659"
        );
    }

    #[test]
    fn reads_newest_first_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        assert_eq!(read(&path, 5), AccessReport::default(), "no log yet");
        write_log(
            &path,
            &[
                (1_000, "token", "token", "SystemService"),
                (2_000, "relay", "Ada (acct-1)", "ProcessService"),
                (3_000, "viewer", "Bob (viewer:bob)", "DesktopService"),
            ],
        );
        let r = read(&path, 2);
        assert_eq!(r.error, None);
        assert_eq!(r.recent.len(), 2);
        assert_eq!(r.recent[0].who, "Bob (viewer:bob)");
        assert_eq!(r.recent[1].what, "ProcessService");
    }

    #[test]
    fn edits_and_deletions_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        write_log(
            &path,
            &[
                (1_000, "token", "token", "SystemService"),
                (2_000, "relay", "Ada (acct-1)", "ProcessService"),
                (3_000, "relay", "Eve (acct-2)", "FilesystemService"),
            ],
        );
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("Eve (acct-2)", "Ada (acct-1)")).unwrap();
        assert!(read(&path, 5).error.unwrap().contains("altered"));
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(read(&path, 5).error.unwrap().contains("chain broken"));
    }

    fn rec(at_ms: u64, via: &str, who: &str, what: &str) -> AccessRecord {
        AccessRecord {
            at_ms,
            via: via.into(),
            who: who.into(),
            what: what.into(),
        }
    }

    #[test]
    fn classifies_by_the_hosts_tags() {
        use AccessKind::*;
        for (who, what, want) in [
            ("Ada (a1)", "SystemService/Health (background)", Background),
            (
                "Ada (a1)",
                "HostSpacesService/GetHostSpaces (background)",
                Background,
            ),
            (
                "Ada (a1)",
                "PresenceService/UpdateCursor (background)",
                Background,
            ),
            (
                "Ada (a1)",
                "ComputerService/Screenshot (thumbnail)",
                OwnerThumbnail,
            ),
            (
                "Bob (b1)",
                "ComputerService/Screenshot (thumbnail, not owner)",
                Thumbnail,
            ),
            (
                "Ada (a1)",
                "refused ComputerService (desktop not shared)",
                Refused,
            ),
            ("refused", "bad assertion", Refused),
            ("Ada (a1)", "StreamService", Connection),
            ("Ada (a1)", "ProcessService", Connection),
            ("Ada (a1)", "FilesystemService", Connection),
            ("Ada (a1)", "TeleportService", Connection),
            ("Ada (a1)", "VolumeService", Connection),
            ("token", "MCP", Connection),
            ("Ada (a1)", "ComputerService", Connection),
            ("Ada (a1)", "SystemService/CreateViewerTicket", Connection),
            // An older driver's bare probe services are background.
            ("token", "SystemService", Background),
            ("token", "PresenceService", Background),
        ] {
            assert_eq!(classify(who, what), want, "{what}");
        }
    }

    #[test]
    fn background_alone_is_never_connected() {
        let now = 10_000_000;
        let rows = [
            rec(
                now - 1_000,
                "relay",
                "Mac mini (a1)",
                "SystemService/Health (background)",
            ),
            rec(
                now - 2_000,
                "relay",
                "Mac mini (a1)",
                "ComputerService/Screenshot (thumbnail)",
            ),
            rec(
                now - 3_000,
                "relay",
                "Mac mini (a1)",
                "refused ComputerService (desktop not shared)",
            ),
            rec(
                now - 4_000,
                "relay",
                "Bob (b1)",
                "ComputerService/Screenshot (thumbnail, not owner)",
            ),
        ];
        assert!(active(&rows, now).is_empty());
        // A user-started call does count.
        let mut rows = rows.to_vec();
        rows.insert(0, rec(now - 500, "relay", "Mac mini (a1)", "StreamService"));
        let a = active(&rows, now);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].what, "StreamService");
    }

    #[test]
    fn a_session_stays_connected_while_it_keeps_calling() {
        let now = 10_000_000;
        // A stream opened six minutes ago, presence every minute since.
        let mut rows: Vec<AccessRecord> = (0..6)
            .map(|m| {
                rec(
                    now - m * 60_000,
                    "relay",
                    "Ada (a1)",
                    "PresenceService/UpdateCursor (background)",
                )
            })
            .collect();
        rows.push(rec(now - 6 * 60_000, "relay", "Ada (a1)", "StreamService"));
        assert_eq!(active(&rows, now).len(), 1);
        // A gap longer than the active window ends it.
        rows.remove(2);
        rows.remove(2);
        assert!(active(&rows, now).is_empty());
        // So does the session window, however busy the poller.
        let rows: Vec<AccessRecord> = (0..20)
            .map(|m| {
                rec(
                    now - m * 60_000,
                    "relay",
                    "Ada (a1)",
                    "SystemService/Health (background)",
                )
            })
            .chain([rec(
                now - 20 * 60_000,
                "relay",
                "Ada (a1)",
                "ProcessService",
            )])
            .collect();
        assert!(active(&rows, now).is_empty());
    }

    #[test]
    fn relay_presence_needs_a_connection_in_the_log() {
        let now = 10_000_000;
        let polls = [rec(
            now - 1_000,
            "relay",
            "Mac mini (acct-1)",
            "SystemService/Health (background)",
        )];
        assert_eq!(
            relay_client_connected(&polls, "acct-1", now - 60_000, now),
            Some(false)
        );
        assert_eq!(
            relay_client_connected(&polls, "acct-2", now - 60_000, now),
            None
        );
        let used = [rec(
            now - 30_000,
            "relay",
            "Mac mini (acct-1)",
            "StreamService",
        )];
        assert_eq!(
            relay_client_connected(&used, "acct-1", now - 60_000, now),
            Some(true)
        );
        // An auto-connect opens a real session, so it counts like any other.
        let auto = [rec(now - 400_000, "relay", "acct-1", "ProcessService")];
        assert_eq!(
            relay_client_connected(&auto, "acct-1", now - 401_000, now),
            Some(true)
        );
    }

    #[test]
    fn active_is_recent_and_distinct() {
        let rows = [
            AccessRecord {
                at_ms: 1_000_000,
                via: "token".into(),
                who: "token".into(),
                what: "MCP".into(),
            },
            AccessRecord {
                at_ms: 990_000,
                via: "token".into(),
                who: "token".into(),
                what: "ProcessService".into(),
            },
            AccessRecord {
                at_ms: 1_000_000 - ACTIVE_WINDOW_MS - 1,
                via: "relay".into(),
                who: "Ada".into(),
                what: "MCP".into(),
            },
        ];
        let now = active(&rows, 1_000_000);
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].who, "token");
        assert!(active(&rows, 1_000_000 + ACTIVE_WINDOW_MS + 1).is_empty());
    }
}
