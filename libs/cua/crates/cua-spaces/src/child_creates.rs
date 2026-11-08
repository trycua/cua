// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The creates an agent asks of its host, as the thread shows them.
//!
//! A Space an agent makes from inside a Space (its spawn channel) is made by
//! the host, which can take minutes (an image pull, a clone, a boot) and can
//! fail. The host records each such create here as it goes (started, which
//! step it is on, ready or failed with a plain reason), under the Space that
//! asked. The thread's supervisor reads the record and writes the thread
//! notes every client draws as one row per create, the way it does for a
//! subagent. Nothing here depends on the runtime that makes the Space.
//!
//! Kept in `<home>/spawn/creates.json`, one entry per create, newest last,
//! at most [`KEEP`] per asking Space.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file, in the Spaces home.
pub const FILE: &str = "spawn/creates.json";

/// Entries kept for one asking Space.
pub const KEEP: usize = 50;

/// Serializes this process's threads; [`Guard`] adds the lock other processes honour.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Exclusive access to the ledger for one read-modify-write: a mutex for this process's threads and an
/// advisory file lock (`spawn/creates.lock`) for every other process (the daemon, the app, the CLI), so no
/// process's update is lost to another's. Released on drop.
struct Guard {
    _file: Option<std::fs::File>,
    _thread: std::sync::MutexGuard<'static, ()>,
}

fn locked(home: &Path) -> Guard {
    let thread = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = (|| {
        let p = home.join("spawn/creates.lock");
        std::fs::create_dir_all(p.parent()?).ok()?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(p)
            .ok()?;
        f.lock().ok()?;
        Some(f)
    })();
    Guard {
        _file: file,
        _thread: thread,
    }
}

/// Where a create is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Creating,
    Ready,
    Failed,
}

/// One create an agent asked of the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// This create's own id, unique across creates (two under one name are two rows). Empty in records
    /// written before it existed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// The agent member that asked, once a thread's supervisor attributed it (see [`claim`]); empty before.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// The thread task that agent was on when it asked; 0 when none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub task: u64,
    /// The asking Space (the parent).
    pub parent: String,
    /// The create's own key: the name the Space is made under.
    pub key: String,
    /// The name people see.
    pub name: String,
    /// `macOS`, `Windows`, `Linux` or empty.
    #[serde(default)]
    pub os: String,
    pub state: State,
    /// The step in plain words ("Cloning the disk"); empty before the first.
    #[serde(default)]
    pub phase: String,
    /// The step as the SDK names it (`pulling`, `booting`, ...), for apps that word it themselves.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    /// How far through the step, 0 to 100, when it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// The Space's id once it exists.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub space: String,
    /// Why it failed, in plain words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// The person removed its row from the list (Remove from list): apps do not draw it again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

fn load(home: &Path) -> Vec<Entry> {
    std::fs::read(path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(home: &Path, all: &[Entry]) -> std::io::Result<()> {
    let p = path(home);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(all).unwrap_or_default())?;
    std::fs::rename(&tmp, &p)
}

/// The plain words for a create's step (the phases of
/// `CreateSpaceProgress`), for `os` ("macOS", "Windows", "Linux" or empty).
pub fn phase_words(phase: &str, os: &str) -> String {
    let what = if os.is_empty() {
        "the Space".to_string()
    } else {
        format!("the {os} Space")
    };
    match phase {
        "preparing" => format!("Preparing {what}"),
        "pulling" => "Downloading the image".to_string(),
        "creating" => format!("Cloning the disk for {what}"),
        "booting" => "Starting it".to_string(),
        "waiting_for_services" | "connecting" => "Waiting for it to be ready".to_string(),
        "ready" => "Ready".to_string(),
        _ => String::new(),
    }
}

/// The OS a create's image runs: `macOS`, `Windows` or `Linux`.
pub fn os_of_image(image: &str) -> &'static str {
    let l = image.to_lowercase();
    if l.contains("macos") || l.contains("mac-") || l.contains("sequoia") || l.contains("tahoe") {
        "macOS"
    } else if l.contains("windows") || l.contains("win11") || l.contains("win10") {
        "Windows"
    } else {
        "Linux"
    }
}

/// A create's failure for the person: the reason without the plumbing
/// (status prefixes, port numbers, ids), in one sentence.
pub fn person_reason(message: &str) -> String {
    let m = message.trim();
    let m = m
        .strip_prefix("invalid request: ")
        .or_else(|| m.strip_prefix("failed precondition: "))
        .unwrap_or(m);
    let m = m
        .strip_prefix("local runtime: ")
        .unwrap_or(m)
        .trim_start_matches("container engine: ")
        .trim();
    let l = m.to_lowercase();
    let out = if l.contains("pull access denied")
        || l.contains("repository does not exist")
        || l.contains("manifest unknown")
    {
        "the image was not found, or it needs a sign-in to download".to_string()
    } else if l.contains("stopped while booting") {
        "it stopped while starting up".to_string()
    } else if l.contains("probe port") || l.contains("is not published") {
        "its network setup did not come up".to_string()
    } else if l.contains("not enough disk space") {
        "there is not enough disk space".to_string()
    } else {
        let first = m
            .split(['\n', ';'])
            .next()
            .unwrap_or(m)
            .trim()
            .trim_end_matches('.');
        first.chars().take(160).collect()
    };
    let mut c = out.chars();
    match c.next() {
        Some(f) => format!("{}{}", f.to_uppercase(), c.as_str()),
        None => "It could not be made".into(),
    }
}

/// Records a new create `key` of `parent` and returns its id. Every call is its own create: two under one
/// name are two rows, each updated by its own id ([`update_id`]).
pub fn start(home: &Path, parent: &str, key: &str, name: &str, os: &str, now_ms: u64) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "{now_ms}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let _g = locked(home);
    let mut all = load(home);
    // Clients tell a row by its name and start time: no two creates share both.
    let mut now_ms = now_ms;
    while all.iter().any(|e| e.key == key && e.started_ms == now_ms) {
        now_ms += 1;
    }
    all.push(Entry {
        id: id.clone(),
        agent: String::new(),
        task: 0,
        parent: parent.into(),
        key: key.into(),
        name: name.into(),
        os: os.into(),
        state: State::Creating,
        phase: String::new(),
        code: String::new(),
        percent: None,
        space: String::new(),
        error: String::new(),
        started_ms: now_ms,
        ended_ms: None,
        hidden: false,
    });
    bound(&mut all, parent);
    let _ = save(home, &all);
    id
}

/// Changes the create `id` (read, change and write under the cross-process lock). Nothing happens when
/// it is gone.
pub fn update_id(home: &Path, id: &str, f: impl FnOnce(&mut Entry)) {
    let _g = locked(home);
    let mut all = load(home);
    if let Some(e) = all.iter_mut().find(|e| e.id == id) {
        f(e);
        let _ = save(home, &all);
    }
}

/// Attributes the creates of `parent` nobody has claimed yet to `agent` and its task `task`: the first
/// member of a Space to see a create is the one working on it, and only that member tells its thread
/// (compare-and-set under the lock, so two members never both claim one).
pub fn claim(home: &Path, parent: &str, agent: &str, task: u64) {
    let _g = locked(home);
    let mut all = load(home);
    let mut changed = false;
    for e in all
        .iter_mut()
        .filter(|e| e.parent == parent && e.agent.is_empty())
    {
        e.agent = agent.into();
        e.task = task;
        changed = true;
    }
    if changed {
        let _ = save(home, &all);
    }
}

/// Bounds what is kept for `parent`.
fn bound(all: &mut Vec<Entry>, parent: &str) {
    let mine = all.iter().filter(|e| e.parent == parent).count();
    if mine > KEEP {
        let mut drop = mine - KEEP;
        all.retain(|e| {
            if drop > 0 && e.parent == parent && e.state != State::Creating {
                drop -= 1;
                false
            } else {
                true
            }
        });
    }
}

/// Drops the records whose asking Space is gone (`present` holds every Space the registry lists): nothing can use
/// them, and the apps must not draw a placeholder for a parent that no longer exists. A create still going is kept.
/// Returns how many were dropped.
pub fn retain_parents(home: &Path, present: &std::collections::BTreeSet<String>) -> usize {
    let _g = locked(home);
    let mut all = load(home);
    let before = all.len();
    all.retain(|e| {
        e.state == State::Creating || e.parent.is_empty() || present.contains(&e.parent)
    });
    let dropped = before - all.len();
    if dropped > 0 {
        let _ = save(home, &all);
    }
    dropped
}

/// Every create the apps show as a row (the ones nobody removed from the list), oldest first.
pub fn visible(home: &Path) -> Vec<Entry> {
    load(home).into_iter().filter(|e| !e.hidden).collect()
}

/// Hides the finished create `key` that started at `started_ms` (the person removed its row).
pub fn hide(home: &Path, key: &str, started_ms: u64) {
    let _g = locked(home);
    let mut all = load(home);
    for e in all
        .iter_mut()
        .filter(|e| e.key == key && e.started_ms == started_ms)
    {
        e.hidden = true;
    }
    let _ = save(home, &all);
}

/// The creates `parent` asked for, oldest first.
pub fn of(home: &Path, parent: &str) -> Vec<Entry> {
    load(home)
        .into_iter()
        .filter(|e| e.parent == parent)
        .collect()
}

/// A create that was still going when its host process died is failed:
/// nothing will finish it.
pub fn fail_orphans(home: &Path, now_ms: u64) {
    let _g = locked(home);
    let mut all = load(home);
    let mut changed = false;
    for e in all.iter_mut().filter(|e| e.state == State::Creating) {
        e.state = State::Failed;
        e.error = "The computer restarted before it finished".into();
        e.ended_ms = Some(now_ms);
        changed = true;
    }
    if changed {
        let _ = save(home, &all);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records (or changes) the create `key` of `parent` that is still going, or starts one.
    fn update(
        home: &Path,
        parent: &str,
        key: &str,
        name: &str,
        os: &str,
        now_ms: u64,
        f: impl FnOnce(&mut Entry),
    ) {
        let going = load(home)
            .into_iter()
            .find(|e| e.parent == parent && e.key == key && e.state == State::Creating);
        let id = match going {
            Some(e) => e.id,
            None => start(home, parent, key, name, os, now_ms),
        };
        update_id(home, &id, f);
    }

    #[test]
    fn a_create_goes_through_its_steps_to_ready() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        update(h, "local:home", "mac", "mac", "macOS", 10, |e| {
            e.phase = phase_words("creating", "macOS")
        });
        update(h, "local:home", "mac", "mac", "macOS", 11, |e| {
            e.phase = phase_words("booting", "macOS")
        });
        update(h, "local:home", "mac", "mac", "macOS", 12, |e| {
            e.state = State::Ready;
            e.space = "local:mac".into();
            e.phase = "Ready".into();
            e.ended_ms = Some(12);
        });
        let all = of(h, "local:home");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, State::Ready);
        assert_eq!(all[0].started_ms, 10);
        assert_eq!(all[0].space, "local:mac");
        assert!(of(h, "local:other").is_empty());
    }

    #[test]
    fn a_failure_keeps_its_plain_reason_and_the_next_create_is_its_own_row() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        update(h, "p", "a", "a", "", 1, |e| {
            e.state = State::Failed;
            e.error = "No disk space".into();
        });
        update(h, "p", "a", "a", "", 2, |_| {});
        let all = of(h, "p");
        assert_eq!(all.len(), 2, "a retry under the same name is a new row");
        assert_eq!(all[0].state, State::Failed);
        assert_eq!(all[1].state, State::Creating);
    }

    #[test]
    fn a_create_cut_off_by_a_restart_is_failed_not_left_running() {
        let d = tempfile::tempdir().unwrap();
        update(d.path(), "p", "a", "a", "", 1, |_| {});
        fail_orphans(d.path(), 9);
        let e = &of(d.path(), "p")[0];
        assert_eq!(e.state, State::Failed);
        assert!(e.error.contains("restarted"));
    }

    #[test]
    fn a_removed_row_is_not_shown_again_but_the_thread_still_has_it() {
        let d = tempfile::tempdir().unwrap();
        update(d.path(), "p", "a", "a", "", 7, |e| e.state = State::Failed);
        assert_eq!(visible(d.path()).len(), 1);
        hide(d.path(), "a", 7);
        assert!(visible(d.path()).is_empty());
        assert_eq!(of(d.path(), "p").len(), 1);
    }

    #[test]
    fn records_of_a_removed_parent_are_dropped_and_nothing_else_is() {
        let d = tempfile::tempdir().unwrap();
        update(d.path(), "local:gone", "a", "a", "", 1, |e| {
            e.state = State::Failed
        });
        update(d.path(), "local:gone", "b", "b", "", 2, |_| {});
        update(d.path(), "local:here", "c", "c", "", 3, |e| {
            e.state = State::Failed
        });
        let present: std::collections::BTreeSet<String> = ["local:here".to_string()].into();
        assert_eq!(
            retain_parents(d.path(), &present),
            1,
            "the failed one goes; one still being made stays"
        );
        let keys: Vec<_> = visible(d.path()).into_iter().map(|e| e.key).collect();
        assert_eq!(keys, ["b", "c"]);
    }

    #[test]
    fn reasons_and_images_read_plainly() {
        assert_eq!(os_of_image("ghcr.io/trycua/macos:26"), "macOS");
        assert_eq!(os_of_image("windows-11"), "Windows");
        assert_eq!(os_of_image("linux:24.04"), "Linux");
        let r = person_reason("invalid request: probe port 3211 is not published by this sandbox");
        assert_eq!(r, "Its network setup did not come up");
        assert!(!r.chars().any(|c| c.is_ascii_digit()));
        let r = person_reason(
            "local runtime: container engine: pull nosuchimage-xyz:1: Docker responded with status code 404: pull access denied for nosuchimage-xyz",
        );
        assert_eq!(
            r,
            "The image was not found, or it needs a sign-in to download"
        );
    }

    #[test]
    fn steps_have_plain_words() {
        assert_eq!(
            phase_words("creating", "macOS"),
            "Cloning the disk for the macOS Space"
        );
        assert_eq!(
            phase_words("waiting_for_services", ""),
            "Waiting for it to be ready"
        );
    }

    #[test]
    fn two_creates_under_one_name_are_two_rows_each_updated_by_its_own_id() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let a = start(h, "p", "same", "same", "", 1);
        let b = start(h, "p", "same", "same", "", 1);
        assert_ne!(a, b);
        update_id(h, &b, |e| e.state = State::Failed);
        let all = of(h, "p");
        assert_eq!(
            (all[0].state, all[1].state),
            (State::Creating, State::Failed)
        );
    }

    #[test]
    fn a_create_is_claimed_once_by_the_first_member_to_see_it() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        start(h, "p", "a", "a", "", 1);
        claim(h, "p", "ada", 7);
        claim(h, "p", "bob", 9);
        let e = &of(h, "p")[0];
        assert_eq!((e.agent.as_str(), e.task), ("ada", 7));
    }

    #[test]
    fn updates_from_many_threads_and_processes_are_not_lost() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path().to_path_buf();
        let ids: Vec<String> = (0..8)
            .map(|i| start(&h, "p", &format!("k{i}"), "n", "", i))
            .collect();
        let threads: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let h = h.clone();
                std::thread::spawn(move || {
                    for n in 0..25u8 {
                        update_id(&h, &id, |e| e.percent = Some(n));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(of(&h, "p").iter().all(|e| e.percent == Some(24)));
    }

    #[test]
    fn the_ledger_is_locked_against_other_processes_while_it_is_changed() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let g = locked(h);
        // Another process opens its own handle to the lock file: it must not get the lock now.
        let other = std::fs::File::open(h.join("spawn/creates.lock")).unwrap();
        assert!(matches!(
            other.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(g);
        assert!(other.try_lock().is_ok());
    }
}
