//! `cua ci connect --open-pr`: point a repository's workflows at Cua CI
//! with a pull request the user reviews.
//!
//! [`plan_file`] rewrites the `runs-on` of jobs that name a GitHub-hosted
//! runner (`ubuntu-*`, `macos-*`) to the matching Cua labels and leaves
//! everything else alone: expressions, matrices, self-hosted and Windows
//! runners are reported, never guessed. A file that runs on
//! `pull_request_target`, `issue_comment` or `workflow_run` is never
//! touched (those events run with secrets for untrusted input and are not
//! served by Cua CI). [`open_pr`] makes the change on a new branch through
//! the user's own GitHub authorization and opens the pull request; it never
//! pushes to the default branch, and the token never leaves this process.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;

use crate::secret::Secret;
use crate::{Error, Result};

/// Events that run with secrets on input a stranger controls: such a file
/// is left alone.
pub const UNTRUSTED_EVENTS: [&str; 3] = ["pull_request_target", "issue_comment", "workflow_run"];

/// What happened to one `runs-on` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineChange {
    /// 1-based line number.
    pub line: usize,
    /// The value as written.
    pub from: String,
    /// The new value (`None` when left alone).
    pub to: Option<String>,
    /// Why it was left alone.
    pub note: Option<String>,
}

/// A planned edit of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePlan {
    /// The text with the changes applied.
    pub new_text: String,
    /// Every `runs-on` line found.
    pub lines: Vec<LineChange>,
    /// The file was skipped whole (untrusted events), with the reason.
    pub skipped: Option<String>,
}

impl FilePlan {
    /// Lines that changed.
    pub fn changed(&self) -> usize {
        self.lines.iter().filter(|l| l.to.is_some()).count()
    }
}

/// The Cua labels for one GitHub-hosted label, or why not.
pub fn map_label(label: &str) -> std::result::Result<Vec<&'static str>, String> {
    let l = label.trim().to_ascii_lowercase();
    if let Some(rest) = l.strip_prefix("ubuntu") {
        let mut out = vec!["cua-linux"];
        if rest.contains("arm") {
            out.push("cua-arm64");
        }
        if rest.contains("xlarge") || rest.contains("large") {
            out.push("cua-large");
        }
        return Ok(out);
    }
    if let Some(rest) = l.strip_prefix("macos") {
        let mut out = vec!["cua-macos"];
        if rest.contains("intel") || rest.ends_with("-13") || rest.ends_with("-12") {
            out.push("cua-x64");
        }
        if rest.contains("xlarge") || rest.contains("large") {
            out.push("cua-large");
        }
        return Ok(out);
    }
    if l.starts_with("windows") {
        return Err("Windows runners are not served by Cua CI yet".into());
    }
    if l == "self-hosted" || l.starts_with("cua") {
        return Err("already self-hosted or Cua CI".into());
    }
    Err(format!("{label:?} is not a GitHub-hosted label"))
}

/// Strips one layer of quotes and trailing spaces.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    for q in ['\'', '"'] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Splits `value  # comment` into the value and the comment (with its
/// leading spaces) when the `#` is outside quotes.
fn split_comment(s: &str) -> (&str, &str) {
    let mut quote: Option<char> = None;
    let mut prev_space = true;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            (None, '#') if prev_space => {
                let v = &s[..i];
                return (v.trim_end(), &s[v.trim_end().len()..]);
            }
            _ => {}
        }
        prev_space = c.is_whitespace();
    }
    (s.trim_end(), "")
}

/// Whether the workflow `text` is triggered by an untrusted event.
pub fn untrusted_trigger(text: &str) -> Option<&'static str> {
    // A key at any indent (`pull_request_target:`) or in a list/flow form
    // on the `on:` line; comments are ignored.
    for raw in text.lines() {
        let (line, _) = split_comment(raw);
        let t = line.trim();
        for e in UNTRUSTED_EVENTS {
            let key = t.trim_start_matches("- ");
            if key == e
                || key.strip_prefix(e).is_some_and(|r| r.starts_with(':'))
                || (t.starts_with("on:")
                    && t[3..]
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|w| w == e))
            {
                return Some(e);
            }
        }
    }
    None
}

/// Plans the edit of one workflow file.
pub fn plan_file(text: &str) -> FilePlan {
    if let Some(e) = untrusted_trigger(text) {
        return FilePlan {
            new_text: text.to_string(),
            lines: vec![],
            skipped: Some(format!(
                "it runs on {e}, which runs with secrets on input a stranger controls: Cua CI \
                 does not serve it and the file is left alone"
            )),
        };
    }
    let mut out = String::with_capacity(text.len());
    let mut lines = Vec::new();
    for (i, raw) in text.split_inclusive('\n').enumerate() {
        let (body, eol) = match raw.strip_suffix("\r\n") {
            Some(b) => (b, "\r\n"),
            None => match raw.strip_suffix('\n') {
                Some(b) => (b, "\n"),
                None => (raw, ""),
            },
        };
        let trimmed = body.trim_start();
        let indent = &body[..body.len() - trimmed.len()];
        let rest = trimmed.strip_prefix("runs-on:");
        let Some(rest) = rest.filter(|r| r.is_empty() || r.starts_with(' ') || r.starts_with('\t'))
        else {
            out.push_str(raw);
            continue;
        };
        let (value, comment) = split_comment(rest.trim_start());
        let from = value.to_string();
        let mut note = None;
        let mut to = None;
        if value.is_empty() {
            note = Some("a block value: change it by hand".to_string());
        } else if value.contains("${{") {
            note = Some("an expression: change it by hand".to_string());
        } else if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            let items: Vec<&str> = inner
                .split(',')
                .map(unquote)
                .filter(|s| !s.is_empty())
                .collect();
            match items.as_slice() {
                [one] => match map_label(one) {
                    Ok(m) => to = Some(format!("[{}]", m.join(", "))),
                    Err(e) => note = Some(e),
                },
                _ => note = Some("a label list: change it by hand".to_string()),
            }
        } else {
            match map_label(unquote(value)) {
                Ok(m) if m.len() == 1 => to = Some(m[0].to_string()),
                Ok(m) => to = Some(format!("[{}]", m.join(", "))),
                Err(e) => note = Some(e),
            }
        }
        match &to {
            Some(t) => {
                out.push_str(&format!("{indent}runs-on: {t}{comment}{eol}"));
            }
            None => out.push_str(raw),
        }
        lines.push(LineChange {
            line: i + 1,
            from,
            to,
            note,
        });
    }
    FilePlan {
        new_text: out,
        lines,
        skipped: None,
    }
}

// ---------------------------------------------------------------------
// The pull request
// ---------------------------------------------------------------------

/// A workflow file at the base branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowFile {
    /// Path in the repository.
    pub path: String,
    /// Blob sha.
    pub sha: String,
    /// Content.
    pub text: String,
}

/// What `open_pr` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrResult {
    /// The pull request's web address.
    pub url: String,
    /// The branch it came from.
    pub branch: String,
    /// Files changed.
    pub files: Vec<String>,
}

/// The user's GitHub authorization for this one action: `GH_TOKEN`,
/// `GITHUB_TOKEN`, else what `gh auth token` prints. Never stored.
pub fn user_token() -> Result<Secret> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return Ok(Secret::new(v.trim()));
        }
    }
    let out = std::process::Command::new("gh")
        .args(["auth", "token"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if t.is_empty() {
                Err(no_token())
            } else {
                Ok(Secret::new(t))
            }
        }
        _ => Err(no_token()),
    }
}

fn no_token() -> Error {
    Error::Config(
        "no GitHub authorization for the pull request: sign in with `gh auth login`, or set \
         GH_TOKEN (a token that can write to the repository)"
            .into(),
    )
}

/// A small GitHub REST client for the pull request, acting as the user.
pub struct PrClient {
    http: reqwest::Client,
    api: String,
    token: Secret,
}

#[derive(Deserialize)]
struct RepoInfo {
    default_branch: String,
    #[serde(default)]
    permissions: Option<Perms>,
    #[serde(default)]
    private: bool,
}

#[derive(Deserialize)]
struct Perms {
    #[serde(default)]
    push: bool,
}

impl PrClient {
    /// A client for `api` (`https://api.github.com`) acting with `token`.
    pub fn new(api: &str, token: Secret) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("cua-ci/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::GitHub(format!("HTTP client: {e}")))?;
        Ok(Self {
            http,
            api: api.trim_end_matches('/').to_string(),
            token,
        })
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(u16, serde_json::Value)> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}/{}", self.api, path))
            .bearer_auth(self.token.expose())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::GitHub(format!("{method} {path}: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let v = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Ok((status, v))
    }

    fn fail(what: &str, status: u16, v: &serde_json::Value) -> Error {
        let msg = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("no message");
        Error::GitHub(format!("{what}: {status} {msg}"))
    }

    /// The repository's workflow files at its default branch, with the
    /// branch name and whether it is private.
    pub async fn workflows(&self, repo: &str) -> Result<(String, bool, Vec<WorkflowFile>)> {
        let (st, v) = self
            .call(reqwest::Method::GET, &format!("repos/{repo}"), None)
            .await?;
        if st == 404 {
            return Err(Error::GitHub(format!(
                "{repo}: not found, or this GitHub authorization cannot see it"
            )));
        }
        if st != 200 {
            return Err(Self::fail(&format!("reading {repo}"), st, &v));
        }
        let info: RepoInfo =
            serde_json::from_value(v).map_err(|e| Error::GitHub(format!("reading {repo}: {e}")))?;
        if info.permissions.as_ref().is_some_and(|p| !p.push) {
            return Err(Error::GitHub(format!(
                "{repo}: this GitHub authorization cannot push a branch there; use an account \
                 with write access, or edit the workflow yourself with `runs-on: cua`"
            )));
        }
        let base = info.default_branch;
        let (st, v) = self
            .call(
                reqwest::Method::GET,
                &format!("repos/{repo}/contents/.github/workflows?ref={base}"),
                None,
            )
            .await?;
        if st == 404 {
            return Ok((base, info.private, vec![]));
        }
        if st != 200 {
            return Err(Self::fail("listing .github/workflows", st, &v));
        }
        let mut files = Vec::new();
        for item in v.as_array().cloned().unwrap_or_default() {
            let path = item["path"].as_str().unwrap_or_default().to_string();
            if !(path.ends_with(".yml") || path.ends_with(".yaml")) || item["type"] != "file" {
                continue;
            }
            let (st, f) = self
                .call(
                    reqwest::Method::GET,
                    &format!("repos/{repo}/contents/{path}?ref={base}"),
                    None,
                )
                .await?;
            if st != 200 {
                return Err(Self::fail(&format!("reading {path}"), st, &f));
            }
            let b64: String = f["content"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .collect();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| Error::GitHub(format!("{path}: {e}")))?;
            files.push(WorkflowFile {
                path,
                sha: f["sha"].as_str().unwrap_or_default().to_string(),
                text: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        Ok((base, info.private, files))
    }

    /// Commits `edits` (path to new text, with the file's blob sha) to a
    /// new branch off `base` and opens a pull request.
    pub async fn open_pr(
        &self,
        repo: &str,
        base: &str,
        edits: &[(WorkflowFile, String)],
        title: &str,
        body: &str,
    ) -> Result<PrResult> {
        if edits.is_empty() {
            return Err(Error::Config("nothing to change".into()));
        }
        let (st, v) = self
            .call(
                reqwest::Method::GET,
                &format!("repos/{repo}/git/ref/heads/{base}"),
                None,
            )
            .await?;
        if st != 200 {
            return Err(Self::fail(&format!("reading {base}"), st, &v));
        }
        let sha = v["object"]["sha"].as_str().unwrap_or_default().to_string();
        // A branch of our own, never the default branch; a name taken
        // already gets a suffix.
        let stamp = crate::hosted::rfc3339(crate::unix_ms())
            .replace([':', '-'], "")
            .replace('T', "-")
            .replace('Z', "");
        let mut branch = format!("cua-ci/connect-{stamp}");
        for n in 0..5 {
            let name = if n == 0 {
                branch.clone()
            } else {
                format!("{branch}-{n}")
            };
            let (st, v) = self
                .call(
                    reqwest::Method::POST,
                    &format!("repos/{repo}/git/refs"),
                    Some(json!({"ref": format!("refs/heads/{name}"), "sha": sha})),
                )
                .await?;
            match st {
                201 => {
                    branch = name;
                    break;
                }
                422 if n < 4 => continue,
                _ => return Err(Self::fail("creating the branch", st, &v)),
            }
        }
        let mut files = Vec::new();
        for (file, new_text) in edits {
            let (st, v) = self
                .call(
                    reqwest::Method::PUT,
                    &format!("repos/{repo}/contents/{}", file.path),
                    Some(json!({
                        "message": format!("Run {} on Cua CI", file.path),
                        "content": base64::engine::general_purpose::STANDARD.encode(new_text),
                        "sha": file.sha,
                        "branch": branch,
                    })),
                )
                .await?;
            if !(st == 200 || st == 201) {
                return Err(Self::fail(&format!("updating {}", file.path), st, &v));
            }
            files.push(file.path.clone());
        }
        let (st, v) = self
            .call(
                reqwest::Method::POST,
                &format!("repos/{repo}/pulls"),
                Some(json!({"title": title, "head": branch, "base": base, "body": body})),
            )
            .await?;
        if st != 201 {
            return Err(Self::fail("opening the pull request", st, &v));
        }
        Ok(PrResult {
            url: v["html_url"].as_str().unwrap_or_default().to_string(),
            branch,
            files,
        })
    }
}

/// The pull request's text.
pub fn pr_body(files: &[(String, usize)]) -> String {
    let mut s = String::from(
        "This pull request points workflow jobs at Cua CI, where each job runs in a fresh \
         Cua Space on your own computers and the Space is deleted afterwards.\n\n\
         Changed:\n",
    );
    for (path, n) in files {
        s.push_str(&format!(
            "- `{path}`: {n} job{}\n",
            if *n == 1 { "" } else { "s" }
        ));
    }
    s.push_str(
        "\nOnly `runs-on` lines that named a GitHub-hosted runner changed (`ubuntu-*` became \
         `cua-linux`, `macos-*` became `cua-macos`; architecture and size labels carry over). \
         Nothing else in the files was touched. Pull requests from forks run without secrets \
         and with read-only caches by default; `cua ci config` changes that.\n\n\
         Review the diff, then merge when you are ready. To go back, change `runs-on` back.\n",
    );
    s
}

/// Groups plans by file for the summary.
pub fn summarize(plans: &BTreeMap<String, FilePlan>) -> Vec<(String, usize)> {
    plans
        .iter()
        .filter(|(_, p)| p.changed() > 0)
        .map(|(path, p)| (path.clone(), p.changed()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_map_to_cua_labels() {
        assert_eq!(map_label("ubuntu-latest").unwrap(), ["cua-linux"]);
        assert_eq!(
            map_label("ubuntu-24.04-arm").unwrap(),
            ["cua-linux", "cua-arm64"]
        );
        assert_eq!(map_label("macos-latest").unwrap(), ["cua-macos"]);
        assert_eq!(map_label("macos-15").unwrap(), ["cua-macos"]);
        assert_eq!(map_label("macos-13").unwrap(), ["cua-macos", "cua-x64"]);
        assert_eq!(
            map_label("macos-15-intel").unwrap(),
            ["cua-macos", "cua-x64"]
        );
        assert_eq!(
            map_label("macos-latest-xlarge").unwrap(),
            ["cua-macos", "cua-large"]
        );
        assert!(map_label("windows-latest").unwrap_err().contains("Windows"));
        assert!(map_label("self-hosted").is_err());
        assert!(map_label("cua").is_err());
        assert!(map_label("gpu-box").is_err());
    }

    const WF: &str = "name: ci\non:\n  push:\n  pull_request:\njobs:\n  a:\n    runs-on: ubuntu-latest # linux\n    steps: []\n  b:\n    runs-on: 'macos-14'\n  c:\n    runs-on: [ubuntu-24.04-arm]\n  d:\n    runs-on: ${{ matrix.os }}\n  e:\n    runs-on: windows-latest\n  f:\n    runs-on: self-hosted\n  g:\n    runs-on: [self-hosted, linux]\n  h:\n    runs-on:\n      - ubuntu-latest\n";

    #[test]
    fn only_simple_hosted_runs_on_lines_change_and_nothing_else() {
        let p = plan_file(WF);
        assert_eq!(p.skipped, None);
        assert_eq!(p.changed(), 3);
        let t = &p.new_text;
        assert!(t.contains("    runs-on: cua-linux # linux\n"), "{t}");
        assert!(t.contains("    runs-on: cua-macos\n"), "{t}");
        assert!(t.contains("    runs-on: [cua-linux, cua-arm64]\n"), "{t}");
        assert!(t.contains("runs-on: ${{ matrix.os }}"));
        assert!(t.contains("runs-on: windows-latest"));
        assert!(t.contains("runs-on: self-hosted\n"));
        assert!(t.contains("runs-on: [self-hosted, linux]"));
        assert!(t.contains("runs-on:\n      - ubuntu-latest\n"));
        // Every other line is byte for byte the same.
        let before: Vec<&str> = WF.lines().filter(|l| !l.contains("runs-on:")).collect();
        let after: Vec<&str> = t.lines().filter(|l| !l.contains("runs-on:")).collect();
        assert_eq!(before, after);
        // The notes say why a line was left.
        let note = |n: usize| {
            p.lines
                .iter()
                .find(|l| l.line == n)
                .and_then(|l| l.note.clone())
                .unwrap_or_default()
        };
        assert!(note(14).contains("expression"));
        assert!(note(16).contains("Windows"));
        assert!(note(18).contains("self-hosted"));
        assert!(note(20).contains("list"));
        assert!(note(22).contains("block"));
    }

    #[test]
    fn crlf_and_a_missing_final_newline_survive() {
        let t = "jobs:\r\n  a:\r\n    runs-on: ubuntu-latest\r\n  b:\r\n    runs-on: macos-latest";
        let p = plan_file(t);
        assert_eq!(
            p.new_text,
            "jobs:\r\n  a:\r\n    runs-on: cua-linux\r\n  b:\r\n    runs-on: cua-macos"
        );
    }

    #[test]
    fn a_file_with_an_untrusted_trigger_is_left_alone() {
        for on in [
            "on:\n  pull_request_target:\n    types: [opened]\n",
            "on:\n  workflow_run:\n    workflows: [x]\n",
            "on: [push, issue_comment]\n",
            "on:\n  - workflow_run\n",
        ] {
            let text = format!("{on}jobs:\n  a:\n    runs-on: ubuntu-latest\n");
            let p = plan_file(&text);
            assert_eq!(p.new_text, text, "{on}");
            assert!(p.skipped.is_some(), "{on}");
            assert_eq!(p.changed(), 0);
        }
        // A comment that names one does not count.
        let text =
            "# no pull_request_target here\non: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n";
        assert_eq!(plan_file(text).changed(), 1);
        // A step named like the event is not a trigger.
        let text = "on: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo workflow_run\n";
        assert_eq!(plan_file(text).changed(), 1);
    }

    #[test]
    fn a_runs_on_key_is_matched_exactly() {
        let t =
            "jobs:\n  a:\n    runs-on-something: ubuntu-latest\n    name: runs-on: ubuntu-latest\n";
        assert_eq!(plan_file(t).changed(), 0);
        assert_eq!(plan_file(t).new_text, t);
    }

    #[test]
    fn the_pr_text_names_the_files() {
        let b = pr_body(&[(".github/workflows/ci.yml".into(), 2)]);
        assert!(b.contains("`.github/workflows/ci.yml`: 2 jobs"));
        assert!(!b.contains('\u{b7}'));
    }

    // ---- the pull request against a loopback GitHub ----

    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Gh {
        pushes: Mutex<Vec<(String, serde_json::Value)>>,
        refs: Mutex<Vec<String>>,
        push_allowed: bool,
        taken: Mutex<u32>,
        pulls: Mutex<Vec<serde_json::Value>>,
    }

    fn bearer(h: &HeaderMap) -> String {
        h.get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    async fn gh_server(gh: Arc<Gh>) -> String {
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let ci = b64("jobs:\n  a:\n    runs-on: ubuntu-latest\n");
        let other = b64("jobs:\n  a:\n    runs-on: self-hosted\n");
        let app = axum::Router::new()
            .route(
                "/repos/acme/app",
                get(|State(g): State<Arc<Gh>>, h: HeaderMap| async move {
                    assert_eq!(bearer(&h), "Bearer USER_TOKEN");
                    axum::Json(json!({"default_branch": "main", "private": true,
                                      "permissions": {"push": g.push_allowed}}))
                }),
            )
            .route(
                "/repos/acme/app/contents/.github/workflows",
                get(|| async {
                    axum::Json(json!([
                        {"path": ".github/workflows/ci.yml", "type": "file"},
                        {"path": ".github/workflows/other.yaml", "type": "file"},
                        {"path": ".github/workflows/README.md", "type": "file"},
                        {"path": ".github/workflows/sub", "type": "dir"}
                    ]))
                }),
            )
            .route(
                "/repos/acme/app/contents/.github/workflows/{name}",
                get(move |Path(name): Path<String>| {
                    let (ci, other) = (ci.clone(), other.clone());
                    async move {
                        let c = if name == "ci.yml" { ci } else { other };
                        axum::Json(json!({"content": c, "sha": format!("sha-{name}")}))
                    }
                })
                .put(
                    |State(g): State<Arc<Gh>>, Path(name): Path<String>, axum::Json(b): axum::Json<serde_json::Value>| async move {
                        g.pushes.lock().unwrap().push((name, b));
                        axum::Json(json!({"content": {}}))
                    },
                ),
            )
            .route(
                "/repos/acme/app/git/ref/heads/main",
                get(|| async { axum::Json(json!({"object": {"sha": "base-sha"}})) }),
            )
            .route(
                "/repos/acme/app/git/refs",
                post(|State(g): State<Arc<Gh>>, axum::Json(b): axum::Json<serde_json::Value>| async move {
                    let r = b["ref"].as_str().unwrap().to_string();
                    assert_ne!(r, "refs/heads/main", "never the default branch");
                    assert_eq!(b["sha"], "base-sha");
                    let mut taken = g.taken.lock().unwrap();
                    if *taken > 0 {
                        *taken -= 1;
                        return (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(json!({"message": "Reference already exists"})));
                    }
                    g.refs.lock().unwrap().push(r);
                    (StatusCode::CREATED, axum::Json(json!({})))
                }),
            )
            .route(
                "/repos/acme/app/pulls",
                post(|State(g): State<Arc<Gh>>, axum::Json(b): axum::Json<serde_json::Value>| async move {
                    g.pulls.lock().unwrap().push(b);
                    (StatusCode::CREATED, axum::Json(json!({"html_url": "https://github.com/acme/app/pull/7"})))
                }),
            )
            .with_state(gh);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_pull_request_goes_on_a_new_branch_and_changes_only_what_the_plan_says() {
        let gh = Arc::new(Gh {
            push_allowed: true,
            taken: Mutex::new(1),
            ..Default::default()
        });
        let url = gh_server(gh.clone()).await;
        let c = PrClient::new(&url, Secret::new("USER_TOKEN")).unwrap();
        let (base, private, files) = c.workflows("acme/app").await.unwrap();
        assert_eq!(base, "main");
        assert!(private);
        let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            names,
            [".github/workflows/ci.yml", ".github/workflows/other.yaml"]
        );
        let mut edits = vec![];
        let mut plans = BTreeMap::new();
        for f in files {
            let p = plan_file(&f.text);
            if p.changed() > 0 {
                edits.push((f.clone(), p.new_text.clone()));
            }
            plans.insert(f.path.clone(), p);
        }
        assert_eq!(edits.len(), 1);
        let body = pr_body(&summarize(&plans));
        let r = c
            .open_pr("acme/app", &base, &edits, "Run workflows on Cua CI", &body)
            .await
            .unwrap();
        assert_eq!(r.url, "https://github.com/acme/app/pull/7");
        assert_eq!(r.files, [".github/workflows/ci.yml"]);
        assert!(r.branch.starts_with("cua-ci/connect-"), "{}", r.branch);
        assert!(
            r.branch.ends_with("-1"),
            "a taken name gets a suffix: {}",
            r.branch
        );
        let pushes = gh.pushes.lock().unwrap();
        assert_eq!(pushes.len(), 1);
        assert_eq!(pushes[0].1["branch"], r.branch.as_str());
        assert_eq!(pushes[0].1["sha"], "sha-ci.yml");
        let sent = base64::engine::general_purpose::STANDARD
            .decode(pushes[0].1["content"].as_str().unwrap())
            .unwrap();
        assert!(
            String::from_utf8(sent)
                .unwrap()
                .contains("runs-on: cua-linux")
        );
        let pulls = gh.pulls.lock().unwrap();
        assert_eq!(pulls[0]["base"], "main");
        assert_eq!(pulls[0]["head"], r.branch.as_str());
    }

    #[tokio::test]
    async fn without_write_access_nothing_is_attempted() {
        let gh = Arc::new(Gh::default());
        let url = gh_server(gh.clone()).await;
        let c = PrClient::new(&url, Secret::new("USER_TOKEN")).unwrap();
        let e = c.workflows("acme/app").await.unwrap_err().to_string();
        assert!(e.contains("cannot push a branch"), "{e}");
        assert!(gh.refs.lock().unwrap().is_empty());
        let e = c
            .open_pr("acme/app", "main", &[], "t", "b")
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("nothing to change"), "{e}");
    }

    #[test]
    fn the_token_never_prints() {
        let c = PrClient::new("http://x", Secret::new("USER_TOKEN")).unwrap();
        let _ = &c;
        assert_eq!(format!("{}", Secret::new("USER_TOKEN")), "[redacted]");
    }
}
