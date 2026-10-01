//! `cua auth` and `cua wif-token`: sign-in through `cua-auth` (browser
//! authorization code + PKCE with a loopback redirect, device code with
//! `--remote` or when the identity provider does not accept the loopback
//! redirect yet), the shared credential store, Fleet identity and user API
//! keys, and GitHub Actions workload identity tokens.
//!
//! Fleet credentials resolve in this order (same as cua-sandbox):
//! `FLEETS_TOKEN`, then `CUA_CLIENT_ID` + `CUA_CLIENT_SECRET`, then the
//! session stored by `cua auth login` (its access token, refreshed when
//! needed).

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

// ------------------------------------------------------------------ Fleet

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

/// Whether Fleet credentials are available without reading the OS
/// credential vault: `FLEETS_TOKEN` or client credentials in the
/// environment, or a session the marker (or the file store) says exists.
/// Implicit Fleet calls (the default `cua sb ls`) check this first.
pub fn fleet_maybe_configured() -> bool {
    let cfg = cua_fleet::FleetConfig::from_env();
    cfg.fleet_token.is_some()
        || (cfg.client_id.is_some() && cfg.client_secret.is_some())
        || cua_auth::may_have_session()
}

/// The Fleet configuration from the environment, falling back to the
/// stored session. `None` when no credentials exist at all.
pub async fn fleet_config() -> Result<Option<(cua_fleet::FleetConfig, FleetAuth)>, CuaError> {
    let mut cfg = cua_fleet::FleetConfig::from_env();
    if cfg.fleet_token.is_some() {
        return Ok(Some((cfg, FleetAuth::WorkloadToken)));
    }
    if let (Some(id), Some(_)) = (&cfg.client_id, &cfg.client_secret) {
        let id = id.clone();
        return Ok(Some((cfg, FleetAuth::ClientCredentials(id))));
    }
    match session_token(&Store::from_env()).await? {
        Some(t) => {
            cfg.fleet_token = Some(t);
            Ok(Some((cfg, FleetAuth::Session)))
        }
        None => Ok(None),
    }
}

/// A connected Fleet client, or `ProviderNotConfigured`.
pub async fn fleet_client() -> Result<(cua_fleet::FleetClient, FleetAuth), CuaError> {
    let Some((cfg, auth)) = fleet_config().await? else {
        return Err(CuaError::ProviderNotConfigured(
            cua_fleet::MISSING_CREDENTIALS.into(),
        ));
    };
    let c = cua_fleet::FleetClient::connect(cfg).map_err(fleet_err)?;
    Ok((c, auth))
}

/// Maps a Fleet error.
pub fn fleet_err(e: impl Into<cua_fleet::Error>) -> CuaError {
    match e.into() {
        cua_fleet::Error::MissingCredentials => {
            CuaError::ProviderNotConfigured(cua_fleet::MISSING_CREDENTIALS.into())
        }
        cua_fleet::Error::InvalidArgument(m) => CuaError::InvalidArgument(m),
        e @ cua_fleet::Error::AdmissionDenied { .. } => {
            CuaError::FleetAdmissionDenied(e.to_string())
        }
        e @ cua_fleet::Error::CreditExhausted { .. } => {
            CuaError::CloudCreditExhausted(e.to_string())
        }
        cua_fleet::Error::Sdk(e) => {
            use cua_fleet::SdkError as S;
            let m = e.to_string();
            match e {
                S::Status { status: 401, .. } | S::Token { .. } => CuaError::Unauthenticated(m),
                S::Status { status: 403, .. } | S::PoolAccessDenied { .. } => {
                    CuaError::PermissionDenied(m)
                }
                S::Status { status: 404, .. } => CuaError::NotFound(m),
                S::Transport { .. } => CuaError::Transport(m),
                S::Configuration { .. } | S::InvalidResourceName { .. } => {
                    CuaError::InvalidArgument(m)
                }
                _ => CuaError::Fleet(m),
            }
        }
        other => CuaError::Fleet(other.to_string()),
    }
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

/// `cua auth whoami`: the active Fleet identity, verified with a read-only
/// namespace listing.
pub async fn whoami(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let Some((cfg, auth)) = fleet_config().await? else {
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
    let base = cfg.base_url.clone();
    let client = cua_fleet::FleetClient::connect(cfg).map_err(fleet_err)?;
    let token = client.access_token(false).await.map_err(fleet_err)?;
    let claims = jwt_claims(&token).unwrap_or_default();
    let namespaces = client
        .sdk()
        .list_namespaces()
        .await
        .map_err(|e| fleet_err(cua_fleet::Error::Sdk(e)))?;
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
                "namespaces": namespaces.len(),
            }),
        );
    } else {
        line(out, format!("Fleet:      {base}"));
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
        line(out, format!("Namespaces: {}", namespaces.len()));
    }
    Ok(0)
}

/// `cua auth keys ls`.
pub async fn keys_list(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (c, _) = fleet_client().await?;
    let keys = c
        .sdk()
        .list_user_api_keys()
        .await
        .map_err(|e| fleet_err(cua_fleet::Error::Sdk(e)))?;
    if json {
        util::json_line(out, &serde_json::to_value(&keys)?);
    } else if keys.is_empty() {
        line(out, "No API keys.");
    } else {
        let rows: Vec<Vec<String>> = keys
            .iter()
            .map(|k| {
                vec![
                    k.id.clone(),
                    k.name.clone(),
                    k.client_id.clone(),
                    k.scope.join(" "),
                ]
            })
            .collect();
        util::table(out, &["ID", "NAME", "CLIENT ID", "SCOPE"], &rows);
    }
    Ok(0)
}

/// `cua auth keys create`: prints the client id and secret once.
pub async fn keys_create(
    name: String,
    scope: Vec<String>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (c, _) = fleet_client().await?;
    let k = c
        .sdk()
        .create_user_api_key(cua_fleet::sdk::CreateUserApiKeyRequest { name, scope })
        .await
        .map_err(|e| fleet_err(cua_fleet::Error::Sdk(e)))?;
    if json {
        util::json_line(out, &serde_json::to_value(&k)?);
    } else {
        line(out, format!("Created API key {}.", k.name));
        line(out, format!("CUA_CLIENT_ID={}", k.client_id));
        line(out, format!("CUA_CLIENT_SECRET={}", k.client_secret));
        line(out, format!("CUA_TOKEN_URL={}", k.token_url));
        line(out, "The secret is shown only once.");
    }
    Ok(0)
}

/// `cua auth keys rm`.
pub async fn keys_delete(id: String, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (c, _) = fleet_client().await?;
    c.sdk()
        .delete_user_api_key(id.clone())
        .await
        .map_err(|e| fleet_err(cua_fleet::Error::Sdk(e)))?;
    line(out, format!("Deleted API key {id}."));
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
