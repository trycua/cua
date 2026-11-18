//! `cua ci app <op>`: what the Cua Spaces apps run for the CI Runners page.
//!
//! The apps (the SwiftUI Mac app and the Electron app) are thin shells over
//! the app core. For this page their one job is to run this command with a
//! fixed operation name and pass one JSON object in on stdin and one JSON
//! line out on stdout, so the rules (what may be asked, what is checked,
//! where a credential goes) live here, once, and not in each shell.
//!
//! ```text
//! cua ci app overview          {"live": true}             -> the read model (`cua ci overview`)
//! cua ci app connect           {"githubOrg": "acme"}      -> hosted GitHub app, one step at a time
//! cua ci app connect_token     {"org": "acme", "token"?}  -> a pool from a GitHub token
//! cua ci app connect_cancel    {}                         -> forget a GitHub install in progress
//! cua ci app repositories      {"allow": [], "remove": []}-> which repositories may run jobs
//! cua ci app goal              {"target","goal"}          -> cheapest, balanced or fastest
//! cua ci app log               {"jobKey","pool","slot"}   -> a job's own log, plain sentences
//! ```
//!
//! Every answer is `{"ok": true, "result": ...}` or `{"ok": false,
//! "error": {"code", "message"}}` and the exit code is 0 for both; a
//! non-zero exit means the command itself failed to run. A GitHub token
//! only ever arrives on stdin, and never appears in an answer.

use std::io::{Read as _, Write};

use crate::ci_signin as github_auth;
use cua_ci::CiPaths;
use cua_ci::events::EventLog;
use cua_ci::hosted::HostedState;
use cua_ci::overview;
use cua_ci::targets::TargetStore;
use cua_sdk::CuaError;
use serde_json::{Value, json};

use crate::ci_hosted::{HOSTED_POOL, InstallProgress};
use crate::util::line;

/// The operations, in the order the apps use them.
pub const OPS: &[&str] = &[
    "overview",
    "connect",
    "connect_token",
    "connect_cancel",
    "repositories",
    "goal",
    "log",
];

/// GitHub's REST API, where a token's organization is looked up.
const GITHUB_API: &str = "https://api.github.com";

/// How the sign-in is read when no token is given: the environment, then `gh`
/// ([`crate::ci_signin`]); tests pass a fake.
pub(crate) type SignIn = std::sync::Arc<
    dyn Fn(&str) -> Result<github_auth::Resolved, github_auth::Unavailable> + Send + Sync,
>;

/// This process's chain.
fn host_sign_in() -> SignIn {
    std::sync::Arc::new(|host| github_auth::global().token(host))
}

/// The chain's refusal as the apps get it: one code per reason, in the
/// chain's own words.
fn unavailable(u: github_auth::Unavailable) -> Fail {
    use github_auth::Unavailable as U;
    let code = match &u {
        U::Nothing(_) => "no_token",
        U::GhRefused { .. } => "gh_refused",
    };
    Fail::new(code, u.message())
}

/// The HTTP status GitHub answered with, from the message the CI client
/// makes (`GET /path: 401 Bad credentials ...`).
fn http_status(message: &str) -> Option<u16> {
    message.split(": ").skip(1).find_map(|rest| {
        let code = rest.split_whitespace().next()?;
        (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
            .then(|| code.parse().ok())
            .flatten()
    })
}

/// A failed operation, as the apps get it.
#[derive(Debug)]
pub(crate) struct Fail {
    code: &'static str,
    message: String,
}

impl Fail {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<CuaError> for Fail {
    fn from(e: CuaError) -> Self {
        let code = match &e {
            CuaError::InvalidArgument(_) => "invalid",
            CuaError::NotFound(_) => "not_found",
            CuaError::Unsupported(_) => "unsupported",
            CuaError::Cancelled(_) => "cancelled",
            CuaError::Timeout(_) => "timeout",
            CuaError::Http(_) => "service",
            _ => "failed",
        };
        Fail::new(code, e.to_string())
    }
}

/// `cua ci app <op>`.
pub async fn run(op: &str, paths: &CiPaths, out: &mut dyn Write) -> Result<i32, CuaError> {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let args: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                answer(out, Err(Fail::new("invalid", format!("arguments: {e}"))));
                return Ok(0);
            }
        }
    };
    answer(out, dispatch(op, &args, paths).await);
    Ok(0)
}

fn answer(out: &mut dyn Write, r: Result<Value, Fail>) {
    let v = match r {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(f) => json!({ "ok": false, "error": { "code": f.code, "message": f.message } }),
    };
    line(out, v.to_string());
}

/// Runs one operation.
pub(crate) async fn dispatch(op: &str, args: &Value, paths: &CiPaths) -> Result<Value, Fail> {
    match op {
        "overview" => {
            let live = args["live"].as_bool().unwrap_or(false);
            let now = cua_ci::unix_ms();
            let o = if live {
                crate::ci_overview::build_live(paths, now).await
            } else {
                crate::ci_overview::build(paths, now)
            };
            serde_json::to_value(o).map_err(|e| Fail::new("failed", e.to_string()))
        }
        "connect" => connect(args, paths).await,
        "connect_token" => connect_token(args, paths, host_sign_in(), GITHUB_API).await,
        "connect_cancel" => {
            crate::ci_hosted::cancel_install(paths);
            Ok(json!({ "state": "idle" }))
        }
        "repositories" => repositories(args, paths).await,
        "goal" => goal(args, paths),
        "log" => log(args, paths),
        other => Err(Fail::new(
            "invalid",
            format!(
                "{other:?} is not a CI Runners operation (one of {})",
                OPS.join(", ")
            ),
        )),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// The hosted GitHub app, one step per call: sign in, start the install on
/// GitHub (the answer carries the page to open), then, on each later call,
/// ask where it has got to; once linked it finishes the connection (the
/// repositories GitHub was given are the ones that may run jobs, this
/// computer registers and its service starts).
async fn connect(args: &Value, paths: &CiPaths) -> Result<Value, Fail> {
    if let Some(h) = HostedState::load(paths).map_err(|e| Fail::new("failed", e.to_string()))? {
        return Ok(connected_answer(paths, &h.github_login));
    }
    {
        use cua_host::AccountTokens as _;
        if crate::host::SessionTokens.access_token().await.is_err() {
            return Ok(json!({ "state": "sign_in" }));
        }
    }
    let org = args["githubOrg"].as_str().filter(|s| !s.trim().is_empty());
    let account = "me";
    let api = crate::ci_hosted::client(account)?;
    let acct = api.ensure_account().await.map_err(crate::ci_cmd::err)?;
    let installation = match crate::ci_hosted::pick_installation(&acct, None, org) {
        Some(i) => i,
        None => match crate::ci_hosted::install_progress(&api, account, org, paths).await? {
            InstallProgress::Waiting { url } => {
                return Ok(json!({ "state": "waiting", "url": url }));
            }
            InstallProgress::Failed(message) => {
                return Ok(json!({ "state": "failed", "message": message }));
            }
            InstallProgress::Installed(i) => i,
        },
    };
    let mut sink = Vec::new();
    crate::ci_hosted::connect(
        crate::ci_hosted::Connect {
            github_org: Some(installation.github_login.clone()).filter(|l| !l.is_empty()),
            yes: true,
            no_browser: true,
            on: "local".into(),
            no_service: args["service"].as_bool() == Some(false),
            github_api: None,
            ..Default::default()
        },
        paths,
        true,
        &mut sink,
    )
    .await?;
    // The install is done with: forget it.
    crate::ci_hosted::cancel_install(paths);
    Ok(connected_answer(paths, &installation.github_login))
}

fn connected_answer(paths: &CiPaths, login: &str) -> Value {
    let github = crate::ci_overview::github_view(paths);
    json!({ "state": "connected", "mode": github.mode, "login": login, "github": github })
}

/// A pool from a GitHub token, for the person who has no GitHub app: the
/// token given now (on stdin, never a command line), or else the host's
/// GitHub sign-in (`sign_in`: the shared chain, so Settings, Connections,
/// `gh` and the per-host off switch apply here as they do to a push). The pool
/// and its credential are the ones `cua ci connect --own-app --token-stdin`
/// makes.
async fn connect_token(
    args: &Value,
    paths: &CiPaths,
    sign_in: SignIn,
    api_url: &str,
) -> Result<Value, Fail> {
    let scope = match (args["org"].as_str(), args["repo"].as_str()) {
        (Some(o), _) if !o.trim().is_empty() => format!("org:{}", o.trim()),
        (_, Some(r)) if !r.trim().is_empty() => format!("repo:{}", r.trim()),
        _ => {
            return Err(Fail::new(
                "invalid",
                "Name the GitHub organization (or owner/name of one repository) to serve",
            ));
        }
    };
    let token = match args["token"]
        .as_str()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        Some(t) => t,
        None => {
            // Asking `gh` runs a program: off the async threads.
            let host = github_auth::DEFAULT_HOST;
            let r = tokio::task::spawn_blocking(move || sign_in(host))
                .await
                .map_err(|e| Fail::new("failed", e.to_string()))?
                .map_err(unavailable)?;
            r.token.expose().trim().to_string()
        }
    };
    if !cua_ci::targets::plausible_token(&token) {
        return Err(Fail::new(
            "invalid",
            "That does not look like a GitHub token (one line, no spaces)",
        ));
    }
    let secret_store = args["secretStore"]
        .as_str()
        .unwrap_or(if cfg!(target_os = "macos") {
            "keychain"
        } else {
            "file"
        })
        .to_string();
    if !matches!(secret_store.as_str(), "keychain" | "file") {
        return Err(Fail::new("invalid", "secretStore: keychain or file"));
    }
    let mut sink = Vec::new();
    crate::ci_cmd::connect_own_token(
        &scope,
        token,
        crate::ci_cmd::OwnTokenOptions {
            secret_store,
            api_url: api_url.to_string(),
            service: args["service"].as_bool().unwrap_or(true),
        },
        paths,
        &mut sink,
    )
    .await
    .map_err(|e| {
        match http_status(&e.to_string()) {
            Some(401 | 403) => Fail::new(
                "refused",
                "GitHub refused the token. Check that it is valid, not expired, and has the repo and admin:org scopes.",
            ),
            Some(404) => Fail::new(
                "refused",
                "GitHub does not know that organization, or the token cannot see it.",
            ),
            _ => Fail::from(e),
        }
    })?;
    Ok(connected_answer(
        paths,
        scope.split_once(':').map_or("", |x| x.1),
    ))
}

/// Which repositories may run jobs (the hosted app's list).
async fn repositories(args: &Value, paths: &CiPaths) -> Result<Value, Fail> {
    if !crate::ci_hosted::is_connected(paths) {
        return Err(Fail::new(
            "unsupported",
            "With your own GitHub credential, repositories are chosen on GitHub (the runner group's access list)",
        ));
    }
    let change = crate::ci_hosted::ConfigChange {
        allow: strings(&args["allow"]),
        remove: strings(&args["remove"]),
        yes: true,
        ..Default::default()
    };
    if change.is_empty() {
        return Err(Fail::new("invalid", "Nothing to change"));
    }
    let mut sink = Vec::new();
    crate::ci_hosted::config(paths, change, true, &mut sink).await?;
    let changed: Value = String::from_utf8_lossy(&sink)
        .lines()
        .last()
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or(json!({}));
    Ok(changed)
}

/// The goal of a target (`cheapest`, `balanced`, `fastest`). The service
/// that runs the computers reads it when it starts; it is restarted now when
/// no job is running, and otherwise the answer says it applies later.
fn goal(args: &Value, paths: &CiPaths) -> Result<Value, Fail> {
    let target = args["target"].as_str().unwrap_or("");
    let goal = args["goal"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !matches!(goal.as_str(), "cheapest" | "balanced" | "fastest") {
        return Err(Fail::new("invalid", "goal: cheapest, balanced or fastest"));
    }
    let store = TargetStore::new(paths);
    let mut t = store
        .get(target)
        .map_err(|e| Fail::new("failed", e.to_string()))?
        .ok_or_else(|| Fail::new("not_found", format!("No cloud named {target:?} is added")))?;
    if t.goal_or_default() == goal {
        return Ok(json!({ "goal": goal, "applied": true }));
    }
    t.goal = goal.clone();
    store
        .save(&t)
        .map_err(|e| Fail::new("failed", e.to_string()))?;
    let o = crate::ci_overview::build(paths, cua_ci::unix_ms());
    let busy = o
        .jobs
        .iter()
        .any(|j| matches!(j.state.as_str(), "queued" | "running"));
    // Read when the service starts: with none installed there is nothing to
    // restart, with jobs in flight it waits for the next restart.
    let applied = if !cua_ci::service::installed(HOSTED_POOL) {
        true
    } else if busy {
        false
    } else {
        cua_ci::service::stop(HOSTED_POOL).is_ok() && cua_ci::service::start(HOSTED_POOL).is_ok()
    };
    Ok(json!({ "goal": goal, "applied": applied }))
}

/// A job's own log: the events of the job and of its computer.
fn log(args: &Value, paths: &CiPaths) -> Result<Value, Fail> {
    let key = args["jobKey"].as_str().unwrap_or("");
    let pool = args["pool"].as_str().unwrap_or(HOSTED_POOL);
    let slot = args["slot"].as_str().unwrap_or("");
    if key.is_empty()
        || !pool.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        || pool.is_empty()
    {
        return Err(Fail::new("invalid", "jobKey and pool are needed"));
    }
    let events = EventLog::new(paths.events(pool)).read();
    let lines = overview::log_lines(&events, key, slot);
    Ok(json!({ "lines": lines }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;
    use cua_ci::events::{Event, EventKind};
    use cua_ci::targets::{Kind, TargetState};
    use std::collections::HashMap;

    fn home() -> (tempfile::TempDir, CiPaths) {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        (d, paths)
    }

    async fn ok(op: &str, args: Value, paths: &CiPaths) -> Value {
        dispatch(op, &args, paths)
            .await
            .unwrap_or_else(|f| panic!("{op}: {} {}", f.code, f.message))
    }

    async fn fail(op: &str, args: Value, paths: &CiPaths) -> (&'static str, String) {
        let f = dispatch(op, &args, paths).await.expect_err(op);
        (f.code, f.message)
    }

    #[tokio::test]
    async fn an_empty_home_reads_as_nothing_connected() {
        let (_d, paths) = home();
        let o = ok("overview", json!({}), &paths).await;
        assert_eq!(o["github"]["connected"], false);
        assert_eq!(o["github"]["mode"], "none");
        assert_eq!(o["setup"][0]["state"], "todo");
        assert_eq!(o["jobs"], json!([]));
    }

    #[tokio::test]
    async fn an_unknown_operation_names_the_known_ones() {
        let (_d, paths) = home();
        let (code, message) = fail("rm", json!({}), &paths).await;
        assert_eq!(code, "invalid");
        for op in OPS {
            assert!(message.contains(op), "{message}");
        }
    }

    #[tokio::test]
    async fn a_jobs_log_is_its_own_events_in_plain_words() {
        let (_d, paths) = home();
        let log = EventLog::new(paths.events("hosted"));
        log.record(&Event::new(1_000, EventKind::JobQueued).with("job_key", "9/1"));
        log.record(
            &Event::new(2_000, EventKind::VmClaimed)
                .slot("s1")
                .with("job_key", "9/1")
                .with("start_mode", "warm_pool"),
        );
        log.record(
            &Event::new(3_000, EventKind::JobStarted)
                .slot("s1")
                .with("job_key", "9/1"),
        );
        let r = ok(
            "log",
            json!({"jobKey": "9/1", "pool": "hosted", "slot": "s1"}),
            &paths,
        )
        .await;
        let text: Vec<&str> = r["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            text,
            [
                "Queued, waiting for a computer",
                "Took a Space that was already running",
                "The job started"
            ]
        );
        // The pool name never walks out of the CI folder.
        let (code, _) = fail("log", json!({"jobKey": "9/1", "pool": "../x"}), &paths).await;
        assert_eq!(code, "invalid");
    }

    #[tokio::test]
    async fn the_goal_is_saved_for_the_cloud_and_refused_when_unknown() {
        let (_d, paths) = home();
        let store = TargetStore::new(&paths);
        store.save(&TargetState::new(Kind::Aws, "aws")).unwrap();
        let r = ok("goal", json!({"target": "aws", "goal": "Fastest"}), &paths).await;
        assert_eq!(r["goal"], "fastest");
        assert_eq!(
            store.get("aws").unwrap().unwrap().goal_or_default(),
            "fastest"
        );
        assert_eq!(
            ok("overview", json!({}), &paths).await["targets"][0]["goal"],
            "fastest"
        );
        assert_eq!(
            fail("goal", json!({"target": "aws", "goal": "free"}), &paths)
                .await
                .0,
            "invalid"
        );
        assert_eq!(
            fail("goal", json!({"target": "gcp", "goal": "cheapest"}), &paths)
                .await
                .0,
            "not_found"
        );
    }

    #[tokio::test]
    async fn repositories_need_the_hosted_app() {
        let (_d, paths) = home();
        let (code, message) = fail("repositories", json!({"allow": ["acme/app"]}), &paths).await;
        assert_eq!(code, "unsupported");
        assert!(message.contains("chosen on GitHub"), "{message}");
    }

    /// A loopback GitHub: an organization with a runner group, for a token.
    async fn github(seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> String {
        let auth = |seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>, h: &HeaderMap| {
            let t = h
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            seen.lock().unwrap().push(t.clone());
            t.ends_with("ghp_good")
        };
        let (s1, s2) = (seen.clone(), seen);
        let app = axum::Router::new()
            .route(
                "/orgs/acme/actions/runner-groups",
                get(move |h: HeaderMap| async move {
                    if !auth(s1.clone(), &h) {
                        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"message": "Bad credentials"})));
                    }
                    (
                        StatusCode::OK,
                        axum::Json(json!({"runner_groups": [
                            {"id": 1, "name": "Default", "default": true, "visibility": "selected", "allows_public_repositories": false}
                        ]})),
                    )
                }),
            )
            .route(
                "/orgs/acme/actions/runners",
                get(move |Query(_q): Query<HashMap<String, String>>, h: HeaderMap| async move {
                    if !auth(s2.clone(), &h) {
                        return (StatusCode::UNAUTHORIZED, axum::Json(json!({"message": "Bad credentials"})));
                    }
                    (StatusCode::OK, axum::Json(json!({"total_count": 0, "runners": []})))
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_token_makes_a_pool_and_a_wrong_one_makes_nothing() {
        let (_d, paths) = home();
        let seen = Default::default();
        let api = github(std::sync::Arc::clone(&seen)).await;
        let opts = |api: &str| crate::ci_cmd::OwnTokenOptions {
            secret_store: "file".into(),
            api_url: api.to_string(),
            service: false,
        };
        // A wrong token: GitHub says so, and nothing is saved.
        let mut sink = Vec::new();
        let e = crate::ci_cmd::connect_own_token(
            "org:acme",
            "ghp_wrong".into(),
            opts(&api),
            &paths,
            &mut sink,
        )
        .await
        .unwrap_err();
        assert!(
            e.to_string().contains("401") || e.to_string().to_lowercase().contains("credentials"),
            "{e}"
        );
        assert!(paths.pools().is_empty());
        assert!(!crate::ci_overview::github_view(&paths).connected);
        // The right one: a pool for the organization, its token kept in the
        // private file store (never the config), no service installed.
        crate::ci_cmd::connect_own_token(
            "org:acme",
            "ghp_good".into(),
            opts(&api),
            &paths,
            &mut sink,
        )
        .await
        .unwrap();
        assert_eq!(paths.pools(), ["acme"]);
        let config = std::fs::read_to_string(paths.config("acme")).unwrap();
        assert!(
            !config.contains("ghp_good"),
            "the token is never in the pool's configuration"
        );
        let g = crate::ci_overview::github_view(&paths);
        assert_eq!(
            (g.connected, g.mode.as_str(), g.login.as_str()),
            (true, "own", "acme")
        );
        assert!(
            g.choose_url
                .contains("/organizations/acme/settings/actions/runner-groups")
        );
        assert!(!sink.is_empty());
        assert!(!String::from_utf8_lossy(&sink).contains("ghp_good"));
        let o = ok("overview", json!({}), &paths).await;
        assert_eq!(o["setup"][0]["state"], "done");
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .all(|a| a.starts_with("Bearer ") || a.starts_with("token "))
        );
    }

    #[tokio::test]
    async fn a_refused_token_says_so_in_plain_words() {
        let (_d, paths) = home();
        let api = github(Default::default()).await;
        let opts = crate::ci_cmd::OwnTokenOptions {
            secret_store: "file".into(),
            api_url: api,
            service: false,
        };
        let mut sink = Vec::new();
        let e = crate::ci_cmd::connect_own_token(
            "org:acme",
            "ghp_wrong".into(),
            opts,
            &paths,
            &mut sink,
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("401"), "{e}");
    }

    #[tokio::test]
    async fn a_token_needs_an_organization_and_looks_like_a_token() {
        let (_d, paths) = home();
        assert_eq!(
            fail("connect_token", json!({"token": "ghp_x"}), &paths)
                .await
                .0,
            "invalid"
        );
        // No token given and the chain finds none: its words, and nothing is made.
        let f = connect_token(&json!({"org": "acme"}), &paths, signed_out(), GITHUB_API)
            .await
            .unwrap_err();
        assert_eq!(f.code, "no_token");
        assert!(f.message.contains("gh auth login"), "{}", f.message);
        assert_eq!(
            fail(
                "connect_token",
                json!({"org": "acme", "token": "two words"}),
                &paths
            )
            .await
            .0,
            "invalid"
        );
        assert_eq!(
            fail(
                "connect_token",
                json!({"org": "acme", "token": "ghp_x", "secretStore": "cloud"}),
                &paths
            )
            .await
            .0,
            "invalid"
        );
    }

    struct Token;
    #[async_trait::async_trait]
    impl cua_ci::hosted::TokenSource for Token {
        async fn token(&self) -> cua_ci::Result<String> {
            Ok("t".into())
        }
    }

    /// A loopback Cua CI service whose install session is in `status`.
    async fn service(status: std::sync::Arc<std::sync::Mutex<&'static str>>) -> String {
        use axum::routing::post;
        let app = axum::Router::new()
            .route(
                "/v1/ci/install/start",
                post(|| async {
                    axum::Json(json!({"session_id": "ins_1", "url": "https://github.com/apps/cua-ci/installations/new?state=ins_1", "poll_interval_s": 1}))
                }),
            )
            .route(
                "/v1/ci/install/ins_1",
                get(move || {
                    let s = *status.lock().unwrap();
                    async move {
                        axum::Json(match s {
                            "installed" => json!({"status": "installed", "installation": {"id": "inst_1", "github_login": "acme"}}),
                            "denied" => json!({"status": "denied", "error": "personal_account_unsupported"}),
                            other => json!({ "status": other }),
                        })
                    }
                }),
            );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn the_github_install_goes_one_step_per_call_and_is_forgotten_when_done() {
        use crate::ci_hosted::{InstallProgress, install_progress};
        let (_d, paths) = home();
        let status = std::sync::Arc::new(std::sync::Mutex::new("pending"));
        let base = service(status.clone()).await;
        let api =
            cua_ci::hosted::ServiceClient::new(&base, "me", std::sync::Arc::new(Token)).unwrap();
        let url = "https://github.com/apps/cua-ci/installations/new?state=ins_1";
        // The first call starts the install and returns GitHub's page.
        assert_eq!(
            install_progress(&api, "me", None, &paths).await.unwrap(),
            InstallProgress::Waiting { url: url.into() }
        );
        assert!(paths.root().join("github-install.json").exists());
        // Still pending: the same page, nothing new started.
        assert_eq!(
            install_progress(&api, "me", None, &paths).await.unwrap(),
            InstallProgress::Waiting { url: url.into() }
        );
        // Installed: the installation, and the waiting install is forgotten.
        *status.lock().unwrap() = "installed";
        match install_progress(&api, "me", None, &paths).await.unwrap() {
            InstallProgress::Installed(i) => assert_eq!(i.github_login, "acme"),
            other => panic!("{other:?}"),
        }
        assert!(!paths.root().join("github-install.json").exists());
    }

    #[tokio::test]
    async fn a_personal_account_is_told_to_use_an_organization_or_a_token() {
        use crate::ci_hosted::{InstallProgress, install_progress};
        let (_d, paths) = home();
        let status = std::sync::Arc::new(std::sync::Mutex::new("pending"));
        let base = service(status.clone()).await;
        let api =
            cua_ci::hosted::ServiceClient::new(&base, "me", std::sync::Arc::new(Token)).unwrap();
        install_progress(&api, "me", None, &paths).await.unwrap();
        *status.lock().unwrap() = "denied";
        match install_progress(&api, "me", None, &paths).await.unwrap() {
            InstallProgress::Failed(m) => {
                assert!(m.contains("organization") && m.contains("token"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        // Cancelling forgets a waiting install.
        *status.lock().unwrap() = "pending";
        install_progress(&api, "me", None, &paths).await.unwrap();
        crate::ci_hosted::cancel_install(&paths);
        assert!(!paths.root().join("github-install.json").exists());
    }

    /// A sign-in chain that finds nothing.
    fn signed_out() -> SignIn {
        std::sync::Arc::new(|h| Err(github_auth::Unavailable::Nothing(h.to_string())))
    }

    struct NoGh;
    impl github_auth::Gh for NoGh {
        fn found(&self) -> bool {
            false
        }
        fn run(&self, _: &[&str]) -> Option<github_auth::GhOut> {
            None
        }
    }

    struct FakeEnv(Vec<(&'static str, &'static str)>);
    impl github_auth::Env for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        }
    }

    /// The real chain (environment, then gh) with fakes under it; the
    /// sign-in the apps use is this chain, not a copy.
    fn through(env: Vec<(&'static str, &'static str)>) -> SignIn {
        let chain = std::sync::Arc::new(github_auth::Chain::new(
            Box::new(NoGh),
            Box::new(FakeEnv(env)),
        ));
        std::sync::Arc::new(move |h| chain.token(h))
    }

    #[tokio::test]
    async fn the_apps_sign_in_is_the_chain_and_a_pasted_token_needs_none() {
        let (_d, paths) = home();
        let seen = Default::default();
        let api = github(std::sync::Arc::clone(&seen)).await;
        // The chain's token makes the pool, though none was given (and no
        // environment variable of this process is read).
        let a = json!({"org": "acme", "secretStore": "file", "service": false});
        connect_token(&a, &paths, through(vec![("GH_TOKEN", "ghp_good")]), &api)
            .await
            .unwrap();
        assert_eq!(paths.pools(), ["acme"]);
        assert!(
            !std::fs::read_to_string(paths.config("acme"))
                .unwrap()
                .contains("ghp_good")
        );
        assert!(!seen.lock().unwrap().is_empty());

        // Nothing signed in: nothing is sent.
        let (_d2, paths2) = home();
        seen.lock().unwrap().clear();
        let f = connect_token(&a, &paths2, through(vec![]), &api)
            .await
            .unwrap_err();
        assert_eq!(f.code, "no_token");
        assert!(paths2.pools().is_empty());
        assert!(
            seen.lock().unwrap().is_empty(),
            "the API was asked with no sign-in"
        );

        // A token pasted now is the person's own choice and does not need the chain.
        let given =
            json!({"org": "acme", "token": "ghp_good", "secretStore": "file", "service": false});
        connect_token(&given, &paths2, signed_out(), &api)
            .await
            .unwrap();
        assert_eq!(paths2.pools(), ["acme"]);
    }

    #[tokio::test]
    async fn each_reason_the_chain_gives_has_its_own_code() {
        use github_auth::Unavailable as U;
        let (_d, paths) = home();
        for (u, code) in [
            (U::Nothing("github.com".into()), "no_token"),
            (
                U::GhRefused {
                    host: "github.com".into(),
                },
                "gh_refused",
            ),
        ] {
            let words = u.message();
            let sign_in: SignIn = std::sync::Arc::new(move |_| Err(u.clone()));
            let f = connect_token(&json!({"org": "acme"}), &paths, sign_in, GITHUB_API)
                .await
                .unwrap_err();
            assert_eq!((f.code, f.message), (code, words));
        }
    }

    #[test]
    fn the_status_comes_from_the_github_message_not_from_any_digits() {
        assert_eq!(
            http_status("GET /orgs/acme: 401 Bad credentials (check)"),
            Some(401)
        );
        assert_eq!(http_status("POST /x/y: 404 Not Found"), Some(404));
        // Digits elsewhere (an org named 4010, a port) are not a status.
        assert_eq!(http_status("GET /orgs/4010: connection refused"), None);
        assert_eq!(http_status("timed out after 403 ms"), None);
    }
}
