//! `Auth` (`Cua.auth()`): sign in to Cua and read the shared session
//! (cua-auth). Browser sign-in with PKCE and a loopback redirect, falling
//! back to a device code; one credential store for the CLI, the SDK, the
//! daemon and the apps (OS vault on macOS and Windows, a 0600 file on
//! Linux).

use super::{Cua, run};
use crate::{CuaError, Result};
use std::sync::Arc;

/// Whether a `cua auth login` session may be stored, decided without
/// reading the OS credential vault (the non-secret session marker, or the
/// file store's file). Implicit callers (the default sandbox listing) check
/// this before reading the session; an explicit read of a session stored
/// before markers existed writes the marker.
#[uniffi::export]
pub fn may_have_fleet_session() -> bool {
    cua_auth::may_have_session()
}

impl From<cua_auth::Error> for CuaError {
    fn from(e: cua_auth::Error) -> Self {
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
}

/// Who is signed in (from token claims; display only).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct AuthIdentity {
    /// `preferred_username`.
    pub username: Option<String>,
    /// `email`.
    pub email: Option<String>,
    /// `name`.
    pub name: Option<String>,
    /// `sub`.
    pub subject: Option<String>,
    /// One display string (email, else username, else subject).
    pub display: Option<String>,
}

impl From<cua_auth::Identity> for AuthIdentity {
    fn from(i: cua_auth::Identity) -> Self {
        AuthIdentity {
            display: i.display(),
            username: i.username,
            email: i.email,
            name: i.name,
            subject: i.subject,
        }
    }
}

/// The stored session, read without network access.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AuthStatus {
    /// A session is stored.
    pub logged_in: bool,
    /// Who.
    pub identity: Option<AuthIdentity>,
    /// RFC 3339 expiry of the current access token (it refreshes itself).
    pub expires_at: Option<String>,
    /// Where credentials live.
    pub store: String,
}

/// How a sign-in proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LoginMethod {
    /// Open `url` in a browser on this machine; it redirects back here.
    Browser,
    /// Open `url` on any device and enter `user_code`.
    Device,
}

/// A started sign-in. Show `url` (and `user_code`), then await `wait()`.
#[derive(uniffi::Object)]
pub struct LoginAttempt {
    method: LoginMethod,
    url: String,
    user_code: Option<String>,
    note: Option<String>,
    session: Arc<cua_auth::Session>,
    pending: tokio::sync::Mutex<Option<cua_auth::PendingLogin>>,
}

#[uniffi::export]
impl LoginAttempt {
    /// Browser or device.
    pub fn method(&self) -> LoginMethod {
        self.method
    }

    /// The URL to open.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Device flow: the code to enter.
    pub fn user_code(&self) -> Option<String> {
        self.user_code.clone()
    }

    /// Why a device code is used although browser sign-in was asked for.
    pub fn note(&self) -> Option<String> {
        self.note.clone()
    }

    /// Waits for the user, stores the session and returns who signed in.
    /// Only the first call waits; later calls fail.
    ///
    /// A device that already enrolled (it has a device key) then
    /// re-registers with the relay (`CUA_RELAY_URL`, else relay.cua.ai), so
    /// the fresh sign-in enrolls or re-verifies it without an approval.
    /// That step is best effort and never fails the sign-in.
    pub async fn wait(self: Arc<Self>) -> Result<AuthIdentity> {
        run(async move {
            let pending = self
                .pending
                .lock()
                .await
                .take()
                .ok_or_else(|| CuaError::Closed("this sign-in already finished".into()))?;
            let creds = pending.complete().await?;
            let who = self.session.install(creds).await?;
            enroll_after_sign_in(&self.session).await;
            Ok(who.into())
        })
        .await
    }
}

/// Re-registers this device after an interactive sign-in (see
/// [`LoginAttempt::wait`]).
#[cfg(feature = "host")]
async fn enroll_after_sign_in(session: &Arc<cua_auth::Session>) {
    let Ok(auth) = cua_host::DeviceAuth::new(
        &cua_host::relay_url_from_env(),
        Arc::new(super::host::SessionTokens(session.clone())),
        Arc::new(session.store().clone()),
        cua_host::device_name(),
    ) else {
        return;
    };
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        auth.enroll_after_sign_in(),
    )
    .await;
    record_sign_in_enrollment(&r);
}

/// `cua_device_enroll` for a sign-in that enrolled this device: `rekey`
/// when it replaced this machine's other key, `sign_in` otherwise. A
/// device that never enrolled (no key) or still waits for an approval
/// records nothing.
#[cfg(feature = "host")]
pub(crate) fn record_sign_in_enrollment<E>(
    r: &std::result::Result<
        std::result::Result<Option<cua_host::relay::Enrollment>, cua_host::Error>,
        E,
    >,
) {
    use cua_telemetry::Outcome;
    let (method, outcome) = match r {
        Ok(Ok(Some(e))) if e.device.state == cua_host::relay::DeviceState::Enrolled => (
            if e.superseded.is_empty() {
                "sign_in"
            } else {
                "rekey"
            },
            Outcome::Ok,
        ),
        Ok(Ok(_)) => return,
        _ => ("sign_in", Outcome::Error),
    };
    if let Some(e) = cua_telemetry::events::device_enroll(method, outcome) {
        cua_telemetry::capture(e);
    }
}

/// Without relay support there is no device to re-register.
#[cfg(not(feature = "host"))]
async fn enroll_after_sign_in(_session: &Arc<cua_auth::Session>) {}

/// Sign-in and the shared session.
#[derive(uniffi::Object)]
pub struct Auth {
    session: Arc<cua_auth::Session>,
}

impl Auth {
    /// On an explicit session (Rust hosts and tests: a file store in a
    /// temporary directory and a fake issuer).
    pub fn with_session(session: cua_auth::Session) -> Arc<Self> {
        Arc::new(Auth {
            session: Arc::new(session),
        })
    }

    /// The underlying session (Rust hosts: a Fleet token provider).
    pub fn session(&self) -> Arc<cua_auth::Session> {
        self.session.clone()
    }
}

#[uniffi::export]
impl Cua {
    /// Sign-in and the shared session (`CUA_OIDC_ISSUER`,
    /// `CUA_CREDENTIAL_STORE`, `CUA_HOME` apply).
    pub fn auth(&self) -> Arc<Auth> {
        Auth::with_session(cua_auth::Session::from_env())
    }
}

#[uniffi::export]
impl Auth {
    /// The stored session (no network).
    pub fn status(&self) -> Result<AuthStatus> {
        let c = self.session.credentials()?;
        Ok(AuthStatus {
            logged_in: c.is_some(),
            identity: c.as_ref().map(|c| c.identity().into()),
            expires_at: c.map(|c| c.expires_at),
            store: self.session.store().describe(),
        })
    }

    /// Starts a sign-in. `flow`: `auto` (default: browser, else device
    /// code), `browser` or `device`.
    pub async fn begin_login(&self, flow: Option<String>) -> Result<Arc<LoginAttempt>> {
        let session = self.session.clone();
        run(async move {
            let flow = cua_auth::Flow::parse(flow.as_deref().unwrap_or("auto"))?;
            let p = session.begin_login(flow).await?;
            Ok(Arc::new(LoginAttempt {
                method: match p.method {
                    cua_auth::Method::Browser => LoginMethod::Browser,
                    cua_auth::Method::Device => LoginMethod::Device,
                },
                url: p.url.clone(),
                user_code: p.user_code.clone(),
                note: p.note.clone(),
                session,
                pending: tokio::sync::Mutex::new(Some(p)),
            }))
        })
        .await
    }

    /// A valid access token (refreshed when it expires within a minute, or
    /// when `force`). `Unauthenticated` when not signed in.
    pub async fn access_token(&self, force: bool) -> Result<String> {
        let session = self.session.clone();
        run(async move { Ok(session.access_token(force).await?) }).await
    }

    /// Revokes (best effort) and removes the session. Returns whether one
    /// existed.
    pub async fn logout(&self) -> Result<bool> {
        let session = self.session.clone();
        run(async move { Ok(session.logout().await?.0) }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_and_errors_map_without_touching_the_keychain() {
        let d = tempfile::tempdir().unwrap();
        let store = cua_auth::Store::File(d.path().join("credentials.json"));
        // Nothing listens on port 9 on loopback.
        let a = Auth::with_session(cua_auth::Session::new(
            cua_auth::Oidc::new("http://127.0.0.1:9", "cua-cli"),
            store.clone(),
        ));
        let s = a.status().unwrap();
        assert!(!s.logged_in && s.identity.is_none());
        assert!(s.store.contains("credentials.json"));
        assert!(matches!(
            a.access_token(false).await.unwrap_err(),
            CuaError::Unauthenticated(_)
        ));
        assert!(matches!(
            a.begin_login(Some("bogus".into())).await.err().unwrap(),
            CuaError::InvalidArgument(_)
        ));
        assert!(matches!(
            a.begin_login(None).await.err().unwrap(),
            CuaError::Http(_)
        ));
        store
            .save(&cua_auth::Credentials::from_token_response(&serde_json::json!({"access_token": "a.eyJlbWFpbCI6ImFAYi5jIn0.s", "expires_in": 3600})).unwrap())
            .unwrap();
        let s = a.status().unwrap();
        assert!(s.logged_in);
        assert_eq!(s.identity.unwrap().display.as_deref(), Some("a@b.c"));
        assert_eq!(
            a.access_token(false).await.unwrap(),
            "a.eyJlbWFpbCI6ImFAYi5jIn0.s"
        );
        assert!(a.logout().await.unwrap());
        assert!(!a.status().unwrap().logged_in);
    }
}
