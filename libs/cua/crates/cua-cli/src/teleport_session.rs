//! MCP `teleport_browser_session`: move the user's signed-in session for
//! specific sites from a browser on this machine into a sandbox, through
//! the Cua Keyvault and only with the user's consent.
//!
//! Contract (Keyvault design, section 5.13):
//!
//! - The caller names the sites. Nothing is selected by default and there
//!   is no "all sessions" option: an empty list, `*` or `all` is refused.
//! - This server never mints consent itself (a model could replay it). The
//!   first call files a Keyvault access request (`RequestAccess` with one
//!   `Selector::Site` per site, the sandbox as the only target, the
//!   teleport action and a duration) and returns `consent_required` with
//!   the request id. The user approves it in Cua (Keyvault page, OS user
//!   presence).
//! - A later call with that `request_id` awaits the decision; a granted
//!   capability token is used for one `Teleport` per site item. The token
//!   never leaves this process.
//!
//! The broker is the [`Broker`] trait so the tool logic is testable; the
//! production implementation is the Keyvault IPC client, which ships with
//! Cua Spaces (source-available) and is registered by its build of `cua`
//! ([`crate::extension`]). Without it [`Unwired`] answers every call with
//! `requires_cua_app`, so the tool can never move a session outside the
//! Keyvault.

use serde_json::{Value, json};
use std::time::Duration;

/// Default and maximum grant duration the tool asks for.
pub const DEFAULT_DURATION: Duration = Duration::from_secs(15 * 60);
pub const MAX_DURATION: Duration = Duration::from_secs(24 * 3600);
/// How long one call waits for the user's decision before reporting that
/// the request is still pending.
pub const DECISION_WAIT: Duration = Duration::from_secs(20);

/// One site of one browser: a Keyvault `Selector::Site`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteSelector {
    pub app: String,
    pub site: String,
}

/// A Keyvault `RequestAccess`.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessRequest {
    pub selectors: Vec<SiteSelector>,
    /// Exactly one target: the sandbox (Space) ref.
    pub targets: Vec<String>,
    pub duration: Duration,
    /// Free text the Keyvault shows as unverified.
    pub reason: String,
}

/// A Keyvault `AwaitDecision` outcome. Constructed by the Keyvault client
/// (and the tests) until that client is wired in.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Pending,
    Denied(String),
    /// A capability token bound to this caller, and the vault items (one per
    /// approved site) it covers.
    Granted {
        token: String,
        items: Vec<GrantedItem>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GrantedItem {
    pub id: String,
    pub site: String,
}

/// Keyvault failures the tool reports with their own kind.
#[derive(Clone, Debug, PartialEq)]
pub enum BrokerError {
    /// No Keyvault to talk to: Cua is not installed, or not running.
    RequiresCuaApp {
        installed: bool,
        install_url: String,
        open_url: String,
    },
    /// Any other Keyvault error code (`denied`, `expired`, `disabled`, ...).
    Keyvault { code: String, message: String },
}

impl BrokerError {
    pub fn kind(&self) -> &str {
        match self {
            Self::RequiresCuaApp { .. } => "requires_cua_app",
            Self::Keyvault { code, .. } => code,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::RequiresCuaApp {
                installed: true,
                open_url,
                ..
            } => format!(
                "teleport goes through the Cua Keyvault, and Cua is installed but not running: ask the user to open Cua ({open_url}), then retry"
            ),
            Self::RequiresCuaApp {
                install_url,
                open_url,
                ..
            } => format!(
                "teleport goes through the Cua Keyvault, which needs the Cua app: install it from {install_url}, open {open_url}, then retry"
            ),
            Self::Keyvault { code, message } => format!("keyvault {code}: {message}"),
        }
    }
}

/// The Keyvault operations the tool uses (design section 8).
#[async_trait::async_trait]
pub trait Broker: Send + Sync {
    /// Files an access request; returns its id. Moves nothing.
    async fn request_access(&self, request: &AccessRequest) -> Result<String, BrokerError>;
    /// Waits at most `timeout` for the user's decision on `id`.
    async fn await_decision(&self, id: &str, timeout: Duration) -> Result<Decision, BrokerError>;
    /// Delivers one vault item to `target` with a granted token.
    async fn teleport(&self, token: &str, item: &str, target: &str) -> Result<Value, BrokerError>;
}

/// The no-Keyvault fallback broker: every call is `requires_cua_app`, so
/// nothing can move outside the Keyvault. Used when no Cua Spaces extension
/// provides the Keyvault broker (see [`default_broker`]) and by the tests.
pub struct Unwired;

#[async_trait::async_trait]
impl Broker for Unwired {
    async fn request_access(&self, _: &AccessRequest) -> Result<String, BrokerError> {
        Err(requires_cua_app())
    }
    async fn await_decision(&self, _: &str, _: Duration) -> Result<Decision, BrokerError> {
        Err(requires_cua_app())
    }
    async fn teleport(&self, _: &str, _: &str, _: &str) -> Result<Value, BrokerError> {
        Err(requires_cua_app())
    }
}

/// The error when no Keyvault is reachable: install or open the Cua app.
pub fn requires_cua_app() -> BrokerError {
    BrokerError::RequiresCuaApp {
        installed: false,
        install_url: "https://cua.ai/download".into(),
        open_url: "cua://keyvault".into(),
    }
}

/// The production broker: the Keyvault's, when the Cua Spaces build
/// registered one ([`crate::extension`]), else [`Unwired`] so the tool
/// fails closed.
pub fn default_broker() -> std::sync::Arc<dyn Broker> {
    crate::extension::get()
        .and_then(|e| e.session_broker())
        .unwrap_or_else(|| std::sync::Arc::new(Unwired))
}

/// Normalizes the `browser` argument (chromium, brave and edge profiles
/// are Chrome-format).
pub fn browser_app(browser: &str) -> Result<&'static str, String> {
    match browser.trim().to_ascii_lowercase().as_str() {
        "firefox" => Ok("firefox"),
        "chrome" | "chromium" | "google-chrome" | "brave" | "edge" => Ok("chrome"),
        other => Err(format!("browser must be firefox or chrome, not {other:?}")),
    }
}

/// The explicit site list: registrable domains, deduplicated, never empty
/// and never a wildcard.
pub fn sites(value: &Value) -> Result<Vec<String>, String> {
    let list = value
        .as_array()
        .ok_or("sites is required: the exact sites to teleport, for example [\"github.com\"]")?;
    let mut out: Vec<String> = vec![];
    for s in list {
        let raw = s.as_str().ok_or("sites must be strings")?.trim();
        let site = raw
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .trim_start_matches("www.")
            .to_ascii_lowercase();
        if site.is_empty() {
            return Err("an empty site was given".into());
        }
        if site == "*" || site == "all" || site.contains('*') {
            return Err("there is no \"all sessions\" option: name each site to teleport".into());
        }
        if !site.contains('.') || site.contains(char::is_whitespace) {
            return Err(format!("{raw:?} is not a site such as github.com"));
        }
        if !out.contains(&site) {
            out.push(site);
        }
    }
    if out.is_empty() {
        return Err(
            "sites is empty: name each site to teleport; nothing is selected by default".into(),
        );
    }
    Ok(out)
}

/// The grant duration from `duration_minutes` (default 15, at most 24 h).
pub fn duration(value: &Value) -> Result<Duration, String> {
    match value.as_u64() {
        None if value.is_null() => Ok(DEFAULT_DURATION),
        None => Err("duration_minutes must be a whole number".into()),
        Some(0) => Err("duration_minutes must be at least 1".into()),
        Some(m) => Ok(Duration::from_secs(m.saturating_mul(60)).min(MAX_DURATION)),
    }
}

/// Records the `cua_teleport_attempted` / `cua_teleport_completed` events
/// of one `teleport_browser_session` call. No broker verified this process,
/// so `caller_kind` is self-reported (`unknown` for the CLI). A pending
/// consent records no completion.
pub fn record_telemetry(
    app: &str,
    first_call: bool,
    out: &Result<Value, BrokerError>,
    started: std::time::Instant,
    sites: usize,
) {
    use cua_telemetry::events::{self, CallerKind, CallerKindSource, TeleportOutcome as O};
    let t = cua_telemetry::global();
    let info = events::TeleportInfo::new(
        app,
        "full",
        "session",
        CallerKind::self_reported(t.product()),
        CallerKindSource::SelfReported,
        "sdk",
    );
    if first_call {
        t.capture(events::teleport_attempted(&info));
    }
    let outcome = match out {
        Ok(v) if v["moved"] == true => Some(O::Ok),
        Ok(_) => None,
        Err(e) => Some(match e.kind() {
            "requires_cua_app" => O::RequiresCuaApp,
            "denied" | "expired" => O::ConsentDenied,
            "disabled" => O::Disabled,
            "locked" => O::Locked,
            "rate_limited" => O::RateLimited,
            "forbidden" => O::Forbidden,
            "not_found" => O::NotFound,
            "invalid" => O::Invalid,
            _ => O::Error,
        }),
    };
    if let Some(o) = outcome {
        t.capture(events::teleport_completed(
            &info,
            o,
            started.elapsed(),
            sites as u64,
        ));
    }
}

/// First call: files the request and returns the consent requirement.
pub async fn request(
    broker: &dyn Broker,
    sandbox: &str,
    app: &str,
    sites: &[String],
    duration: Duration,
    reason: &str,
) -> Result<Value, BrokerError> {
    let req = AccessRequest {
        selectors: sites
            .iter()
            .map(|s| SiteSelector {
                app: app.into(),
                site: s.clone(),
            })
            .collect(),
        targets: vec![sandbox.into()],
        duration,
        reason: reason.into(),
    };
    let id = broker.request_access(&req).await?;
    Ok(consent_required(&id, sandbox, app, sites, duration))
}

fn consent_required(id: &str, sandbox: &str, app: &str, sites: &[String], d: Duration) -> Value {
    json!({
        "consent_required": true,
        "moved": false,
        "request_id": id,
        "browser": app,
        "sites": sites,
        "target": sandbox,
        "duration_minutes": d.as_secs() / 60,
        "summary": format!(
            "Teleport your {app} sign-in for {} into the sandbox {sandbox} for {} minutes. Nothing else from your browser moves.",
            sites.join(", "),
            d.as_secs() / 60
        ),
        "approve": format!(
            "The user approves request {id} in Cua (Keyvault page), which asks for Touch ID or the login password."
        ),
        "instructions": "Show the summary to the user and ask them to approve it in Cua. Then call teleport_browser_session again with the same name, browser and sites and this request_id. Do not retry without the user.",
    })
}

/// Later call: awaits the decision and, when granted, delivers each site.
pub async fn complete(
    broker: &dyn Broker,
    sandbox: &str,
    app: &str,
    sites: &[String],
    request_id: &str,
) -> Result<Value, BrokerError> {
    match broker.await_decision(request_id, DECISION_WAIT).await? {
        Decision::Pending => Ok(json!({
            "consent_required": true,
            "moved": false,
            "request_id": request_id,
            "status": "pending",
            "instructions": "The user has not decided yet. Remind them to approve it in Cua, then call again with this request_id.",
        })),
        Decision::Denied(why) => Err(BrokerError::Keyvault {
            code: "denied".into(),
            message: format!(
                "the user declined the teleport ({why}); do not ask again unless they bring it up"
            ),
        }),
        Decision::Granted { token, items } => {
            let mut delivered = vec![];
            for item in items.iter().filter(|i| sites.contains(&i.site)) {
                let r = broker.teleport(&token, &item.id, sandbox).await?;
                delivered.push(json!({"site": item.site, "result": r}));
            }
            Ok(json!({
                "consent_required": false,
                "moved": true,
                "browser": app,
                "target": sandbox,
                "delivered": delivered,
                "next": "The sandbox's browser now has these sites' sessions. Delete the sandbox when done; Wipe in Cua's Keyvault also removes them.",
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        requests: Mutex<Vec<AccessRequest>>,
        decision: Mutex<Option<Decision>>,
        delivered: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl Broker for Fake {
        async fn request_access(&self, r: &AccessRequest) -> Result<String, BrokerError> {
            self.requests.lock().unwrap().push(r.clone());
            Ok("req-1".into())
        }
        async fn await_decision(&self, id: &str, _: Duration) -> Result<Decision, BrokerError> {
            assert_eq!(id, "req-1");
            Ok(self
                .decision
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Decision::Pending))
        }
        async fn teleport(
            &self,
            token: &str,
            item: &str,
            target: &str,
        ) -> Result<Value, BrokerError> {
            self.delivered
                .lock()
                .unwrap()
                .push((token.into(), item.into(), target.into()));
            Ok(json!({"import_id": format!("imp-{item}")}))
        }
    }

    #[test]
    fn sites_are_explicit_and_never_all() {
        assert_eq!(
            sites(&json!([
                "https://www.GitHub.com/login",
                "github.com",
                "news.ycombinator.com"
            ]))
            .unwrap(),
            ["github.com", "news.ycombinator.com"]
        );
        for bad in [
            json!([]),
            json!(null),
            json!(["*"]),
            json!(["all"]),
            json!(["*.google.com"]),
            json!(["localhost"]),
            json!([1]),
        ] {
            assert!(sites(&bad).is_err(), "{bad}");
        }
        assert_eq!(duration(&Value::Null).unwrap(), DEFAULT_DURATION);
        assert_eq!(duration(&json!(100000)).unwrap(), MAX_DURATION);
        assert!(duration(&json!(0)).is_err());
        assert_eq!(browser_app("Brave").unwrap(), "chrome");
        assert!(browser_app("safari").is_err());
    }

    #[tokio::test]
    async fn first_call_files_a_request_and_moves_nothing() {
        let fake = Fake::default();
        let s = vec!["github.com".to_string()];
        let v = request(
            &fake,
            "local:web",
            "chrome",
            &s,
            DEFAULT_DURATION,
            "log in to GitHub",
        )
        .await
        .unwrap();
        assert_eq!(v["consent_required"], true);
        assert_eq!(v["moved"], false);
        assert_eq!(v["request_id"], "req-1");
        assert_eq!(v["sites"], json!(["github.com"]));
        assert!(
            v["approve"]
                .as_str()
                .unwrap()
                .contains("request req-1 in Cua")
        );
        let req = &fake.requests.lock().unwrap()[0];
        assert_eq!(req.targets, ["local:web"]);
        assert_eq!(
            req.selectors,
            [SiteSelector {
                app: "chrome".into(),
                site: "github.com".into()
            }]
        );
        assert!(fake.delivered.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn only_a_granted_request_delivers_and_only_the_named_sites() {
        let fake = Fake::default();
        let s = vec!["github.com".to_string()];
        let pending = complete(&fake, "local:web", "chrome", &s, "req-1")
            .await
            .unwrap();
        assert_eq!(pending["status"], "pending");
        *fake.decision.lock().unwrap() = Some(Decision::Denied("user".into()));
        assert_eq!(
            complete(&fake, "local:web", "chrome", &s, "req-1")
                .await
                .unwrap_err()
                .kind(),
            "denied"
        );
        assert!(fake.delivered.lock().unwrap().is_empty());
        *fake.decision.lock().unwrap() = Some(Decision::Granted {
            token: "cuakv1.t".into(),
            items: vec![
                GrantedItem {
                    id: "i-gh".into(),
                    site: "github.com".into(),
                },
                GrantedItem {
                    id: "i-x".into(),
                    site: "example.org".into(),
                },
            ],
        });
        let done = complete(&fake, "local:web", "chrome", &s, "req-1")
            .await
            .unwrap();
        assert_eq!(done["moved"], true);
        assert_eq!(
            *fake.delivered.lock().unwrap(),
            [(
                "cuakv1.t".to_string(),
                "i-gh".to_string(),
                "local:web".to_string()
            )]
        );
        assert!(
            !done.to_string().contains("cuakv1"),
            "the token never leaves: {done}"
        );
    }

    #[tokio::test]
    async fn unwired_never_moves_anything() {
        let s = vec!["github.com".to_string()];
        let e = request(&Unwired, "local:web", "firefox", &s, DEFAULT_DURATION, "")
            .await
            .unwrap_err();
        assert_eq!(e.kind(), "requires_cua_app");
        assert!(e.message().contains("https://cua.ai/download"));
    }
}
