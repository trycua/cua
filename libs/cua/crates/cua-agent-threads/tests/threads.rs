//! The lister and reader against made-up stores laid out as each agent lays
//! out its own (`tests/fixtures`, and a Hermes `state.db` built here).

use std::path::{Path, PathBuf};

use cua_agent_threads::{Agent, ListOptions, Role, Roots, history, recent};
use rusqlite::Connection;

const KETTLE: &str = "0b1e5c2a-1111-4222-8333-944445555666";
const GARDEN: &str = "77777777-aaaa-4bbb-8ccc-dddddddddddd";
const META_ONLY: &str = "99999999-0000-4000-8000-000000000000";
const COUPON: &str = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
const EXEC: &str = "1a2b3c4d-0000-4000-8000-aaaaaaaaaaaa";
const OLD: &str = "0ld0ld00-1111-4111-8111-111111111111";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(from, to).unwrap();
}

/// A home with all three agents' stores.
fn home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let h = d.path();
    let f = fixtures();
    for (id, slug) in [
        (KETTLE, "-Users-sam-code-kettle"),
        (GARDEN, "-Users-sam-code-garden-planner"),
        (META_ONLY, "-tmp"),
    ] {
        copy(
            &f.join(format!("claude/{id}.jsonl")),
            &h.join(format!(".claude/projects/{slug}/{id}.jsonl")),
        );
    }
    // A subagent's chat one level deeper is not a chat of its own.
    copy(
        &f.join(format!("claude/{GARDEN}.jsonl")),
        &h.join(format!(
            ".claude/projects/-Users-sam-code-garden-planner/{GARDEN}/subagents/agent-1.jsonl"
        )),
    );
    for (name, day) in [
        (
            format!("rollout-2026-11-03T10-00-00-{COUPON}.jsonl"),
            "2026/11/03",
        ),
        (
            format!("rollout-2026-11-04T08-00-00-{EXEC}.jsonl"),
            "2026/11/04",
        ),
        (
            format!("rollout-2025-06-01T12-00-00-{OLD}.jsonl"),
            "2025/06/01",
        ),
    ] {
        copy(
            &f.join("codex").join(&name),
            &h.join(".codex/sessions").join(day).join(&name),
        );
    }
    copy(
        &f.join("codex/session_index.jsonl"),
        &h.join(".codex/session_index.jsonl"),
    );
    hermes_db(&h.join(".hermes/state.db"), true);
    d
}

/// A Hermes store; `full` has the newer columns, otherwise only the oldest.
fn hermes_db(path: &Path, full: bool) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let c = Connection::open(path).unwrap();
    if full {
        c.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT, started_at REAL NOT NULL,
               ended_at REAL, cwd TEXT, title TEXT, last_activity_at REAL, archived INTEGER, hidden INTEGER, future_col BLOB);
             CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT,
               tool_calls TEXT, timestamp REAL NOT NULL, active INTEGER NOT NULL DEFAULT 1);
             INSERT INTO sessions VALUES ('20261103_201500_ab12cd', 'cli', NULL, 1793736900.0, NULL, '/Users/sam/notes', NULL, 1793740000.0, 0, 0, NULL);
             INSERT INTO sessions VALUES ('20261104_060000_cron01', 'cron', NULL, 1793772000.0, NULL, '/', 'Nightly digest', 1793772100.0, 0, 0, NULL);
             INSERT INTO sessions VALUES ('20261104_061000_child1', 'cli', '20261103_201500_ab12cd', 1793772600.0, NULL, '/', 'child', 1793772700.0, 0, 0, NULL);
             INSERT INTO sessions VALUES ('20261104_062000_hidden', 'cli', NULL, 1793773200.0, NULL, '/', 'hidden', 1793773300.0, 0, 1, NULL);
             INSERT INTO sessions VALUES ('20261101_090000_slack1', 'slack', NULL, 1793523600.0, NULL, NULL, 'Release checklist', NULL, 0, 0, NULL);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261103_201500_ab12cd', 'user', 'Summarise my notes on the bakery lease', 1793736901.0);
             INSERT INTO messages (session_id, role, content, tool_calls, timestamp) VALUES ('20261103_201500_ab12cd', 'assistant', NULL, '[{}]', 1793736902.0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261103_201500_ab12cd', 'tool', 'file contents', 1793736903.0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261103_201500_ab12cd', 'assistant', '[{\"type\":\"text\",\"text\":\"The lease renews in March.\"}]', 1793736904.0);
             INSERT INTO messages (session_id, role, content, timestamp, active) VALUES ('20261103_201500_ab12cd', 'user', 'a rewound message', 1793736905.0, 0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261104_060000_cron01', 'user', 'run the digest', 1793772001.0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261104_061000_child1', 'user', 'child work', 1793772601.0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261104_062000_hidden', 'user', 'hidden work', 1793773201.0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20261101_090000_slack1', 'user', 'what is left for the release?', 1793523601.0);",
        )
        .unwrap();
    } else {
        c.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, started_at REAL NOT NULL);
             CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT, timestamp REAL NOT NULL);
             INSERT INTO sessions VALUES ('20250101_000000_old001', 'cli', 1735689600.0);
             INSERT INTO messages (session_id, role, content, timestamp) VALUES ('20250101_000000_old001', 'user', 'hello from an old Hermes', 1735689601.0);",
        )
        .unwrap();
    }
}

fn all(limit: usize) -> ListOptions {
    ListOptions {
        agents: Agent::ALL.to_vec(),
        limit,
    }
}

#[test]
fn lists_every_agents_chats_newest_first() {
    let h = home();
    let list = recent(&Roots::under_home(h.path()), &all(50));
    let ids: Vec<&str> = list.iter().map(|t| t.id.as_str()).collect();
    // Hermes's cron, child and hidden sessions, Codex's exec run, a Claude
    // chat with nothing typed and a subagent's file are left out.
    assert_eq!(
        ids,
        vec![
            "20261103_201500_ab12cd",
            GARDEN,
            COUPON,
            KETTLE,
            "20261101_090000_slack1",
            OLD
        ]
    );

    let kettle = list.iter().find(|t| t.id == KETTLE).unwrap();
    assert_eq!(kettle.agent, Agent::ClaudeCode);
    // The name the person gave it wins over the generated one.
    assert_eq!(kettle.title, "Kettle timer on CI");
    assert_eq!(kettle.cwd.as_deref(), Some("/Users/sam/code/kettle"));
    assert_eq!(kettle.last_active_ms, 1_793_610_120_000);
    assert_eq!(kettle.resume, vec!["claude", "--resume", KETTLE]);

    // No title: the first prompt, on one line and cut.
    let garden = list.iter().find(|t| t.id == GARDEN).unwrap();
    assert!(garden.title.starts_with("Add a frost date column"));
    assert!(garden.title.chars().count() <= 80);

    let coupon = list.iter().find(|t| t.id == COUPON).unwrap();
    assert_eq!(
        (coupon.agent, coupon.title.as_str()),
        (Agent::Codex, "Checkout double charge")
    );
    assert_eq!(coupon.cwd.as_deref(), Some("/Users/sam/code/bike-shop"));
    assert_eq!(coupon.resume, vec!["codex", "resume", COUPON]);
    let old = list.iter().find(|t| t.id == OLD).unwrap();
    assert_eq!(old.title, "rename the config loader");

    let notes = list
        .iter()
        .find(|t| t.id == "20261103_201500_ab12cd")
        .unwrap();
    assert_eq!(
        (notes.agent, notes.title.as_str()),
        (Agent::Hermes, "Summarise my notes on the bakery lease")
    );
    assert_eq!(notes.cwd.as_deref(), Some("/Users/sam/notes"));
    assert_eq!(notes.last_active_ms, 1_793_740_000_000);
    let slack = list
        .iter()
        .find(|t| t.id == "20261101_090000_slack1")
        .unwrap();
    assert_eq!(slack.title, "Release checklist");
}

#[test]
fn limit_and_agent_filter() {
    let h = home();
    let roots = Roots::under_home(h.path());
    assert_eq!(recent(&roots, &all(2)).len(), 2);
    let only = recent(
        &roots,
        &ListOptions {
            agents: vec![Agent::Codex],
            limit: 4,
        },
    );
    assert!(only.iter().all(|t| t.agent == Agent::Codex));
    assert_eq!(only.len(), 2);
}

#[test]
fn missing_or_broken_stores_list_nothing() {
    let d = tempfile::tempdir().unwrap();
    assert!(recent(&Roots::under_home(d.path()), &all(4)).is_empty());
    // Not a database, a projects file where a folder should be.
    std::fs::create_dir_all(d.path().join(".hermes")).unwrap();
    std::fs::write(d.path().join(".hermes/state.db"), b"not sqlite").unwrap();
    std::fs::create_dir_all(d.path().join(".claude")).unwrap();
    std::fs::write(d.path().join(".claude/projects"), b"x").unwrap();
    assert!(recent(&Roots::under_home(d.path()), &all(4)).is_empty());
    assert!(recent(&Roots::default(), &all(4)).is_empty());
}

#[test]
fn an_older_hermes_without_the_newer_columns() {
    let d = tempfile::tempdir().unwrap();
    hermes_db(&d.path().join(".hermes/state.db"), false);
    let list = recent(&Roots::under_home(d.path()), &all(4));
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0].title.as_str(), list[0].cwd.as_deref()),
        ("hello from an old Hermes", None)
    );
    let h = history(
        &Roots::under_home(d.path()),
        Agent::Hermes,
        "20250101_000000_old001",
        None,
    )
    .unwrap();
    assert_eq!(h.messages.len(), 1);
}

#[test]
fn claude_history_is_what_was_said() {
    let h = home();
    let t = history(
        &Roots::under_home(h.path()),
        Agent::ClaudeCode,
        KETTLE,
        None,
    )
    .unwrap();
    let said: Vec<(Role, &str)> = t
        .messages
        .iter()
        .map(|m| (m.role, m.text.as_str()))
        .collect();
    // No meta, slash-command echo, thinking, tool call, tool result or
    // subagent; one reply split by a tool call reads as one.
    assert_eq!(
        said,
        vec![
            (
                Role::Person,
                "The kettle timer test fails on CI but passes locally, can you find out why?"
            ),
            (
                Role::Agent,
                "Let me look at the timer test.\n\nIt depends on the machine's time zone."
            ),
            (Role::Person, "Pin it to UTC then."),
            (Role::Agent, "Done: the test now sets TZ=UTC."),
        ]
    );
    assert_eq!(t.messages[0].key, "m2");
    assert_eq!(t.messages[0].ts_ms, Some(1_793_610_002_000));
    assert_eq!(t.earlier_omitted, 0);
    assert_eq!(t.thread.title, "Kettle timer on CI");
}

#[test]
fn history_keeps_the_newest_and_says_how_many_it_left_out() {
    let h = home();
    let t = history(
        &Roots::under_home(h.path()),
        Agent::ClaudeCode,
        KETTLE,
        Some(2),
    )
    .unwrap();
    assert_eq!(t.earlier_omitted, 2);
    assert_eq!(t.messages[0].text, "Pin it to UTC then.");
}

#[test]
fn codex_history_skips_instructions_and_duplicates() {
    let h = home();
    let roots = Roots::under_home(h.path());
    let t = history(&roots, Agent::Codex, COUPON, None).unwrap();
    let said: Vec<(Role, &str)> = t
        .messages
        .iter()
        .map(|m| (m.role, m.text.as_str()))
        .collect();
    assert_eq!(
        said,
        vec![
            (
                Role::Person,
                "Why does checkout double-charge when the coupon is applied twice?"
            ),
            (Role::Agent, "The coupon handler isn't idempotent."),
        ]
    );
    // An older rollout with only event messages.
    let old = history(&roots, Agent::Codex, OLD, None).unwrap();
    assert_eq!(old.messages.len(), 2);
    // A scripted exec run can't be opened as a chat.
    assert!(history(&roots, Agent::Codex, EXEC, None).is_err());
}

#[test]
fn hermes_history_is_active_user_and_assistant_text() {
    let h = home();
    let t = history(
        &Roots::under_home(h.path()),
        Agent::Hermes,
        "20261103_201500_ab12cd",
        None,
    )
    .unwrap();
    let said: Vec<(Role, &str)> = t
        .messages
        .iter()
        .map(|m| (m.role, m.text.as_str()))
        .collect();
    assert_eq!(
        said,
        vec![
            (Role::Person, "Summarise my notes on the bakery lease"),
            (Role::Agent, "The lease renews in March.")
        ]
    );
    assert_eq!(t.messages[0].ts_ms, Some(1_793_736_901_000));
}

#[test]
fn history_refuses_paths_and_unknown_ids() {
    let h = home();
    let roots = Roots::under_home(h.path());
    for bad in ["../../etc/passwd", "a/b", ""] {
        assert!(matches!(
            history(&roots, Agent::ClaudeCode, bad, None),
            Err(cua_agent_threads::Error::Invalid(_))
        ));
    }
    assert!(matches!(
        history(
            &roots,
            Agent::ClaudeCode,
            "00000000-0000-4000-8000-000000000001",
            None
        ),
        Err(cua_agent_threads::Error::NotFound { .. })
    ));
    // A file with nothing typed is not a chat either.
    assert!(history(&roots, Agent::ClaudeCode, META_ONLY, None).is_err());
}

#[test]
fn long_messages_are_cut() {
    let d = tempfile::tempdir().unwrap();
    let long = "é".repeat(10_000);
    let line = serde_json::json!({"type":"user","uuid":"u","timestamp":"2026-11-01T00:00:00Z","cwd":"/x","message":{"role":"user","content":long}});
    let p = d.path().join(".claude/projects/-x/abc.jsonl");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, format!("{line}\n")).unwrap();
    let t = history(&Roots::under_home(d.path()), Agent::ClaudeCode, "abc", None).unwrap();
    assert!(t.messages[0].cut);
    assert!(t.messages[0].text.len() <= cua_agent_threads::MESSAGE_MAX_BYTES);
}

/// A realistic history (hundreds of chats, a few of them huge) lists its newest
/// chats from their ends only (CUA-3548).
#[test]
fn listing_hundreds_of_chats_with_huge_files_stays_fast() {
    use std::io::Write;
    let home = tempfile::tempdir().unwrap();
    for i in 0..300 {
        let id = format!("00000000-0000-4000-8000-{i:012}");
        let path = home.path().join(format!(
            ".claude/projects/-Users-sam-p{}/{id}.jsonl",
            i % 20
        ));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        let rec = |n: usize, text: &str| {
            format!(
                "{{\"type\":\"user\",\"uuid\":\"u{n}\",\"cwd\":\"/k\",\"timestamp\":\"2026-11-04T10:00:{:02}Z\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}\n",
                n % 60
            )
        };
        f.write_all(rec(0, &format!("chat {i}")).as_bytes())
            .unwrap();
        if i % 50 == 0 {
            // 300 MB of the middle that must never be read.
            f.set_len(300 * 1024 * 1024).unwrap();
            f.seek_end_and_write(rec(1, "last words").as_bytes());
        }
    }
    let t = std::time::Instant::now();
    let roots = Roots::under_home(home.path());
    let got = recent(
        &roots,
        &ListOptions {
            agents: Agent::ALL.to_vec(),
            limit: 50,
        },
    );
    assert_eq!(got.len(), 50);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        t.elapsed()
    );
}

trait SeekEnd {
    fn seek_end_and_write(&mut self, bytes: &[u8]);
}
impl SeekEnd for std::fs::File {
    fn seek_end_and_write(&mut self, bytes: &[u8]) {
        use std::io::{Seek, SeekFrom, Write};
        self.seek(SeekFrom::End(0)).unwrap();
        self.write_all(bytes).unwrap();
    }
}

/// The Ready screen asks for the chosen agents only (the Codex chats of a
/// person who picked Claude Code stay out), and every host asks the same way.
#[test]
fn only_the_chosen_agents_are_listed() {
    let h = home();
    let roots = Roots::under_home(h.path());
    let list = |agents: Vec<Agent>| recent(&roots, &ListOptions { agents, limit: 10 });
    let claude = list(vec![Agent::ClaudeCode]);
    assert!(!claude.is_empty());
    assert!(claude.iter().all(|t| t.agent == Agent::ClaudeCode));
    let codex = list(vec![Agent::Codex]);
    assert!(!codex.is_empty());
    assert!(codex.iter().all(|t| t.agent == Agent::Codex));
    let both = list(vec![Agent::Codex, Agent::ClaudeCode, Agent::Codex]);
    assert_eq!(both.len(), claude.len() + codex.len());
    assert!(
        both.windows(2)
            .all(|w| w[0].last_active_ms >= w[1].last_active_ms)
    );
    assert!(list(vec![]).is_empty());
}

/// A store of years of Codex chats is read from its newest day folders only,
/// and a chat taken up again within the week before still shows.
#[test]
fn codex_reads_its_newest_days_only() {
    use std::time::{Duration, SystemTime};
    let d = tempfile::tempdir().unwrap();
    let sessions = d.path().join(".codex/sessions");
    let rollout = |day: &str, n: u32| {
        let id = format!("01a00000-0000-7000-8000-{n:012}");
        let p = sessions.join(day).join(format!(
            "rollout-{}T10-00-00-{id}.jsonl",
            day.replace('/', "-")
        ));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"source\":\"cli\"}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"task {n}\"}}]}}}}\n"
            ),
        )
        .unwrap();
        (p, id)
    };
    let mut n = 0;
    let mut days = vec![];
    for m in 1..=12 {
        for day in 1..=28 {
            days.push(format!("2025/{m:02}/{day:02}"));
        }
    }
    for day in &days {
        for _ in 0..3 {
            n += 1;
            rollout(day, n);
        }
    }
    // An old chat (start of the year) taken up again now: outside the week back, it is not read.
    let (old, _) = rollout("2025/01/01", 9_000);
    // One started 3 day folders back and taken up again now: it leads.
    let (resumed, resumed_id) = rollout("2025/12/25", 9_001);
    let now = SystemTime::now();
    for p in [&old, &resumed] {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(now + Duration::from_secs(60))
            .unwrap();
    }
    let roots = Roots::under_home(d.path());
    let got = recent(
        &roots,
        &ListOptions {
            agents: vec![Agent::Codex],
            limit: 4,
        },
    );
    assert_eq!(got.len(), 4);
    assert_eq!(got[0].id, resumed_id);
    assert!(
        got.iter()
            .all(|t| t.id != "01a00000-0000-7000-8000-000000009000")
    );
}

#[test]
fn a_cached_listing_answers_again_without_reading() {
    let h = home();
    let roots = Roots::under_home(h.path());
    let opts = ListOptions {
        agents: vec![Agent::ClaudeCode],
        limit: 4,
    };
    let first = cua_agent_threads::recent_cached(&roots, &opts);
    assert!(!first.is_empty());
    std::fs::remove_dir_all(h.path().join(".claude")).unwrap();
    assert_eq!(cua_agent_threads::recent_cached(&roots, &opts), first);
    // Another set of agents is its own listing.
    let codex = cua_agent_threads::recent_cached(
        &roots,
        &ListOptions {
            agents: vec![Agent::Codex],
            limit: 4,
        },
    );
    assert!(codex.iter().all(|t| t.agent == Agent::Codex));
}
