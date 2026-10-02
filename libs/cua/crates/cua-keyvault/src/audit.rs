// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The append-only, hash-chained audit log.
//!
//! One JSON line per event. Each line carries the SHA-256 of the previous
//! line's hash plus its own canonical body (`hash`), and an HMAC-SHA256 of
//! that hash under a key derived from the vault master key (`mac`). So:
//!
//! - editing, reordering or deleting a line in the middle breaks the chain;
//! - recomputing the chain after an edit needs the MAC key, which only the
//!   unlocked vault has;
//! - truncating the tail is caught by the head anchor (sequence and hash)
//!   stored inside the encrypted vault metadata.
//!
//! Events that happen while the vault is locked (a refused unlock, a caller
//! rejected before unlock) are still chained but carry no MAC; verification
//! reports them as unauthenticated rather than hiding them.
//!
//! Entries never contain secret values: only ids, labels, caller identity,
//! targets and decisions.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto::{SecretKey, sha256};
use crate::{Error, Result};

/// The genesis "previous hash".
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Upper bound on lines read when verifying or listing (a tampered, huge
/// file must not exhaust memory).
pub const MAX_LINES: usize = 1_000_000;
/// Upper bound on one line's length.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// What happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Event kind (`vault.create`, `vault.unlock`, `item.import`,
    /// `item.delete`, `consent.request`, `consent.allow`, `consent.deny`,
    /// `grant.revoke`, `teleport.deliver`, `teleport.deny`, `target.wipe`,
    /// `rule.add`, `rule.remove`, `vault.disable`, `vault.enable`,
    /// `caller.reject`, `login.request`, `login.authorize`, `login.fill`,
    /// `login.denied`).
    pub kind: String,
    /// Who asked: the verified caller's display identity.
    #[serde(default)]
    pub actor: String,
    /// The caller fingerprint (stable id of the verified identity).
    #[serde(default)]
    pub caller_fp: String,
    /// Item id, when one is involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Target Space, when one is involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `allow`, `deny`, `ok` or `error`.
    pub decision: String,
    /// Free text (reason). Never a secret value.
    #[serde(default)]
    pub detail: String,
}

/// One stored line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// 1-based sequence number.
    pub seq: u64,
    /// Unix milliseconds.
    pub ts_ms: u64,
    /// The event.
    #[serde(flatten)]
    pub event: AuditEvent,
    /// Previous entry's `hash` (or [`GENESIS`]).
    pub prev: String,
    /// Written while the vault was locked, so it legitimately carries no MAC.
    /// Part of the hashed body (see [`Canonical`]), so an attacker cannot flip
    /// a MAC'd entry into a "locked" one, and cannot strip the MAC from an
    /// authenticated (`authless == false`) entry without the hash catching it
    /// (red-team F15).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub authless: bool,
    /// SHA-256 over `prev` and this entry's canonical body.
    pub hash: String,
    /// HMAC-SHA256 of `hash` under the audit key; empty while locked.
    #[serde(default)]
    pub mac: String,
}

#[derive(Serialize)]
struct Canonical<'a> {
    seq: u64,
    ts_ms: u64,
    event: &'a AuditEvent,
    prev: &'a str,
    authless: bool,
}

fn entry_hash(seq: u64, ts_ms: u64, event: &AuditEvent, prev: &str, authless: bool) -> String {
    let body = serde_json::to_vec(&Canonical {
        seq,
        ts_ms,
        event,
        prev,
        authless,
    })
    .expect("serializable");
    let mut buf = Vec::with_capacity(prev.len() + 1 + body.len());
    buf.extend_from_slice(prev.as_bytes());
    buf.push(b'\n');
    buf.extend_from_slice(&body);
    hex::encode(sha256(&buf))
}

/// The chain head: what the vault metadata anchors.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// Last sequence number (0 when empty).
    pub seq: u64,
    /// Last hash ([`GENESIS`] when empty).
    pub hash: String,
}

impl Head {
    fn genesis() -> Self {
        Self {
            seq: 0,
            hash: GENESIS.to_string(),
        }
    }
}

/// Why verification failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tamper {
    /// The first bad line (1-based line number).
    pub line: u64,
    /// What is wrong.
    pub reason: String,
}

/// The result of verifying a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// Entries read.
    pub entries: u64,
    /// Entries without a MAC (written while locked).
    pub unauthenticated: u64,
    /// The head.
    pub head: Head,
    /// The first problem, if any.
    pub tamper: Option<Tamper>,
}

impl Verification {
    /// True when the chain is intact.
    pub fn ok(&self) -> bool {
        self.tamper.is_none()
    }
}

/// An audit log file.
pub struct AuditLog {
    path: PathBuf,
    head: Head,
}

impl AuditLog {
    /// Opens (or creates) the log at `path` and reads its head. The file is
    /// created 0600.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if !path.exists() {
            crate::store::write_private(&path, b"")?;
        }
        let v = verify_file(&path, None, None)?;
        // Opening never refuses a damaged log: appends keep chaining from the
        // last readable line and verification keeps reporting the damage.
        Ok(Self { path, head: v.head })
    }

    /// The current head.
    pub fn head(&self) -> &Head {
        &self.head
    }

    /// Appends an event. `key` is the audit MAC key (`None` while locked).
    pub fn append(&mut self, event: AuditEvent, key: Option<&SecretKey>) -> Result<AuditEntry> {
        let seq = self.head.seq + 1;
        let ts_ms = crate::now_ms();
        let prev = self.head.hash.clone();
        let authless = key.is_none();
        let hash = entry_hash(seq, ts_ms, &event, &prev, authless);
        let mac = key
            .map(|k| hex::encode(k.mac(hash.as_bytes())))
            .unwrap_or_default();
        let entry = AuditEntry {
            seq,
            ts_ms,
            event,
            prev,
            authless,
            hash: hash.clone(),
            mac,
        };
        let mut line = serde_json::to_vec(&entry).expect("serializable");
        line.push(b'\n');
        if line.len() > MAX_LINE_BYTES {
            return Err(Error::Invalid("audit entry too large".into()));
        }
        let mut f = OpenOptions::new().append(true).open(&self.path)?;
        f.write_all(&line)?;
        f.sync_data()?;
        self.head = Head { seq, hash };
        Ok(entry)
    }

    /// Reads the most recent `limit` entries (newest last).
    pub fn tail(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        let mut out = std::collections::VecDeque::with_capacity(limit.min(4096));
        for (i, line) in read_lines(&self.path)?.enumerate() {
            if i >= MAX_LINES {
                break;
            }
            let Ok(line) = line else { break };
            if let Ok(e) = serde_json::from_str::<AuditEntry>(&line) {
                if out.len() == limit {
                    out.pop_front();
                }
                if limit > 0 {
                    out.push_back(e);
                }
            }
        }
        Ok(out.into())
    }

    /// Verifies the whole file.
    pub fn verify(&self, key: Option<&SecretKey>, anchor: Option<&Head>) -> Result<Verification> {
        verify_file(&self.path, key, anchor)
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn read_lines(path: &Path) -> Result<impl Iterator<Item = std::io::Result<String>>> {
    let f = std::fs::File::open(path)?;
    Ok(BufReader::new(f).lines())
}

/// Verifies a log file. With `key`, every MAC present must verify. With
/// `anchor`, the chain must reach that head (catches tail truncation) and
/// may extend beyond it.
pub fn verify_file(
    path: &Path,
    key: Option<&SecretKey>,
    anchor: Option<&Head>,
) -> Result<Verification> {
    let mut head = Head::genesis();
    let mut entries = 0u64;
    let mut unauthenticated = 0u64;
    let mut anchored = anchor.is_none_or(|a| a.seq == 0);
    let mut tamper = None;
    for (i, line) in read_lines(path)?.enumerate() {
        let lineno = i as u64 + 1;
        if i >= MAX_LINES {
            tamper = Some(Tamper {
                line: lineno,
                reason: "log exceeds the line bound".into(),
            });
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(_) => {
                tamper = Some(Tamper {
                    line: lineno,
                    reason: "unreadable line".into(),
                });
                break;
            }
        };
        if line.len() > MAX_LINE_BYTES {
            tamper = Some(Tamper {
                line: lineno,
                reason: "line too long".into(),
            });
            break;
        }
        let e: AuditEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => {
                tamper = Some(Tamper {
                    line: lineno,
                    reason: "malformed entry".into(),
                });
                break;
            }
        };
        let fail = |reason: &str| {
            Some(Tamper {
                line: lineno,
                reason: reason.to_string(),
            })
        };
        if e.seq != head.seq + 1 {
            tamper = fail("sequence gap or reorder");
            break;
        }
        if e.prev != head.hash {
            tamper = fail("previous-hash link broken");
            break;
        }
        // Only entries chained to (at or before) the sealed head are part of
        // the verified log. An attacker with write access can append
        // well-formed lines after the anchor; refuse them (red-team F15).
        if let Some(a) = anchor
            && e.seq > a.seq
        {
            tamper = fail("entry appended after the sealed head (injected past the anchor)");
            break;
        }
        if entry_hash(e.seq, e.ts_ms, &e.event, &e.prev, e.authless) != e.hash {
            tamper = fail("entry hash mismatch (edited)");
            break;
        }
        if e.authless {
            // A legitimately locked entry never carries a MAC. A MAC on one is
            // structurally impossible for the daemon to produce, so it is
            // tamper (red-team F15). Otherwise count it as unauthenticated.
            if !e.mac.is_empty() {
                tamper = fail("a locked (unauthenticated) entry must carry no MAC");
                break;
            }
            unauthenticated += 1;
        } else if e.mac.is_empty() {
            // An authenticated entry always had a MAC written. An empty MAC
            // here means it was stripped (which without `authless` in the hash
            // would silently pass as merely unauthenticated) (red-team F15).
            tamper = fail("MAC stripped from an authenticated entry");
            break;
        } else if let Some(k) = key {
            // Only the canonical (lowercase) hex the writer produces: hex
            // decoding ignores case, so an edited line would otherwise
            // verify.
            let canonical = e
                .mac
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            let ok = canonical
                && hex::decode(&e.mac)
                    .map(|tag| k.verify_mac(e.hash.as_bytes(), &tag))
                    .unwrap_or(false);
            if !ok {
                tamper = fail("MAC mismatch (rewritten without the vault key)");
                break;
            }
        }
        head = Head {
            seq: e.seq,
            hash: e.hash.clone(),
        };
        entries += 1;
        if let Some(a) = anchor
            && a.seq == e.seq
        {
            if a.hash != e.hash {
                tamper = fail("does not match the anchored head");
                break;
            }
            anchored = true;
        }
    }
    if tamper.is_none() && !anchored {
        tamper = Some(Tamper {
            line: entries + 1,
            reason: format!(
                "log ends at {} but the vault anchored {} (truncated)",
                head.seq,
                anchor.map(|a| a.seq).unwrap_or(0)
            ),
        });
    }
    Ok(Verification {
        entries,
        unauthenticated,
        head,
        tamper,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ev(kind: &str) -> AuditEvent {
        AuditEvent {
            kind: kind.into(),
            actor: "Cua CLI".into(),
            caller_fp: "fp".into(),
            item: Some("item-1".into()),
            target: None,
            decision: "ok".into(),
            detail: String::new(),
        }
    }

    fn log_with(n: usize, key: &SecretKey) -> (tempfile::TempDir, AuditLog) {
        let dir = tempfile::tempdir().unwrap();
        let mut log = AuditLog::open(dir.path().join("audit.log")).unwrap();
        for i in 0..n {
            log.append(ev(&format!("e{i}")), Some(key)).unwrap();
        }
        (dir, log)
    }

    fn rewrite(path: &Path, f: impl FnOnce(&mut Vec<String>)) {
        let text = std::fs::read_to_string(path).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        f(&mut lines);
        let mut out = lines.join("\n");
        out.push('\n');
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn intact_log_verifies_and_reopens_at_its_head() {
        let key = SecretKey::generate().unwrap();
        let (_d, log) = log_with(5, &key);
        let v = log.verify(Some(&key), Some(log.head())).unwrap();
        assert!(v.ok(), "{v:?}");
        assert_eq!(v.entries, 5);
        let again = AuditLog::open(log.path()).unwrap();
        assert_eq!(again.head(), log.head());
        assert_eq!(log.tail(2).unwrap().len(), 2);
        assert_eq!(log.tail(2).unwrap()[1].event.kind, "e4");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(log.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn edits_deletions_reorders_and_truncation_are_detected() {
        let key = SecretKey::generate().unwrap();
        let (_d, log) = log_with(5, &key);
        let anchor = log.head().clone();
        let path = log.path().to_path_buf();
        let pristine = std::fs::read_to_string(&path).unwrap();

        rewrite(&path, |l| {
            l[2] = l[2].replace("\"decision\":\"ok\"", "\"decision\":\"deny\"")
        });
        let v = verify_file(&path, Some(&key), Some(&anchor)).unwrap();
        assert_eq!(v.tamper.unwrap().line, 3);

        std::fs::write(&path, &pristine).unwrap();
        rewrite(&path, |l| {
            l.remove(1);
        });
        assert!(!verify_file(&path, Some(&key), Some(&anchor)).unwrap().ok());

        std::fs::write(&path, &pristine).unwrap();
        rewrite(&path, |l| l.swap(1, 2));
        assert!(!verify_file(&path, Some(&key), Some(&anchor)).unwrap().ok());

        std::fs::write(&path, &pristine).unwrap();
        rewrite(&path, |l| l.truncate(3));
        let v = verify_file(&path, Some(&key), Some(&anchor)).unwrap();
        assert!(v.tamper.unwrap().reason.contains("truncated"));
        // Without the anchor, a clean truncation is invisible: this is why the
        // head lives in the sealed metadata.
        assert!(verify_file(&path, Some(&key), None).unwrap().ok());
    }

    #[test]
    fn a_rechained_forgery_without_the_key_fails_the_mac() {
        let key = SecretKey::generate().unwrap();
        let (_d, log) = log_with(3, &key);
        let path = log.path().to_path_buf();
        // The attacker rebuilds a whole valid-looking chain with their own key.
        std::fs::write(&path, b"").unwrap();
        let mut forged = AuditLog::open(&path).unwrap();
        let attacker = SecretKey::generate().unwrap();
        for i in 0..3 {
            forged
                .append(ev(&format!("fake{i}")), Some(&attacker))
                .unwrap();
        }
        let v = verify_file(&path, Some(&key), None).unwrap();
        assert!(v.tamper.unwrap().reason.contains("MAC"));
    }

    #[test]
    fn stripping_the_mac_of_an_authenticated_entry_is_tamper() {
        // Red-team F15: before the fix, blanking a MAC'd entry's `mac` made it
        // look like a legitimately-locked (unauthenticated) line, and
        // verification stayed green. Now an authenticated entry with an empty
        // MAC is tamper.
        let key = SecretKey::generate().unwrap();
        let (_d, log) = log_with(4, &key);
        let anchor = log.head().clone();
        let path = log.path().to_path_buf();
        let pristine = std::fs::read_to_string(&path).unwrap();

        // Rename the "mac" field on line 2 so serde drops it (mac defaults to
        // empty) while every hashed field stays intact.
        rewrite(&path, |l| l[1] = l[1].replace("\"mac\":", "\"nac\":"));
        let v = verify_file(&path, Some(&key), Some(&anchor)).unwrap();
        assert!(!v.ok(), "a stripped MAC must be detected: {v:?}");
        assert_eq!(v.tamper.as_ref().unwrap().line, 2);

        // Even without the key (as `AuditLog::open` verifies), the structural
        // "authenticated entry has no MAC" is caught.
        assert!(!verify_file(&path, None, None).unwrap().ok());

        // Blanking the value the same way is also caught.
        std::fs::write(&path, &pristine).unwrap();
        rewrite(&path, |l| {
            let start = l[1].find("\"mac\":\"").unwrap() + 7;
            let end = l[1][start..].find('"').unwrap() + start;
            l[1].replace_range(start..end, "");
        });
        assert!(!verify_file(&path, Some(&key), Some(&anchor)).unwrap().ok());
    }

    #[test]
    fn entries_injected_after_the_sealed_head_are_refused() {
        // Red-team F15: a `~/.cua` writer appends well-formed, correctly-chained
        // lines after the anchored head (with empty MACs). The chain still links
        // and the SHA-256 recomputes, but they are past the sealed head, so
        // verification must refuse them rather than count them as merely
        // unauthenticated.
        let key = SecretKey::generate().unwrap();
        let (_d, log) = log_with(3, &key);
        let anchor = log.head().clone();
        // The vault vouches for seq 3; the attacker appends seq 4 and 5.
        let path = log.path().to_path_buf();
        let mut forger = AuditLog::open(&path).unwrap();
        forger.append(ev("attacker.note"), None).unwrap();
        forger.append(ev("attacker.note"), None).unwrap();
        assert!(forger.head().seq > anchor.seq);
        let v = verify_file(&path, Some(&key), Some(&anchor)).unwrap();
        assert!(!v.ok(), "post-anchor injection must be refused: {v:?}");
        assert!(
            v.tamper
                .as_ref()
                .unwrap()
                .reason
                .contains("after the sealed head")
        );
        // The genuine, anchored prefix on its own still verifies clean.
        assert_eq!(v.entries, 3);
    }

    #[test]
    fn a_mac_in_another_case_is_tampering() {
        let key = SecretKey::generate().unwrap();
        let (_d, log) = log_with(2, &key);
        let anchor = log.head().clone();
        let path = log.path().to_path_buf();
        let text = std::fs::read_to_string(&path).unwrap();
        let first: AuditEntry = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let upper = first.mac.to_ascii_uppercase();
        assert_ne!(upper, first.mac, "the MAC has a hex letter to flip");
        std::fs::write(&path, text.replacen(&first.mac, &upper, 1)).unwrap();
        let v = verify_file(&path, Some(&key), Some(&anchor)).unwrap();
        assert!(!v.ok(), "an edited MAC must not verify: {v:?}");
        assert!(v.tamper.as_ref().unwrap().reason.contains("MAC mismatch"));
    }

    #[test]
    fn locked_entries_are_chained_but_flagged() {
        let key = SecretKey::generate().unwrap();
        let (_d, mut log) = log_with(1, &key);
        log.append(ev("caller.reject"), None).unwrap();
        log.append(ev("e"), Some(&key)).unwrap();
        let v = log.verify(Some(&key), Some(log.head())).unwrap();
        assert!(v.ok());
        assert_eq!(v.unauthenticated, 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        /// Any single-byte change to any line is detected (or makes the
        /// line unparseable, which is also detected).
        #[test]
        fn any_byte_flip_is_detected(n in 2usize..6, line_pick in any::<usize>(), pos in any::<usize>(), byte in 0x20u8..0x7f) {
            let key = SecretKey::generate().unwrap();
            let (_d, log) = log_with(n, &key);
            let anchor = log.head().clone();
            let path = log.path().to_path_buf();
            let text = std::fs::read_to_string(&path).unwrap();
            let mut lines: Vec<Vec<u8>> = text.lines().map(|l| l.as_bytes().to_vec()).collect();
            let li = line_pick % lines.len();
            let pi = pos % lines[li].len();
            prop_assume!(lines[li][pi] != byte);
            lines[li][pi] = byte;
            let mut out = Vec::new();
            for l in &lines { out.extend_from_slice(l); out.push(b'\n'); }
            std::fs::write(&path, &out).unwrap();
            let v = verify_file(&path, Some(&key), Some(&anchor)).unwrap();
            // A flip inside JSON whitespace-insensitive spots can still parse
            // to the identical entry; then nothing changed semantically.
            let reparsed: Vec<Option<AuditEntry>> = lines.iter()
                .map(|l| serde_json::from_slice(l).ok()).collect();
            let original: Vec<AuditEntry> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
            let same = reparsed.iter().zip(&original).all(|(a, b)| a.as_ref() == Some(b));
            prop_assert!(same || !v.ok());
        }
    }
}
