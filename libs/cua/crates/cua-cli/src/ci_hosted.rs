//! `cua ci` against the hosted Cua CI service: `connect`, `status`,
//! `logs`, `config`, `disconnect` and the `serve` loop that runs jobs on
//! this computer. The service contract is in `cua-ci`'s `service_api`; this
//! module is the command layer over `cua_ci::hosted`.
//!
//! No GitHub credential is kept here: the hosted App's key stays with the
//! service, which gives this computer a one-job runner configuration per
//! job. The only GitHub authorization this module ever uses is the
//! user's own, for `--open-pr`, once, from `gh` or `GH_TOKEN`.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use cua_ci::CiPaths;
use cua_ci::hosted::{
    AgentTiming, ClassSizes, HostedAgent, HostedState, ServiceApi as _, ServiceClient, TokenSource,
    default_slots, rfc3339,
};
use cua_ci::job_runner::{ClassImage, SpaceJobRunner};
use cua_ci::service_api::*;
use cua_sdk::CuaError;

use crate::ci_cmd::err;
use crate::util::line;

/// The pool name the service (launchd or systemd) runs the hosted loop
/// under: `cua ci serve --pool hosted`.
pub const HOSTED_POOL: &str = "hosted";

// ---------------------------------------------------------------------
// Account tokens
// ---------------------------------------------------------------------

/// The signed-in Cua session as the service client's token source.
pub struct CliTokens;

#[async_trait]
impl TokenSource for CliTokens {
    async fn token(&self) -> cua_ci::Result<String> {
        use cua_host::AccountTokens as _;
        crate::host::SessionTokens
            .access_token()
            .await
            .map_err(|e| {
                cua_ci::Error::Service(ApiError {
                    status: 401,
                    code: "unauthenticated".into(),
                    detail: e.to_string(),
                    ..Default::default()
                })
            })
    }
}

pub(crate) fn client(account: &str) -> Result<ServiceClient, CuaError> {
    ServiceClient::new(
        &cua_home::env::spaces_api_url(),
        account,
        Arc::new(CliTokens),
    )
    .map_err(err)
}

/// The hosted state, or the way to make one.
fn connected(paths: &CiPaths) -> Result<HostedState, CuaError> {
    HostedState::load(paths).map_err(err)?.ok_or_else(|| {
        CuaError::NotFound("this computer is not connected to Cua CI; run `cua ci connect`".into())
    })
}

/// Whether this computer is connected to the hosted service.
pub fn is_connected(paths: &CiPaths) -> bool {
    matches!(HostedState::load(paths), Ok(Some(_)))
}

// ---------------------------------------------------------------------
// connect
// ---------------------------------------------------------------------

/// The options of a hosted `connect`.
#[derive(Clone, Debug, Default)]
pub struct Connect {
    /// A Cua organization to connect (id, alias or name); default: you.
    pub team: Option<String>,
    /// The GitHub organization to install on (a hint for the install page).
    pub github_org: Option<String>,
    /// Repositories to serve (`owner/name`).
    pub allow: Vec<String>,
    /// Open a pull request that points workflows at Cua CI.
    pub open_pr: bool,
    /// Only these workflow files (paths in the repository).
    pub workflows: Vec<String>,
    /// Do not ask.
    pub yes: bool,
    /// Print the sign-in and install URLs, do not open a browser.
    pub no_browser: bool,
    /// Where jobs' Spaces run: `local`, `auto` or `host:<computer>`.
    pub on: String,
    /// macOS jobs at once.
    pub macos_slots: Option<u32>,
    /// Linux jobs at once.
    pub linux_slots: Option<u32>,
    /// The macOS image.
    pub macos_image: Option<String>,
    /// The Linux image.
    pub linux_image: Option<String>,
    /// The name shown in `cua ci status`.
    pub name: Option<String>,
    /// Do not install the service (run `cua ci serve` yourself).
    pub no_service: bool,
    /// The GitHub REST API for `--open-pr` (default `https://api.github.com`).
    pub github_api: Option<String>,
}

/// The host facts registrations use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostFacts {
    /// `macos` or `linux`.
    pub os: &'static str,
    /// `arm64` or `x64`.
    pub arch: &'static str,
    /// Logical CPUs.
    pub cpus: u32,
    /// Memory in MiB (0: unknown).
    pub memory_mb: u64,
}

impl HostFacts {
    /// This computer.
    pub fn detect() -> Result<Self, CuaError> {
        let os = match std::env::consts::OS {
            "macos" => "macos",
            "linux" => "linux",
            other => {
                return Err(CuaError::Unsupported(format!(
                    "Cua CI serves jobs on macOS and Linux computers, not {other} yet"
                )));
            }
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => {
                return Err(CuaError::Unsupported(format!(
                    "Cua CI serves jobs on arm64 and x64 computers, not {other}"
                )));
            }
        };
        let host = cua_ci::config::HostResources::detect();
        Ok(Self {
            os,
            arch,
            cpus: host.map(|h| h.cpus).unwrap_or(4),
            memory_mb: host.map(|h| h.memory_mb).unwrap_or(0),
        })
    }
}

/// The slots, from the flags and the host: two macOS jobs on an Apple
/// silicon Mac, Linux by cores. A flag above what the host can run is an
/// error, never silently lowered.
pub fn slots_for(
    host: &HostFacts,
    macos: Option<u32>,
    linux: Option<u32>,
) -> Result<(u32, u32), CuaError> {
    let (dm, dl) = default_slots(host.os, host.arch, host.cpus);
    let m = macos.unwrap_or(dm);
    let l = linux.unwrap_or(dl);
    if m > 0 && !(host.os == "macos" && host.arch == "arm64") {
        return Err(CuaError::Unsupported(
            "macOS jobs need an Apple silicon Mac".into(),
        ));
    }
    if m > 2 {
        return Err(CuaError::InvalidArgument(
            "--macos-slots: a Mac runs at most 2 macOS VMs (Apple's license); use --on auto to \
             spread jobs over your Macs"
                .into(),
        ));
    }
    if l > 64 {
        return Err(CuaError::InvalidArgument(
            "--linux-slots: at most 64".into(),
        ));
    }
    if m == 0 && l == 0 {
        return Err(CuaError::InvalidArgument(
            "this computer would serve nothing: pass --macos-slots or --linux-slots".into(),
        ));
    }
    Ok((m, l))
}

/// vCPUs and memory per job of each class (a Mac's cores and memory split
/// between its macOS jobs; Linux jobs get up to four cores).
pub fn sizes_for(host: &HostFacts, macos_slots: u32, linux_slots: u32) -> ClassSizes {
    ClassSizes {
        macos_cpus: (macos_slots > 0).then(|| (host.cpus / macos_slots).max(2)),
        macos_memory_mb: (macos_slots > 0 && host.memory_mb > 0)
            .then(|| (host.memory_mb.saturating_sub(8192) / u64::from(macos_slots)).max(4096)),
        linux_cpus: (linux_slots > 0).then(|| (host.cpus / linux_slots).clamp(1, 4)),
        linux_memory_mb: None,
    }
}

fn class_images(state: &HostedState, sizes: ClassSizes) -> Vec<ClassImage> {
    let mut v = Vec::new();
    if state.macos_slots > 0 {
        let mut i = ClassImage::default_for(Class::Macos);
        if let Some(img) = &state.macos_image {
            i.image = img.clone();
        }
        i.cpus = sizes.macos_cpus;
        i.memory_mb = sizes.macos_memory_mb;
        v.push(i);
    }
    if state.linux_slots > 0 {
        let mut i = ClassImage::default_for(Class::Linux);
        if let Some(img) = &state.linux_image {
            i.image = img.clone();
        }
        i.cpus = sizes.linux_cpus;
        v.push(i);
    }
    v
}

fn machine_name(a: &Connect) -> String {
    a.name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(cua_host::device_name)
}

/// This computer's id: the device id when it has one, else one made once
/// and kept in the state file.
fn machine_id(previous: Option<&HostedState>, home: &std::path::Path) -> String {
    if let Some(p) = previous {
        return p.machine_id.clone();
    }
    if let Ok(auth) = crate::devices_cmd::device_auth(&crate::host::relay_url(None, home))
        && let Ok(Some(id)) = auth.device_id()
        && !id.is_empty()
    {
        return id;
    }
    format!("m-{:016x}", rand_u64())
}

fn rand_u64() -> u64 {
    // The state file keeps it; a clock and the process id are plenty for an
    // id that only has to differ between a user's computers.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    t ^ (u64::from(std::process::id()) << 32)
}

/// What the user picks on a list of repositories.
pub fn choose_repos(
    visible: &[String],
    allowed_now: &[String],
    flag: &[String],
) -> Result<Vec<String>, String> {
    let known: BTreeMap<String, &String> = visible.iter().map(|r| (r.to_lowercase(), r)).collect();
    if !flag.is_empty() {
        let mut out = Vec::new();
        for r in flag {
            let n = parse_repo(r, false)?;
            match known.get(&n.to_lowercase()) {
                Some(real) => {
                    if !out.contains(*real) {
                        out.push((*real).clone());
                    }
                }
                None => {
                    return Err(format!(
                        "{n}: the Cua CI GitHub App cannot see this repository. Add it to the \
                         installation at GitHub (Settings, Applications, Cua CI, Configure), \
                         then run this again"
                    ));
                }
            }
        }
        return Ok(out);
    }
    Ok(allowed_now.to_vec())
}

/// Connects this computer (the whole flow of `cua ci connect`).
pub async fn connect(
    a: Connect,
    paths: &CiPaths,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let home = crate::util::cua_home();
    let previous = HostedState::load(paths).map_err(err)?;
    let host = HostFacts::detect()?;
    let (macos_slots, linux_slots) = slots_for(&host, a.macos_slots, a.linux_slots)?;

    // 1. Signed in (the sign-in is the stored Cua session; the service keeps
    //    running as that account).
    {
        use cua_host::AccountTokens as _;
        if crate::host::SessionTokens.access_token().await.is_err() {
            crate::auth::login(
                &crate::auth::LoginOptions {
                    no_browser: a.no_browser,
                    ..Default::default()
                },
                out,
            )
            .await?;
        }
    }
    let account = match &a.team {
        Some(t) => crate::team_cmd::resolve_team(&home, t).await?,
        None => previous
            .as_ref()
            .map(|p| p.account.clone())
            .unwrap_or_else(|| "me".into()),
    };
    let api = client(&account)?;
    let acct = api.ensure_account().await.map_err(err)?;

    // 2. The hosted GitHub App.
    let installation = match pick_installation(&acct, previous.as_ref(), a.github_org.as_deref()) {
        Some(i) => i,
        None => install_app(&api, &a, out).await?,
    };

    // 3. Repositories.
    let (repos, etag) = api.repos().await.map_err(err)?;
    let visible: Vec<String> = repos
        .repos
        .iter()
        .filter(|r| r.visible)
        .map(|r| r.full_name.clone())
        .collect();
    let allowed_now: Vec<String> = repos
        .repos
        .iter()
        .filter(|r| r.allowed)
        .map(|r| r.full_name.clone())
        .collect();
    let mut chosen =
        choose_repos(&visible, &allowed_now, &a.allow).map_err(CuaError::InvalidArgument)?;
    if chosen.is_empty() && !visible.is_empty() {
        chosen = ask_repos(&visible, a.yes, out)?;
    }
    if chosen != allowed_now {
        let list = AllowList {
            installation: installation.id.clone(),
            allow: chosen.clone(),
            overrides: repos
                .repos
                .iter()
                .filter(|r| {
                    chosen.contains(&r.full_name)
                        && (r.fork_pr_policy.is_some() || r.max_concurrent.is_some())
                })
                .map(|r| {
                    (
                        r.full_name.clone(),
                        RepoOverride {
                            fork_pr_policy: r.fork_pr_policy,
                            max_concurrent: r.max_concurrent,
                        },
                    )
                })
                .collect(),
        };
        list.validate(false).map_err(CuaError::InvalidArgument)?;
        api.put_allow(&list, etag.as_deref(), false)
            .await
            .map_err(err)?;
    }

    // 4. This computer.
    let state = HostedState {
        account: account.clone(),
        installation: installation.id.clone(),
        github_login: installation.github_login.clone(),
        machine_id: machine_id(previous.as_ref(), &home),
        machine_name: machine_name(&a),
        macos_slots,
        linux_slots,
        macos_image: a
            .macos_image
            .clone()
            .or_else(|| previous.as_ref().and_then(|p| p.macos_image.clone())),
        linux_image: a
            .linux_image
            .clone()
            .or_else(|| previous.as_ref().and_then(|p| p.linux_image.clone())),
        on: if a.on.trim().is_empty() {
            "local".into()
        } else {
            a.on.clone()
        },
        connected_at: rfc3339(cua_ci::unix_ms()),
    };
    let sizes = sizes_for(&host, macos_slots, linux_slots);
    let reg = state.registration(host.os, host.arch, sizes);
    api.register(&state.machine_id, &reg).await.map_err(err)?;
    state.save(paths).map_err(err)?;

    // 5. The service that serves jobs.
    let service_path = if a.no_service {
        None
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| CuaError::Internal(format!("this cua's path: {e}")))?;
        let spec = cua_ci::service::ServiceSpec {
            pool: HOSTED_POOL.into(),
            exe,
            cua_home: std::env::var_os("CUA_HOME")
                .filter(|v| !v.is_empty())
                .map(|_| cua_home::cua_home()),
            log: paths.serve_log(HOSTED_POOL),
        };
        Some(cua_ci::service::install(&spec).map_err(err)?)
    };

    // 6. The workflow pull request.
    let pr = if a.open_pr {
        Some(open_prs(&a, &chosen, out, json).await?)
    } else {
        None
    };

    if json {
        line(
            out,
            serde_json::json!({
                "account_id": acct.account_id,
                "installation": installation.id,
                "github": installation.github_login,
                "repos": chosen,
                "machine_id": state.machine_id,
                "machine": state.machine_name,
                "macos_slots": macos_slots,
                "linux_slots": linux_slots,
                "service": service_path,
                "runs_on": runs_on_line(),
                "pull_requests": pr,
            })
            .to_string(),
        );
        return Ok(0);
    }
    line(out, connected_line(&chosen));
    line(
        out,
        format!(
            "  organization  {}",
            if installation.github_login.is_empty() {
                "your GitHub organization".to_string()
            } else {
                installation.github_login.clone()
            }
        ),
    );
    line(
        out,
        format!(
            "  repositories  {}",
            if chosen.is_empty() {
                "none yet (choose with `cua ci config --allow OWNER/NAME`)".to_string()
            } else {
                chosen.join(", ")
            }
        ),
    );
    line(
        out,
        format!(
            "  capacity      {}",
            capacity_text(macos_slots, linux_slots)
        ),
    );
    match &service_path {
        Some(p) => line(out, format!("  service       {} (running)", p.display())),
        None => line(out, "  service       none: run `cua ci serve`"),
    }
    line(out, "Put this in a workflow:");
    line(out, format!("  {}", runs_on_line()));
    line(
        out,
        "More capacity when you want it: `cua ci add aws` (or gcp, azure, hetzner, modal, macstadium, \
         or `host USER@ADDRESS`).",
    );
    line(
        out,
        "Use cua-macos or cua-linux to pick a class. Fork pull requests run without secrets and \
         with read-only caches (`cua ci config` changes that).",
    );
    if let Some(prs) = pr {
        for p in prs {
            line(out, format!("Opened a pull request: {p}"));
        }
    } else if !chosen.is_empty() {
        line(
            out,
            "Or let Cua open a pull request that changes your workflows: `cua ci connect --open-pr`.",
        );
    }
    Ok(0)
}

/// The line that says it works.
pub fn connected_line(repos: &[String]) -> String {
    match repos.len() {
        0 => {
            "Connected. No repository can use runs-on: cua yet. This computer serves jobs once you \
              choose some."
                .to_string()
        }
        1 => "Connected. 1 repository can use runs-on: cua. This computer serves jobs now."
            .to_string(),
        n => format!(
            "Connected. {n} repositories can use runs-on: cua. This computer serves jobs now."
        ),
    }
}

fn capacity_text(macos: u32, linux: u32) -> String {
    let mut p = Vec::new();
    if macos > 0 {
        p.push(format!(
            "{macos} macOS job{} at once",
            if macos == 1 { "" } else { "s" }
        ));
    }
    if linux > 0 {
        p.push(format!(
            "{linux} Linux job{} at once",
            if linux == 1 { "" } else { "s" }
        ));
    }
    p.join(", ")
}

/// The linked installation to use: the one this computer used, the only
/// one, or the one named by `github_org`.
pub fn pick_installation(
    acct: &Account,
    previous: Option<&HostedState>,
    github_org: Option<&str>,
) -> Option<Installation> {
    let live: Vec<&Installation> = acct.installations.iter().filter(|i| !i.suspended).collect();
    if let Some(org) = github_org {
        return live
            .iter()
            .find(|i| i.github_login.eq_ignore_ascii_case(org))
            .map(|i| (*i).clone());
    }
    if let Some(p) = previous
        && let Some(i) = live.iter().find(|i| i.id == p.installation)
    {
        return Some((*i).clone());
    }
    match live.as_slice() {
        [one] => Some((*one).clone()),
        _ => None,
    }
}

async fn install_app(
    api: &ServiceClient,
    a: &Connect,
    out: &mut dyn Write,
) -> Result<Installation, CuaError> {
    let session = api
        .install_start(&InstallStart {
            loopback_port: None,
            github_login: a.github_org.clone(),
        })
        .await
        .map_err(err)?;
    line(
        out,
        "Opening GitHub to install the Cua CI app. Pick your organization and the repositories:",
    );
    line(out, format!("  {}", session.url));
    if a.no_browser || !crate::util::open_browser(&session.url) {
        line(out, "Open the URL above in a browser to continue.");
    } else {
        line(out, "Waiting for you to finish on GitHub...");
    }
    let _ = out.flush();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    let every = std::time::Duration::from_secs(u64::from(session.poll_interval_s.clamp(1, 10)));
    loop {
        tokio::time::sleep(every).await;
        let st = api.install_state(&session.session_id).await.map_err(err)?;
        match st.status {
            InstallStatus::Installed => {
                return st.installation.ok_or_else(|| {
                    CuaError::Http("the service said installed but sent no installation".into())
                });
            }
            InstallStatus::Denied => {
                return Err(CuaError::Cancelled(match st.error.as_deref() {
                    Some("personal_account_unsupported") => {
                        "Cua CI installs on a GitHub organization for now (not a personal \
                         account). Create or pick an organization, or use `cua ci connect \
                         --own-app` with your own GitHub App."
                            .to_string()
                    }
                    Some(e) => format!("the GitHub install was not completed ({e})"),
                    None => "the GitHub install was cancelled".to_string(),
                }));
            }
            InstallStatus::Expired => {
                return Err(CuaError::Timeout(
                    "the GitHub install link expired; run `cua ci connect` again".into(),
                ));
            }
            InstallStatus::Pending => {}
        }
        if std::time::Instant::now() > deadline {
            return Err(CuaError::Timeout(
                "no GitHub install within 10 minutes; run `cua ci connect` again".into(),
            ));
        }
    }
}

/// A GitHub install the Cua Spaces app started and is waiting for.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PendingInstall {
    session_id: String,
    url: String,
    account: String,
    started_ms: u64,
}

/// Where the install the app is waiting for is kept.
fn pending_install_path(paths: &CiPaths) -> std::path::PathBuf {
    paths.root().join("github-install.json")
}

/// How long an install link may wait before the next try starts a new one.
const INSTALL_WAIT_MS: u64 = 10 * 60 * 1000;

/// Where a GitHub install the app started has got to.
#[derive(Clone, Debug, PartialEq)]
pub enum InstallProgress {
    /// The person has not finished on GitHub; open this page.
    Waiting {
        /// The GitHub page of the install.
        url: String,
    },
    /// Linked.
    Installed(Installation),
    /// It cannot go on, and why, in a sentence.
    Failed(String),
}

/// One step of the GitHub install without blocking: starts an install and
/// returns its page, or, when one is already waiting, asks the service once
/// where it has got to. The waiting install is kept in the CI folder, so the
/// next call (the app calls it every few seconds) carries on from it.
pub async fn install_progress(
    api: &ServiceClient,
    account: &str,
    github_org: Option<&str>,
    paths: &CiPaths,
) -> Result<InstallProgress, CuaError> {
    let file = pending_install_path(paths);
    let now = cua_ci::unix_ms();
    let kept: Option<PendingInstall> = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|p: &PendingInstall| {
            p.account == account && now.saturating_sub(p.started_ms) < INSTALL_WAIT_MS
        });
    if let Some(p) = kept {
        let st = api.install_state(&p.session_id).await.map_err(err)?;
        return Ok(match st.status {
            InstallStatus::Pending => InstallProgress::Waiting { url: p.url },
            InstallStatus::Installed => {
                let _ = std::fs::remove_file(&file);
                match st.installation {
                    Some(i) => InstallProgress::Installed(i),
                    None => InstallProgress::Failed(
                        "GitHub said the app was installed but sent no installation".into(),
                    ),
                }
            }
            InstallStatus::Denied => {
                let _ = std::fs::remove_file(&file);
                InstallProgress::Failed(match st.error.as_deref() {
                    Some("personal_account_unsupported") => {
                        "Cua CI installs on a GitHub organization for now, not a personal \
                         account. Pick an organization, or use a token."
                            .to_string()
                    }
                    Some(e) => format!("The GitHub install was not completed ({e})"),
                    None => "The GitHub install was cancelled".to_string(),
                })
            }
            InstallStatus::Expired => {
                let _ = std::fs::remove_file(&file);
                InstallProgress::Failed("The GitHub link expired. Connect again.".into())
            }
        });
    }
    let session = api
        .install_start(&InstallStart {
            loopback_port: None,
            github_login: github_org.map(str::to_string),
        })
        .await
        .map_err(err)?;
    let pending = PendingInstall {
        session_id: session.session_id,
        url: session.url.clone(),
        account: account.to_string(),
        started_ms: now,
    };
    if let Some(d) = file.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(
        &file,
        serde_json::to_string(&pending).map_err(|e| CuaError::Internal(e.to_string()))?,
    )
    .map_err(|e| CuaError::Internal(format!("{}: {e}", file.display())))?;
    Ok(InstallProgress::Waiting { url: session.url })
}

/// Forgets a GitHub install the app was waiting for.
pub fn cancel_install(paths: &CiPaths) {
    let _ = std::fs::remove_file(pending_install_path(paths));
}

/// Which repositories to serve when none are chosen yet: all the App can
/// see (when a few) after a yes, else a typed list.
fn ask_repos(visible: &[String], yes: bool, out: &mut dyn Write) -> Result<Vec<String>, CuaError> {
    if yes {
        return Ok(visible.to_vec());
    }
    if !crate::util::interactive() {
        line(
            out,
            "No repositories chosen yet: pass --allow OWNER/NAME (the app can see: ".to_string()
                + &visible
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
                + if visible.len() > 10 { ", ..." } else { "" }
                + ").",
        );
        return Ok(vec![]);
    }
    line(out, "The Cua CI app can see these repositories:");
    for r in visible.iter().take(50) {
        line(out, format!("  {r}"));
    }
    if visible.len() > 50 {
        line(out, format!("  and {} more", visible.len() - 50));
    }
    if visible.len() <= 50 && crate::util::confirm("Run jobs of all of them on Cua CI?", false) {
        return Ok(visible.to_vec());
    }
    let Some(typed) =
        crate::util::prompt_line("Repositories to serve (owner/name, comma separated): ")
    else {
        return Ok(vec![]);
    };
    let wanted: Vec<String> = typed
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    choose_repos(visible, &[], &wanted).map_err(CuaError::InvalidArgument)
}

async fn open_prs(
    a: &Connect,
    repos: &[String],
    out: &mut dyn Write,
    json: bool,
) -> Result<Vec<String>, CuaError> {
    if repos.is_empty() {
        return Err(CuaError::InvalidArgument(
            "--open-pr needs repositories: pass --allow OWNER/NAME".into(),
        ));
    }
    if repos.len() > 5 {
        return Err(CuaError::InvalidArgument(
            "--open-pr opens one pull request per repository; pass --allow with up to 5".into(),
        ));
    }
    let token = cua_ci::workflow_pr::user_token().map_err(err)?;
    let pr = cua_ci::workflow_pr::PrClient::new(
        a.github_api.as_deref().unwrap_or("https://api.github.com"),
        token,
    )
    .map_err(err)?;
    let mut urls = Vec::new();
    for repo in repos {
        let (base, _private, files) = pr.workflows(repo).await.map_err(err)?;
        let mut edits = Vec::new();
        let mut plans = BTreeMap::new();
        for f in files {
            if !a.workflows.is_empty()
                && !a
                    .workflows
                    .iter()
                    .any(|w| f.path.ends_with(w.trim_start_matches("./")))
            {
                continue;
            }
            let plan = cua_ci::workflow_pr::plan_file(&f.text);
            if !json {
                if let Some(why) = &plan.skipped {
                    line(out, format!("  {repo} {}: skipped, {why}", f.path));
                }
                for l in plan.lines.iter().filter(|l| l.to.is_none()) {
                    line(
                        out,
                        format!(
                            "  {repo} {} line {}: left as `runs-on: {}` ({})",
                            f.path,
                            l.line,
                            l.from,
                            l.note.clone().unwrap_or_default()
                        ),
                    );
                }
            }
            if plan.changed() > 0 {
                edits.push((f.clone(), plan.new_text.clone()));
            }
            plans.insert(f.path.clone(), plan);
        }
        if edits.is_empty() {
            if !json {
                line(
                    out,
                    format!("{repo}: no workflow names a GitHub-hosted runner, nothing to change."),
                );
            }
            continue;
        }
        let body = cua_ci::workflow_pr::pr_body(&cua_ci::workflow_pr::summarize(&plans));
        let r = pr
            .open_pr(repo, &base, &edits, "Run workflows on Cua CI", &body)
            .await
            .map_err(err)?;
        urls.push(r.url);
    }
    Ok(urls)
}

// ---------------------------------------------------------------------
// status
// ---------------------------------------------------------------------

fn secs(s: Option<f64>) -> String {
    match s {
        Some(v) if v < 10.0 => format!("{v:.1} s"),
        Some(v) => format!("{v:.0} s"),
        None => "none yet".into(),
    }
}

fn age(s: u64) -> String {
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

/// The text of `cua ci status` (no middle dots, plain words).
pub fn status_text(st: &Status, state: Option<&HostedState>) -> Vec<String> {
    let mut v = Vec::new();
    let who = st
        .installations
        .iter()
        .map(|i| i.github_login.clone())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    v.push(format!(
        "Cua CI for {}",
        if who.is_empty() {
            "no GitHub organization yet"
        } else {
            &who
        }
    ));
    if st.machines.is_empty() {
        v.push("  computers  none: run `cua ci connect`".into());
    }
    for m in &st.machines {
        let classes: Vec<String> = m
            .classes
            .iter()
            .map(|c| {
                format!(
                    "{} {} of {} free",
                    if c.class == "macos" { "macOS" } else { "Linux" },
                    c.free_slots,
                    c.max_slots
                )
            })
            .collect();
        let me = state.is_some_and(|s| s.machine_id == m.machine_id);
        v.push(format!(
            "  computer   {}{}  {}  {}{}",
            m.name,
            if me { " (this one)" } else { "" },
            m.state,
            classes.join(", "),
            if m.kind == "byoc" { "  cloud" } else { "" }
        ));
    }
    if st.queue.is_empty() {
        v.push("  waiting   none".into());
    } else {
        v.push(format!("  waiting   {}", st.queue.len()));
        for q in st.queue.iter().take(10) {
            v.push(format!(
                "            {} {} for {}{}",
                q.repo,
                q.class,
                age(q.waiting_s),
                match (&q.state[..], &q.reason) {
                    ("unserved", Some(r)) => format!(", not served: {r}"),
                    ("unserved", None) => ", not served".into(),
                    _ => String::new(),
                }
            ));
        }
    }
    if st.running.is_empty() {
        v.push("  running   none".into());
    } else {
        v.push(format!("  running   {}", st.running.len()));
        for r in st.running.iter().take(10) {
            let machine = st
                .machines
                .iter()
                .find(|m| m.machine_id == r.machine_id)
                .map(|m| m.name.as_str())
                .unwrap_or("a computer");
            v.push(format!("            {} on {}", r.repo, machine));
        }
    }
    let w = &st.last_7d;
    v.push(format!(
        "  7 days    {} jobs, {:.0} minutes, {} passed, {} failed, {} cancelled; start median {}, slowest 1 in 20 {}",
        w.jobs, w.minutes, w.success, w.failure, w.cancelled,
        secs(w.median_start_s), secs(w.p95_start_s)
    ));
    if w.infra_failures > 0 {
        v.push(format!(
            "            {} stopped by Cua CI itself, not by the job",
            w.infra_failures
        ));
    }
    let m = &st.month;
    let mut month = format!("  month     {:.0} minutes", m.minutes);
    match m.budget_micros {
        Some(b) => month.push_str(&format!(
            "; cloud spend {} of {} budget{}",
            format_dollars(m.cost_micros),
            format_dollars(b),
            m.budget_fraction
                .map(|f| format!(" ({:.0} percent)", f * 100.0))
                .unwrap_or_default()
        )),
        None => month.push_str(&format!(
            "; cloud spend {} (no budget set: cloud computers are not used)",
            format_dollars(m.cost_micros)
        )),
    }
    v.push(month);
    for n in &st.notices {
        v.push(format!("Note: {n}"));
    }
    v
}

pub async fn status(paths: &CiPaths, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let state = connected(paths)?;
    let st = client(&state.account)?.status().await.map_err(err)?;
    let store = cua_ci::targets::TargetStore::new(paths);
    let targets = store.list();
    if json {
        let mut v = serde_json::to_value(&st).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.insert(
                "targets".into(),
                serde_json::Value::Array(
                    targets
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "target": t,
                                "report": store.read_report(&t.name),
                            })
                        })
                        .collect(),
                ),
            );
        }
        line(out, v.to_string());
        return Ok(0);
    }
    for l in status_text(&st, Some(&state)) {
        line(out, l);
    }
    for l in crate::ci_targets::status_lines(paths) {
        line(out, l);
    }
    if targets.iter().all(|t| !t.kind.is_cloud()) && st.queue.iter().any(|q| q.state != "offered") {
        line(
            out,
            "Note: a job is waiting for a computer. Add capacity: cua ci add aws",
        );
    }
    Ok(0)
}

// ---------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------

pub async fn logs(
    paths: &CiPaths,
    job: Option<String>,
    repo: Option<String>,
    follow: bool,
    limit: u32,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let state = connected(paths)?;
    let api = client(&state.account)?;
    let Some(job) = job else {
        let page = api
            .jobs(None, repo.as_deref(), limit.clamp(1, 200))
            .await
            .map_err(err)?;
        if json {
            line(out, serde_json::to_string(&page).unwrap_or_default());
            return Ok(0);
        }
        if page.jobs.is_empty() {
            line(out, "No jobs yet.");
            return Ok(0);
        }
        for j in &page.jobs {
            line(
                out,
                format!(
                    "{}  {}  {}  {}  {}",
                    j.job_key,
                    j.repo,
                    if j.job_name.is_empty() {
                        &j.workflow
                    } else {
                        &j.job_name
                    },
                    match (&j.state[..], &j.result) {
                        ("finished", Some(r)) => r.clone(),
                        (s, _) => s.to_string(),
                    },
                    j.minutes.map(|m| format!("{m:.1} min")).unwrap_or_default()
                ),
            );
        }
        line(out, "Show one job's Cua CI log with `cua ci logs JOB`.");
        return Ok(0);
    };
    if !json {
        let detail = api.job(&job).await.map_err(err)?;
        if let Some(tl) = detail["timeline"].as_array() {
            for e in tl {
                line(
                    out,
                    format!(
                        "{}  {}{}",
                        e["at"].as_str().unwrap_or_default(),
                        e["kind"].as_str().unwrap_or_default(),
                        e["detail"]
                            .as_str()
                            .filter(|d| !d.is_empty())
                            .map(|d| format!(": {d}"))
                            .unwrap_or_default()
                    ),
                );
            }
        }
    }
    let mut after = 0u64;
    loop {
        let page = api.logs(&job, after, 500).await.map_err(err)?;
        for l in &page.lines {
            if json {
                line(out, serde_json::to_string(l).unwrap_or_default());
            } else {
                line(out, format!("{} {}", l.at, l.text));
            }
        }
        let _ = out.flush();
        after = page.next_seq;
        if page.complete || !follow {
            return Ok(0);
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

// ---------------------------------------------------------------------
// config
// ---------------------------------------------------------------------

/// What `cua ci config` was asked to change.
#[derive(Clone, Debug, Default)]
pub struct ConfigChange {
    /// Repositories to add to the allow list.
    pub allow: Vec<String>,
    /// Repositories to remove.
    pub remove: Vec<String>,
    /// `owner/*` entries are accepted.
    pub allow_org_wide: bool,
    /// `restricted`, `deny` or `allow`.
    pub fork_prs: Option<String>,
    /// Apply `fork_prs` and `max_jobs` to this repository only.
    pub repo: Option<String>,
    /// Concurrent jobs, or `none`.
    pub max_jobs: Option<String>,
    /// Concurrent macOS jobs, or `none`.
    pub max_macos_jobs: Option<String>,
    /// Longest job in minutes, or `none`.
    pub max_job_minutes: Option<String>,
    /// How long a job may wait for capacity (`15m`), or `none`.
    pub queue_timeout: Option<String>,
    /// Monthly budget in dollars, or `none`.
    pub budget: Option<String>,
    /// `stop` or `warn`.
    pub on_budget_limit: Option<String>,
    /// `byoc` or `all`.
    pub budget_scope: Option<String>,
    /// `deny-private` or `off`.
    pub egress: Option<String>,
    /// Confirmed the risky ones.
    pub yes: bool,
}

impl ConfigChange {
    /// Whether anything changes.
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty()
            && self.remove.is_empty()
            && self.fork_prs.is_none()
            && self.max_jobs.is_none()
            && self.max_macos_jobs.is_none()
            && self.max_job_minutes.is_none()
            && self.queue_timeout.is_none()
            && self.budget.is_none()
            && self.on_budget_limit.is_none()
            && self.budget_scope.is_none()
            && self.egress.is_none()
    }
}

fn cap(flag: &str, v: &str) -> Result<Option<u32>, String> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    match v.parse::<u32>() {
        Ok(n) if n >= 1 => Ok(Some(n)),
        _ => Err(format!("{flag}: a number of 1 or more, or none")),
    }
}

fn duration_s(flag: &str, v: &str) -> Result<Option<u32>, String> {
    if v.trim().eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let d = humantime::parse_duration(v.trim())
        .map_err(|e| format!("{flag}: {e} (for example 15m)"))?;
    let s = d.as_secs();
    if s == 0 || s > u64::from(u32::MAX) {
        return Err(format!("{flag}: 1 second or more"));
    }
    Ok(Some(s as u32))
}

/// Applies `c` to `settings` and `allow`, returning one plain sentence per
/// change. Nothing is sent here. A fork policy of `allow` and egress `off`
/// need `yes`.
pub fn apply_config(
    settings: &mut Settings,
    allow: &mut AllowList,
    c: &ConfigChange,
) -> Result<Vec<String>, String> {
    let mut said = Vec::new();
    for r in &c.allow {
        let n = parse_repo(r, c.allow_org_wide)?;
        if !allow.allow.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
            allow.allow.push(n.clone());
            said.push(format!("serving {n}"));
        }
    }
    for r in &c.remove {
        let n = parse_repo(r, true)?;
        let before = allow.allow.len();
        allow.allow.retain(|x| !x.eq_ignore_ascii_case(&n));
        allow.overrides.retain(|k, _| !k.eq_ignore_ascii_case(&n));
        if allow.allow.len() == before {
            return Err(format!("{n} is not on the list"));
        }
        said.push(format!("no longer serving {n}"));
    }
    if let Some(p) = &c.fork_prs {
        let policy = ForkPrPolicy::parse(p)?;
        if policy == ForkPrPolicy::Allow && !c.yes {
            return Err(
                "--fork-prs allow treats a stranger's pull request like your own code (caches \
                 and network); GitHub still sends it no secrets. Add --yes if you mean it"
                    .into(),
            );
        }
        match &c.repo {
            Some(r) => {
                let n = parse_repo(r, false)?;
                let key = allow
                    .allow
                    .iter()
                    .find(|x| x.eq_ignore_ascii_case(&n))
                    .cloned()
                    .ok_or_else(|| format!("{n} is not on the list; add it with --allow"))?;
                allow.overrides.entry(key).or_default().fork_pr_policy = Some(policy);
                said.push(format!("fork pull requests of {n}: {}", policy.as_str()));
            }
            None => {
                settings.defaults.fork_pr_policy = policy;
                said.push(format!("fork pull requests: {}", policy.as_str()));
            }
        }
    }
    if let Some(v) = &c.max_jobs {
        let n = cap("--max-jobs", v)?;
        match &c.repo {
            Some(r) => {
                let name = parse_repo(r, false)?;
                let key = allow
                    .allow
                    .iter()
                    .find(|x| x.eq_ignore_ascii_case(&name))
                    .cloned()
                    .ok_or_else(|| format!("{name} is not on the list; add it with --allow"))?;
                allow.overrides.entry(key).or_default().max_concurrent = n;
                said.push(format!(
                    "jobs at once for {name}: {}",
                    n.map(|n| n.to_string()).unwrap_or("no cap".into())
                ));
            }
            None => {
                settings.caps.max_concurrent_jobs = n;
                said.push(format!(
                    "jobs at once: {}",
                    n.map(|n| n.to_string()).unwrap_or("no cap".into())
                ));
            }
        }
    }
    if let Some(v) = &c.max_macos_jobs {
        settings.caps.max_concurrent_macos = cap("--max-macos-jobs", v)?;
        said.push(format!("macOS jobs at once: {v}"));
    }
    if let Some(v) = &c.max_job_minutes {
        settings.caps.max_job_minutes = cap("--max-job-minutes", v)?;
        said.push(format!("longest job: {v} minutes"));
    }
    if let Some(v) = &c.queue_timeout {
        settings.caps.queue_timeout_s = duration_s("--queue-timeout", v)?;
        said.push(format!("a job waits for capacity at most {v}"));
    }
    if let Some(v) = &c.budget {
        if v.trim().eq_ignore_ascii_case("none") {
            settings.budget.monthly_micros = None;
            said.push("no monthly budget: cloud computers are not used".into());
        } else {
            let m = parse_dollars(v)?;
            if m == 0 {
                return Err("--budget: more than 0, or none".into());
            }
            settings.budget.monthly_micros = Some(m);
            said.push(format!(
                "monthly budget for cloud computers: {}",
                format_dollars(m)
            ));
        }
    }
    if let Some(v) = &c.on_budget_limit {
        settings.budget.on_limit = match v.as_str() {
            "stop" => OnLimit::Stop,
            "warn" => OnLimit::Warn,
            _ => return Err("--on-budget-limit: stop or warn".into()),
        };
        said.push(format!("at the budget limit: {v}"));
    }
    if let Some(v) = &c.budget_scope {
        settings.budget.scope = match v.as_str() {
            "byoc" => BudgetScope::Byoc,
            "all" => BudgetScope::All,
            _ => return Err("--budget-scope: byoc or all".into()),
        };
        said.push(format!("the budget counts: {v}"));
    }
    if let Some(v) = &c.egress {
        if v == "off" && !c.yes {
            return Err(
                "--egress off lets job Spaces reach your private network, tailnet and this \
                 computer's neighbours. Add --yes if you mean it"
                    .into(),
            );
        }
        if !matches!(v.as_str(), "deny-private" | "off") {
            return Err("--egress: deny-private or off".into());
        }
        settings.defaults.egress = v.clone();
        said.push(format!("network for jobs: {v}"));
    }
    settings.validate()?;
    allow.validate(c.allow_org_wide)?;
    Ok(said)
}

fn config_text(settings: &Settings, repos: &Repos) -> Vec<String> {
    let mut v = Vec::new();
    let allowed: Vec<&RepoRow> = repos.repos.iter().filter(|r| r.allowed).collect();
    if allowed.is_empty() {
        v.push("Repositories served  none (add with `cua ci config --allow OWNER/NAME`)".into());
    } else {
        v.push("Repositories served".into());
        for r in allowed {
            let mut extra = Vec::new();
            if let Some(p) = r.fork_pr_policy {
                extra.push(format!("fork pull requests {}", p.as_str()));
            }
            if let Some(n) = r.max_concurrent {
                extra.push(format!("{n} jobs at once"));
            }
            v.push(format!(
                "  {}{}",
                r.full_name,
                if extra.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", extra.join(", "))
                }
            ));
        }
    }
    let others = repos
        .repos
        .iter()
        .filter(|r| r.visible && !r.allowed)
        .count();
    if others > 0 {
        v.push(format!("  {others} more visible to the app, not served"));
    }
    v.push(format!(
        "Fork pull requests    {}",
        settings.defaults.fork_pr_policy.as_str()
    ));
    v.push(format!(
        "Network for jobs      {}",
        settings.defaults.egress
    ));
    let c = &settings.caps;
    let n = |o: Option<u32>, unit: &str| o.map(|n| format!("{n}{unit}")).unwrap_or("no cap".into());
    v.push(format!(
        "Jobs at once          {}",
        n(c.max_concurrent_jobs, "")
    ));
    v.push(format!(
        "macOS jobs at once    {}",
        n(c.max_concurrent_macos, "")
    ));
    v.push(format!(
        "Longest job           {}",
        n(c.max_job_minutes, " minutes")
    ));
    v.push(format!(
        "Wait for capacity     {}",
        c.queue_timeout_s
            .map(|s| age(u64::from(s)))
            .unwrap_or("no limit".into())
    ));
    match settings.budget.monthly_micros {
        Some(b) => v.push(format!(
            "Monthly budget        {} for {} ({} at the limit)",
            format_dollars(b),
            match settings.budget.scope {
                BudgetScope::Byoc => "cloud computers",
                BudgetScope::All => "all minutes",
            },
            match settings.budget.on_limit {
                OnLimit::Stop => "stop",
                OnLimit::Warn => "warn",
            }
        )),
        None => v.push(
            "Monthly budget        none (cloud computers are not used until you set one)".into(),
        ),
    }
    v
}

pub async fn config(
    paths: &CiPaths,
    c: ConfigChange,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let state = connected(paths)?;
    let api = client(&state.account)?;
    let (mut settings, settings_etag) = api.settings().await.map_err(err)?;
    let (repos, repos_etag) = api.repos().await.map_err(err)?;
    let mut allow = AllowList {
        installation: if repos.installation.is_empty() {
            state.installation.clone()
        } else {
            repos.installation.clone()
        },
        allow: repos
            .repos
            .iter()
            .filter(|r| r.allowed)
            .map(|r| r.full_name.clone())
            .collect(),
        overrides: repos
            .repos
            .iter()
            .filter(|r| r.allowed && (r.fork_pr_policy.is_some() || r.max_concurrent.is_some()))
            .map(|r| {
                (
                    r.full_name.clone(),
                    RepoOverride {
                        fork_pr_policy: r.fork_pr_policy,
                        max_concurrent: r.max_concurrent,
                    },
                )
            })
            .collect(),
    };
    if c.is_empty() {
        if json {
            line(
                out,
                serde_json::json!({"settings": settings, "repos": repos}).to_string(),
            );
        } else {
            for l in config_text(&settings, &repos) {
                line(out, l);
            }
        }
        return Ok(0);
    }
    let before_allow = allow.clone();
    let said = apply_config(&mut settings, &mut allow, &c).map_err(CuaError::InvalidArgument)?;
    // Visible check for new repositories before anything is sent.
    for r in &c.allow {
        if let Ok(n) = parse_repo(r, false)
            && !repos
                .repos
                .iter()
                .any(|x| x.visible && x.full_name.eq_ignore_ascii_case(&n))
        {
            return Err(CuaError::InvalidArgument(format!(
                "{n}: the Cua CI GitHub App cannot see this repository. Add it to the installation at GitHub, then run this again"
            )));
        }
    }
    if allow != before_allow {
        api.put_allow(&allow, repos_etag.as_deref(), c.allow_org_wide)
            .await
            .map_err(err)?;
    }
    api.put_settings(&settings, settings_etag.as_deref())
        .await
        .map_err(err)?;
    if json {
        line(out, serde_json::json!({"changed": said}).to_string());
    } else {
        for s in said {
            line(out, format!("Set: {s}."));
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------
// disconnect
// ---------------------------------------------------------------------

/// What `cua ci disconnect` removes.
#[derive(Clone, Debug, Default)]
pub struct Disconnect {
    /// Remove every computer of the account, not just this one.
    pub all_machines: bool,
    /// Also unlink the GitHub installation (removes the Cua runner group
    /// and runners).
    pub unlink: bool,
    /// Also uninstall the App at GitHub.
    pub uninstall: bool,
}

pub async fn disconnect(
    paths: &CiPaths,
    d: Disconnect,
    cua: Arc<cua_sdk::Cua>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let state = connected(paths)?;
    let api = client(&state.account)?;
    let mut said = Vec::new();
    // This computer stops serving first: the service, then the Spaces of its
    // jobs, then its place at the service.
    let was = cua_ci::service::stop(HOSTED_POOL).map_err(err)?;
    let _ = cua_ci::service::uninstall(HOSTED_POOL);
    if was {
        said.push("stopped the service".to_string());
    }
    let runner = SpaceJobRunner::new(
        Arc::new(crate::ci_sdk::SdkSpaces::new(cua)),
        Arc::new(NoTarballs),
        paths.clone(),
        &state.machine_id,
        vec![],
        &state.on,
    );
    match runner.sweep().await {
        Ok(0) => {}
        Ok(n) => said.push(format!(
            "deleted {n} job Space{}",
            if n == 1 { "" } else { "s" }
        )),
        Err(e) => said.push(format!("could not list Spaces to clean up ({e})")),
    }
    let ids: Vec<String> = if d.all_machines {
        api.machines()
            .await
            .map_err(err)?
            .into_iter()
            .map(|m| m.machine_id)
            .collect()
    } else {
        vec![state.machine_id.clone()]
    };
    for id in &ids {
        match api.delete_machine(id).await {
            Ok(()) => {}
            Err(cua_ci::Error::Service(e)) if e.status == 404 => {}
            Err(e) => return Err(err(e)),
        }
    }
    said.push(if ids.len() == 1 {
        "removed this computer from Cua CI".to_string()
    } else {
        format!("removed {} computers from Cua CI", ids.len())
    });
    if d.unlink || d.uninstall {
        api.delete_installation(&state.installation, d.uninstall)
            .await
            .map_err(err)?;
        said.push(if d.uninstall {
            "uninstalled the Cua CI app from GitHub".to_string()
        } else {
            "unlinked the GitHub installation".to_string()
        });
    }
    HostedState::remove(paths).map_err(err)?;
    if json {
        line(out, serde_json::json!({"disconnected": said}).to_string());
    } else {
        for s in &said {
            line(out, format!("Done: {s}."));
        }
        if !d.unlink && !d.uninstall {
            line(
                out,
                "The Cua CI app stays installed on GitHub; `cua ci connect` serves jobs again.",
            );
        }
    }
    Ok(0)
}

/// A tarball source that is never asked (a sweep needs a runner object,
/// not a runner download).
struct NoTarballs;

#[async_trait]
impl cua_ci::cache::TarballSource for NoTarballs {
    async fn tarball(&self, _platform: &str) -> cua_ci::Result<cua_ci::cache::Tarball> {
        Err(cua_ci::Error::Config("no runner downloads here".into()))
    }
}

// ---------------------------------------------------------------------
// serve
// ---------------------------------------------------------------------

/// Runs the hosted loop: registers this computer, claims jobs, runs each
/// in a fresh Space.
pub async fn serve(
    paths: &CiPaths,
    cua: Arc<cua_sdk::Cua>,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let state = connected(paths)?;
    let host = HostFacts::detect()?;
    let sizes = sizes_for(&host, state.macos_slots, state.linux_slots);
    let api = Arc::new(client(&state.account)?);
    // Releases of the actions runner are public: asked without a credential.
    let public = cua_ci::github::GitHubClient::new(
        "https://api.github.com",
        cua_ci::config::Scope::parse("org:cua-ci").map_err(err)?,
        cua_ci::github::Credentials::Token(cua_ci::Secret::new("")),
    )
    .map_err(err)?;
    let spaces: Arc<dyn cua_ci::Spaces> = Arc::new(crate::ci_sdk::SdkSpaces::new(cua));
    let tarballs: Arc<dyn cua_ci::cache::TarballSource> = Arc::new(
        cua_ci::cache::TarballCache::new(paths.cache(), Arc::new(public)),
    );
    let images = class_images(&state, sizes);
    let runner = SpaceJobRunner::new(
        spaces.clone(),
        tarballs.clone(),
        paths.clone(),
        &state.machine_id,
        images.clone(),
        &state.on,
    );
    match runner.sweep().await {
        Ok(0) => {}
        Ok(n) => line(
            out,
            format!(
                "Deleted {n} Space{} an earlier run left behind.",
                if n == 1 { "" } else { "s" }
            ),
        ),
        Err(e) => tracing::warn!("sweeping leftover Spaces: {e}"),
    }
    // The performance core (warm VMs, saved-state starts, caches, the
    // repository mirror) is on unless a pool's `perf.toml` says
    // `enabled = false` or CUA_CI_PERF=off; then every class takes the
    // plain cold path.
    let perf_classes: Vec<cua_ci::perf::job_runner::PerfClass> = images
        .iter()
        .map(|i| cua_ci::perf::job_runner::PerfClass {
            image: i.clone(),
            arch: host.arch.to_string(),
            slots: if i.class == Class::Macos {
                state.macos_slots
            } else {
                state.linux_slots
            },
            warm: 1,
        })
        .collect();
    for c in &perf_classes {
        cua_ci::perf::job_runner::ensure_default_perf_file(
            paths,
            &cua_ci::perf::job_runner::hosted_pool_name(&state.machine_id, c.image.class),
        )
        .map_err(err)?;
    }
    let runner: Arc<dyn cua_ci::hosted::JobRunner> = match cua_ci::perf::job_runner::perf_runner(
        paths,
        api.clone(),
        Some(api.clone()),
        &state.machine_id,
        &state.account,
        spaces.clone(),
        tarballs.clone(),
        &state.on,
        &perf_classes,
    )
    .await
    .map_err(err)?
    {
        Some(r) => {
            line(
                out,
                "Performance core on: warm Spaces, saved-state starts and caches.",
            );
            Arc::new(r)
        }
        None => Arc::new(runner),
    };
    let reg = state.registration(host.os, host.arch, sizes);
    reg.validate().map_err(CuaError::InvalidArgument)?;
    let mut agents = vec![Arc::new(HostedAgent::new(
        api.clone(),
        runner,
        &state.machine_id,
        reg,
        AgentTiming::default(),
    ))];
    // Cloud pools added with `cua ci add`: each has its own capacity manager.
    let managers = crate::ci_targets::runners(spaces.clone(), tarballs.clone());
    for t in cua_ci::targets::TargetStore::new(paths)
        .list()
        .into_iter()
        .filter(|t| t.has_manager())
    {
        let Some(mid) = t.machine_id.clone() else {
            continue;
        };
        match managers.runner_for(&t, paths) {
            Some(r) => agents.push(Arc::new(HostedAgent::new(
                api.clone(),
                r,
                &mid,
                t.registration(env!("CARGO_PKG_VERSION")),
                AgentTiming::default(),
            ))),
            None => line(
                out,
                format!(
                    "{}: this build of cua has no capacity manager for it yet, so its jobs stay \
                     with your other machines.",
                    t.name
                ),
            ),
        }
    }
    line(
        out,
        format!(
            "Serving Cua CI jobs on {} ({}){}. Ctrl-C to stop.",
            state.machine_name,
            capacity_text(state.macos_slots, state.linux_slots),
            if agents.len() > 1 {
                format!(
                    " and {} cloud pool{}",
                    agents.len() - 1,
                    if agents.len() == 2 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        ),
    );
    let _ = out.flush();
    cua_ci::hosted::run_all(agents, crate::ci_cmd::shutdown_signal())
        .await
        .map_err(err)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(os: &'static str, arch: &'static str, cpus: u32) -> HostFacts {
        HostFacts {
            os,
            arch,
            cpus,
            memory_mb: 32768,
        }
    }

    #[test]
    fn slots_follow_the_host_and_never_exceed_it() {
        let mac = host("macos", "arm64", 10);
        assert_eq!(slots_for(&mac, None, None).unwrap(), (2, 2));
        assert_eq!(slots_for(&mac, Some(1), Some(0)).unwrap(), (1, 0));
        assert!(
            slots_for(&mac, Some(3), None)
                .unwrap_err()
                .to_string()
                .contains("at most 2")
        );
        assert!(
            slots_for(&mac, Some(0), Some(0))
                .unwrap_err()
                .to_string()
                .contains("nothing")
        );
        let intel = host("macos", "x64", 8);
        assert!(
            slots_for(&intel, Some(1), None)
                .unwrap_err()
                .to_string()
                .contains("Apple silicon")
        );
        assert_eq!(slots_for(&intel, None, None).unwrap(), (0, 2));
        let linux = host("linux", "x64", 16);
        assert_eq!(slots_for(&linux, None, None).unwrap(), (0, 4));
        assert!(slots_for(&linux, None, Some(65)).is_err());
    }

    #[test]
    fn sizes_split_the_mac_between_its_jobs() {
        let mac = host("macos", "arm64", 10);
        let s = sizes_for(&mac, 2, 2);
        assert_eq!(s.macos_cpus, Some(5));
        assert_eq!(s.macos_memory_mb, Some((32768 - 8192) / 2));
        assert_eq!(s.linux_cpus, Some(4));
        let s = sizes_for(&mac, 0, 8);
        assert_eq!(s.macos_cpus, None);
        assert_eq!(s.linux_cpus, Some(1));
    }

    fn inst(id: &str, login: &str, suspended: bool) -> Installation {
        Installation {
            id: id.into(),
            github_login: login.into(),
            suspended,
            ..Default::default()
        }
    }

    #[test]
    fn the_installation_is_the_one_in_use_the_only_one_or_the_named_one() {
        let mut acct = Account::default();
        assert_eq!(pick_installation(&acct, None, None), None);
        acct.installations = vec![inst("inst_1", "acme", false)];
        assert_eq!(pick_installation(&acct, None, None).unwrap().id, "inst_1");
        acct.installations.push(inst("inst_2", "other", false));
        assert_eq!(pick_installation(&acct, None, None), None);
        assert_eq!(
            pick_installation(&acct, None, Some("OTHER")).unwrap().id,
            "inst_2"
        );
        assert_eq!(pick_installation(&acct, None, Some("nope")), None);
        let prev = HostedState {
            account: "me".into(),
            installation: "inst_2".into(),
            github_login: String::new(),
            machine_id: "m".into(),
            machine_name: "n".into(),
            macos_slots: 0,
            linux_slots: 1,
            macos_image: None,
            linux_image: None,
            on: "local".into(),
            connected_at: String::new(),
        };
        assert_eq!(
            pick_installation(&acct, Some(&prev), None).unwrap().id,
            "inst_2"
        );
        acct.installations[1].suspended = true;
        assert_eq!(
            pick_installation(&acct, Some(&prev), None).unwrap().id,
            "inst_1"
        );
    }

    #[test]
    fn repositories_are_chosen_from_what_the_app_sees() {
        let visible = vec!["acme/app".to_string(), "acme/api".to_string()];
        assert_eq!(
            choose_repos(&visible, &["acme/app".into()], &[]).unwrap(),
            ["acme/app"]
        );
        assert_eq!(
            choose_repos(&visible, &[], &["ACME/api".into(), "acme/api".into()]).unwrap(),
            ["acme/api"]
        );
        let e = choose_repos(&visible, &[], &["acme/secret".into()]).unwrap_err();
        assert!(e.contains("cannot see"), "{e}");
        assert!(choose_repos(&visible, &[], &["acme/*".into()]).is_err());
        assert!(choose_repos(&visible, &[], &["nope".into()]).is_err());
    }

    fn base() -> (Settings, AllowList) {
        (
            Settings::default(),
            AllowList {
                installation: "inst_1".into(),
                allow: vec!["acme/app".into()],
                overrides: BTreeMap::new(),
            },
        )
    }

    #[test]
    fn config_changes_apply_and_say_what_changed() {
        let (mut s, mut a) = base();
        let said = apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                allow: vec!["acme/api".into(), "acme/app".into()],
                max_jobs: Some("4".into()),
                max_macos_jobs: Some("2".into()),
                max_job_minutes: Some("90".into()),
                queue_timeout: Some("15m".into()),
                budget: Some("$50".into()),
                on_budget_limit: Some("warn".into()),
                budget_scope: Some("all".into()),
                fork_prs: Some("deny".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.allow, ["acme/app", "acme/api"]);
        assert_eq!(s.caps.max_concurrent_jobs, Some(4));
        assert_eq!(s.caps.max_concurrent_macos, Some(2));
        assert_eq!(s.caps.max_job_minutes, Some(90));
        assert_eq!(s.caps.queue_timeout_s, Some(900));
        assert_eq!(s.budget.monthly_micros, Some(50_000_000));
        assert_eq!(s.budget.on_limit, OnLimit::Warn);
        assert_eq!(s.budget.scope, BudgetScope::All);
        assert_eq!(s.defaults.fork_pr_policy, ForkPrPolicy::Deny);
        assert!(said.iter().any(|l| l.contains("serving acme/api")));
        assert!(!said.iter().any(|l| l.contains("serving acme/app")));
        assert!(said.iter().any(|l| l.contains("$50.00")));
        assert!(said.iter().all(|l| !l.contains('\u{b7}')));
    }

    #[test]
    fn risky_changes_need_yes() {
        let (mut s, mut a) = base();
        let e = apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                fork_prs: Some("allow".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.contains("--yes"), "{e}");
        let e = apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                egress: Some("off".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.contains("--yes"), "{e}");
        apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                egress: Some("off".into()),
                yes: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.defaults.egress, "off");
        assert_eq!(s.defaults.fork_pr_policy, ForkPrPolicy::Restricted);
    }

    #[test]
    fn per_repository_overrides_need_the_repository_on_the_list() {
        let (mut s, mut a) = base();
        apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                repo: Some("acme/app".into()),
                fork_prs: Some("deny".into()),
                max_jobs: Some("2".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let o = &a.overrides["acme/app"];
        assert_eq!(o.fork_pr_policy, Some(ForkPrPolicy::Deny));
        assert_eq!(o.max_concurrent, Some(2));
        assert_eq!(s.defaults.fork_pr_policy, ForkPrPolicy::Restricted);
        assert_eq!(s.caps.max_concurrent_jobs, None);
        let e = apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                repo: Some("acme/other".into()),
                max_jobs: Some("2".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.contains("not on the list"), "{e}");
        // Removing a repository drops its override.
        apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                remove: vec!["acme/app".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(a.allow.is_empty() && a.overrides.is_empty());
        let e = apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                remove: vec!["acme/app".into()],
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(e.contains("not on the list"), "{e}");
    }

    #[test]
    fn bad_values_are_refused_with_the_flag_named() {
        for (c, needle) in [
            (
                ConfigChange {
                    max_jobs: Some("0".into()),
                    ..Default::default()
                },
                "--max-jobs",
            ),
            (
                ConfigChange {
                    max_jobs: Some("x".into()),
                    ..Default::default()
                },
                "--max-jobs",
            ),
            (
                ConfigChange {
                    queue_timeout: Some("soon".into()),
                    ..Default::default()
                },
                "--queue-timeout",
            ),
            (
                ConfigChange {
                    budget: Some("-5".into()),
                    ..Default::default()
                },
                "dollar",
            ),
            (
                ConfigChange {
                    budget: Some("0".into()),
                    ..Default::default()
                },
                "--budget",
            ),
            (
                ConfigChange {
                    fork_prs: Some("open".into()),
                    ..Default::default()
                },
                "fork policy",
            ),
            (
                ConfigChange {
                    allow: vec!["acme/*".into()],
                    ..Default::default()
                },
                "--allow-org-wide",
            ),
            (
                ConfigChange {
                    max_macos_jobs: Some("5".into()),
                    max_jobs: Some("3".into()),
                    ..Default::default()
                },
                "more than",
            ),
        ] {
            let (mut s, mut a) = base();
            let e = apply_config(&mut s, &mut a, &c).unwrap_err();
            assert!(e.contains(needle), "{needle}: {e}");
        }
        // The wildcard is accepted only with its flag.
        let (mut s, mut a) = base();
        apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                allow: vec!["acme/*".into()],
                allow_org_wide: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(a.allow.contains(&"acme/*".to_string()));
    }

    #[test]
    fn none_clears_a_cap_and_the_budget() {
        let (mut s, mut a) = base();
        s.caps.max_concurrent_jobs = Some(3);
        s.budget.monthly_micros = Some(1_000_000);
        apply_config(
            &mut s,
            &mut a,
            &ConfigChange {
                max_jobs: Some("none".into()),
                budget: Some("none".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.caps.max_concurrent_jobs, None);
        assert_eq!(s.budget.monthly_micros, None);
    }

    #[test]
    fn an_empty_change_changes_nothing() {
        assert!(ConfigChange::default().is_empty());
        assert!(
            !ConfigChange {
                budget: Some("5".into()),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !ConfigChange {
                allow: vec!["a/b".into()],
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn status_reads_in_plain_words() {
        let st: Status = serde_json::from_value(serde_json::json!({
            "account_id": "acct_1",
            "installations": [{"id": "inst_1", "github_login": "acme", "repos_allowed": 2}],
            "machines": [
                {"machine_id": "m1", "name": "mac-studio", "kind": "local", "state": "online",
                 "classes": [{"class": "macos", "free_slots": 1, "busy_slots": 1, "max_slots": 2}]},
                {"machine_id": "m2", "name": "aws-pool", "kind": "byoc", "state": "online", "classes": []}
            ],
            "queue": [{"job_id": "job_1", "repo": "acme/app", "class": "macos", "waiting_s": 4, "state": "unserved", "reason": "budget"}],
            "running": [{"job_key": "1/2", "repo": "acme/api", "machine_id": "m1"}],
            "last_7d": {"jobs": 140, "minutes": 810.4, "success": 131, "failure": 8, "cancelled": 1, "infra_failures": 2, "median_start_s": 2.4, "p95_start_s": 9.1},
            "month": {"minutes": 2511.4, "cost_micros": 11200000, "budget_micros": 50000000, "budget_fraction": 0.224, "budget_state": "ok"},
            "notices": ["Budget at 80 percent"]
        })).unwrap();
        let state = HostedState {
            account: "me".into(),
            installation: "inst_1".into(),
            github_login: "acme".into(),
            machine_id: "m1".into(),
            machine_name: "mac-studio".into(),
            macos_slots: 2,
            linux_slots: 0,
            macos_image: None,
            linux_image: None,
            on: "local".into(),
            connected_at: String::new(),
        };
        let text = status_text(&st, Some(&state)).join("\n");
        assert!(text.contains("Cua CI for acme"), "{text}");
        assert!(
            text.contains("mac-studio (this one)  online  macOS 1 of 2 free"),
            "{text}"
        );
        assert!(text.contains("aws-pool"), "{text}");
        assert!(
            text.contains("acme/app macos for 4 s, not served: budget"),
            "{text}"
        );
        assert!(text.contains("acme/api on mac-studio"), "{text}");
        assert!(text.contains("140 jobs, 810 minutes"), "{text}");
        assert!(text.contains("start median 2.4 s"), "{text}");
        assert!(text.contains("2 stopped by Cua CI itself"), "{text}");
        assert!(
            text.contains("cloud spend $11.20 of $50.00 budget (22 percent)"),
            "{text}"
        );
        assert!(text.contains("Note: Budget at 80 percent"), "{text}");
        assert!(!text.contains('\u{b7}'), "no middle dots: {text}");
        let none = status_text(&Status::default(), None).join("\n");
        assert!(
            none.contains("no GitHub organization yet") && none.contains("cua ci connect"),
            "{none}"
        );
        assert!(none.contains("no budget set"), "{none}");
    }

    #[test]
    fn the_connected_line_says_it_works() {
        assert_eq!(
            connected_line(&["a/b".into(), "a/c".into(), "a/d".into()]),
            "Connected. 3 repositories can use runs-on: cua. This computer serves jobs now."
        );
        assert!(connected_line(&["a/b".into()]).contains("1 repository can use"));
        assert!(connected_line(&[]).contains("No repository can use"));
    }

    #[test]
    fn capacity_reads_naturally() {
        assert_eq!(
            capacity_text(2, 2),
            "2 macOS jobs at once, 2 Linux jobs at once"
        );
        assert_eq!(capacity_text(1, 0), "1 macOS job at once");
        assert_eq!(capacity_text(0, 1), "1 Linux job at once");
    }

    #[test]
    fn durations_and_ages() {
        assert_eq!(age(4), "4 s");
        assert_eq!(age(120), "2 min");
        assert_eq!(age(7200), "2 h");
        assert_eq!(secs(Some(2.44)), "2.4 s");
        assert_eq!(secs(Some(31.0)), "31 s");
        assert_eq!(secs(None), "none yet");
        assert_eq!(duration_s("--q", "15m").unwrap(), Some(900));
        assert_eq!(duration_s("--q", "none").unwrap(), None);
        assert!(duration_s("--q", "0s").is_err());
    }

    #[test]
    fn config_text_shows_safe_defaults() {
        let t = config_text(&Settings::default(), &Repos::default()).join("\n");
        assert!(t.contains("Repositories served  none"), "{t}");
        assert!(t.contains("Fork pull requests    restricted"), "{t}");
        assert!(t.contains("Network for jobs      deny-private"), "{t}");
        assert!(t.contains("Monthly budget        none"), "{t}");
    }
}
