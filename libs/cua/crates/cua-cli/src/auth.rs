//! `cua auth` and `cua wif-token`: sign-in through `cua-auth` (browser
//! authorization code + PKCE with a loopback redirect, device code with
//! `--remote` or when the identity provider does not accept the loopback
//! redirect yet), the shared credential store, the account identity, and
//! GitHub Actions workload identity tokens.
//!
//! Account credentials resolve in this order: `FLEETS_TOKEN`, then
//! `CUA_CLIENT_ID` + `CUA_CLIENT_SECRET`, then the session stored by `cua
//! auth login` (its access token, refreshed when needed).

use crate::util::{self, http, line};
use cua_sdk::CuaError;
use std::{io::Write, time::Duration};

pub use cua_auth::{Store, jwt_claims};

/// GitHub OIDC audience Fleets trusts.
pub const GITHUB_WIF_AUDIENCE: &str = "fleets";

/// Maps a cua-auth error.
pub fn auth_err(e: cua_auth::Error) -> CuaError {
    use cua_auth::Error as E;
    let m = e.to_string();
    match e {
        E::NotLoggedIn | E::Unauthenticated(_) => CuaError::Unauthenticated(m),
        E::Http(_) => CuaError::Http(m),
        E::Timeout(_) => CuaError::Timeout(m),
        E::Unsupported(_) => CuaError::Unsupported(m),
        E::InvalidArgument(_) => CuaError::InvalidArgument(m),
        E::Store(_) => CuaError::Internal(m),
    }
}

/// The shared session (`cua-auth`), after migrating any legacy session.
pub fn session() -> cua_auth::Session {
    cua_auth::Session::from_env()
}

/// A valid access token from the stored session (refreshed and persisted
/// when it expires within 60 s), or `None` when not logged in.
pub async fn session_token(_store: &Store) -> Result<Option<String>, CuaError> {
    match session().access_token(false).await {
        Ok(t) => Ok(Some(t)),
        Err(cua_auth::Error::NotLoggedIn) => Ok(None),
        Err(e) => Err(auth_err(e)),
    }
}

// ---------------------------------------------------------------- account

/// Where Fleet credentials come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetAuth {
    /// `FLEETS_TOKEN`.
    WorkloadToken,
    /// `CUA_CLIENT_ID` + `CUA_CLIENT_SECRET`.
    ClientCredentials(String),
    /// The `cua auth login` session.
    Session,
}

impl FleetAuth {
    fn describe(&self) -> String {
        match self {
            FleetAuth::WorkloadToken => "FLEETS_TOKEN".into(),
            FleetAuth::ClientCredentials(id) => format!("client credentials ({id})"),
            FleetAuth::Session => "cua auth login session".into(),
        }
    }
}

/// Whether account credentials are available without reading the OS
/// credential vault: `FLEETS_TOKEN` or client credentials in the
/// environment, or a session the marker (or the file store) says exists.
pub fn fleet_maybe_configured() -> bool {
    cua_auth::account::AccountApi::from_env().is_some() || cua_auth::may_have_session()
}

/// The account API from the environment, falling back to the stored
/// session. `None` when no credentials exist at all.
pub fn account() -> Option<(cua_auth::account::AccountApi, FleetAuth)> {
    let get = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    if let Some(a) = cua_auth::account::AccountApi::from_env() {
        let auth = match get("FLEETS_TOKEN") {
            Some(_) => FleetAuth::WorkloadToken,
            None => FleetAuth::ClientCredentials(get("CUA_CLIENT_ID").unwrap_or_default()),
        };
        return Some((a, auth));
    }
    cua_auth::account::AccountApi::from_stored_session().map(|a| (a, FleetAuth::Session))
}

/// What every Cua Cloud (Fleet) command says now that it has closed.
pub fn cloud_closed() -> CuaError {
    CuaError::Fleet(cua_sandbox_core::CLOUD_CLOSED.into())
}

// --------------------------------------------------------------- commands

/// How `cua auth login` signs in.
#[derive(Clone, Debug, Default)]
pub struct LoginOptions {
    /// Do not open a browser (print the URL only).
    pub no_browser: bool,
    /// Device code flow (for a machine without a local browser).
    pub remote: bool,
    /// Force a flow: `pkce` / `browser` or `device` (`CUA_AUTH_FLOW`).
    pub flow: Option<String>,
}

/// `cua auth login`: signs in, stores the session and prints the identity.
pub async fn login(opts: &LoginOptions, out: &mut dyn Write) -> Result<i32, CuaError> {
    let flow = match opts.flow.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(f) => cua_auth::Flow::parse(f).map_err(auth_err)?,
        None if opts.remote => cua_auth::Flow::Device,
        None => cua_auth::Flow::Auto,
    };
    let s = session();
    // Fail before the user approves anything when the session could not be
    // saved afterwards (the macOS login keychain from an SSH session).
    s.store().check_writable().map_err(auth_err)?;
    let pending = s.begin_login(flow).await.map_err(auth_err)?;
    if let Some(note) = &pending.note {
        line(out, format!("{}.", note.trim_end_matches('.')));
    }
    match pending.method {
        cua_auth::Method::Browser => {
            line(out, "Opening your browser to sign in to Cua:");
            line(out, format!("  {}", pending.url));
            if !opts.no_browser && util::open_browser(&pending.url) {
                line(out, "Waiting for you to finish in the browser...");
            } else {
                line(
                    out,
                    "Open the URL above in a browser on this machine to continue.",
                );
            }
        }
        cua_auth::Method::Device => {
            line(
                out,
                format!("Open this URL in any browser: {}", pending.url),
            );
            if let Some(code) = &pending.user_code {
                line(out, format!("Enter this code if prompted: {code}"));
            }
            if !opts.no_browser && !opts.remote && util::open_browser(&pending.url) {
                line(out, "Opened the verification URL in your browser.");
            }
        }
    }
    let _ = out.flush();
    let creds = pending.complete().await.map_err(auth_err)?;
    let id = s.install(creds).await.map_err(auth_err)?;
    // Activation funnel: a sign-in from the CLI (browser or device code)
    // counts like one in the Spaces app's first run.
    cua_telemetry::global().capture_step("signed_in", cua_telemetry::Outcome::Ok);
    let who = match (&id.username, &id.email) {
        (Some(u), Some(e)) if u != e => format!(" as {u} ({e})"),
        (Some(u), _) | (None, Some(u)) => format!(" as {u}"),
        _ => String::new(),
    };
    line(out, format!("Logged in to run.cua.ai{who}."));
    Ok(0)
}

/// `cua auth logout`.
pub async fn logout(out: &mut dyn Write) -> Result<i32, CuaError> {
    let (existed, revoked) = session().logout().await.map_err(auth_err)?;
    if !existed {
        line(out, "Not logged in.");
        return Ok(0);
    }
    if !revoked {
        line(
            out,
            "Remote token revocation was unavailable; removing local credentials anyway.",
        );
    }
    line(out, "Logged out.");
    Ok(0)
}

/// `cua auth status`: local session state, without network access.
pub async fn status(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let s = session();
    let store = s.store().clone();
    let c = store.load().map_err(auth_err)?;
    if json {
        util::json_line(
            out,
            &serde_json::json!({
                "logged_in": c.is_some(),
                "expires_at": c.as_ref().map(|c| c.expires_at.clone()),
                "identity": c.as_ref().map(|c| c.identity()),
                "store": store.describe(),
            }),
        );
        return Ok(if c.is_some() { 0 } else { 1 });
    }
    let Some(c) = c else {
        line(out, "Not logged in. Run 'cua auth login'.");
        return Ok(1);
    };
    if c.expires().map_err(auth_err)? <= chrono::Utc::now() {
        line(
            out,
            "Logged in; access token expired and will refresh on the next run.cua.ai request.",
        );
    } else {
        line(
            out,
            format!(
                "Logged in to run.cua.ai. Access token expires {}.",
                c.expires_at
            ),
        );
    }
    Ok(0)
}

/// `cua auth whoami`: the active account identity (its token is fetched,
/// or refreshed, from the identity provider).
pub async fn whoami(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let Some((account, auth)) = account() else {
        if json {
            util::json_line(out, &serde_json::json!({"authenticated": false}));
        } else {
            line(
                out,
                "Not authenticated. Run 'cua auth login', or set CUA_CLIENT_ID and CUA_CLIENT_SECRET, or FLEETS_TOKEN.",
            );
        }
        return Ok(1);
    };
    let base = account.base_url().to_string();
    let token = account.access_token(false).await.map_err(auth_err)?;
    let claims = jwt_claims(&token).unwrap_or_default();
    let pick = |k: &str| claims[k].as_str().map(str::to_string);
    let user = pick("preferred_username").or_else(|| pick("email"));
    let expires = claims["exp"]
        .as_i64()
        .and_then(|e| chrono::DateTime::from_timestamp(e, 0))
        .map(|d| d.to_rfc3339());
    if json {
        util::json_line(
            out,
            &serde_json::json!({
                "authenticated": true,
                "fleet": base,
                "source": auth.describe(),
                "user": user,
                "subject": pick("sub"),
                "client": pick("azp"),
                "issuer": pick("iss"),
                "expires_at": expires,
            }),
        );
    } else {
        line(out, format!("Account:    {base}"));
        line(out, format!("Credential: {}", auth.describe()));
        if let Some(u) = &user {
            line(out, format!("User:       {u}"));
        }
        if let Some(c) = pick("azp") {
            line(out, format!("Client:     {c}"));
        }
        if let Some(e) = &expires {
            line(out, format!("Expires:    {e}"));
        }
    }
    Ok(0)
}

// -------------------------------------------------------------------- WIF

fn with_audience(request_url: &str, audience: &str) -> Result<String, CuaError> {
    let mut u = url::Url::parse(request_url)
        .map_err(|e| CuaError::InvalidArgument(format!("ACTIONS_ID_TOKEN_REQUEST_URL: {e}")))?;
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| k != "audience")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    u.query_pairs_mut()
        .clear()
        .extend_pairs(kept)
        .append_pair("audience", audience);
    Ok(u.to_string())
}

/// Requests a GitHub Actions OIDC token for Fleets.
pub async fn github_wif_token(audience: &str) -> Result<String, CuaError> {
    let url = std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").unwrap_or_default();
    let token = std::env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").unwrap_or_default();
    let hint = "run in GitHub Actions with permissions: id-token: write";
    if url.is_empty() {
        return Err(CuaError::ProviderNotConfigured(format!(
            "ACTIONS_ID_TOKEN_REQUEST_URL is missing; {hint}"
        )));
    }
    if token.is_empty() {
        return Err(CuaError::ProviderNotConfigured(format!(
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN is missing; {hint}"
        )));
    }
    let r = http()
        .get(with_audience(&url, audience)?)
        .header("accept", "application/json")
        .header("authorization", format!("bearer {token}"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| CuaError::Http("GitHub OIDC request failed".into()))?;
    let status = r.status().as_u16();
    if status != 200 {
        return Err(CuaError::Http(format!(
            "GitHub OIDC request failed with HTTP {status}"
        )));
    }
    let v: serde_json::Value = r
        .json()
        .await
        .map_err(|_| CuaError::Http("GitHub OIDC endpoint returned invalid JSON".into()))?;
    v["value"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CuaError::Http("GitHub OIDC response did not contain a token".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audience_replaces_existing_query_value() {
        let u = with_audience("https://gh.test/token?api-version=2&audience=x", "fleets").unwrap();
        assert!(u.contains("api-version=2"), "{u}");
        assert!(u.ends_with("audience=fleets"), "{u}");
        assert_eq!(u.matches("audience=").count(), 1);
    }

    #[test]
    fn jwt_claims_decode_unverified_payload() {
        use base64::Engine;
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"u1"}"#);
        assert_eq!(jwt_claims(&format!("h.{p}.s")).unwrap()["sub"], "u1");
        assert!(jwt_claims("not-a-jwt").is_none());
    }
}
