// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The whole Ask first flow against a real Linux Space (`CUA_E2E_REPO_PUSH=1`):
//! cua-spacesd runs in an alpine container with git, an agent member calls
//! `repo_push` through its bridge, the host measures the repository inside
//! the Space and posts the card into the thread, nothing reaches the remote
//! while the card is open (an agent's own answer does not release it), a
//! person's yes pushes the branch from the host to a local bare repository
//! (the developer-only remote base), and a second push that the person
//! declines sends nothing.
//!
//! | env | meaning |
//! |---|---|
//! | `CUA_SPACESD_STATIC` | the static Linux cua-spacesd to run |
//! | `DOCKER_HOST` | the engine (the `docker` command must be on `PATH`) |
//!
//! A second test (`CUA_E2E_SEED_HOME=<folder>`) makes a Cua home to look at in
//! the app: one push answered yes, one asking with a pull request, and the
//! Space left running. It is how the screenshots of the card are taken.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces_ext::SpacesPersistent as _;
use cua_spaces_ext::persistent::{AgentSpec, bridge};
use cua_spaces_ext::thread::{Answer, Author, Draft, EntryKind};
use serde_json::{Value, json};

fn docker(args: &[&str]) -> String {
    let out = std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("the docker command");
    assert!(
        out.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Removes the container however the test ends.
struct Container(String);
impl Drop for Container {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.0])
            .output();
    }
}

fn uniq() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
            % 0xffffff
    )
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A real Space with a repository in it and a local bare repository standing in for GitHub.
struct World {
    container: Container,
    home: std::path::PathBuf,
    _tmp: Option<tempfile::TempDir>,
    root: tempfile::TempDir,
    spaces: Spaces,
    info: cua_spaces::SpaceInfo,
    space: cua_spaces::Space,
    remote: std::path::PathBuf,
}

async fn world(home: Option<std::path::PathBuf>) -> World {
    let bin = std::path::PathBuf::from(
        std::env::var("CUA_SPACESD_STATIC").expect("set CUA_SPACESD_STATIC"),
    );
    let name = format!("cua-e2e-repo-push-{}", uniq());
    let container = Container(name.clone());
    let token = format!("tok-{}", uniq());
    docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "-p",
        "127.0.0.1::3211",
        "-e",
        &format!("CUA_ENV_TOKEN={token}"),
        "-v",
        &format!("{}:/cua-spacesd:ro", bin.display()),
        "alpine:3.20",
        "sh",
        "-c",
        "while true; do /cua-spacesd; sleep 1; done",
    ]);
    let addr = docker(&["port", &name, "3211"]);
    let addr = addr.lines().next().unwrap().to_string();

    // A local bare repository stands in for GitHub: the developer-only remote base.
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remotes/acme/app.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "-q", "."]);
    // SAFETY: the tests of this file run one at a time (nextest runs each in its own process).
    unsafe {
        std::env::set_var(
            "CUA_GITHUB_REMOTE_BASE",
            format!("file://{}", root.path().join("remotes").display()),
        );
    }

    let (tmp, home) = match home {
        Some(h) => (None, h),
        None => {
            let t = tempfile::tempdir().unwrap();
            let h = t.path().to_path_buf();
            (Some(t), h)
        }
    };
    let spaces = cua_spaces_ext::register(
        Spaces::builder()
            .home(&home)
            .download_dir(home.join("downloads"))
            .operator_display(Arc::new(cua_spaces::operator::NoDisplay)),
        cua_volume::Drive::open_local(&home),
        None,
    )
    .build();
    let info = spaces
        .add(&format!("http://{addr}"), Some(token), Some("push".into()))
        .await
        .expect("add the Space");
    let up = Instant::now() + Duration::from_secs(60);
    let space = loop {
        if let Ok(s) = spaces.space(&info.id).await
            && s.bash("true", Duration::from_secs(10))
                .await
                .is_ok_and(|o| o.success())
        {
            break s;
        }
        assert!(Instant::now() < up, "the Space did not answer");
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    // The agent's work: a repository with two commits, in the Space.
    let setup = space
        .bash(
            "set -e; apk add --no-cache git >/dev/null; mkdir -p /work && cd /work && \
             git init -q -b main . && git config user.email ada@example.com && git config user.name Ada && \
             echo one > a.txt && git add . && git commit -q -m 'Start' && \
             git checkout -q -b fix && echo two >> a.txt && echo new > b.txt && git add . && git commit -q -m 'Fix the retry'",
            Duration::from_secs(180),
        )
        .await
        .expect("set up the repository");
    assert!(setup.success(), "{}{}", setup.stdout, setup.stderr);
    World {
        container,
        home,
        _tmp: tmp,
        root,
        spaces,
        info,
        space,
        remote,
    }
}

/// What the tests drive: the agent's bridge in the Space and the Space's thread.
struct Agent {
    server: Arc<cua_spaces::mcp::McpServer>,
    store: cua_spaces_ext::thread::ThreadStore,
}

impl Agent {
    fn new(w: &World) -> Agent {
        let p = w.spaces.persistent();
        let rec = p
            .create(AgentSpec {
                name: "ada".into(),
                harness: "claude-code".into(),
                space: w.info.id.clone(),
                ..Default::default()
            })
            .unwrap();
        Agent {
            server: Arc::new(bridge::server(&w.spaces, p.clone(), &rec, "run-00000001")),
            store: p.thread_store(&w.info.id),
        }
    }

    /// The agent calls `repo_push`; the call waits for the answer.
    fn push(&self, branch: &'static str, open_pr: bool) -> tokio::task::JoinHandle<Value> {
        let server = self.server.clone();
        tokio::spawn(async move {
            server
                .handle(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "repo_push", "arguments": {
                        "path": "/work", "repo": "acme/app", "branch": branch, "open_pr": open_pr,
                        "title": "Retry with a backoff", "body": "Waits 1s, 2s, 4s between reconnects.\n\nFixes the flaky sync test."}}}))
                .await
                .unwrap()
        })
    }

    /// The first approval after `after`.
    async fn card(&self, after: u64) -> cua_spaces_ext::thread::ThreadEntry {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            for e in self.store.read(after, 100).unwrap() {
                if e.kind == EntryKind::Approval {
                    return e;
                }
            }
            assert!(Instant::now() < until, "no card was posted");
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn answer(&self, author: Author, seq: u64, approve: bool) {
        let mut d = Draft::new(author, EntryKind::Answer, "");
        d.answer = Some(Answer {
            approval: seq,
            approve,
            note: String::new(),
            ..Default::default()
        });
        self.store.append(d).unwrap();
    }
}

fn result_of(reply: &Value) -> Value {
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap_or_else(|_| panic!("{reply}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approved_push_leaves_the_space_only_after_a_person_says_yes() {
    if std::env::var("CUA_E2E_REPO_PUSH").as_deref() != Ok("1") {
        eprintln!("skipping: set CUA_E2E_REPO_PUSH=1 to run");
        return;
    }
    let w = world(None).await;
    let agent = Agent::new(&w);

    // Yes.
    let first = agent.push("fix", false);
    let card = agent.card(0).await;
    assert_eq!(card.author.id, "system:host", "the host posts the card");
    let a = card.approval.as_ref().unwrap();
    assert_eq!(a.action, "export:repo.push");
    let measured = a.card.as_ref().expect("the measured card data");
    assert_eq!(measured["repo"], "acme/app");
    assert_eq!(measured["branch"], "fix");
    assert_eq!(measured["commitsTotal"], 1, "{measured}");
    assert_eq!(measured["filesTotal"], 2, "{measured}");
    // Nothing is on the remote while the card is open, and the agent's own yes is not an answer.
    agent.answer(Author::agent("ada"), card.seq, true);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!first.is_finished(), "an agent answered its own push");
    assert!(!w.remote.join("refs/heads/fix").exists());
    agent.answer(Author::owner(), card.seq, true);
    let reply = tokio::time::timeout(Duration::from_secs(90), first)
        .await
        .expect("the push finished")
        .unwrap();
    let done = result_of(&reply);
    assert_eq!(done["pushed"], true, "{done}");
    let head = w
        .space
        .bash("cd /work && git rev-parse HEAD", Duration::from_secs(20))
        .await
        .unwrap()
        .stdout
        .trim()
        .to_string();
    assert_eq!(git(&w.remote, &["rev-parse", "refs/heads/fix"]), head);

    // No: nothing is sent.
    let second = agent.push("later", false);
    let card2 = agent.card(card.seq).await;
    agent.answer(Author::owner(), card2.seq, false);
    let reply = tokio::time::timeout(Duration::from_secs(60), second)
        .await
        .expect("the decline finished")
        .unwrap();
    let declined = result_of(&reply);
    assert_eq!(declined["pushed"], false, "{declined}");
    assert!(!w.remote.join("refs/heads/later").exists());
    // The audit log records the ask, the answer and the push.
    let audit = std::fs::read_to_string(w.home.join("persistent/host-tools-audit.jsonl"))
        .unwrap_or_default();
    for word in ["ask", "pushed", "declined"] {
        assert!(audit.contains(word), "no {word} in the audit log: {audit}");
    }
    drop(w.root);
}

/// A Cua home to look at in the app: a push answered yes (one muted line) and a
/// push asking, with a pull request, left open. The Space keeps running; remove
/// its container (`cua-e2e-repo-push-*`) when done.
#[tokio::test(flavor = "multi_thread")]
async fn a_home_with_cards_to_look_at() {
    let Ok(dir) = std::env::var("CUA_E2E_SEED_HOME") else {
        eprintln!("skipping: set CUA_E2E_SEED_HOME=<folder> to make a home to look at");
        return;
    };
    let w = world(Some(std::path::PathBuf::from(dir))).await;
    let agent = Agent::new(&w);
    let first = agent.push("fix", false);
    let card = agent.card(0).await;
    agent.answer(Author::owner(), card.seq, true);
    let _ = tokio::time::timeout(Duration::from_secs(90), first).await;
    // The next push asks with a pull request: a sign-in from the environment, and the
    // addresses of GitHub's own API on a port nothing listens on (the card is what is looked at).
    // SAFETY: as above.
    unsafe {
        std::env::set_var("GH_TOKEN", "ghp_E2E_NOT_A_REAL_TOKEN");
        std::env::set_var("CUA_GITHUB_REMOTE_BASE", "http://127.0.0.1:9");
        std::env::set_var("CUA_GITHUB_API_BASE", "http://127.0.0.1:9");
    }
    let _second = agent.push("retry-backoff", true);
    let card2 = agent.card(card.seq + 1).await;
    eprintln!("card {} is open in {}", card2.seq, w.home.display());
    std::mem::forget(w.container);
    // Leave the card open for a moment so the home is complete on disk.
    tokio::time::sleep(Duration::from_secs(2)).await;
}
