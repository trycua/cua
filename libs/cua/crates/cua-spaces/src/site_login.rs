//! The seam between the `request_site_login` MCP tool and the Cua Keyvault.
//!
//! "Log me into this site" must never hand a password to the caller, which
//! is usually a model. The tool only files a Keyvault request and, on a
//! retry, asks the broker for the decision. When the user approved (or an
//! unattended rule they wrote matches), the broker, running in the daemon's
//! own hardened process, picks the saved login for the page's exact origin
//! and fills the form through the Space's cua-driver. The tool reports what
//! happened with a masked username and nothing else.
//!
//! The trait keeps `cua-spaces` free of a `cua-keyvault` dependency: the
//! daemon supplies the implementation (`cua_daemon::keyvault`) through
//! [`crate::Spaces::set_site_login_broker`]. With none wired in the tool
//! is fail-closed.
//!
//! [`request_site_login`] is the whole tool, reusable by other callers that
//! act for a named agent (the persistent-agent bridge passes `agent`).

use std::time::Duration;

use serde_json::{Value, json};

use crate::{Error, Result, Spaces};

/// A Keyvault failure, surfaced with its code (`denied`, `locked`,
/// `not_found`, `forbidden`, `disabled`, ...). Never carries a secret.
#[derive(Clone, Debug)]
pub struct SiteLoginError {
    /// The Keyvault error code.
    pub code: String,
    /// A human-readable message.
    pub message: String,
}

/// The browser tab to sign in (cua-driver ids, all optional).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct BrowserTab {
    /// cua-driver lifecycle session label.
    pub session: Option<String>,
    /// cua-driver browser target id.
    pub target_id: Option<String>,
    /// cua-driver tab id.
    pub tab_id: Option<String>,
}

/// What a sign-in did. Never the password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteLoginFilled {
    /// Site (registrable domain) of the saved login.
    pub site: String,
    /// The origin signed in to.
    pub origin: String,
    /// Username, masked (`a***@example.test`).
    pub username_hint: String,
    /// The form was submitted.
    pub submitted: bool,
    /// The tab's URL after the fill.
    pub page_url: String,
    /// The tab it filled.
    pub browser: BrowserTab,
}

/// The outcome of awaiting a decision and (when granted) signing in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SiteLoginOutcome {
    /// The user has not decided yet.
    Pending,
    /// The user declined (with the reason).
    Denied(String),
    /// Signed in.
    Filled(SiteLoginFilled),
}

/// One sign-in request.
#[derive(Clone, Debug, Default)]
pub struct SiteLoginAsk {
    /// Target Space (its canonical id).
    pub target: String,
    /// The sign-in page.
    pub url: String,
    /// Which saved username, when there are several.
    pub username: Option<String>,
    /// The named agent asking (approval wording and audit).
    pub agent: Option<String>,
    /// The tab to fill.
    pub browser: BrowserTab,
    /// See [`cua_spaces_contract::inputs::RequestSiteLogin::relay_plaintext_ack`].
    pub relay_plaintext_ack: bool,
}

/// The Keyvault operations `request_site_login` uses. The caller is an
/// unverified third party, so every sign-in needs the user's approval (one
/// use per approval by default) or an unattended rule the user wrote.
#[async_trait::async_trait]
pub trait SiteLoginBroker: Send + Sync {
    /// Files a request to sign in to `ask.url`'s site in `ask.target`.
    /// Returns the request id. Types nothing.
    async fn request_login(
        &self,
        ask: &SiteLoginAsk,
    ) -> std::result::Result<String, SiteLoginError>;

    /// Waits up to `timeout` for the decision on `request_id` and, when it
    /// is granted, signs in.
    async fn await_and_fill(
        &self,
        request_id: &str,
        ask: &SiteLoginAsk,
        timeout: Duration,
    ) -> std::result::Result<SiteLoginOutcome, SiteLoginError>;
}

/// Default and maximum wait for the user's decision on a retry.
pub const DEFAULT_WAIT_SECS: u32 = 20;
/// See [`DEFAULT_WAIT_SECS`].
pub const MAX_WAIT_SECS: u32 = 120;

/// The `request_site_login` tool: files a request, or with `request_id`
/// waits for the decision and signs in. `agent` overrides the argument's
/// (a caller that knows which agent it acts for passes it here, so the
/// agent cannot name another one). Returns the tool's JSON result; a
/// refusal is `Err(Error::LoginRefused)`.
pub async fn request_site_login(
    spaces: &Spaces,
    a: cua_spaces_contract::inputs::RequestSiteLogin,
    agent: Option<String>,
) -> Result<Value> {
    let space = spaces.space(&a.space).await?;
    let target = space.id().to_string();
    let url = a.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::invalid(format!(
            "url must be the site's http(s) sign-in page, not {url:?}"
        )));
    }
    let ask = SiteLoginAsk {
        target: target.clone(),
        url: url.clone(),
        username: a.username.filter(|u| !u.is_empty()),
        agent: agent.or(a.agent).filter(|x| !x.is_empty()),
        browser: BrowserTab {
            session: a.session.filter(|x| !x.is_empty()),
            target_id: a.target_id.filter(|x| !x.is_empty()),
            tab_id: a.tab_id.filter(|x| !x.is_empty()),
        },
        relay_plaintext_ack: a.relay_plaintext_ack,
    };
    let Some(broker) = spaces.site_login_broker() else {
        return Err(Error::host(
            "keyvault",
            "site login needs the Cua Keyvault in `cua daemon` (open Cua, or run `cua daemon`); \
             nothing was typed",
        ));
    };
    let refused = |e: SiteLoginError| Error::LoginRefused(format!("{}: {}", e.code, e.message));
    match a.request_id.filter(|r| !r.is_empty()) {
        None => {
            let id = broker.request_login(&ask).await.map_err(refused)?;
            Ok(json!({
                "status": "pending",
                "request_id": id,
                "space": target,
                "url": url,
                "message": "Signing in with the user's saved password needs their approval in Cua.",
                "instructions": "Tell the user to approve the sign-in in Cua (it asks for Touch ID or their login password). Then call request_site_login again with the same space and url and this request_id. Do not retry without the user, and never ask the user for the password.",
            }))
        }
        Some(id) => {
            let wait = a
                .wait_secs
                .unwrap_or(DEFAULT_WAIT_SECS)
                .clamp(1, MAX_WAIT_SECS);
            match broker
                .await_and_fill(&id, &ask, Duration::from_secs(u64::from(wait)))
                .await
                .map_err(refused)?
            {
                SiteLoginOutcome::Pending => Ok(json!({
                    "status": "pending",
                    "request_id": id,
                    "space": target,
                    "instructions": "The user has not decided yet. Remind them to approve it in Cua, then call again with this request_id.",
                })),
                SiteLoginOutcome::Denied(why) => Err(Error::LoginRefused(format!(
                    "denied: the user declined the sign-in ({why}); do not ask again unless they bring it up"
                ))),
                SiteLoginOutcome::Filled(f) => Ok(json!({
                    "status": "filled",
                    "space": target,
                    "site": f.site,
                    "origin": f.origin,
                    "username_hint": f.username_hint,
                    "submitted": f.submitted,
                    "page_url": f.page_url,
                    "session": f.browser.session,
                    "target_id": f.browser.target_id,
                    "tab_id": f.browser.tab_id,
                    "next": "The page is signed in. Continue in that tab (get_browser_state with this target_id and tab_id) and check the page shows the signed-in state.",
                })),
            }
        }
    }
}
