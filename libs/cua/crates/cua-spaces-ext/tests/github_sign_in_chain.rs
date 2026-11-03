// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The push an owner approved, signed in through the whole chain: a fake `gh`
//! on PATH is asked for its token only after a person says yes, the token
//! reaches the fake GitHub API as the pull request's credential and appears
//! nowhere else, and a decline reads nothing and sends nothing. The remote is
//! a local folder and the API a local server; no real account is involved.
//! The default test in this binary sets the process environment, so the
//! opt-in real-repository test below is run alone.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cua_spaces_ext::persistent::host_tools::{
    Binding, Decision, Endpoints, GithubConnection, Measured, PushLimits, PushRequest, Released,
    card, release,
};

const TOKEN: &str = "gho_FAKE_github.com_ada_SECRET";

fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// What a card of the repository at `work` is bound to (one commit; git's own counts).
fn binding_of(work: &Path) -> Binding {
    let head = git(work, &["rev-parse", "HEAD"]);
    let has_parent = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD~1"])
        .current_dir(work)
        .output()
        .unwrap()
        .status
        .success();
    let base = if has_parent {
        git(work, &["rev-parse", "HEAD~1"])
    } else {
        git(work, &["hash-object", "-t", "tree", "/dev/null"])
    };
    let stat = git(
        work,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--diff-algorithm=myers",
            "--shortstat",
            &base,
            &head,
        ],
    );
    let n = |word: &str| -> u64 {
        stat.split(',')
            .find(|p| p.contains(word))
            .and_then(|p| p.split_whitespace().next())
            .and_then(|x| x.parse().ok())
            .unwrap_or(0)
    };
    let range = if has_parent {
        format!("{base}..{head}")
    } else {
        head.clone()
    };
    Binding {
        commits: git(work, &["rev-list", "--count", &range]).parse().unwrap(),
        files: n("file"),
        additions: n("insertion"),
        deletions: n("deletion"),
        head,
        base,
    }
}

async fn fake_api(seen: Arc<Mutex<Vec<String>>>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let n = c.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = if req.starts_with("GET") {
                r#"{"default_branch":"main"}"#
            } else {
                r#"{"html_url":"https://github.com/acme/demo/pull/9"}"#
            };
            seen.lock().unwrap().push(req);
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = c.write_all(resp.as_bytes()).await;
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn gh_is_asked_only_after_a_yes_and_its_token_goes_nowhere_but_the_pull_request() {
    let root = tempfile::tempdir().unwrap();
    let remotes = root.path().join("remotes");
    let remote = remotes.join("acme/demo.git");
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "--bare", "-q", "."]);
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main", "."]);
    std::fs::write(work.join("a.txt"), "hi").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "fix"]);
    let head = git(&work, &["rev-parse", "HEAD"]);
    let bundle = root.path().join("x.bundle");
    git(
        &work,
        &["bundle", "create", bundle.to_str().unwrap(), "HEAD"],
    );

    // A fake gh: one account, and a log of every call.
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = root.path().join("gh.log");
    let script = format!(
        r#"#!/bin/sh
echo "$*" >> "{log}"
case "$1 $2" in
  "auth status") echo '{{"hosts":{{"github.com":[{{"state":"success","active":true,"host":"github.com","login":"ada","scopes":"repo"}}]}}}}'; exit 0;;
  "auth token") echo "{TOKEN}"; exit 0;;
esac
[ "$1" = "--version" ] && echo "gh version 2.101.0 (2026-09-15)" && exit 0
exit 2
"#,
        log = log.display()
    );
    let gh = bin.join("gh");
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();

    // GitHub's address is rewritten to the local folder, by git itself.
    let gitconfig = root.path().join("gitconfig");
    std::fs::write(
        &gitconfig,
        format!(
            "[url \"file://{}/\"]\n\tinsteadOf = http://github.fake.test/\n",
            remotes.display()
        ),
    )
    .unwrap();

    let seen = Arc::new(Mutex::new(vec![]));
    let api = fake_api(seen.clone()).await;
    // SAFETY: the only test in this binary; nothing else reads the environment meanwhile.
    unsafe {
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        std::env::set_var("HOME", root.path());
        std::env::set_var("GIT_CONFIG_GLOBAL", &gitconfig);
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("GITHUB_TOKEN");
    }

    let calls = || std::fs::read_to_string(&log).unwrap_or_default();
    // The addresses are passed in (a developer-only seam, `dev-remotes`, which this
    // crate's tests turn on): no environment variable moves an approved push.
    let endpoints = Endpoints::custom(
        "ssh://git@github.fake.test",
        "http://github.fake.test",
        &api,
    );
    let conn = tokio::task::spawn_blocking(move || {
        GithubConnection::from_host_with("github.com", endpoints)
    })
    .await
    .unwrap();
    // The card names the account; gh was asked who is signed in, never for a token.
    assert_eq!(conn.account(false), "ada, signed in with gh");
    assert!(conn.can_open_pr(false));
    assert!(calls().contains("auth status"), "{}", calls());
    assert!(!calls().contains("auth token"), "{}", calls());

    let req = PushRequest {
        repo: "acme/demo".into(),
        host: "github.com".into(),
        branch: "fix/it".into(),
        title: "Fix it".into(),
        body: "why".into(),
        open_pr: true,
        base: None,
        ssh_remote: false,
    };
    let fetch = || async { Ok((tempfile::tempdir()?, bundle.clone())) };
    let binding = binding_of(&work);
    let limits = PushLimits::default();

    // Not now: nothing is read, fetched or sent.
    let out = release(
        Decision::Declined("later".into()),
        &conn,
        &req,
        &binding,
        &limits,
        fetch,
    )
    .await
    .unwrap();
    assert!(matches!(out, Released::Declined(_)));
    assert!(
        !calls().contains("auth token"),
        "a decline read the token: {}",
        calls()
    );
    assert!(!remote.join("refs/heads/fix/it").exists());
    assert!(seen.lock().unwrap().is_empty());

    // Push and open: gh gives the token now, once.
    let out = release(Decision::Approved, &conn, &req, &binding, &limits, fetch)
        .await
        .unwrap();
    let Released::Pushed(done) = out else {
        panic!("not pushed")
    };
    assert_eq!(
        done.pull_request.as_deref(),
        Some("https://github.com/acme/demo/pull/9")
    );
    assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix/it"]), head);
    assert_eq!(calls().matches("auth token").count(), 1, "{}", calls());
    // The token is the pull request's credential, and appears in nothing the host answers.
    let api_calls = seen.lock().unwrap().join("\n");
    assert!(
        api_calls.contains(&format!("Bearer {TOKEN}"))
            || api_calls
                .to_lowercase()
                .contains(&format!("bearer {}", TOKEN.to_lowercase())),
        "{api_calls}"
    );
    assert!(!format!("{done:?}").contains(TOKEN));
    assert!(!format!("{conn:?}").contains(TOKEN));
    // gh was never given a sign-in variable (it answers for its own login).
    assert!(!std::env::vars().any(|(_, v)| v.contains(TOKEN)));
    // The bare remote keeps no credential.
    let config = std::fs::read_to_string(remote.join("config")).unwrap();
    assert!(!config.contains(TOKEN));
}

/// Opt-in, against a real repository the signed-in `gh` account may push to:
/// `CUA_REAL_GITHUB_DEMO=owner/repo cargo test -p cua-spaces-ext --test
/// github_sign_in_chain -- --ignored real_repository_round_trip`. It clones
/// the (public) repository, pushes a throwaway branch `demo/test-<random>`
/// through the whole chain after a person's yes, opens a pull request, then
/// always closes the pull request and deletes the branch. It prints the
/// account and links, never a token. Run it with an isolated HOME and `gh`
/// reachable on PATH; nothing but the one throwaway branch and its pull
/// request is touched.
#[tokio::test]
#[ignore = "talks to a real GitHub repository"]
async fn real_repository_round_trip() {
    let Ok(repo) = std::env::var("CUA_REAL_GITHUB_DEMO") else {
        eprintln!("set CUA_REAL_GITHUB_DEMO=owner/repo to run this");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let suffix: u32 = rand::random();
    let branch = format!("demo/test-{suffix:08x}");
    let work = root.path().join("work");
    let o = std::process::Command::new("git")
        .args([
            "clone",
            "-q",
            &format!("https://github.com/{repo}.git"),
            work.to_str().unwrap(),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    std::fs::write(
        work.join(format!(".cua-test-{suffix:08x}")),
        "throwaway branch from a Cua test; safe to delete\n",
    )
    .unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "Cua test: throwaway commit"]);
    let head = git(&work, &["rev-parse", "HEAD"]);
    let bundle = root.path().join("x.bundle");
    git(
        &work,
        &["bundle", "create", bundle.to_str().unwrap(), "HEAD"],
    );

    let conn = tokio::task::spawn_blocking(|| GithubConnection::from_host("github.com"))
        .await
        .unwrap();
    assert!(
        conn.can_open_pr(false),
        "no GitHub sign-in on this computer: {}",
        conn.account(false)
    );
    println!("ACCOUNT: {}", conn.account(false));
    let req = PushRequest {
        repo: repo.clone(),
        host: "github.com".into(),
        branch: branch.clone(),
        title: "Cua test: throwaway pull request (closed automatically)".into(),
        body: "Opened by a Cua test through the host sign-in chain, then closed. Safe to ignore."
            .into(),
        open_pr: true,
        base: None,
        ssh_remote: false,
    };
    let (text, details) = card(
        "hermes",
        &req,
        &Measured {
            head: head.clone(),
            stat: "1 file changed, 1 insertion(+)".into(),
            files: 1,
            ..Default::default()
        },
        "release-checks",
        &conn.account(false),
        conn.can_open_pr(false),
    );
    println!("CARD: {text} | {details}");
    let result = release(
        Decision::Approved,
        &conn,
        &req,
        &binding_of(&work),
        &PushLimits::default(),
        || async { Ok((tempfile::tempdir()?, bundle.clone())) },
    )
    .await;

    // Always clean up: close the pull request, delete the branch (the token is read again, for this only).
    let cleanup = async {
        let t = tokio::task::spawn_blocking(|| {
            cua_spaces::agents::github_auth::global().token("github.com")
        })
        .await
        .unwrap()
        .expect("sign-in for cleanup");
        let http = reqwest::Client::builder()
            .user_agent("cua-spaces-test")
            .build()
            .unwrap();
        let mut closed = None;
        if let Ok(Released::Pushed(p)) = &result
            && let Some(url) = &p.pull_request
            && let Some(n) = url.rsplit('/').next()
        {
            let r = http
                .patch(format!("https://api.github.com/repos/{repo}/pulls/{n}"))
                .bearer_auth(t.token.expose())
                .json(&serde_json::json!({"state": "closed"}))
                .send()
                .await
                .unwrap();
            closed = Some(r.status().as_u16());
        }
        let d = http
            .delete(format!(
                "https://api.github.com/repos/{repo}/git/refs/heads/{branch}"
            ))
            .bearer_auth(t.token.expose())
            .send()
            .await
            .unwrap();
        let gone = http
            .get(format!(
                "https://api.github.com/repos/{repo}/git/ref/heads/{branch}"
            ))
            .bearer_auth(t.token.expose())
            .send()
            .await
            .unwrap()
            .status()
            .as_u16();
        (closed, d.status().as_u16(), gone)
    }
    .await;
    println!("BRANCH: {branch} PUSHED COMMIT: {head}");
    if let Ok(Released::Pushed(p)) = &result {
        println!("PULL REQUEST: {:?}", p.pull_request);
    }
    println!(
        "CLEANUP: pull request close status {:?}, branch delete status {}, branch lookup after delete {}",
        cleanup.0, cleanup.1, cleanup.2
    );
    assert!(matches!(result, Ok(Released::Pushed(_))), "{result:?}");
    assert_eq!(cleanup.1, 204, "the branch was not deleted");
    assert_eq!(cleanup.2, 404, "the branch is still there");
    assert_eq!(cleanup.0, Some(200), "the pull request was not closed");
}
