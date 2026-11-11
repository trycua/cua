use super::*;
use std::collections::BTreeMap;

struct MapEnv(BTreeMap<String, String>);

impl Env for MapEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
}

fn env(pairs: &[(&str, &str)]) -> Box<MapEnv> {
    Box::new(MapEnv(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    ))
}

fn temp_keys() -> (AgentKeys, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "cua-github-auth-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    (
        AgentKeys::new(cua_auth::Store::TestKeychain(dir.clone())),
        dir,
    )
}

/// A fake `gh` on a private PATH: it answers `--version`, `auth status
/// --json hosts` and `auth token --hostname H [--user U]` from fixed
/// accounts, and writes every call (and whether a sign-in variable was in
/// its environment) to a log.
struct Fake {
    dir: tempfile::TempDir,
}

const ACCOUNTS: &str = r#"{"hosts":{
  "github.com":[
    {"state":"success","active":true,"host":"github.com","login":"ada","tokenSource":"keyring","scopes":"gist, read:org, repo","gitProtocol":"https"},
    {"state":"success","active":false,"host":"github.com","login":"bob","tokenSource":"keyring","scopes":"repo, workflow, admin:org","gitProtocol":"https"}
  ],
  "ghe.example.com":[
    {"state":"success","active":true,"host":"ghe.example.com","login":"ada-corp","tokenSource":"keyring","scopes":"repo","gitProtocol":"ssh"}
  ]}}"#;

impl Fake {
    fn new(version: &str, status_json: &str) -> Fake {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let script = format!(
            r#"#!/bin/sh
echo "$* GH_TOKEN=${{GH_TOKEN-unset}} GITHUB_TOKEN=${{GITHUB_TOKEN-unset}}" >> "{log}"
host=github.com; user=""
prev=""
for a in "$@"; do
  [ "$prev" = "--hostname" ] && host="$a"
  [ "$prev" = "--user" ] && user="$a"
  prev="$a"
done
case "$1" in
  --version) echo "gh version {version} (2026-09-15)"; exit 0;;
esac
if [ "$1 $2" = "auth status" ]; then
  case "$*" in
    *--json*) cat <<'JSON'
{status_json}
JSON
    exit 0;;
    *) cat >&2 <<'TXT'
github.com
  ✓ Logged in to github.com account ada (keyring)
  - Active account: true
  - Git operations protocol: https
  - Token: gho_************************************
  - Token scopes: 'gist', 'read:org', 'repo', 'workflow'
TXT
    exit 0;;
  esac
fi
if [ "$1 $2" = "auth token" ]; then
  if [ -z "$user" ]; then
    case "$host" in github.com) user=ada;; ghe.example.com) user=ada-corp;; esac
  fi
  case "$host/$user" in
    github.com/ada|github.com/bob|ghe.example.com/ada-corp) echo "gho_FAKE_${{host}}_${{user}}_SECRET"; exit 0;;
  esac
  echo "no oauth token found" >&2; exit 1
fi
exit 2
"#,
            log = dir.path().join("calls.log").display(),
        );
        let bin = dir.path().join("gh");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fake { dir }
    }

    fn gh(&self) -> Box<SystemGh> {
        Box::new(SystemGh::in_dirs(&[self.dir.path()]))
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }
}

fn auth<'k>(keys: &'k AgentKeys, f: &Fake, e: &[(&str, &str)]) -> GithubAuth<'k> {
    GithubAuth::new(keys, f.gh(), env(e))
}

#[test]
fn saved_token_wins_over_the_environment_and_gh() {
    let (keys, dir) = temp_keys();
    keys.set("other", Some(SAVED_KEY), "ghp_SAVED_SECRET")
        .unwrap();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[("GH_TOKEN", "ghp_ENV_SECRET")]);
    let r = a.token(DEFAULT_HOST).unwrap();
    assert_eq!(r.source, Source::Saved);
    assert_eq!(r.token.expose(), "ghp_SAVED_SECRET");
    // The card names the account without a token.
    let p = a.plan(DEFAULT_HOST).unwrap();
    assert_eq!(p.source, Source::Saved);
    assert!(p.account.contains("saved in Cua"), "{}", p.account);
    // gh was never asked for a token.
    assert!(!f.calls().contains("auth token"), "{}", f.calls());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn environment_token_wins_over_gh_and_github_token_is_the_second_name() {
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[("GITHUB_TOKEN", "ghp_ENV2_SECRET")]);
    let r = a.token(DEFAULT_HOST).unwrap();
    assert_eq!(
        (r.source, r.env_name.as_deref()),
        (Source::Env, Some("GITHUB_TOKEN"))
    );
    let a = auth(
        &keys,
        &f,
        &[
            ("GITHUB_TOKEN", "ghp_ENV2_SECRET"),
            ("GH_TOKEN", "ghp_ENV1_SECRET"),
        ],
    );
    let r = a.token(DEFAULT_HOST).unwrap();
    assert_eq!(r.env_name.as_deref(), Some("GH_TOKEN"));
    assert_eq!(r.token.expose(), "ghp_ENV1_SECRET");
    assert!(!f.calls().contains("auth token"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn gh_is_the_third_source_and_its_token_is_read_only_when_asked_for() {
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[]);
    // Planning and the status page read no token.
    let p = a.plan(DEFAULT_HOST).unwrap();
    assert_eq!((p.source, p.login.as_deref()), (Source::Gh, Some("ada")));
    assert_eq!(p.account, "ada, signed in with gh");
    let s = a.status();
    assert_eq!(s.hosts[0].line, "Signed in as ada with gh");
    assert!(!f.calls().contains("auth token"), "{}", f.calls());
    // The approved action reads it, for the account gh uses (no --user), with no sign-in variable.
    let r = a.token(DEFAULT_HOST).unwrap();
    assert_eq!(r.token.expose(), "gho_FAKE_github.com_ada_SECRET");
    let calls = f.calls();
    let line = calls.lines().find(|l| l.starts_with("auth token")).unwrap();
    assert!(
        line.contains("--hostname github.com") && !line.contains("--user"),
        "{line}"
    );
    assert!(
        line.contains("GH_TOKEN=unset") && line.contains("GITHUB_TOKEN=unset"),
        "{line}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_chosen_account_is_used_and_a_gone_one_is_never_replaced() {
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[]);
    let s = a.set_account(DEFAULT_HOST, Some("bob")).unwrap();
    let h = &s.hosts[0];
    assert_eq!(h.line, "Signed in as bob with gh");
    assert!(
        h.accounts
            .iter()
            .any(|x| x.login == "bob" && x.selected && x.chosen)
    );
    let r = a.token(DEFAULT_HOST).unwrap();
    assert_eq!(r.token.expose(), "gho_FAKE_github.com_bob_SECRET");
    assert!(f.calls().contains("--user bob"));
    // A login gh does not have cannot be chosen.
    assert!(a.set_account(DEFAULT_HOST, Some("mallory")).is_err());
    // The choice is kept for the next process; when the account goes, nothing else stands in.
    let f2 = Fake::new(
        "2.101.0",
        r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"ada","scopes":"repo"}]}}"#,
    );
    let b = auth(&keys, &f2, &[]);
    let e = b.token(DEFAULT_HOST).unwrap_err();
    assert!(
        matches!(e, Unavailable::PickedGone { ref login, .. } if login == "bob"),
        "{e:?}"
    );
    assert!(e.message().contains("bob") && e.message().contains("no longer signed in"));
    assert!(!f2.calls().contains("auth token"));
    // Back to gh's own account.
    let s = b.set_account(DEFAULT_HOST, None).unwrap();
    assert_eq!(s.hosts[0].line, "Signed in as ada with gh");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_host_can_be_switched_off_and_the_other_hosts_keep_working() {
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[]);
    // GitHub Enterprise is listed from what gh knows, github.com first.
    let s = a.status();
    let names: Vec<_> = s.hosts.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(names, ["github.com", "ghe.example.com"]);
    assert_eq!(s.hosts[1].line, "Signed in as ada-corp with gh");
    let s = a.set_enabled(DEFAULT_HOST, false).unwrap();
    assert!(!s.hosts[0].enabled && s.hosts[0].login.is_none());
    assert!(s.hosts[0].line.starts_with("Turned off"));
    assert!(matches!(a.plan(DEFAULT_HOST), Err(Unavailable::Off(_))));
    assert!(matches!(a.token(DEFAULT_HOST), Err(Unavailable::Off(_))));
    // Off beats a saved token too.
    keys.set("other", Some(SAVED_KEY), "ghp_SAVED_SECRET")
        .unwrap();
    assert!(matches!(a.token(DEFAULT_HOST), Err(Unavailable::Off(_))));
    let r = a.token("ghe.example.com").unwrap();
    assert_eq!(r.token.expose(), "gho_FAKE_ghe.example.com_ada-corp_SECRET");
    let s = a.set_enabled(DEFAULT_HOST, true).unwrap();
    assert!(s.hosts[0].enabled);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn enterprise_takes_gh_enterprise_token_only_for_gh_host() {
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", r#"{"hosts":{}}"#);
    let a = auth(
        &keys,
        &f,
        &[
            ("GH_ENTERPRISE_TOKEN", "ghe_ENT_SECRET"),
            ("GH_HOST", "ghe.example.com"),
            ("GH_TOKEN", "ghp_DOTCOM_SECRET"),
        ],
    );
    let r = a.token("ghe.example.com").unwrap();
    assert_eq!(r.env_name.as_deref(), Some("GH_ENTERPRISE_TOKEN"));
    assert_eq!(r.token.expose(), "ghe_ENT_SECRET");
    // GH_TOKEN is for github.com only; another Enterprise host gets nothing from the environment.
    assert!(matches!(
        a.token("other.example.com"),
        Err(Unavailable::Nothing(_))
    ));
    assert_eq!(
        a.token(DEFAULT_HOST).unwrap().token.expose(),
        "ghp_DOTCOM_SECRET"
    );
    // The Enterprise host is on the page even though only an environment token knows it.
    let s = a.status();
    assert!(
        s.hosts
            .iter()
            .any(|h| h.host == "ghe.example.com" && h.source == Some(Source::Env))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn nothing_signed_in_says_what_to_do() {
    let (keys, dir) = temp_keys();
    let empty = tempfile::tempdir().unwrap();
    let a = GithubAuth::new(
        &keys,
        Box::new(SystemGh::in_dirs(&[empty.path()])),
        env(&[]),
    );
    let s = a.status();
    assert!(!s.gh.found);
    assert_eq!(
        s.hosts[0].line,
        "No GitHub sign-in found: run gh auth login or paste a token"
    );
    let e = a.token(DEFAULT_HOST).unwrap_err();
    assert_eq!(
        e.message(),
        "No GitHub sign-in found: run gh auth login or paste a token"
    );
    assert_eq!(
        nothing_line("ghe.example.com"),
        "No sign-in found for ghe.example.com: run gh auth login --hostname ghe.example.com"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_old_gh_is_read_from_its_text_and_cannot_pick_another_account() {
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.40.1", "unused");
    let a = auth(&keys, &f, &[]);
    let s = a.status();
    assert!(s.gh.old);
    assert_eq!(s.hosts[0].line, "Signed in as ada with gh");
    assert_eq!(
        s.hosts[0].accounts[0].scopes,
        ["gist", "read:org", "repo", "workflow"]
    );
    assert!(
        s.hosts[0]
            .warning
            .as_deref()
            .unwrap()
            .contains("change GitHub Actions workflows")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn scopes_beyond_push_and_pull_requests_are_warned_about() {
    let l = |s: &str| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .collect::<Vec<_>>()
    };
    assert!(broader_scopes(&l("repo, read:org, gist")).is_empty());
    assert!(broader_scopes(&[]).is_empty());
    assert_eq!(
        broader_scopes(&l(
            "repo, workflow, admin:org, admin:public_key, delete_repo"
        )),
        [
            "change GitHub Actions workflows",
            "administer organizations and settings",
            "delete repositories"
        ]
    );
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[]);
    // ada has only repo-ish scopes: no warning; bob has workflow and admin:org.
    assert!(a.status().hosts[0].warning.is_none());
    let s = a.set_account(DEFAULT_HOST, Some("bob")).unwrap();
    let w = s.hosts[0].warning.clone().unwrap();
    assert!(
        w.contains("change GitHub Actions workflows and administer organizations and settings"),
        "{w}"
    );
    assert!(w.contains("only needs the repo scope"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_token_never_prints_and_is_scrubbed_from_messages() {
    let t = Token::new("ghp_SUPERSECRET");
    assert_eq!(format!("{t:?}"), "Token((hidden))");
    let header = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode("x-access-token:ghp_SUPERSECRET")
    };
    let s = t.scrub(&format!(
        "fatal: url ghp_SUPERSECRET and Authorization: Basic {header}"
    ));
    assert!(!s.contains("SUPERSECRET") && !s.contains(&header), "{s}");
    let r = Resolved {
        host: "github.com".into(),
        source: Source::Gh,
        login: Some("ada".into()),
        env_name: None,
        token: t,
    };
    assert!(!format!("{r:?}").contains("SUPERSECRET"));
}

#[test]
fn status_plan_and_errors_never_carry_a_token() {
    let (keys, dir) = temp_keys();
    keys.set("other", Some(SAVED_KEY), "ghp_SAVED_SECRET")
        .unwrap();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(
        &keys,
        &f,
        &[
            ("GH_TOKEN", "ghp_ENV_SECRET"),
            ("GH_ENTERPRISE_TOKEN", "ghe_ENT_SECRET"),
            ("GH_HOST", "ghe.example.com"),
        ],
    );
    let mut all = serde_json::to_string(&a.status()).unwrap();
    all += &serde_json::to_string(&a.plan(DEFAULT_HOST).unwrap()).unwrap();
    all += &a
        .set_enabled("ghe.example.com", false)
        .map(|r| serde_json::to_string(&r).unwrap())
        .unwrap();
    all += &a.token("ghe.example.com").unwrap_err().message();
    all += &a
        .set_account(DEFAULT_HOST, Some("mallory"))
        .unwrap_err()
        .to_string();
    for secret in ["SAVED_SECRET", "ENV_SECRET", "ENT_SECRET", "gho_FAKE"] {
        assert!(!all.contains(secret), "{secret} leaked into: {all}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_host_only_names_are_refused_for_a_space() {
    for n in [
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_DEPLOY_KEY",
    ] {
        assert!(crate::agents::HOST_ONLY_KEYS.contains(&n), "{n}");
        let e = crate::agents::refuse_host_only(n).unwrap_err();
        assert!(e.to_string().contains("never goes into a Space"));
    }
    // A run's env is built from saved keys and the host's environment: neither holds the sign-in.
    let (keys, dir) = temp_keys();
    keys.set("other", Some(SAVED_KEY), "ghp_SAVED_SECRET")
        .unwrap();
    let (env, _) =
        crate::agents::run_env_with(&keys, "claude-code", &BTreeMap::new(), &[], false).unwrap();
    assert!(!env.values().any(|v| v.contains("SAVED_SECRET")), "{env:?}");
    for n in ["GITHUB_TOKEN", "GH_TOKEN"] {
        assert!(crate::agents::env_from_host_with(&keys, &[n.to_string()]).is_err());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_app_methods_are_not_contract_tools_and_answer_json() {
    for m in APP_METHODS {
        assert!(
            cua_spaces_contract::tool(m).is_none(),
            "{m} is a contract tool"
        );
        assert!(crate::agents::keys::is_app_method(m));
    }
    let (keys, dir) = temp_keys();
    let f = Fake::new("2.101.0", ACCOUNTS);
    let a = auth(&keys, &f, &[]);
    let r = app_call(&a, "github.status", Value::Null).unwrap().unwrap();
    assert_eq!(r["hosts"][0]["line"], "Signed in as ada with gh");
    assert_eq!(r["hosts"][0]["source"], "gh");
    let r = app_call(
        &a,
        "github.use",
        serde_json::json!({"host": "github.com", "login": "bob"}),
    )
    .unwrap()
    .unwrap();
    assert_eq!(r["hosts"][0]["login"], "bob");
    let r = app_call(
        &a,
        "github.enable",
        serde_json::json!({"host": "github.com", "enabled": false}),
    )
    .unwrap()
    .unwrap();
    assert_eq!(r["hosts"][0]["enabled"], false);
    assert!(
        app_call(
            &a,
            "github.enable",
            serde_json::json!({"host": "no/slash", "enabled": false})
        )
        .unwrap()
        .is_err()
    );
    assert!(
        app_call(&a, "github.use", serde_json::json!({"token": "x"}))
            .unwrap()
            .is_err()
    );
    assert!(app_call(&a, "agent_start", Value::Null).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// A tiny HTTP server that answers `GET /user` like GitHub.
async fn fake_api(status: u16, login: &str, scopes: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (login, scopes) = (login.to_string(), scopes.to_string());
    tokio::spawn(async move {
        loop {
            let (mut c, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = c.read(&mut buf).await;
            let body = format!(r#"{{"login":"{login}"}}"#);
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\nx-oauth-scopes: {scopes}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = c.write_all(resp.as_bytes()).await;
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn a_rescan_asks_the_host_whose_token_it_is_and_warns_about_scopes() {
    let (keys, dir) = temp_keys();
    keys.set("other", Some(SAVED_KEY), "ghp_SAVED_SECRET")
        .unwrap();
    let f = Fake::new("2.101.0", r#"{"hosts":{}}"#);
    let base = fake_api(200, "carol", "repo, workflow").await;
    let a = auth(&keys, &f, &[]).with_api(&base);
    // Before a rescan the page does not know the account.
    assert_eq!(a.status().hosts[0].line, "Signed in with a saved token");
    let r = a.rescan().await;
    assert_eq!(r.hosts[0].line, "Signed in as carol with a saved token");
    assert!(r.hosts[0].warning.as_deref().unwrap().contains("workflows"));
    assert!(r.scanned_ms > 0);
    assert!(!serde_json::to_string(&r).unwrap().contains("SAVED_SECRET"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_token_the_host_refuses_is_said_so() {
    let (keys, dir) = temp_keys();
    keys.set("other", Some(SAVED_KEY), "ghp_SAVED_SECRET")
        .unwrap();
    let f = Fake::new("2.101.0", r#"{"hosts":{}}"#);
    let base = fake_api(401, "", "").await;
    let a = auth(&keys, &f, &[]).with_api(&base);
    let r = a.rescan().await;
    assert!(r.hosts[0].problem);
    assert!(
        r.hosts[0].line.contains("was refused by github.com"),
        "{}",
        r.hosts[0].line
    );
    let _ = std::fs::remove_dir_all(dir);
}
