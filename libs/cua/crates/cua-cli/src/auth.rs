//! `cua auth`: sign-in through `cua-auth` (browser authorization code +
//! PKCE with a loopback redirect, device code with `--remote` or when the
//! identity provider does not accept the loopback redirect yet), who is
//! signed in (`whoami`) and the shared credential store.

use crate::util::{self, line};
use cua_host::spaces_api::SpacesApiClient;
use cua_sdk::CuaError;
use std::io::Write;

pub use cua_auth::{Store, jwt_claims};

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

// --------------------------------------------------------------- commands

/// How `cua auth login` signs in.
#[derive(Clone, Debug, Default)]
pub struct LoginOptions {
    /// Do not open a browser (print the URL only).
    pub no_browser: bool,
    /// Device code flow (for a computer without a local browser).
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
    let mut pending = s.begin_login(flow).await.map_err(auth_err)?;
    pending.return_to = Some(cua_auth::ReturnTo::Terminal);
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
                    "Open the URL above in a browser on this computer to continue.",
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
    link_usage(&s).await;
    let who = match (&id.username, &id.email) {
        (Some(u), Some(e)) if u != e => format!(" as {u} ({e})"),
        (Some(u), _) | (None, Some(u)) => format!(" as {u}"),
        _ => String::new(),
    };
    line(out, format!("Logged in to Cua{who}."));
    Ok(0)
}

/// Links this install's usage data to the signed-in account when the
/// Spaces API offers an account id (CUA-1881), and prints the one-time
/// notice when this machine saw only the older "anonymous" notice. Best
/// effort: a sign-in never fails or waits long on it.
async fn link_usage(s: &cua_auth::Session) {
    let t = cua_telemetry::global();
    if !t.is_enabled() {
        return;
    }
    let Ok(Ok(token)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), s.access_token(false)).await
    else {
        return;
    };
    let Ok(api) = SpacesApiClient::current() else {
        return;
    };
    if let Ok(Ok(me)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), api.me(&token)).await
    {
        offer_account(&me);
    }
}

/// Hands the account id `GET /v1/me` answered with (or its absence) to
/// telemetry, then shows the linking notice if it is due.
fn offer_account(me: &cua_host::spaces_api::ApiMe) {
    let t = cua_telemetry::global();
    t.offer_account(me.analytics_id.as_deref());
    t.show_notice_if_needed();
}

/// `cua auth logout`.
pub async fn logout(out: &mut dyn Write) -> Result<i32, CuaError> {
    let (existed, revoked) = session().logout().await.map_err(auth_err)?;
    // Signed out: usage data goes back to a new anonymous install id.
    let _ = cua_telemetry::global().forget_account();
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
    let endpoints = cua_home::env::Endpoints::current();
    if json {
        util::json_line(
            out,
            &serde_json::json!({
                "env": endpoints.env.name(),
                "issuer": s.oidc().issuer,
                "relay": endpoints.relay_url,
                "logged_in": c.is_some(),
                "expires_at": c.as_ref().map(|c| c.expires_at.clone()),
                "identity": c.as_ref().map(|c| c.identity()),
                "store": store.describe(),
            }),
        );
        return Ok(if c.is_some() { 0 } else { 1 });
    }
    if !endpoints.env.is_prod() {
        line(
            out,
            format!(
                "Environment: {} (issuer {}, relay {}, home {}).",
                endpoints.env.name(),
                s.oidc().issuer,
                endpoints.relay_url,
                util::cua_home().display()
            ),
        );
    }
    let Some(c) = c else {
        line(out, "Not logged in. Run 'cua auth login'.");
        return Ok(1);
    };
    if c.expires().map_err(auth_err)? <= chrono::Utc::now() {
        line(
            out,
            "Logged in; access token expired and will refresh on the next request.",
        );
    } else {
        line(
            out,
            format!("Logged in to Cua. Access token expires {}.", c.expires_at),
        );
    }
    Ok(0)
}

/// `cua auth keychain [--prompt | --forget]`: one JSON line, the
/// [`cua_auth::KeychainCheck`] of the store (`--forget`: after removing the
/// stored session, which reads nothing). Always exit 0 when it ran.
pub fn keychain(prompt: bool, forget: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let store = Store::from_env();
    let check = if forget {
        store.forget_keychain().map_err(auth_err)?
    } else {
        store.check_keychain(prompt)
    };
    line(out, serde_json::to_string(&check).unwrap_or_default());
    Ok(0)
}

/// `cua auth whoami`: who the signed-in Cua session is. The identity comes
/// from the access token's claims; the Spaces API adds the plan and the
/// number of teams, best effort.
pub async fn whoami(json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let Some(token) = session_token(&Store::from_env()).await? else {
        if json {
            util::json_line(out, &serde_json::json!({"authenticated": false}));
        } else {
            line(out, "Not logged in. Run 'cua auth login'.");
        }
        return Ok(1);
    };
    let api = SpacesApiClient::current().map_err(crate::host::host_err)?;
    whoami_with(&api, &token, json, out).await
}

/// [`whoami`] for the access token `token`, asking `api` for the account.
pub async fn whoami_with(
    api: &SpacesApiClient,
    token: &str,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    use cua_host::Error as E;
    let claims = jwt_claims(token).unwrap_or_default();
    let pick = |k: &str| {
        claims[k]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    // A refused token means the session is no longer valid; any other
    // failure (offline, a server error, no Teams routes yet) only costs the
    // plan and team count.
    let me = match api.me(token).await {
        Ok(me) => {
            offer_account(&me);
            Some(me)
        }
        Err(e @ (E::Unauthenticated(_) | E::PermissionDenied(_))) => {
            return Err(CuaError::Unauthenticated(format!(
                "your Cua sign-in is no longer valid ({e}); run 'cua auth login' again"
            )));
        }
        Err(_) => None,
    };
    let user = pick("preferred_username").or_else(|| pick("email"));
    let email = pick("email").or_else(|| {
        me.as_ref()
            .map(|m| m.email.clone())
            .filter(|e| !e.is_empty())
    });
    let expires = claims["exp"]
        .as_i64()
        .and_then(|e| chrono::DateTime::from_timestamp(e, 0))
        .map(|d| d.to_rfc3339());
    let plan = me.as_ref().map(|m| match m.entitlements.plan.as_str() {
        "" => "free".to_string(),
        p => p.to_string(),
    });
    let teams = me.as_ref().map(|m| m.orgs.len());
    if json {
        util::json_line(
            out,
            &serde_json::json!({
                "authenticated": true,
                "user": user,
                "email": email,
                "subject": pick("sub"),
                "client": pick("azp"),
                "issuer": pick("iss"),
                "expires_at": expires,
                "plan": plan,
                "teams": teams,
            }),
        );
        return Ok(0);
    }
    let who = match (&user, &email) {
        (Some(u), Some(e)) if u != e => format!(" as {u} ({e})"),
        (Some(u), _) | (None, Some(u)) => format!(" as {u}"),
        _ => String::new(),
    };
    line(out, format!("Logged in to Cua{who}."));
    match (&plan, teams) {
        (Some(plan), Some(teams)) => {
            line(out, format!("Plan:    {plan}"));
            line(out, format!("Teams:   {teams}"));
        }
        _ => line(
            out,
            "Plan and teams are unavailable: the Cua API could not be reached.",
        ),
    }
    if let Some(c) = pick("azp") {
        line(out, format!("Client:  {c}"));
    }
    if let Some(e) = &expires {
        line(out, format!("Expires: {e}"));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use cua_host::testing::FakeSpacesApi;

    fn jwt(claims: serde_json::Value) -> String {
        let e = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        format!(
            "{}.{}.sig",
            e(br#"{"alg":"none"}"#),
            e(claims.to_string().as_bytes())
        )
    }

    async fn run(
        api: &SpacesApiClient,
        token: &str,
        json: bool,
    ) -> (Result<i32, CuaError>, String) {
        let mut out = Vec::new();
        let r = whoami_with(api, token, json, &mut out).await;
        (r, String::from_utf8(out).unwrap())
    }

    #[test]
    fn jwt_claims_decode_unverified_payload() {
        assert_eq!(
            jwt_claims(&jwt(serde_json::json!({"sub": "u1"}))).unwrap()["sub"],
            "u1"
        );
        assert!(jwt_claims("not-a-jwt").is_none());
    }

    #[tokio::test]
    async fn whoami_reads_the_token_and_the_cua_api() {
        let fake = FakeSpacesApi::start().await;
        let token = jwt(serde_json::json!({
            "preferred_username": "ada", "email": "ada@aigis.test", "sub": "u-ada",
            "azp": "cua-cli", "iss": "https://auth.cua.test/realms/cyclops-cs", "exp": 1_900_000_000,
        }));
        fake.add_account(&token, "u-ada", "ada@aigis.test");
        fake.add_org("aigis", "AIGIS", "u-ada", &["team_machines"]);
        let api = SpacesApiClient::new(&fake.url).unwrap();

        let (r, o) = run(&api, &token, false).await;
        assert_eq!(r.unwrap(), 0);
        assert!(
            o.starts_with("Logged in to Cua as ada (ada@aigis.test).\n"),
            "{o}"
        );
        assert!(
            o.contains("Plan:    free\n") && o.contains("Teams:   1\n"),
            "{o}"
        );
        assert!(
            o.contains("Client:  cua-cli\n") && o.contains("Expires: 2030-"),
            "{o}"
        );
        for gone in ["Fleet", "Namespaces", "Credential", "run.cua.ai"] {
            assert!(!o.contains(gone), "{gone}: {o}");
        }

        let (r, o) = run(&api, &token, true).await;
        assert_eq!(r.unwrap(), 0);
        let v: serde_json::Value = serde_json::from_str(o.trim()).unwrap();
        assert_eq!(
            (
                v["authenticated"].clone(),
                v["user"].clone(),
                v["email"].clone()
            ),
            (true.into(), "ada".into(), "ada@aigis.test".into())
        );
        assert_eq!(
            (v["subject"].as_str(), v["client"].as_str()),
            (Some("u-ada"), Some("cua-cli"))
        );
        assert_eq!(v["issuer"], "https://auth.cua.test/realms/cyclops-cs");
        assert_eq!(
            (v["plan"].as_str(), v["teams"].as_u64()),
            (Some("free"), Some(1))
        );
        assert!(
            v["expires_at"].as_str().unwrap().starts_with("2030-"),
            "{v}"
        );
        for gone in ["fleet", "source", "namespaces"] {
            assert!(v.get(gone).is_none(), "{gone}: {v}");
        }
    }

    #[tokio::test]
    async fn whoami_without_the_api_still_names_the_user() {
        // Nothing listens on port 9: the API cannot be reached.
        let api = SpacesApiClient::new("http://127.0.0.1:9").unwrap();
        let token = jwt(serde_json::json!({"preferred_username": "ada", "sub": "u-ada"}));
        let (r, o) = run(&api, &token, false).await;
        assert_eq!(r.unwrap(), 0);
        assert!(o.starts_with("Logged in to Cua as ada.\n"), "{o}");
        assert!(o.contains("the Cua API could not be reached"), "{o}");
        let (r, o) = run(&api, &token, true).await;
        assert_eq!(r.unwrap(), 0);
        let v: serde_json::Value = serde_json::from_str(o.trim()).unwrap();
        assert_eq!(v["user"], "ada");
        assert!(v["plan"].is_null() && v["teams"].is_null(), "{v}");
    }

    #[tokio::test]
    async fn whoami_fails_when_the_api_refuses_the_session() {
        let fake = FakeSpacesApi::start().await;
        let api = SpacesApiClient::new(&fake.url).unwrap();
        // The fake API knows no account for this token: it answers 401.
        let token = jwt(serde_json::json!({"preferred_username": "ada"}));
        let (r, o) = run(&api, &token, false).await;
        assert!(matches!(r, Err(CuaError::Unauthenticated(_))), "{r:?}");
        assert!(o.is_empty(), "nothing is printed: {o}");
    }
}
