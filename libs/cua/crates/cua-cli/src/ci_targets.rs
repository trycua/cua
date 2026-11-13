//! `cua ci add` and `cua ci remove`: more capacity for Cua CI, one command
//! each. A target is a cloud account (AWS, Google Cloud, Azure, Hetzner,
//! Modal, MacStadium) or a computer reached over SSH.
//!
//! Every cloud add asks for one thing, a monthly budget (default 50
//! dollars). The hard per-hour cap is prefilled from it, the region is the
//! nearest one, computers end when idle, and both caps are always set. AWS,
//! Google Cloud and Azure are linked without a stored key (a role or
//! identity your account trusts, through the same flow as `cua cloud
//! connect`). Token providers keep their token on this computer only and
//! never send it to the hosted service. The capacity manager that starts
//! and ends cloud computers runs inside `cua ci serve`; `remove` asks it to
//! tear down through its ledger, which deletes only what Cua recorded and
//! tagged.

use std::io::{IsTerminal as _, Read as _, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_ci::CiPaths;
use cua_ci::hosted::{HostedState, ServiceApi as _, rfc3339};
use cua_ci::secret::{Secret, StoreKind};
use cua_ci::service_api::BudgetScope;
use cua_ci::targets::*;
use cua_sdk::CuaError;

use crate::ci_cmd::err;
use crate::ci_hosted::{HOSTED_POOL, client};
use crate::util::line;

/// The options of `cua ci add`.
#[derive(Clone, Debug, Default)]
pub struct Add {
    /// `aws`, `gcp`, `azure`, `hetzner`, `modal`, `macstadium` or `host`.
    pub target: String,
    /// The SSH address of a `host`.
    pub address: Option<String>,
    /// Monthly budget in dollars (default: ask, 50).
    pub budget: Option<f64>,
    /// Hard cap in dollars an hour (default: from the budget).
    pub max_per_hour: Option<f64>,
    /// Free slots to keep ready, fixed by the owner (default: the autopilot
    /// chooses from the job history).
    pub warm: Option<u32>,
    /// `cheapest`, `balanced` (default) or `fastest`.
    pub goal: Option<String>,
    /// Region or location (default: the nearest).
    pub region: Option<String>,
    /// AWS: Mac hosts (macOS jobs).
    pub macos: bool,
    /// Do not ask.
    pub yes: bool,
    /// Print links instead of opening a browser.
    pub no_browser: bool,
    /// Read the token from stdin.
    pub token_stdin: bool,
    /// AWS: the 12-digit account id.
    pub account_id: Option<String>,
    /// Google Cloud: the project number.
    pub project_number: Option<String>,
    /// Azure: the subscription id.
    pub subscription: Option<String>,
    /// Azure: the tenant id.
    pub tenant: Option<String>,
    /// Azure: the managed identity's client id.
    pub client_id: Option<String>,
    /// host: a private key file; with it Cua manages the computer.
    pub key: Option<String>,
    /// host: read the private key from stdin.
    pub key_stdin: bool,
    /// host: the computer's public host key (`<type> <base64>`).
    #[allow(dead_code)]
    pub host_key: Option<String>,
    /// host: dollars a month when the computer is rented.
    #[allow(dead_code)]
    pub monthly: Option<f64>,
}

/// The managers of a build without the cloud providers: none, so cloud
/// pools wait (their jobs stay with the other computers).
struct NoManagers;

impl TargetRunners for NoManagers {
    fn runner_for(
        &self,
        _target: &TargetState,
        _paths: &CiPaths,
    ) -> Option<Arc<dyn cua_ci::hosted::JobRunner>> {
        None
    }
}

/// The capacity managers of this build (none without the cloud providers).
pub fn runners(
    _spaces: Arc<dyn cua_ci::Spaces>,
    _tarballs: Arc<dyn cua_ci::cache::TarballSource>,
) -> Box<dyn TargetRunners> {
    Box::new(NoManagers)
}

/// Whether adding or removing `target` needs the Spaces handle (the cloud
/// link goes through the daemon).
pub fn needs_spaces(target: &str) -> bool {
    Kind::parse(target).map(Kind::is_federated).unwrap_or(false)
}

async fn local_utc_offset() -> Option<i32> {
    let o = tokio::process::Command::new("date")
        .arg("+%z")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    parse_utc_offset(&String::from_utf8_lossy(&o.stdout))
}

/// A 12-digit AWS account id.
pub fn valid_account_id(s: &str) -> bool {
    s.len() == 12 && s.chars().all(|c| c.is_ascii_digit())
}

/// The AWS account this computer's own `aws` sign-in is in, when it has
/// one (a read-only call). Nothing is stored from it.
async fn detect_aws_account() -> Option<String> {
    let o = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new("aws")
            .args([
                "sts",
                "get-caller-identity",
                "--query",
                "Account",
                "--output",
                "text",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let id = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (o.status.success() && valid_account_id(&id)).then_some(id)
}

/// The notice before the first AWS Mac host: Apple's 24 hour minimum.
pub fn mac_notice(per_hour_cap_usd: f64) -> String {
    format!(
        "AWS bills a Mac host for at least 24 hours, so the first macOS job costs a full day of \
         the host. Cua refuses a host that your hourly cap ({}) or your monthly budget cannot \
         hold, and the minimum shows in `cua ci status` as it counts down.",
        cua_ci::service_api::format_dollars((per_hour_cap_usd * 1_000_000.0) as u64)
    )
}

fn ask_budget(default: f64, yes: bool) -> Result<f64, CuaError> {
    if yes || !crate::util::interactive() {
        return Ok(default);
    }
    match crate::util::prompt_line(&format!("Monthly budget in dollars [{default:.0}]: ")) {
        None => Ok(default),
        Some(s) => parse_usd(&s).map_err(CuaError::InvalidArgument),
    }
}

/// The lines a cloud connect result adds when it is not done: the link to
/// open or the command to paste, and why.
pub fn cloud_next_lines(c: &cua_sdk::CloudConnected) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(n) = &c.next {
        if !n.why.is_empty() {
            v.push(n.why.clone());
        }
        if let Some(o) = &n.open {
            v.push(format!("Open: {o}"));
        }
        if let Some(p) = &n.paste {
            v.push(format!("Paste there: {p}"));
        }
        if let Some(r) = &n.run {
            v.push(format!("Run: {r}"));
        }
    }
    v
}

pub(crate) fn secret_store(paths: &CiPaths) -> Box<dyn cua_ci::secret::SecretStore> {
    let kind = if cfg!(target_os = "macos") {
        StoreKind::Keychain
    } else {
        StoreKind::File
    };
    cua_ci::secret::store(kind, paths.secrets())
}

fn cloud_target(kind: Kind, region: &Option<String>) -> cua_sdk::CloudTarget {
    cua_sdk::CloudTarget {
        provider: kind.as_str().to_string(),
        profile: None,
        region: region.clone(),
        zone: None,
        project: None,
        environment: None,
        token: None,
    }
}

/// Whether this build runs cloud capacity managers (jobs on cloud computers it
/// starts and ends). It does not: a cloud target would be registered and then
/// never serve a job, so adding one is refused.
pub(crate) const CLOUD_CAPACITY: bool = false;

/// Adds a target.
pub async fn add(
    a: Add,
    paths: &CiPaths,
    cua: Option<Arc<cua_sdk::Cua>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let state = HostedState::load(paths)
        .map_err(err)?
        .ok_or_else(|| CuaError::NotFound("connect first: `cua ci connect`".into()))?;
    let kind = Kind::parse(&a.target).map_err(CuaError::InvalidArgument)?;
    if kind == Kind::Host {
        return add_host(&a, paths, json, out).await;
    }
    if !CLOUD_CAPACITY {
        return Err(CuaError::Unsupported(format!(
            "{} capacity is not in this build of cua yet; add a computer with \
             `cua ci add host USER@ADDRESS`, or serve jobs from this one",
            kind.as_str()
        )));
    }
    if a.address.is_some() {
        return Err(CuaError::InvalidArgument(format!(
            "{} takes no address; for a computer over SSH use `cua ci add host USER@ADDRESS`",
            kind.as_str()
        )));
    }
    if a.macos && kind != Kind::Aws {
        return Err(CuaError::InvalidArgument(
            "--macos is for aws (EC2 Mac hosts)".into(),
        ));
    }
    if a.macos && a.warm.unwrap_or(0) > 0 {
        return Err(CuaError::InvalidArgument(
            "--warm is not for Mac hosts: they bill a day each, so Cua never buys one ahead of a job"
                .into(),
        ));
    }
    let name = target_name(kind, None).map_err(CuaError::InvalidArgument)?;
    let store = TargetStore::new(paths);
    let prev = store.get(&name).map_err(err)?;
    let mut t = prev
        .clone()
        .unwrap_or_else(|| TargetState::new(kind, &name));

    // The one question: the monthly budget. Both caps are always set.
    if let Some(b) = a.budget {
        t.budget_usd_month = b;
        t.max_per_hour_usd = a.max_per_hour.unwrap_or_else(|| default_max_per_hour(b));
    } else if prev.is_none() {
        t.budget_usd_month = ask_budget(DEFAULT_BUDGET_USD, a.yes)?;
        t.max_per_hour_usd = a
            .max_per_hour
            .unwrap_or_else(|| default_max_per_hour(t.budget_usd_month));
    } else if let Some(h) = a.max_per_hour {
        t.max_per_hour_usd = h;
    }
    validate_caps(t.budget_usd_month, t.max_per_hour_usd).map_err(CuaError::InvalidArgument)?;
    if let Some(w) = a.warm {
        t.warm = Some(w);
        t.warm_slots = 0;
    }
    if let Some(g) = &a.goal {
        t.goal = g.trim().to_ascii_lowercase();
    }
    t.region = a.region.clone().or(t.region.clone());
    if t.region.is_none() {
        let off = local_utc_offset().await.unwrap_or(0);
        t.region = nearest_region(kind, off).map(str::to_string);
    }
    if a.macos {
        t.macos = true;
        t.max_slots = 2;
        line(out, mac_notice(t.max_per_hour_usd));
        if !a.yes && !crate::util::confirm("Add EC2 Mac hosts?", false) {
            return Err(CuaError::Cancelled("nothing added".into()));
        }
    }

    // The link to the provider.
    let linked = if kind.is_federated() {
        let cua = cua.ok_or_else(|| CuaError::Internal("no Spaces handle".into()))?;
        link_cloud(&cua, kind, &mut t, &a, out).await?
    } else {
        link_token(kind, &name, prev.is_some(), &a, paths, out).await?;
        true
    };
    if !linked {
        return Ok(2);
    }

    // The service learns the pool and the budget (never a credential).
    t.added_at = rfc3339(cua_ci::unix_ms());
    let mid = t
        .machine_id
        .clone()
        .unwrap_or_else(|| format!("{}-{}", state.machine_id, name));
    t.machine_id = Some(mid.clone());
    let mut all: Vec<TargetState> = store
        .list()
        .into_iter()
        .filter(|x| x.name != t.name)
        .collect();
    all.push(t.clone());
    let api = client(&state.account)?;
    let (mut settings, etag) = api.settings().await.map_err(err)?;
    settings.budget.monthly_micros = Some(account_budget_micros(&all));
    settings.budget.scope = BudgetScope::Byoc;
    api.put_settings(&settings, etag.as_deref())
        .await
        .map_err(err)?;
    api.register(&mid, &t.registration(env!("CARGO_PKG_VERSION")))
        .await
        .map_err(err)?;
    store.save(&t).map_err(err)?;
    let running = ensure_service(kind);

    let report = store.read_report(&name);
    if json {
        line(
            out,
            serde_json::json!({
                "target": name,
                "kind": kind.as_str(),
                "region": t.region,
                "budget_usd_month": t.budget_usd_month,
                "max_per_hour_usd": t.max_per_hour_usd,
                "scale_to_zero": t.scale_to_zero,
                "machine_id": mid,
                "service_running": running,
                "cost": report.as_ref().map(|r| &r.cost),
            })
            .to_string(),
        );
        return Ok(0);
    }
    line(out, format!("{} connected.", kind.title()));
    for l in target_lines(&t, report.as_ref()) {
        line(out, l);
    }
    for l in cost_lines(&report.as_ref().map(|r| r.cost.clone()).unwrap_or_default()) {
        line(out, l);
    }
    line(
        out,
        format!(
            "Change the budget later with `cua ci add {} --budget 80`; remove it with `cua ci \
             remove {}`.",
            a.target, a.target
        ),
    );
    if kind.is_token() {
        line(
            out,
            "This computer runs the capacity manager because the token stays here: keep it on.",
        );
    }
    if !running {
        line(out, "Start the service that serves jobs: `cua ci serve`.");
    }
    Ok(0)
}

/// Links AWS, Google Cloud or Azure with no stored key. Returns whether it
/// is connected now (false: waiting on a step a person must take).
async fn link_cloud(
    cua: &Arc<cua_sdk::Cua>,
    kind: Kind,
    t: &mut TargetState,
    a: &Add,
    out: &mut dyn Write,
) -> Result<bool, CuaError> {
    let spaces = cua.spaces();
    let mut account = a.account_id.clone().or_else(|| t.account_id.clone());
    if let Some(id) = &account
        && !valid_account_id(id)
    {
        return Err(CuaError::InvalidArgument(
            "--account-id: the 12 digits of your AWS account id".into(),
        ));
    }
    if kind == Kind::Aws && account.is_none() {
        account = detect_aws_account().await;
    }
    let ready = match kind {
        Kind::Aws => account.is_some(),
        Kind::Gcp => a.project_number.is_some(),
        _ => a.subscription.is_some() && a.tenant.is_some() && a.client_id.is_some(),
    };
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut opened = false;
    let mut waiting = false;
    let mut ready = ready;
    loop {
        let opts = cua_sdk::CloudConnectOptions {
            auth: Some("oidc".into()),
            budget_usd_month: Some(t.budget_usd_month),
            account_id: account.clone(),
            project_number: a.project_number.clone(),
            subscription_id: a.subscription.clone(),
            tenant_id: a.tenant.clone(),
            client_id: a.client_id.clone(),
            check: ready,
            agent: cua_sandbox_core::byoc::caller_agent(),
            ..Default::default()
        };
        let c = spaces
            .cloud_connect(cloud_target(kind, &t.region), false, None, Some(opts))
            .await?;
        if c.provider.connected && c.provider.status == "connected" {
            t.account_id = account;
            if t.region.is_none() && !c.provider.region.is_empty() {
                t.region = Some(c.provider.region.clone());
            }
            return Ok(true);
        }
        if !opened {
            opened = true;
            let lines = cloud_next_lines(&c);
            for l in &lines {
                line(out, l);
            }
            if let Some(url) = c.next.as_ref().and_then(|n| n.open.as_ref())
                && !a.no_browser
            {
                crate::util::open_browser(url);
            }
        }
        if kind != Kind::Aws {
            // Google Cloud and Azure: a person runs the printed step, then
            // runs this again with the ids it prints.
            if !ready {
                line(
                    out,
                    format!(
                        "Run `cua ci add {}` again with the ids that step prints \
                         (see `cua cloud connect {} --help`).",
                        kind.as_str(),
                        kind.as_str()
                    ),
                );
            }
            return Ok(false);
        }
        if account.is_none() {
            if !crate::util::interactive() {
                line(
                    out,
                    "When the stack shows CREATE_COMPLETE, run this again with --account-id (the \
                     12 digits at the top right of the AWS console).",
                );
                return Ok(false);
            }
            let Some(id) = crate::util::prompt_line(
                "AWS account id (12 digits, top right of the console) once the stack is created: ",
            ) else {
                return Ok(false);
            };
            if !valid_account_id(id.trim()) {
                return Err(CuaError::InvalidArgument("that is not 12 digits".into()));
            }
            account = Some(id.trim().to_string());
            ready = true;
            continue;
        }
        if !waiting {
            waiting = true;
            line(
                out,
                "Waiting for the stack (press Create stack in the browser)...",
            );
            let _ = out.flush();
        }
        if Instant::now() > deadline {
            line(
                out,
                "AWS does not trust Cua yet. Create the stack, then run this again.",
            );
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Asks for a provider token once and keeps it on this computer only.
async fn link_token(
    kind: Kind,
    name: &str,
    existed: bool,
    a: &Add,
    paths: &CiPaths,
    out: &mut dyn Write,
) -> Result<(), CuaError> {
    let service = kind.token_service().unwrap_or("cua-ci-token");
    let keys = secret_store(paths);
    let have = keys.get(service, name).map_err(err)?.is_some();
    if existed && have && !a.token_stdin {
        return Ok(()); // a budget change keeps the token it has
    }
    let token = if a.token_stdin {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| CuaError::InvalidArgument(format!("reading stdin: {e}")))?;
        s.trim().to_string()
    } else {
        crate::providers::read_secret(&format!(
            "Paste {} (kept only on this computer, never sent to Cua): ",
            kind.token_what()
        ))?
    };
    if !plausible_token(&token) {
        return Err(CuaError::InvalidArgument(
            "that does not look like a token (one line, no spaces)".into(),
        ));
    }
    verify_token(kind, &token)
        .await
        .map_err(CuaError::InvalidArgument)?;
    keys.set(service, name, &Secret::new(token)).map_err(err)?;
    line(
        out,
        "The token is stored only on this computer and is never sent to Cua.",
    );
    Ok(())
}

/// Makes sure the service that runs the capacity managers is running.
fn ensure_service(kind: Kind) -> bool {
    let _ = kind;
    if cua_ci::service::is_running(HOSTED_POOL) {
        return true;
    }
    cua_ci::service::start(HOSTED_POOL).is_ok() && cua_ci::service::is_running(HOSTED_POOL)
}

async fn add_host(
    a: &Add,
    paths: &CiPaths,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let address = a.address.clone().ok_or_else(|| {
        CuaError::InvalidArgument("host needs an SSH address: cua ci add host USER@ADDRESS".into())
    })?;
    let name = target_name(Kind::Host, Some(&address)).map_err(CuaError::InvalidArgument)?;
    if a.key.is_some() || a.key_stdin {
        return add_managed_host(a, &address, &name, paths, json, out).await;
    }
    let args = ssh_args(&address, &host_install_script(None)).map_err(CuaError::InvalidArgument)?;
    if !json {
        line(
            out,
            format!(
                "Connecting to {address} over SSH. It installs cua if needed and joins as one \
                 more computer of your account; you may be asked for its password and to sign in \
                 to Cua there."
            ),
        );
    }
    let _ = out.flush();
    let status = tokio::process::Command::new("ssh")
        .args(&args)
        .status()
        .await
        .map_err(|e| CuaError::Internal(format!("running ssh: {e}")))?;
    if !status.success() {
        return Err(CuaError::Http(format!(
            "ssh to {address} did not finish (exit {})",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        )));
    }
    let mut t = TargetState::new(Kind::Host, &name);
    t.address = Some(address.clone());
    t.added_at = rfc3339(cua_ci::unix_ms());
    TargetStore::new(paths).save(&t).map_err(err)?;
    if json {
        line(
            out,
            serde_json::json!({"target": name, "address": address}).to_string(),
        );
    } else {
        line(
            out,
            format!(
                "{address} serves jobs now, as one more computer. See it with `cua ci status`."
            ),
        );
        line(
            out,
            format!("Take it out with `cua ci remove host {address}`."),
        );
    }
    Ok(0)
}

/// Without the cloud providers a computer cannot be managed.
async fn add_managed_host(
    _a: &Add,
    _address: &str,
    _name: &str,
    _paths: &CiPaths,
    _json: bool,
    _out: &mut dyn Write,
) -> Result<i32, CuaError> {
    Err(CuaError::Unsupported(
        "this build of cua cannot manage computers over SSH; leave out --key and the computer \
         signs in to Cua itself"
            .into(),
    ))
}

fn forget_managed_host(_name: &str) -> Result<(), CuaError> {
    Ok(())
}

/// Removes a target: asks its capacity manager to tear down through the
/// ledger (only recorded, tagged resources), then forgets it.
pub async fn remove(
    target: String,
    address: Option<String>,
    yes: bool,
    paths: &CiPaths,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let kind = Kind::parse(&target).map_err(CuaError::InvalidArgument)?;
    let name = target_name(kind, address.as_deref()).map_err(CuaError::InvalidArgument)?;
    let store = TargetStore::new(paths);
    let t = store.get(&name).map_err(err)?.ok_or_else(|| {
        CuaError::NotFound(format!(
            "{name} is not added; `cua ci status` lists what is"
        ))
    })?;
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(CuaError::InvalidArgument(
                "removing a target ends its computers; pass --yes".into(),
            ));
        }
        let q = if kind == Kind::Host && !t.managed {
            format!("Take {name} out of Cua CI (it stops serving jobs)?")
        } else {
            format!(
                "Remove {name}: end the computers Cua started there (only the ones it recorded \
                 and tagged) and forget it?"
            )
        };
        if !crate::util::confirm(&q, false) {
            return Err(CuaError::Cancelled("kept the target".into()));
        }
    }
    let mut said = Vec::new();
    if kind == Kind::Host && !t.managed {
        if let Some(addr) = &t.address
            && let Ok(args) = ssh_args(addr, HOST_REMOVE_SCRIPT)
        {
            let ok = tokio::process::Command::new("ssh")
                .args(&args)
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            said.push(if ok {
                format!("{addr} stopped serving jobs")
            } else {
                format!(
                    "could not reach {addr}; run `cua ci disconnect` on it yourself, or it drops \
                     out of status when it stops calling"
                )
            });
        }
    } else {
        let req = TeardownRequest {
            requested_at: rfc3339(cua_ci::unix_ms()),
            dry_run: false,
        };
        store.request_teardown(&name, &req).map_err(err)?;
        if !ensure_service(kind) {
            return Err(CuaError::Unsupported(
                "the service that ends the computers is not running; start it with `cua ci serve` \
                 (or `cua ci connect`) and run this again"
                    .into(),
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(180);
        let result = loop {
            if let Some(r) = store.read_teardown_result(&name)
                && r.requested_at == req.requested_at
            {
                break r;
            }
            if Instant::now() > deadline {
                return Err(CuaError::Timeout(
                    "the capacity manager did not answer in 3 minutes; is `cua ci serve` \
                     running? Nothing was forgotten: run this again"
                        .into(),
                ));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        for i in &result.items {
            said.push(format!("{} {}: {}", i.action, i.name, i.reason));
        }
        let left = result.left();
        if !left.is_empty() {
            if json {
                line(
                    out,
                    serde_json::json!({"target": name, "left": left.len(), "items": result.items})
                        .to_string(),
                );
            } else {
                for l in &said {
                    line(out, l);
                }
                line(
                    out,
                    format!(
                        "{} computer{} could not be ended yet (a held minimum, or a failure). {name} \
                         stays added: run `cua ci remove {}` again later.",
                        left.len(),
                        if left.len() == 1 { "" } else { "s" },
                        target
                    ),
                );
            }
            return Ok(2);
        }
        // The service forgets the pool, the token goes, the budget shrinks.
        if let Some(state) = HostedState::load(paths).map_err(err)? {
            let api = client(&state.account)?;
            if let Some(mid) = &t.machine_id {
                match api.delete_machine(mid).await {
                    Ok(()) => {}
                    Err(cua_ci::Error::Service(e)) if e.status == 404 => {}
                    Err(e) => return Err(err(e)),
                }
            }
            let rest: Vec<TargetState> = store
                .list()
                .into_iter()
                .filter(|x| x.name != name)
                .collect();
            let (mut s, etag) = api.settings().await.map_err(err)?;
            let m = account_budget_micros(&rest);
            s.budget.monthly_micros = (m > 0).then_some(m);
            api.put_settings(&s, etag.as_deref()).await.map_err(err)?;
        }
        if let Some(service) = kind.token_service() {
            let _ = secret_store(paths).delete(service, &name);
        }
        // A managed SSH host: Cua's units are gone now, so the computer's
        // registry entry and its key in the keychain go too.
        if t.managed {
            forget_managed_host(&name)?;
        }
    }
    store.remove(&name).map_err(err)?;
    if json {
        line(
            out,
            serde_json::json!({"removed": name, "steps": said}).to_string(),
        );
    } else {
        for l in &said {
            line(out, l);
        }
        line(out, format!("Removed {name}."));
        if kind.is_federated() {
            line(
                out,
                format!(
                    "The trust in your {} account stays; `cua cloud disconnect {}` removes it.",
                    kind.title(),
                    kind.as_str()
                ),
            );
        }
    }
    Ok(0)
}

/// `cua ci why`: what the autopilot chose for each target and why.
pub fn why(
    target: Option<String>,
    paths: &CiPaths,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let store = TargetStore::new(paths);
    let targets: Vec<TargetState> = store
        .list()
        .into_iter()
        .filter(|t| {
            target
                .as_deref()
                .is_none_or(|n| t.name == n || t.kind.as_str() == n)
        })
        .collect();
    if targets.is_empty() {
        return Err(CuaError::InvalidArgument(match target {
            Some(t) => format!("no capacity target {t:?}: add one with `cua ci add <cloud>`"),
            None => "no capacity targets yet: add one with `cua ci add <cloud>`".into(),
        }));
    }
    if json {
        let v: Vec<serde_json::Value> = targets
            .iter()
            .map(|t| {
                serde_json::json!({
                    "target": t.name,
                    "goal": t.goal_or_default(),
                    "warm": t.fixed_warm(),
                    "autopilot": store.read_report(&t.name).and_then(|r| r.status.autopilot),
                })
            })
            .collect();
        line(out, serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(0);
    }
    for t in &targets {
        for l in why_lines(t, store.read_report(&t.name).as_ref()) {
            line(out, &l);
        }
    }
    Ok(0)
}

/// The lines `cua ci status` adds for the targets.
pub fn status_lines(paths: &CiPaths) -> Vec<String> {
    let store = TargetStore::new(paths);
    let mut v = Vec::new();
    for t in store.list() {
        v.extend(target_lines(&t, store.read_report(&t.name).as_ref()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_ids_are_twelve_digits() {
        assert!(valid_account_id("123456789012"));
        for bad in [
            "",
            "12345678901",
            "1234567890123",
            "12345678901a",
            "arn:aws:iam::1",
        ] {
            assert!(!valid_account_id(bad), "{bad}");
        }
    }

    #[test]
    fn only_the_federated_clouds_need_the_spaces_handle() {
        for t in ["aws", "gcp", "azure"] {
            assert!(needs_spaces(t), "{t}");
        }
        for t in ["hetzner", "modal", "macstadium", "host", "nope"] {
            assert!(!needs_spaces(t), "{t}");
        }
    }

    #[test]
    fn the_budget_defaults_without_asking_when_told_or_unattended() {
        assert_eq!(ask_budget(50.0, true).unwrap(), 50.0);
        // The test process has no terminal: the default is used.
        assert_eq!(ask_budget(50.0, false).unwrap(), 50.0);
    }

    #[test]
    fn the_mac_notice_says_the_minimum_in_plain_words() {
        let n = mac_notice(2.5);
        assert!(
            n.contains("at least 24 hours") && n.contains("$2.50"),
            "{n}"
        );
        assert!(!n.contains('\u{b7}'));
    }

    #[test]
    fn next_steps_read_in_order() {
        let c = cua_sdk::CloudConnected {
            next: Some(cua_sdk::CloudNext {
                run: None,
                open: Some("https://console.aws.amazon.com/x".into()),
                paste: Some("aws cloudformation deploy".into()),
                why: "AWS does not trust Cua yet.".into(),
            }),
            ..Default::default()
        };
        assert_eq!(
            cloud_next_lines(&c),
            [
                "AWS does not trust Cua yet.",
                "Open: https://console.aws.amazon.com/x",
                "Paste there: aws cloudformation deploy"
            ]
        );
        assert!(cloud_next_lines(&cua_sdk::CloudConnected::default()).is_empty());
    }

    #[test]
    fn why_prints_each_targets_plan_and_refuses_an_unknown_one() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        let mut out = Vec::new();
        let e = why(None, &paths, false, &mut out).unwrap_err().to_string();
        assert!(e.contains("cua ci add"), "{e}");
        let store = TargetStore::new(&paths);
        let mut t = TargetState::new(Kind::Aws, "aws");
        t.goal = "cheapest".into();
        store.save(&t).unwrap();
        let mut out = Vec::new();
        assert_eq!(why(Some("aws".into()), &paths, false, &mut out).unwrap(), 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("aws: the cheapest goal.") && text.contains("No plan yet"),
            "{text}"
        );
        let mut out = Vec::new();
        why(None, &paths, true, &mut out).unwrap();
        let j: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(j[0]["goal"], "cheapest");
        assert!(why(Some("gcp".into()), &paths, false, &mut Vec::new()).is_err());
    }

    #[test]
    fn status_lists_every_target_with_its_report() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        assert!(status_lines(&paths).is_empty());
        let store = TargetStore::new(&paths);
        let mut t = TargetState::new(Kind::Aws, "aws");
        t.region = Some("us-east-2".into());
        store.save(&t).unwrap();
        let mut h = TargetState::new(Kind::Host, "host-box");
        h.address = Some("root@box".into());
        store.save(&h).unwrap();
        let text = status_lines(&paths).join("\n");
        assert!(
            text.contains("aws in us-east-2  budget $50.00 a month, hard cap $2.50 an hour"),
            "{text}"
        );
        assert!(text.contains("host-box (root@box)"), "{text}");
        store
            .write_report(
                "aws",
                &TargetReport {
                    cost: CostReportView {
                        jobs: 3,
                        billed_minutes: 9.0,
                        per_minute_all_in_micros: 5000.0,
                        per_minute_compute_micros: 3000.0,
                        utilization: 0.4,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        let text = status_lines(&paths).join("\n");
        assert!(text.contains("$0.0050 a minute all in"), "{text}");
    }

    #[test]
    fn a_target_whose_cloud_is_not_linked_gets_no_manager_and_the_run_goes_on() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        let spaces: Arc<dyn cua_ci::Spaces> = Arc::new(cua_ci::testing::FakeSpaces::new());
        let tarballs: Arc<dyn cua_ci::cache::TarballSource> = Arc::new(NoTarballsForTests);
        // Macstadium has no provider; a build without the cloud providers
        // has none for any target. Either way: no runner, no panic.
        assert!(
            runners(spaces, tarballs)
                .runner_for(&TargetState::new(Kind::Macstadium, "macstadium"), &paths)
                .is_none()
        );
    }

    struct NoTarballsForTests;

    #[async_trait::async_trait]
    impl cua_ci::cache::TarballSource for NoTarballsForTests {
        async fn tarball(&self, _p: &str) -> cua_ci::Result<cua_ci::cache::Tarball> {
            Err(cua_ci::Error::Config("none".into()))
        }
    }
}
