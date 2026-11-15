// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cloud_connect` with every option ([`ConnectRequest`]), for people and
//! agents alike. It never prompts and is idempotent: run the same command
//! again and it picks up where it stopped. Every run ends with a `status`,
//! the `next` step (a command for the agent, or a page for a person) and
//! the command that `verify`s it:
//!
//! | status | means | next |
//! |---|---|---|
//! | `connected` | Spaces can be created there | none |
//! | `needs-trust` | the cloud does not trust Cua yet | a person opens a page |
//! | `checking` | recorded, not proven yet | the agent runs the check |
//! | `waiting-for-approval` | an agent linked it or raised a limit | a person approves in Cua Spaces |
//! | `failed` | it did not work | `checks` say why; maybe a command |
//!
//! Without `auth` (or `auth: cli`) it is today's link through the cloud
//! CLI's sign-in, plus the connection's own limits. With `auth: oidc`
//! (AWS, Google Cloud, Azure; Azure always) no key is stored anywhere: the
//! account's connection lives on Cua's issuer, the customer's cloud trusts
//! tokens it signs for this account (the CloudFormation stack, the
//! workload identity pool, the managed identity's federated credential:
//! [`crate::trust`]), and this device proves it works with a `check` token
//! before it saves the link.
//!
//! With `auth: sealed` (Hetzner, Modal) the cloud's API token is checked,
//! then sealed on this device to the account's devices ([`crate::sealed`])
//! and only its ciphertext goes to the issuer. Without `auth`, Hetzner and
//! Modal are sealed when this device has the account's pool keys and a
//! budget is given (or the link is sealed already); otherwise they keep
//! today's token on this device.

// Without `federation` (Cua's issuer) only the CLI-sign-in path is built.
#![cfg_attr(not(feature = "federation"), allow(unused_imports, dead_code))]

use std::sync::Arc;

use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::{CloudConnected, CloudNext, CloudProvider, ConnectRequest, status};

use crate::api::{CloudApi, Result, Tested};
use crate::manager::Clouds;
use crate::model::{self, Connection, MaxSize};

/// The checked inputs every path shares.
struct Limits {
    budget: Option<f64>,
    max_size: Option<MaxSize>,
}

fn limits_of(req: &ConnectRequest) -> Result<Limits> {
    let budget = match req.budget_usd_month {
        Some(b) if !b.is_finite() || !(0.0..=1_000_000.0).contains(&b) => {
            return Err(Error::InvalidArgument(format!(
                "budget must be between 0 and 1000000 US dollars a month, not {b}"
            )));
        }
        b => b,
    };
    let max_size = match req.max_size.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(s) => Some(MaxSize::parse(s).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "max size {s:?} is not <vCPUs>x<GB> (for example 4x16)"
            ))
        })?),
    };
    Ok(Limits { budget, max_size })
}

/// `cloud_connect`.
pub(crate) async fn connect_with(clouds: &Clouds, req: &ConnectRequest) -> Result<CloudConnected> {
    let p = clouds.api(&req.target.provider)?.clone();
    let limits = limits_of(req)?;
    // Asked, or implied (Azure; Google Cloud with a project number), else
    // how it is linked already or the default for the cloud.
    let explicit = req
        .auth
        .as_deref()
        .map(str::trim)
        .is_some_and(|a| !a.is_empty());
    let implied = req.auth_word();
    let auth = if explicit || implied != "cli" {
        implied
    } else {
        default_auth(clouds, p.as_ref(), &limits).await.into()
    };
    match auth.as_str() {
        // Azure is linked through Cua's issuer only.
        "cli" if p.name() == "azure" => connect_oidc(clouds, p.as_ref(), req, &limits).await,
        "cli" => {
            if p.name() == "modal" && req.target.token.is_some() {
                return Err(Error::InvalidArgument(
                    "a Modal token given to cua is sealed to your devices (add --budget, or \
                     --sealed); without sealing, Modal signs in on this device with \
                     `modal token new`"
                        .into(),
                ));
            }
            connect_cli(clouds, p.as_ref(), req, &limits).await
        }
        "oidc" => connect_oidc(clouds, p.as_ref(), req, &limits).await,
        "sealed" => connect_sealed(clouds, p.as_ref(), req, &limits).await,
        other => Err(Error::InvalidArgument(format!(
            "auth {other:?} is not cli, oidc or sealed"
        ))),
    }
}

/// How many machines (and sandboxes) this home records on `provider`
/// that may still run.
pub(crate) fn recorded_machines(clouds: &Clouds, provider: &str) -> usize {
    clouds
        .store()
        .resources()
        .unwrap_or_default()
        .iter()
        .filter(|r| {
            r.provider == provider
                && matches!(r.resource_type.as_str(), "instance" | "sandbox")
                && !matches!(r.state.as_str(), "gone" | "terminated")
        })
        .count()
}

/// How a cloud is linked when nobody said: a token cloud (Hetzner, Modal)
/// is sealed when its link is sealed already, or when this device has the
/// account's pool keys and a budget is given; everything else, and a token
/// cloud otherwise, keeps today's sign-in on this device.
async fn default_auth(clouds: &Clouds, p: &dyn CloudApi, limits: &Limits) -> &'static str {
    // A link keeps how it is linked: an OIDC or sealed link stays so when a
    // run only changes its budget or largest machine.
    if let Some(kept) = clouds
        .store()
        .connection(p.name())
        .ok()
        .flatten()
        .filter(|c| !c.auth.is_cli())
    {
        return kept.auth.as_str();
    }
    if !p.seals() {
        return "cli";
    }
    #[cfg(feature = "federation")]
    {
        let local = clouds.store().connection(p.name()).ok().flatten();
        if local
            .as_ref()
            .is_some_and(|c| matches!(c.auth, model::Auth::Sealed { .. }))
        {
            return "sealed";
        }
        let budget = limits
            .budget
            .or_else(|| local.as_ref().and_then(|c| c.budget_usd_month));
        if budget.is_none() || issuer(clouds).is_none() {
            return "cli";
        }
        let Some(sealer) = clouds.sealed.sealer() else {
            return "cli";
        };
        match tokio::time::timeout(std::time::Duration::from_secs(10), sealer.generation()).await {
            Ok(Ok(_)) => "sealed",
            Ok(Err(e)) => {
                tracing::info!(cloud = p.name(), "keeping the token on this device: {e}");
                "cli"
            }
            Err(_) => "cli",
        }
    }
    #[cfg(not(feature = "federation"))]
    {
        let _ = (clouds, limits);
        "cli"
    }
}

/// The command that links `provider` with its CLI sign-in.
fn cli_command(provider: &str) -> String {
    match provider {
        "hetzner" => "cua cloud connect hetzner --no-sealed --token-stdin".into(),
        other => format!("cua cloud connect {other}"),
    }
}

#[cfg(not(feature = "federation"))]
async fn connect_sealed(
    clouds: &Clouds,
    p: &dyn CloudApi,
    _req: &ConnectRequest,
    _limits: &Limits,
) -> Result<CloudConnected> {
    Ok(outcome(
        clouds,
        p,
        None,
        status::FAILED,
        Some(CloudNext {
            run: Some(cli_command(p.name())),
            why: "This build of cua keeps cloud tokens on this device only.".into(),
            ..Default::default()
        }),
        vec![],
    ))
}

/// The identifier that names a sealed connection on the issuer.
#[cfg(feature = "federation")]
fn sealed_key(provider: &str) -> &'static str {
    match provider {
        "modal" => "workspace",
        _ => "project",
    }
}

/// `auth: sealed`: the token checked, sealed here to the account's devices
/// and put on the account's connection (see the module docs).
#[cfg(feature = "federation")]
async fn connect_sealed(
    clouds: &Clouds,
    p: &dyn CloudApi,
    req: &ConnectRequest,
    limits: &Limits,
) -> Result<CloudConnected> {
    use crate::federation::{IssuerError, IssuerLimits, IssuerMaxSize, NewConnection};
    let name = p.name();
    let failed = |why: String, run: Option<String>| {
        outcome(
            clouds,
            p,
            None,
            status::FAILED,
            Some(CloudNext {
                run,
                why,
                ..Default::default()
            }),
            vec![],
        )
    };
    if !p.seals() {
        return Ok(failed(
            format!(
                "{} takes no API token to seal: {}",
                p.title(),
                if name == "aws" {
                    "link it without any key with --oidc"
                } else {
                    "link it with its own sign-in on this device"
                }
            ),
            Some(if name == "aws" {
                "cua cloud connect aws --oidc --region <region> --budget <USD>".into()
            } else {
                cli_command(name)
            }),
        ));
    }
    let issuer_failed = |e: &IssuerError| {
        let run = matches!(e, IssuerError::SignIn(_) | IssuerError::Device(_))
            .then(|| "cua auth login".to_string());
        failed(e.to_string(), run)
    };
    let Some(issuer) = issuer(clouds) else {
        return Ok(failed(
            "This device is not signed in to Cua yet.".into(),
            Some("cua auth login".into()),
        ));
    };
    let tokens = clouds.sealed.clone();
    let Some(sealer) = tokens.sealer() else {
        return Ok(failed(
            format!(
                "This cua has no pool keys to seal the {} token with (the cua of the Cua Spaces \
                 app has them). Keep the token on this device instead:",
                p.title()
            ),
            Some(cli_command(name)),
        ));
    };
    if let Err(e) = sealer.generation().await {
        return Ok(failed(e.to_string(), None));
    }
    let store = clouds.store();
    let local = store.connection(name).map_err(Error::Io)?;
    let servers = match issuer.connections().await {
        Ok(v) => v,
        Err(e) => return Ok(issuer_failed(&e)),
    };
    let key = sealed_key(name);
    let named = req
        .target
        .project
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let local_id = local
        .as_ref()
        .map(|c| c.auth.conn_id().to_string())
        .unwrap_or_default();
    let live: Vec<&crate::federation::ServerConnection> = servers
        .iter()
        .filter(|c| c.cloud == name && c.status != "revoked")
        .collect();
    let server = live
        .iter()
        .find(|c| !local_id.is_empty() && c.id == local_id && named.is_none())
        .or_else(|| {
            live.iter().find(|c| {
                named
                    .as_deref()
                    .is_none_or(|n| c.identifiers.get(key).map(String::as_str) == Some(n))
            })
        })
        .map(|c| (*c).clone());

    // Where, and the limits (asked now, else the account's, else this
    // device's).
    let mut target = req.target.clone();
    target.token = None;
    if target.region.as_deref().is_none_or(|r| r.trim().is_empty()) {
        target.region = server
            .as_ref()
            .and_then(|s| s.region.clone())
            .or_else(|| local.as_ref().map(|l| l.region.clone()))
            .filter(|r| !r.is_empty());
    }
    let mut candidate = p.resolve(&target)?;
    // A sealed link signs in with its own token, never a profile.
    candidate.profile = String::new();
    let budget = limits
        .budget
        .or_else(|| server.as_ref().and_then(|s| s.limits.budget_usd_month))
        .or_else(|| local.as_ref().and_then(|l| l.budget_usd_month));
    let max_size = limits
        .max_size
        .or_else(|| {
            server.as_ref().and_then(|s| {
                s.limits.max_size.map(|m| MaxSize {
                    vcpu: m.vcpu,
                    mem_gib: m.memory_gib,
                })
            })
        })
        .or_else(|| local.as_ref().and_then(|l| l.max_size));
    let ttl_hours = req
        .ttl_hours
        .or_else(|| server.as_ref().and_then(|s| s.limits.ttl_hours))
        .or_else(|| local.as_ref().map(|l| l.ttl_hours))
        .unwrap_or(model::DEFAULT_TTL_HOURS);
    let ttl_hours = match (name, ttl_hours) {
        // A Modal sandbox lives at most 24 hours.
        ("modal", 0) => 24,
        ("modal", h) => h.min(24),
        (_, h) => h,
    };
    let agent = req.agent.as_deref().filter(|a| !a.trim().is_empty());
    let line = |budget: Option<f64>| {
        let mut s = format!("cua cloud connect {name} --sealed --token-stdin");
        if !candidate.region.is_empty() {
            s.push_str(&format!(" --region {}", candidate.region));
        }
        match budget {
            Some(b) => s.push_str(&format!(" --budget {b}")),
            None => s.push_str(" --budget <USD>"),
        }
        if let Some(m) = max_size {
            s.push_str(&format!(" --max-size {m}"));
        }
        s
    };

    // 1. A budget first: no machine without one.
    if budget.is_none() {
        return Ok(failed(
            format!(
                "{} needs a monthly budget before Cua creates anything there. Ask the person \
                 for one in US dollars, then run this with it (the token on standard input). \
                 New machines are refused once it is reached; stopping and deleting always works.",
                p.title()
            ),
            Some(line(None)),
        ));
    }

    // 2. The token: given now; else the account's (sealed already); else
    // the one this device has (`--sealed` seals it).
    let given = req
        .target
        .token
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty());
    let (token, fresh) = if given {
        match p.local_secret(&req.target) {
            Some(t) => (t, true),
            None => {
                return Err(Error::InvalidArgument(format!(
                    "that is not a {} token{}",
                    p.title(),
                    if name == "modal" {
                        " (give <token id>:<token secret>, as `modal token new` prints them)"
                    } else {
                        ""
                    }
                )));
            }
        }
    } else if let Some(s) = server.as_ref().filter(|s| s.sealed_token.is_some()) {
        match tokens.open_server(s).await {
            Ok(t) => (t, false),
            Err(e) => return Ok(failed(e.to_string(), None)),
        }
    } else {
        match p.local_secret(&req.target) {
            Some(t) => (t, true),
            None => {
                return Ok(failed(
                    format!(
                        "No {} API token yet. Ask the person for a Read & Write token (Hetzner \
                         Cloud Console, Security, API tokens; Modal: `modal token new`) and pass \
                         it on standard input.",
                        p.title()
                    ),
                    Some(line(budget)),
                ));
            }
        }
    };

    // 3. The checks, with the token in memory only (nothing is created).
    let mut staged = target.clone();
    staged.token = Some(token.expose().to_string());
    p.stage_secret(&staged);
    if let Some(t) = staged.token.as_mut() {
        zeroize::Zeroize::zeroize(t);
    }
    let tested = p.test(&candidate).await;
    p.unstage_secret();
    if !tested.ok() {
        return Ok(outcome(
            clouds,
            p,
            None,
            status::FAILED,
            Some(CloudNext {
                run: Some(line(budget)),
                why: "The token did not pass the checks (see checks); nothing was stored. Fix \
                      what they say (a Read & Write token, the region), then run this again."
                    .into(),
                ..Default::default()
            }),
            tested.checks,
        ));
    }

    // 4. The account's connection: created, its limits in line, the token
    // sealed here and swapped in.
    let wanted = IssuerLimits {
        budget_usd_month: budget,
        max_size: max_size.map(|m| IssuerMaxSize {
            vcpu: m.vcpu,
            memory_gib: m.mem_gib,
        }),
        ttl_hours: Some(ttl_hours),
    };
    let server = match server {
        Some(s) => s,
        None => {
            let label = named.clone().unwrap_or_else(|| "default".into());
            let new = NewConnection {
                cloud: name.into(),
                identifiers: [(key.to_string(), label)].into(),
                region: Some(candidate.region.clone()).filter(|r| !r.is_empty()),
                limits: wanted.clone(),
            };
            match issuer.create_connection(&new, agent).await {
                Ok(c) => c,
                Err(IssuerError::Exists(id)) => {
                    match issuer
                        .connections()
                        .await
                        .map(|v| v.into_iter().find(|c| c.id == id))
                    {
                        Ok(Some(c)) => c,
                        Ok(None) => return Ok(issuer_failed(&IssuerError::Exists(id))),
                        Err(e) => return Ok(issuer_failed(&e)),
                    }
                }
                Err(e) => return Ok(issuer_failed(&e)),
            }
        }
    };
    let (server, limits_wait) = if server.limits == wanted {
        (server, false)
    } else {
        match issuer.set_limits(&server.id, &wanted, agent).await {
            Ok(r) => r,
            Err(e) => return Ok(issuer_failed(&e)),
        }
    };
    let server = if fresh || server.sealed_token.is_none() {
        match tokens.put(&server, &token, agent).await {
            Ok(s) => s,
            Err(e) => return Ok(failed(e.to_string(), Some(line(budget)))),
        }
    } else {
        server
    };
    drop(token);

    // 5. Linked on this device (no token stored here).
    candidate.account = tested.account.clone();
    candidate.ttl_hours = ttl_hours;
    candidate.connected_at = model::rfc3339(model::now());
    candidate.auth = model::Auth::Sealed {
        conn_id: server.id.clone(),
    };
    candidate.budget_usd_month = server.limits.budget_usd_month.or(budget);
    candidate.max_size = server
        .limits
        .max_size
        .map(|m| MaxSize {
            vcpu: m.vcpu,
            mem_gib: m.memory_gib,
        })
        .or(max_size);
    store.connect(candidate.clone()).map_err(Error::Io)?;
    tokens.forget(name);
    let mut a = crate::audit::entry("connect", name, agent.unwrap_or("user"));
    a.detail = format!(
        "{} sealed {} generation {} budget ${} max {}",
        candidate.label(p.title()),
        server.id,
        server.sealed_token.as_ref().map_or(0, |b| b.generation),
        candidate.budget_usd_month.unwrap_or_default(),
        candidate
            .max_size
            .map(|m| m.to_string())
            .unwrap_or_else(|| "none".into())
    );
    store.audit(a);
    if req.make_default {
        clouds.make_default(name)?;
    }
    let approval = if limits_wait && server.approval().is_empty() {
        "limits"
    } else {
        server.approval()
    };
    let holders = sealer.holders().await.unwrap_or_default();
    let mut out = if approval.is_empty() {
        outcome(
            clouds,
            p,
            Some(&candidate),
            status::CONNECTED,
            None,
            tested.checks,
        )
    } else {
        let mut o = outcome(
            clouds,
            p,
            Some(&candidate),
            status::WAITING_FOR_APPROVAL,
            Some(CloudNext {
                why: format!(
                    "An agent {} {}, so a person approves it in the Cua Spaces app (Computers, \
                     {}, Approve). Machines there are refused until then. Ask them to approve, \
                     then run: cua cloud status {name}",
                    if approval == "limits" {
                        "raised the limits of"
                    } else {
                        "gave the token of"
                    },
                    p.title(),
                    p.title()
                ),
                ..Default::default()
            }),
            tested.checks,
        );
        o.provider.approval = approval.into();
        o
    };
    out.provider.holders = holders;
    Ok(out)
}

/// Today's link, plus the connection's own limits.
async fn connect_cli(
    clouds: &Clouds,
    p: &dyn CloudApi,
    req: &ConnectRequest,
    limits: &Limits,
) -> Result<CloudConnected> {
    use cua_sandbox_core::byoc::CloudManager as _;
    let mut c = clouds
        .connect(&req.target, req.make_default, req.ttl_hours)
        .await?;
    if limits.budget.is_some() || limits.max_size.is_some() {
        let store = clouds.store();
        if let Some(mut conn) = store.connection(p.name()).map_err(Error::Io)? {
            conn.budget_usd_month = limits.budget.or(conn.budget_usd_month);
            conn.max_size = limits.max_size.or(conn.max_size);
            store.connect(conn.clone()).map_err(Error::Io)?;
            c.provider = clouds.row(p, Some(&conn), &clouds.default_on());
        }
    }
    c.provider.status = status::CONNECTED.into();
    c.verify = format!("cua cloud status {}", p.name());
    Ok(c)
}

/// The result of a run: the cloud as `cloud_status` lists it (`conn`: the
/// link as it stands, saved or not), the outcome and the next step.
fn outcome(
    clouds: &Clouds,
    p: &dyn CloudApi,
    conn: Option<&Connection>,
    word: &str,
    next: Option<CloudNext>,
    checks: Vec<cua_sandbox_core::byoc::CloudCheck>,
) -> CloudConnected {
    let saved = clouds
        .store()
        .connection(p.name())
        .ok()
        .flatten()
        .is_some_and(|s| Some(&s) == conn);
    let mut row: CloudProvider = clouds.row(p, conn, &clouds.default_on());
    row.connected = saved;
    row.status = word.into();
    CloudConnected {
        provider: row,
        checks,
        next,
        verify: format!("cua cloud status {}", p.name()),
    }
}

#[cfg(not(feature = "federation"))]
async fn connect_oidc(
    clouds: &Clouds,
    p: &dyn CloudApi,
    _req: &ConnectRequest,
    _limits: &Limits,
) -> Result<CloudConnected> {
    Ok(outcome(
        clouds,
        p,
        None,
        status::FAILED,
        Some(CloudNext {
            run: Some(cli_command(p.name())),
            why: "This build of cua links clouds through their CLI sign-in only.".into(),
            ..Default::default()
        }),
        vec![],
    ))
}

#[cfg(feature = "federation")]
pub(crate) fn issuer(clouds: &Clouds) -> Option<Arc<dyn crate::federation::IssuerClient>> {
    crate::sealed::issuer_of(&clouds.issuer)
}

/// A cloud's identifiers as the issuer records them (`account_id`,
/// `project_number`, `subscription_id`, ...).
#[cfg(feature = "federation")]
type Ids = std::collections::BTreeMap<String, String>;

/// The clouds Cua links through its issuer.
#[cfg(feature = "federation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fed {
    Aws,
    Gcp,
    Azure,
}

#[cfg(feature = "federation")]
fn given(o: &Option<String>) -> Option<String> {
    o.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Whether `s` is a GUID (`8-4-4-4-12` hex digits), as Azure ids are.
#[cfg(feature = "federation")]
fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(feature = "federation")]
impl Fed {
    fn of(name: &str) -> Option<Fed> {
        match name {
            "aws" => Some(Fed::Aws),
            "gcp" => Some(Fed::Gcp),
            "azure" => Some(Fed::Azure),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Fed::Aws => "aws",
            Fed::Gcp => "gcp",
            Fed::Azure => "azure",
        }
    }

    /// The identifier that names the connection (one live connection per
    /// value on the account).
    fn key(self) -> &'static str {
        match self {
            Fed::Aws => "account_id",
            Fed::Gcp => "project_number",
            Fed::Azure => "subscription_id",
        }
    }

    fn default_region(self) -> &'static str {
        match self {
            Fed::Aws => "us-west-2",
            Fed::Gcp => "us-central1",
            Fed::Azure => "westus2",
        }
    }

    /// The identifiers the request gives, checked.
    fn given(self, req: &ConnectRequest) -> Result<Ids> {
        let mut ids = Ids::new();
        let bad = |what: &str, v: &str| Err(Error::InvalidArgument(format!("{v:?} is not {what}")));
        match self {
            Fed::Aws => {
                use crate::federation::aws_account_of;
                let account = given(&req.account_id).or_else(|| {
                    req.role_arn
                        .as_deref()
                        .and_then(aws_account_of)
                        .map(str::to_string)
                });
                if let Some(a) = account {
                    if !(a.len() == 12 && a.bytes().all(|b| b.is_ascii_digit())) {
                        return Err(Error::InvalidArgument(format!(
                            "{a:?} is not an AWS account id (12 digits)"
                        )));
                    }
                    ids.insert("account_id".into(), a);
                }
                if let Some(r) = given(&req.role_arn) {
                    ids.insert("role_arn".into(), r);
                }
            }
            Fed::Gcp => {
                if let Some(n) = given(&req.project_number) {
                    if n.len() > 20 || !n.bytes().all(|b| b.is_ascii_digit()) {
                        return bad("a Google Cloud project number (digits)", &n);
                    }
                    ids.insert("project_number".into(), n);
                }
                if let Some(sa) = given(&req.service_account) {
                    if !sa.contains('@') || sa.contains(char::is_whitespace) {
                        return bad("a service account email", &sa);
                    }
                    ids.insert("service_account".into(), sa);
                }
                if let Some(p) = given(&req.target.project)
                    && !(p.len() >= 6
                        && p.len() <= 30
                        && p.starts_with(|c: char| c.is_ascii_lowercase())
                        && p.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
                {
                    return bad("a Google Cloud project id", &p);
                }
            }
            Fed::Azure => {
                for (key, v, what) in [
                    (
                        "subscription_id",
                        &req.subscription_id,
                        "an Azure subscription id",
                    ),
                    ("tenant_id", &req.tenant_id, "a Microsoft Entra tenant id"),
                    ("client_id", &req.client_id, "a managed identity client id"),
                ] {
                    if let Some(v) = given(v) {
                        if !is_guid(&v) {
                            return bad(what, &v);
                        }
                        ids.insert(key.into(), v.to_ascii_lowercase());
                    }
                }
                if let Some(rg) = given(&req.resource_group) {
                    let ok = rg.len() <= 90
                        && !rg.ends_with('.')
                        && rg
                            .chars()
                            .all(|c| c.is_alphanumeric() || "-_.()".contains(c));
                    if !ok {
                        return bad("an Azure resource group name", &rg);
                    }
                    ids.insert("resource_group".into(), rg);
                }
            }
        }
        Ok(ids)
    }

    /// What a saved link on this device knows.
    fn ids_of_link(self, c: &Connection) -> Ids {
        let mut ids = Ids::new();
        let mut put = |k: &str, v: &str| {
            if !v.is_empty() {
                ids.insert(k.to_string(), v.to_string());
            }
        };
        if let model::Auth::Oidc {
            role_arn,
            gcp_provider,
            service_account,
            tenant_id,
            client_id,
            ..
        } = &c.auth
        {
            match self {
                Fed::Aws => {
                    put("account_id", &c.account);
                    put("role_arn", role_arn);
                }
                Fed::Gcp => {
                    put("project_number", &c.account);
                    put("service_account", service_account);
                    if let Some(n) = crate::trust::gcp::project_number_of(gcp_provider) {
                        put("project_number", n);
                    }
                }
                Fed::Azure => {
                    put("subscription_id", &c.account);
                    put("resource_group", &c.project);
                    put("tenant_id", tenant_id);
                    put("client_id", client_id);
                }
            }
        }
        ids
    }

    /// Fills in what follows from the rest (the AWS role, the Google pool
    /// and provider, the Azure resource group).
    fn complete(self, ids: &mut Ids) {
        let defaults: Vec<(&str, String)> = match self {
            Fed::Aws => ids
                .get("account_id")
                .map(|a| vec![("role_arn", crate::federation::aws_role_arn(a))])
                .unwrap_or_default(),
            Fed::Gcp if ids.contains_key("project_number") => vec![
                ("pool", crate::trust::gcp::POOL.into()),
                ("provider", crate::trust::gcp::PROVIDER.into()),
            ],
            Fed::Gcp => vec![],
            Fed::Azure => vec![(
                "resource_group",
                crate::trust::azure::DEFAULT_RESOURCE_GROUP.into(),
            )],
        };
        for (k, v) in defaults {
            ids.entry(k.to_string()).or_insert(v);
        }
    }

    /// What is still missing before the link can be recorded and checked.
    fn missing(self, ids: &Ids) -> Vec<&'static str> {
        let need: &[&'static str] = match self {
            Fed::Aws => &["account_id"],
            Fed::Gcp => &["project_number"],
            Fed::Azure => &["subscription_id", "tenant_id", "client_id"],
        };
        need.iter()
            .copied()
            .filter(|k| !ids.contains_key(*k))
            .collect()
    }

    /// The link this device saves once the check passes.
    fn candidate(
        self,
        conn_id: &str,
        ids: &Ids,
        region: &str,
        req: &ConnectRequest,
        local: Option<&Connection>,
    ) -> Connection {
        let id = |k: &str| ids.get(k).cloned().unwrap_or_default();
        let mut c = Connection {
            provider: self.name().into(),
            region: region.into(),
            ..Default::default()
        };
        match self {
            Fed::Aws => {
                c.account = id("account_id");
                c.auth = model::Auth::aws(conn_id, id("role_arn"));
            }
            Fed::Gcp => {
                let number = id("project_number");
                c.project = given(&req.target.project)
                    .or_else(|| {
                        local
                            .map(|l| l.project.clone())
                            .filter(|p| !p.is_empty() && *p != number)
                    })
                    .unwrap_or_else(|| number.clone());
                c.zone = given(&req.target.zone)
                    .filter(|z| z.starts_with(&format!("{region}-")))
                    .or_else(|| {
                        local
                            .map(|l| l.zone.clone())
                            .filter(|z| z.starts_with(&format!("{region}-")))
                    })
                    .unwrap_or_else(|| format!("{region}-a"));
                c.auth = model::Auth::gcp(
                    conn_id,
                    crate::trust::gcp::provider_name(&number, &id("pool"), &id("provider")),
                    id("service_account"),
                );
                c.account = number;
            }
            Fed::Azure => {
                c.account = id("subscription_id");
                c.project = id("resource_group");
                c.auth = model::Auth::azure(conn_id, id("tenant_id"), id("client_id"));
            }
        }
        c
    }

    /// The flags that name the account in the command that repeats the
    /// run; `holes`: placeholders for what a person still has to give.
    fn flags(self, ids: &Ids, project: Option<&str>, holes: bool) -> String {
        let flag = |name: &str, key: &str, hole: &str| match ids.get(key) {
            Some(v) => format!(" --{name} {v}"),
            None if holes && !hole.is_empty() => format!(" --{name} {hole}"),
            None => String::new(),
        };
        match self {
            Fed::Aws => {
                let mut s = flag("account-id", "account_id", "<12-digit AWS account id>");
                // Only a role that is not the stack's own.
                if let (Some(a), Some(r)) = (ids.get("account_id"), ids.get("role_arn"))
                    && *r != crate::federation::aws_role_arn(a)
                {
                    s.push_str(&format!(" --role-arn {r}"));
                }
                s
            }
            Fed::Gcp => {
                let project = project
                    .map(|p| format!(" --project {p}"))
                    .unwrap_or_default();
                format!(
                    "{project}{}{}",
                    flag("project-number", "project_number", "<project number>"),
                    flag("service-account", "service_account", "")
                )
            }
            Fed::Azure => [
                flag("subscription", "subscription_id", "<subscription id>"),
                flag("tenant", "tenant_id", "<tenant id>"),
                flag("client-id", "client_id", "<client id>"),
                flag("resource-group", "resource_group", ""),
            ]
            .concat(),
        }
    }

    /// What the check proves, in plain words.
    fn check_words(self) -> &'static str {
        match self {
            Fed::Aws => "exchanges it with AWS STS and dry-runs a machine",
            Fed::Gcp => {
                "exchanges it with Google's Security Token Service and checks the role's \
                 permissions"
            }
            Fed::Azure => {
                "exchanges it with Microsoft Entra, reads the resource group and validates a \
                 machine"
            }
        }
    }
}

/// How the run was asked for, as the command that repeats it.
#[cfg(feature = "federation")]
struct Command<'a> {
    fed: Fed,
    region: &'a str,
    budget: Option<f64>,
    max_size: Option<MaxSize>,
    ids: &'a Ids,
    project: Option<&'a str>,
}

#[cfg(feature = "federation")]
impl Command<'_> {
    /// `cua cloud connect <cloud> [--oidc] --region … --budget …
    /// [--max-size …] <the account's flags> [--check]`; `holes`:
    /// placeholders for what the person gives.
    fn line(&self, check: bool, holes: bool) -> String {
        let oidc = if self.fed == Fed::Azure {
            ""
        } else {
            " --oidc"
        };
        let mut s = format!(
            "cua cloud connect {}{oidc} --region {}",
            self.fed.name(),
            self.region
        );
        match self.budget {
            Some(b) => s.push_str(&format!(" --budget {b}")),
            None => s.push_str(" --budget <USD>"),
        }
        if let Some(m) = self.max_size {
            s.push_str(&format!(" --max-size {m}"));
        }
        s.push_str(&self.fed.flags(self.ids, self.project, holes));
        if check {
            s.push_str(" --check");
        }
        s
    }
}

/// The step a person takes to make AWS trust Cua: the quick-create link
/// when an S3 copy of the template is known (`CUA_AWS_TEMPLATE_URL`), else
/// AWS CloudShell and the command to paste there.
#[cfg(feature = "aws")]
fn aws_trust_step(region: &str, issuer_base: &str, acct: &str, then: String) -> CloudNext {
    use crate::federation::aws;
    let host = aws::issuer_host(issuer_base);
    let base = issuer_base.trim_end_matches('/');
    let after = format!(
        " When the stack shows CREATE_COMPLETE, ask them for the 12-digit AWS account id \
         (top right of the AWS console) and run: {then}"
    );
    match std::env::var("CUA_AWS_TEMPLATE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    {
        Some(template) => CloudNext {
            open: Some(aws::quick_create_url(region, template.trim(), acct, &host)),
            why: format!(
                "AWS does not trust Cua yet. A person opens this link signed in to their AWS \
                 account, ticks \"I acknowledge that AWS CloudFormation might create IAM \
                 resources with custom names\" and presses Create stack (an OIDC provider and \
                 the cua-spaces role; no keys).{after}"
            ),
            ..Default::default()
        },
        None => CloudNext {
            open: Some(format!(
                "https://{region}.console.aws.amazon.com/cloudshell/home?region={region}"
            )),
            paste: Some(format!(
                "curl -fsSL '{base}/v1/templates/aws.yaml?acct={acct}' -o cua-spaces.yaml && \
                 aws cloudformation deploy --stack-name {} --template-file cua-spaces.yaml \
                 --capabilities CAPABILITY_NAMED_IAM --region {region}",
                aws::STACK_NAME
            )),
            why: format!(
                "AWS does not trust Cua yet. A person opens AWS CloudShell signed in to their \
                 AWS account and pastes the command: it creates the cua-spaces stack (an OIDC \
                 provider and the cua-spaces role; no keys).{after}"
            ),
            ..Default::default()
        },
    }
}

/// The step that makes `fed` trust Cua's tokens for account `acct`: an
/// exact command when an agent asks and this machine is signed in to the
/// cloud's CLI, else a page a person opens (and what they paste there).
#[cfg(feature = "federation")]
#[allow(clippy::too_many_arguments)]
fn trust_step(
    fed: Fed,
    p: &dyn CloudApi,
    region: &str,
    acct: &str,
    ids: &Ids,
    project: Option<&str>,
    agent: Option<&str>,
    then: String,
) -> CloudNext {
    use crate::trust;
    let base = crate::federation::issuer_url();
    let host = trust::issuer_host(&base);
    let agent_cli = agent.is_some() && p.detect().found;
    match fed {
        #[cfg(feature = "aws")]
        Fed::Aws => aws_trust_step(region, &base, acct, then),
        #[cfg(not(feature = "aws"))]
        Fed::Aws => CloudNext {
            why: format!("This build of cua does not include AWS. Then run: {then}"),
            ..Default::default()
        },
        Fed::Gcp => {
            let project = project.unwrap_or("<project id>");
            let after = format!(
                " It prints the project number. Run this with it (the person can read it in \
                 the output, or on the project's dashboard): {then}"
            );
            if agent_cli {
                CloudNext {
                    run: Some(trust::gcp::agent_command(project, acct, &host)),
                    why: format!(
                        "Google Cloud does not trust Cua yet, and this machine is signed in to \
                         gcloud: run this. It creates the workload identity pool cua with its \
                         provider, the custom role cuaSpaces and one binding for this Cua \
                         account (no keys, no service accounts), and needs rights to manage IAM \
                         in the project. If gcloud is refused, send the person `cua cloud \
                         connect gcp --oidc` without your agent name for the Cloud Shell \
                         link.{after}"
                    ),
                    ..Default::default()
                }
            } else {
                CloudNext {
                    open: Some(trust::gcp::cloud_shell_url(
                        &trust::repo(),
                        &trust::git_ref(),
                    )),
                    paste: Some(trust::gcp::paste(project, acct, &host)),
                    why: format!(
                        "Google Cloud does not trust Cua yet. A person opens Google Cloud Shell \
                         with this link (it opens Cua's trust script), signed in to the project, \
                         and pastes the command: it creates the workload identity pool cua with \
                         its provider, the custom role cuaSpaces and one binding for this Cua \
                         account. No keys, no service accounts.{after}"
                    ),
                    ..Default::default()
                }
            }
        }
        Fed::Azure => {
            let rg = ids
                .get("resource_group")
                .map(String::as_str)
                .unwrap_or(trust::azure::DEFAULT_RESOURCE_GROUP);
            let after = format!(
                " Then ask for the deployment's outputs clientId, tenantId and subscriptionId \
                 (Deployments, cua-spaces, Outputs) and run: {then}"
            );
            if agent_cli {
                CloudNext {
                    run: Some(trust::azure::agent_command(
                        ids.get("subscription_id").map(String::as_str),
                        rg,
                        region,
                        acct,
                        &host,
                    )),
                    why: format!(
                        "Azure does not trust Cua yet, and this machine is signed in to the az \
                         CLI: run this. It creates the resource group {rg} and, in it, the \
                         cua-spaces managed identity that trusts Cua's tokens for this account, \
                         with Virtual Machine Contributor and Network Contributor on that \
                         resource group only (no keys). It needs rights to assign roles there \
                         (Owner or User Access Administrator).{after}"
                    ),
                    ..Default::default()
                }
            } else {
                let issuer = if host == "oidc.cua.ai" {
                    String::new()
                } else {
                    format!(", Issuer Host {host}")
                };
                CloudNext {
                    open: Some(trust::azure::deploy_url(&trust::azure::template_url())),
                    why: format!(
                        "Azure does not trust Cua yet. A person opens this Deploy to Azure link \
                         signed in to Azure, picks the subscription, creates (or picks) the \
                         resource group {rg} in {region}, enters Cua Account {acct}{issuer}, \
                         and presses Review + create. It creates the cua-spaces managed \
                         identity that trusts Cua's tokens for this account, with Virtual \
                         Machine Contributor and Network Contributor on that resource group \
                         only. No keys.{after}"
                    ),
                    ..Default::default()
                }
            }
        }
    }
}

#[cfg(feature = "federation")]
async fn connect_oidc(
    clouds: &Clouds,
    p: &dyn CloudApi,
    req: &ConnectRequest,
    limits: &Limits,
) -> Result<CloudConnected> {
    use crate::federation::{
        IssuerError, IssuerLimits, IssuerMaxSize, NewConnection, ServerConnection,
    };
    let failed = |why: String, run: Option<String>| {
        outcome(
            clouds,
            p,
            None,
            status::FAILED,
            Some(CloudNext {
                run,
                why,
                ..Default::default()
            }),
            vec![],
        )
    };
    let Some(fed) = Fed::of(p.name()) else {
        return Ok(failed(
            format!(
                "Linking {} without a stored key is coming next. For now link it with its own \
                 sign-in on this device.",
                p.title()
            ),
            Some(cli_command(p.name())),
        ));
    };
    let title = p.title();
    let issuer_failed = |e: &IssuerError| {
        let run = matches!(e, IssuerError::SignIn(_) | IssuerError::Device(_))
            .then(|| "cua auth login".to_string());
        failed(e.to_string(), run)
    };
    let given_ids = fed.given(req)?;
    let Some(issuer) = issuer(clouds) else {
        return Ok(failed(
            "This device is not signed in to Cua yet.".into(),
            Some("cua auth login".into()),
        ));
    };
    let store = clouds.store();
    // The link this device has: an OIDC one (this flow, run again), or the
    // CLI sign-in it replaces, whose region, limits and lifetime carry over
    // (and which stays in force until the check passes).
    let linked = store.connection(p.name()).map_err(Error::Io)?;
    let local = linked.clone().filter(|c| !c.auth.is_cli());
    let cli_link = linked.filter(|c| c.auth.is_cli());
    let carried = local.as_ref().or(cli_link.as_ref());
    let servers = match issuer.connections().await {
        Ok(v) => v,
        Err(e) => return Ok(issuer_failed(&e)),
    };
    let key = fed.key();
    let wanted_key = given_ids.get(key).cloned();
    let live = |c: &&ServerConnection| c.cloud == fed.name() && c.status != "revoked";
    let server = servers
        .iter()
        .filter(live)
        .find(|c| {
            wanted_key
                .as_deref()
                .is_none_or(|k| c.identifiers.get(key).map(String::as_str) == Some(k))
        })
        .cloned();
    // The account: what the run says, over the issuer's record, over this
    // device's link (the latter two only when they name the same account).
    let anchor = wanted_key.clone().or_else(|| {
        server
            .as_ref()
            .and_then(|s| s.identifiers.get(key).cloned())
    });
    let mut ids = local
        .as_ref()
        .map(|l| fed.ids_of_link(l))
        .filter(|l| anchor.is_none() || l.get(key) == anchor.as_ref())
        .unwrap_or_default();
    if let Some(s) = &server {
        ids.extend(s.identifiers.clone());
    }
    ids.extend(given_ids.clone());
    fed.complete(&mut ids);
    let region = req
        .target
        .region
        .clone()
        .filter(|r| !r.trim().is_empty())
        .or_else(|| server.as_ref().and_then(|s| s.region.clone()))
        .or_else(|| carried.map(|l| l.region.clone()))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| fed.default_region().into());
    let budget = limits
        .budget
        .or_else(|| server.as_ref().and_then(|s| s.limits.budget_usd_month))
        .or_else(|| carried.and_then(|l| l.budget_usd_month));
    let max_size = limits
        .max_size
        .or_else(|| {
            server.as_ref().and_then(|s| {
                s.limits.max_size.map(|m| MaxSize {
                    vcpu: m.vcpu,
                    mem_gib: m.memory_gib,
                })
            })
        })
        .or_else(|| carried.and_then(|l| l.max_size));
    let ttl_hours = req
        .ttl_hours
        .or_else(|| server.as_ref().and_then(|s| s.limits.ttl_hours))
        .or_else(|| carried.map(|l| l.ttl_hours))
        .unwrap_or(model::DEFAULT_TTL_HOURS);
    // Google Cloud: the project id people know (the script takes it).
    let project = match fed {
        Fed::Gcp => given(&req.target.project).or_else(|| {
            local
                .as_ref()
                .map(|l| l.project.clone())
                .filter(|p| !p.is_empty() && Some(p) != ids.get("project_number"))
        }),
        _ => None,
    };
    let cmd = Command {
        fed,
        region: &region,
        budget,
        max_size,
        ids: &ids,
        project: project.as_deref(),
    };
    let agent = req.agent.as_deref().filter(|a| !a.trim().is_empty());

    // 1. A budget first: no machine without one.
    if budget.is_none() {
        return Ok(failed(
            format!(
                "{title} needs a monthly budget before Cua creates anything there. Ask the \
                 person for one in US dollars, then run this with it. New machines are refused \
                 once it is reached; stopping and deleting always works."
            ),
            Some(cmd.line(false, false)),
        ));
    }

    // 2. The trust: without the account's ids, nothing to check yet.
    if !fed.missing(&ids).is_empty() {
        let acct = match issuer.account().await {
            Ok(a) => a,
            Err(e) => return Ok(issuer_failed(&e)),
        };
        let mut out = outcome(
            clouds,
            p,
            None,
            status::NEEDS_TRUST,
            Some(trust_step(
                fed,
                p,
                &region,
                &acct.account,
                &ids,
                project.as_deref(),
                agent,
                cmd.line(true, true),
            )),
            vec![],
        );
        out.provider.region = region.clone();
        out.provider.auth = "oidc".into();
        out.provider.budget_usd_month = budget;
        out.provider.max_size = max_size.map(|m| m.to_string()).unwrap_or_default();
        return Ok(out);
    }

    // Never orphan: what Cua made in the account the CLI sign-in reaches
    // would be out of reach once the link points at another account (AWS).
    let account_id = ids.get("account_id").cloned().unwrap_or_default();
    if let Some(old) = cli_link
        .as_ref()
        .filter(|c| !account_id.is_empty() && !c.account.is_empty() && c.account != account_id)
    {
        let left = recorded_machines(clouds, p.name());
        if left > 0 {
            return Ok(failed(
                format!(
                    "AWS is linked to account {} through the AWS CLI sign-in, and Cua still \
                     records {left} {} it made there. Switching to account {account_id} would \
                     leave {} out of reach, so nothing was changed. Delete {} first, then run \
                     this again.",
                    old.account,
                    if left == 1 { "machine" } else { "machines" },
                    if left == 1 { "it" } else { "them" },
                    if left == 1 { "it" } else { "them" },
                ),
                Some("cua cloud sweep aws --all --delete".into()),
            ));
        }
    }

    // 3. The account's connection on the issuer: created, or its limits
    // brought in line (an agent's raise waits for approval).
    let wanted = IssuerLimits {
        budget_usd_month: budget,
        max_size: max_size.map(|m| IssuerMaxSize {
            vcpu: m.vcpu,
            memory_gib: m.mem_gib,
        }),
        ttl_hours: Some(ttl_hours),
    };
    let (server, limits_wait) = match server {
        Some(s) if s.limits == wanted => (s, false),
        Some(s) => match issuer.set_limits(&s.id, &wanted, agent).await {
            Ok(r) => r,
            Err(e) => return Ok(issuer_failed(&e)),
        },
        None => {
            let new = NewConnection {
                cloud: fed.name().into(),
                identifiers: ids.clone(),
                region: Some(region.clone()),
                limits: wanted.clone(),
            };
            match issuer.create_connection(&new, agent).await {
                Ok(c) => (c, false),
                // Linked meanwhile (another device or run): use that one.
                Err(IssuerError::Exists(id)) => {
                    match issuer.connections().await.map(|v| {
                        v.into_iter().find(|c| {
                            c.id == id
                                || (c.cloud == fed.name()
                                    && c.status != "revoked"
                                    && c.identifiers.get(key) == ids.get(key))
                        })
                    }) {
                        Ok(Some(c)) => (c, false),
                        Ok(None) => {
                            return Ok(issuer_failed(&IssuerError::Exists(id)));
                        }
                        Err(e) => return Ok(issuer_failed(&e)),
                    }
                }
                Err(e) => return Ok(issuer_failed(&e)),
            }
        }
    };
    // The issuer's record wins for what it holds (an id it recorded).
    let mut recorded = ids.clone();
    for (k, v) in &server.identifiers {
        recorded.entry(k.clone()).or_insert_with(|| v.clone());
    }
    // What is in force (an agent's raise waits for approval).
    let budget = server.limits.budget_usd_month.or(budget);
    let max_size = server
        .limits
        .max_size
        .map(|m| MaxSize {
            vcpu: m.vcpu,
            mem_gib: m.memory_gib,
        })
        .or(max_size);
    let mut candidate = fed.candidate(&server.id, &recorded, &region, req, local.as_ref());
    candidate.ttl_hours = ttl_hours;
    candidate.connected_at = model::rfc3339(model::now());
    candidate.budget_usd_month = budget;
    candidate.max_size = max_size;
    let approval = if limits_wait && server.approval().is_empty() {
        "limits"
    } else {
        server.approval()
    };

    // A link this device proved already (the same issuer connection and
    // account) takes new limits without a new check.
    let proven = local
        .as_ref()
        .is_some_and(|l| l.auth.conn_id() == server.id && l.account == candidate.account);
    let candidate = match local.as_ref().filter(|_| proven) {
        Some(l) => Connection {
            connected_at: l.connected_at.clone(),
            ..candidate
        },
        None => candidate,
    };

    // 4. The check: a `check` token, the exchange, a dry run.
    if !req.check && !proven {
        let mut out = outcome(
            clouds,
            p,
            Some(&candidate),
            status::CHECKING,
            Some(CloudNext {
                run: Some(cmd.line(true, false)),
                why: format!(
                    "The link is recorded. Run the check: it gets a check token from Cua, {}. \
                     Nothing is created.",
                    fed.check_words()
                ),
                ..Default::default()
            }),
            vec![],
        );
        out.provider.approval = approval.into();
        return Ok(out);
    }
    let tested: Tested = if req.check || !proven {
        p.test(&candidate).await
    } else {
        let mut t = Tested {
            account: candidate.account.clone(),
            checks: vec![],
        };
        t.check(
            "credentials",
            true,
            "proven before on this device (add --check to prove it again)",
        );
        t
    };
    if !tested.ok() {
        let untrusted = tested
            .checks
            .iter()
            .any(|c| !c.ok && c.name == "credentials" && c.detail.contains("did not accept"));
        if untrusted {
            let acct = match issuer.account().await {
                Ok(a) => a,
                Err(e) => return Ok(issuer_failed(&e)),
            };
            let mut out = outcome(
                clouds,
                p,
                Some(&candidate),
                status::NEEDS_TRUST,
                Some(trust_step(
                    fed,
                    p,
                    &region,
                    &acct.account,
                    &ids,
                    project.as_deref(),
                    agent,
                    cmd.line(true, false),
                )),
                tested.checks,
            );
            out.provider.approval = approval.into();
            return Ok(out);
        }
        let mut out = outcome(
            clouds,
            p,
            Some(&candidate),
            status::FAILED,
            Some(CloudNext {
                run: Some(cmd.line(true, false)),
                why: "The check failed (see checks). Fix what it says, then run the check again."
                    .into(),
                ..Default::default()
            }),
            tested.checks,
        );
        out.provider.approval = approval.into();
        return Ok(out);
    }

    // 5. Proven: saved on this device (replacing a CLI sign-in link).
    store.connect(candidate.clone()).map_err(Error::Io)?;
    let mut a = crate::audit::entry("connect", p.name(), agent.unwrap_or("user"));
    a.detail = format!(
        "{} oidc {} budget ${} max {}",
        candidate.label(title),
        server.id,
        budget.unwrap_or_default(),
        max_size
            .map(|m| m.to_string())
            .unwrap_or_else(|| "none".into())
    );
    store.audit(a);
    if req.make_default {
        clouds.make_default(p.name())?;
    }
    if approval.is_empty() {
        return Ok(outcome(
            clouds,
            p,
            Some(&candidate),
            status::CONNECTED,
            None,
            tested.checks,
        ));
    }
    let mut out = outcome(
        clouds,
        p,
        Some(&candidate),
        status::WAITING_FOR_APPROVAL,
        Some(CloudNext {
            why: format!(
                "An agent {} {title}, so a person approves it in the Cua Spaces app (Computers, \
                 {title}, Approve). New machines are refused until then. Ask them to approve, \
                 then run: cua cloud status {}",
                if approval == "limits" {
                    "raised the limits of"
                } else {
                    "linked"
                },
                p.name()
            ),
            ..Default::default()
        }),
        tested.checks,
    );
    out.provider.approval = approval.into();
    Ok(out)
}

/// `cloud_audit`'s account entries: what Cua's issuer recorded for the
/// clouds linked to the account (OIDC and sealed links), `source:
/// account`. An issuer that does not answer in 3 seconds, or this build
/// without federation, adds nothing.
pub(crate) async fn account_audit(
    clouds: &Clouds,
    conns: &[Connection],
    provider: Option<&str>,
    limit: usize,
) -> Vec<cua_sandbox_core::byoc::CloudAuditEntry> {
    #[cfg(feature = "federation")]
    {
        let linked: Vec<&Connection> = conns
            .iter()
            .filter(|c| !c.auth.is_cli())
            .filter(|c| provider.is_none_or(|p| c.provider.eq_ignore_ascii_case(p)))
            .collect();
        if linked.is_empty() {
            return vec![];
        }
        let Some(issuer) = issuer(clouds) else {
            return vec![];
        };
        let one = (linked.len() == 1).then(|| linked[0].auth.conn_id().to_string());
        let Ok(Ok(events)) = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            issuer.audit(one.as_deref(), limit),
        )
        .await
        else {
            return vec![];
        };
        events
            .into_iter()
            .filter_map(|e| {
                let conn = linked
                    .iter()
                    .find(|c| Some(c.auth.conn_id()) == e.connection.as_deref())?;
                let mut detail = Vec::new();
                if let Some(p) = e.purpose.as_deref().filter(|p| !p.is_empty()) {
                    detail.push(format!("purpose {p}"));
                }
                if let Some(d) = e.device.as_deref().filter(|d| !d.is_empty()) {
                    detail.push(format!("device {d}"));
                }
                if let Some(c) = e.code.as_deref().filter(|c| !c.is_empty()) {
                    detail.push(format!("code {c}"));
                }
                Some(cua_sandbox_core::byoc::CloudAuditEntry {
                    ts: model::rfc3339(e.at),
                    action: e.kind.clone(),
                    provider: conn.provider.clone(),
                    actor: e
                        .agent
                        .clone()
                        .filter(|a| !a.is_empty())
                        .unwrap_or_else(|| "user".into()),
                    outcome: if e.kind.ends_with("_refused") {
                        "refused".into()
                    } else {
                        "ok".into()
                    },
                    detail: crate::audit::redact(&detail.join(", ")),
                    source: "account".into(),
                    ..Default::default()
                })
            })
            .collect()
    }
    #[cfg(not(feature = "federation"))]
    {
        let _ = (clouds, conns, provider, limit);
        vec![]
    }
}

/// `cloud_status` for OIDC links: the issuer's word on each (waiting for
/// approval, revoked). An issuer that does not answer in 3 seconds leaves
/// the rows as this device knows them.
pub(crate) async fn merge_server_status(
    clouds: &Clouds,
    rows: &mut [CloudProvider],
    conns: &[Connection],
) {
    #[cfg(feature = "federation")]
    {
        // Every linked cloud's spend goes to the account in the background.
        for c in conns.iter().filter(|c| !c.auth.is_cli()) {
            clouds.store().share_spend(&c.provider);
        }
        let oidc: Vec<&Connection> = conns.iter().filter(|c| !c.auth.is_cli()).collect();
        if oidc.is_empty() {
            return;
        }
        let Some(issuer) = issuer(clouds) else {
            return;
        };
        let Ok(Ok(servers)) =
            tokio::time::timeout(std::time::Duration::from_secs(3), issuer.connections()).await
        else {
            return;
        };
        for c in oidc {
            let Some(row) = rows.iter_mut().find(|r| r.name == c.provider) else {
                continue;
            };
            let server = servers.iter().find(|s| s.id == c.auth.conn_id());
            // The account's month spend, when the issuer adds it up.
            if let Some(total) = server.and_then(|s| s.spent_month_usd) {
                clouds.spend.heard(c.auth.conn_id(), total);
                row.spent_month = (row.spent_month.max(total) * 100.0).round() / 100.0;
            }
            if matches!(c.auth, model::Auth::Sealed { .. }) {
                // Who can open it, and a re-seal under the pool's newer keys
                // when a device was revoked (in the background).
                if let Some(sealer) = clouds.sealed.sealer()
                    && let Ok(Ok(h)) =
                        tokio::time::timeout(std::time::Duration::from_secs(3), sealer.holders())
                            .await
                {
                    row.holders = h;
                }
                if let Some(s) = server.filter(|s| s.status == "connected").cloned() {
                    let tokens = clouds.sealed.clone();
                    tokio::spawn(async move { tokens.upkeep(&s).await });
                }
            }
            match server {
                // Revoked, or removed from the account: by another device,
                // the app, or a person on the issuer.
                Some(s) if s.status == "revoked" => {
                    row.status = status::FAILED.into();
                    row.approval = String::new();
                    row.issue = revoked_issue(clouds, c);
                }
                None => {
                    row.status = status::FAILED.into();
                    row.issue = revoked_issue(clouds, c);
                }
                Some(s) => {
                    row.approval = s.approval().into();
                    if !row.approval.is_empty() {
                        row.status = status::WAITING_FOR_APPROVAL.into();
                        row.approval_agent = s.approval_agent().unwrap_or_default().into();
                        if let Some(p) = s.pending_limits.as_ref() {
                            row.pending_budget_usd_month = p.budget_usd_month;
                            row.pending_max_size = p
                                .max_size
                                .map(|m| {
                                    MaxSize {
                                        vcpu: m.vcpu,
                                        mem_gib: m.memory_gib,
                                    }
                                    .to_string()
                                })
                                .unwrap_or_default();
                        }
                        row.can_approve = person_can_approve(clouds);
                    }
                }
            }
        }
    }
    #[cfg(not(feature = "federation"))]
    let _ = (clouds, rows, conns);
}

/// Whether this process acts for a person, who may approve what an agent
/// asked for (never an agent: neither a Cua agent nor a coding agent's
/// shell).
pub(crate) fn person_can_approve(clouds: &Clouds) -> bool {
    clouds.caller_agent().is_none()
}

/// What to do about a link the account no longer has, in one sentence
/// (`c`: this device's link; what this home still records there).
#[cfg(feature = "federation")]
fn revoked_issue(clouds: &Clouds, c: &Connection) -> String {
    let title = clouds
        .api(&c.provider)
        .map(|p| p.title().to_string())
        .unwrap_or_else(|_| c.provider.clone());
    let left = recorded_machines(clouds, &c.provider);
    let relink = format!("cua cloud connect {} --oidc", c.provider);
    if left == 0 {
        return format!(
            "{title} was disconnected from your Cua account (on another device or in the app). \
             Link it again with `{relink}`, or forget it here with `cua cloud disconnect {}`.",
            c.provider
        );
    }
    format!(
        "{title} was disconnected from your Cua account (on another device or in the app), and \
         Cua still records {left} {} it made from this device that may still run and bill. Link \
         {title} again with `{relink}`, then delete {} with `cua cloud disconnect {} --delete`.",
        if left == 1 { "machine" } else { "machines" },
        if left == 1 { "it" } else { "them" },
        c.provider,
    )
}

/// `cloud_approve`: a person approves (or with `deny` declines) what an
/// agent asked for on `provider`'s link: the link itself, or a raise of its
/// limits. Refused when an agent calls; the issuer refuses agents as well.
pub(crate) async fn approve_with(
    clouds: &Clouds,
    provider: &str,
    deny: bool,
) -> Result<CloudProvider> {
    let p = clouds.api(provider)?.clone();
    if !person_can_approve(clouds) {
        return Err(Error::InvalidArgument(format!(
            "only a person can {} {} for an agent: ask them to open Cua Spaces (Computers, {}) \
             or run `cua cloud approve {}` themselves",
            if deny { "deny" } else { "approve" },
            p.title(),
            p.title(),
            p.name()
        )));
    }
    #[cfg(feature = "federation")]
    {
        let store = clouds.store();
        let Some(conn) = store
            .connection(p.name())
            .map_err(Error::Io)?
            .filter(|c| !c.auth.is_cli())
        else {
            return Err(Error::InvalidArgument(format!(
                "{} has nothing waiting for approval (it is not linked through your Cua \
                 account on this device)",
                p.title()
            )));
        };
        let Some(issuer) = issuer(clouds) else {
            return Err(Error::Cloud(
                "this device is not signed in to Cua; run `cua auth login`".into(),
            ));
        };
        let server = issuer
            .approve(conn.auth.conn_id(), deny)
            .await
            .map_err(Error::from)?;
        // What is in force now is the issuer's word.
        let mut saved = conn.clone();
        saved.budget_usd_month = server.limits.budget_usd_month.or(saved.budget_usd_month);
        if let Some(m) = server.limits.max_size {
            saved.max_size = Some(MaxSize {
                vcpu: m.vcpu,
                mem_gib: m.memory_gib,
            });
        }
        if server.status == "revoked" {
            // A denied link: nothing may run there; forget it like a
            // disconnect once nothing Cua made is recorded there.
            if recorded_machines(clouds, p.name()) == 0 {
                store.disconnect(p.name()).map_err(Error::Io)?;
            }
        } else {
            store.connect(saved.clone()).map_err(Error::Io)?;
        }
        let mut a = crate::audit::entry(if deny { "deny" } else { "approve" }, p.name(), "user");
        a.detail = format!("{} {}", server.id, server.status);
        store.audit(a);
        let linked = store.connection(p.name()).map_err(Error::Io)?;
        let mut row = clouds.row(p.as_ref(), linked.as_ref(), &clouds.default_on());
        let mut rows = [row.clone()];
        if let Some(l) = linked {
            merge_server_status(clouds, &mut rows, std::slice::from_ref(&l)).await;
            row = rows[0].clone();
        } else {
            row.status = status::NOT_CONNECTED.into();
        }
        Ok(row)
    }
    #[cfg(not(feature = "federation"))]
    {
        let _ = (clouds, deny);
        Err(Error::InvalidArgument(format!(
            "{} has nothing waiting for approval in this build",
            p.title()
        )))
    }
}

/// Before an OIDC link is forgotten: the issuer stops minting tokens for it
/// (every device of the account). Fails closed: a link the issuer could not
/// revoke is kept, so the person knows tokens may still be minted.
pub(crate) async fn revoke_on_issuer(clouds: &Clouds, conn: &Connection) -> Result<()> {
    #[cfg(feature = "federation")]
    if let model::Auth::Oidc { conn_id, .. } | model::Auth::Sealed { conn_id } = &conn.auth {
        use crate::federation::IssuerError;
        clouds.sealed.forget(&conn.provider);
        let Some(issuer) = issuer(clouds) else {
            return Err(Error::Cloud(format!(
                "this device is not signed in to Cua, so the {} link could not be revoked; \
                 run `cua auth login` and disconnect again",
                conn.provider
            )));
        };
        match issuer
            .revoke(conn_id, cua_sandbox_core::byoc::caller_agent().as_deref())
            .await
        {
            Ok(()) | Err(IssuerError::NotFound(_)) | Err(IssuerError::Revoked) => {}
            Err(e) => {
                return Err(Error::Cloud(format!(
                    "{} was NOT disconnected: Cua could not stop its tokens ({e}). Try again.",
                    conn.provider
                )));
            }
        }
    }
    #[cfg(not(feature = "federation"))]
    let _ = (clouds, conn);
    Ok(())
}
