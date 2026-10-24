//! Which Cua deployment a client talks to: `CUA_ENV` (`prod`, the default,
//! or `staging`).
//!
//! The environment picks the default of every Cua endpoint a client uses,
//! and the cua home, so a staging session, host enrollment and Spaces
//! registry never mix with production ones:
//!
//! | Setting       | Override              | prod                                     | staging                                     |
//! |---------------|-----------------------|------------------------------------------|---------------------------------------------|
//! | OIDC issuer   | `CUA_OIDC_ISSUER`     | `https://auth.cua.ai/realms/cyclops-cs`  | `https://auth.staging.cua.ai/realms/cua-staging` |
//! | Relay         | `CUA_RELAY_URL`       | `https://relay.cua.ai`                   | `https://relay.staging.cua.ai`              |
//! | Spaces API    | `CUA_SPACES_API_URL`  | `https://api.cua.ai`                     | `https://api.staging.cua.ai`                |
//! | Account app   | `CUA_ACCOUNT_APP_URL` | `https://app.cua.ai`                     | `https://app.staging.cua.ai`                |
//! | Spaces site   | `CUA_SPACES_SITE_URL` | `https://cua.ai`                         | `https://spaces.staging.cua.ai`             |
//! | cua home      | `CUA_HOME`            | `~/.cua`                                 | `~/.cua/envs/staging`                       |
//!
//! A per-setting override always wins over `CUA_ENV`. With `CUA_ENV` unset,
//! the build's default applies: production, or staging for a beta channel
//! build (compiled with `CUA_DEFAULT_ENV=staging`).

use std::path::PathBuf;

/// The environment variable that selects the deployment.
pub const CUA_ENV_VAR: &str = "CUA_ENV";

/// A Cua deployment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CuaEnv {
    /// Production (the default).
    #[default]
    Prod,
    /// The staging deployment.
    Staging,
}

impl CuaEnv {
    /// Parses a `CUA_ENV` value: empty, `prod` or `production` is
    /// [`CuaEnv::Prod`], `staging` is [`CuaEnv::Staging`] (any case).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "prod" | "production" => Some(Self::Prod),
            "staging" => Some(Self::Staging),
            _ => None,
        }
    }

    /// `CUA_ENV`, or an error naming the accepted values.
    pub fn try_current() -> Result<Self, String> {
        Self::try_from_value(std::env::var(CUA_ENV_VAR).ok().as_deref())
    }

    /// The default when `CUA_ENV` is unset: the build's `CUA_DEFAULT_ENV`
    /// (beta channel builds are compiled with `staging`), else production.
    pub fn build_default() -> Self {
        // TODO(CUA-1776, PR 1b): the Local channel defaults to staging
        // (`crate::channel::CuaChannel::default_env`); wire it in here.
        option_env!("CUA_DEFAULT_ENV")
            .and_then(Self::parse)
            .unwrap_or_default()
    }

    /// [`CuaEnv::try_current`] for a given value (`None`: unset, the
    /// [`CuaEnv::build_default`]).
    pub fn try_from_value(value: Option<&str>) -> Result<Self, String> {
        match value {
            None => Ok(Self::build_default()),
            Some(v) => Self::parse(v).ok_or_else(|| {
                format!("{CUA_ENV_VAR}={v:?} is not a Cua environment (prod or staging)")
            }),
        }
    }

    /// `CUA_ENV`; an unknown value is production (the `cua` CLI refuses
    /// one at startup instead, see [`CuaEnv::try_current`]).
    pub fn current() -> Self {
        Self::try_current().unwrap_or_default()
    }

    /// `prod` or `staging`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Prod => "prod",
            Self::Staging => "staging",
        }
    }

    /// Whether this is production.
    pub fn is_prod(self) -> bool {
        self == Self::Prod
    }

    /// The default OIDC issuer.
    pub fn oidc_issuer(self) -> &'static str {
        match self {
            Self::Prod => "https://auth.cua.ai/realms/cyclops-cs",
            // Staging has its own Keycloak; the `cua-staging` realm on
            // auth.cua.ai is a different realm with other users and keys.
            Self::Staging => "https://auth.staging.cua.ai/realms/cua-staging",
        }
    }

    /// The default relay.
    pub fn relay_url(self) -> &'static str {
        match self {
            Self::Prod => "https://relay.cua.ai",
            Self::Staging => "https://relay.staging.cua.ai",
        }
    }

    /// The default Spaces control plane (entitlements) API.
    pub fn spaces_api_url(self) -> &'static str {
        match self {
            Self::Prod => "https://api.cua.ai",
            Self::Staging => "https://api.staging.cua.ai",
        }
    }

    /// The account app: billing, plans, team administration and invites
    /// (the apps open it in the browser).
    pub fn account_app_url(self) -> &'static str {
        match self {
            Self::Prod => "https://app.cua.ai",
            Self::Staging => "https://app.staging.cua.ai",
        }
    }

    /// The default Spaces site (pairing links are `<site>/add/<code>`).
    pub fn spaces_site_url(self) -> &'static str {
        match self {
            Self::Prod => "https://cua.ai",
            Self::Staging => "https://spaces.staging.cua.ai",
        }
    }

    /// The OS credential vault service the session, the device key and the
    /// other named secrets are stored under. The name is neutral on purpose:
    /// it used to be the retired Cloud API host (see
    /// [`CuaEnv::legacy_keyring_service`]).
    pub fn keyring_service(self) -> &'static str {
        match self {
            Self::Prod => "ai.cua.session",
            Self::Staging => "ai.cua.session.staging",
        }
    }

    /// The service older builds stored the session under (the retired Cloud
    /// API host). Builds that use [`CuaEnv::keyring_service`] move an item
    /// they find here once, then delete it. Remove this, and the move in
    /// `cua-auth`, two releases or 60 days after the first release that has
    /// the move, whichever is later (CUA-1877).
    pub fn legacy_keyring_service(self) -> &'static str {
        match self {
            Self::Prod => "run.cua.ai",
            Self::Staging => "staging.run.cua.ai",
        }
    }

    /// The cua home below the default `~/.cua` (`None`: `~/.cua` itself).
    pub fn home_subdir(self) -> Option<&'static str> {
        match self {
            Self::Prod => None,
            Self::Staging => Some("envs/staging"),
        }
    }
}

/// Every endpoint a client uses, resolved for one environment and its
/// per-setting overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// The deployment (`CUA_ENV`).
    pub env: CuaEnv,
    /// `CUA_OIDC_ISSUER`, else the environment's issuer.
    pub oidc_issuer: String,
    /// `CUA_RELAY_URL`, else the environment's relay.
    pub relay_url: String,
    /// `CUA_SPACES_API_URL`, else the environment's Spaces API.
    pub spaces_api_url: String,
    /// `CUA_SPACES_SITE_URL`, else the environment's Spaces site.
    pub spaces_site_url: String,
    /// `CUA_ACCOUNT_APP_URL`, else the environment's account app.
    pub account_app_url: String,
}

impl Endpoints {
    /// From the process environment (an unknown `CUA_ENV` is production).
    pub fn current() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// From `get` (a variable lookup; for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let env = CuaEnv::try_from_value(get(CUA_ENV_VAR).as_deref()).unwrap_or_default();
        let setting = |var: &str, default: &str| {
            get(var)
                .map(|v| v.trim().trim_end_matches('/').to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self {
            env,
            oidc_issuer: setting("CUA_OIDC_ISSUER", env.oidc_issuer()),
            relay_url: setting("CUA_RELAY_URL", env.relay_url()),
            spaces_api_url: setting("CUA_SPACES_API_URL", env.spaces_api_url()),
            spaces_site_url: setting("CUA_SPACES_SITE_URL", env.spaces_site_url()),
            account_app_url: setting("CUA_ACCOUNT_APP_URL", env.account_app_url()),
        }
    }
}

/// `CUA_OIDC_ISSUER`, else the environment's issuer.
pub fn oidc_issuer() -> String {
    Endpoints::current().oidc_issuer
}

/// `CUA_RELAY_URL`, else the environment's relay.
pub fn relay_url() -> String {
    Endpoints::current().relay_url
}

/// `CUA_SPACES_API_URL`, else the environment's Spaces API.
pub fn spaces_api_url() -> String {
    Endpoints::current().spaces_api_url
}

/// `CUA_SPACES_SITE_URL`, else the environment's Spaces site.
pub fn spaces_site_url() -> String {
    Endpoints::current().spaces_site_url
}

/// `CUA_ACCOUNT_APP_URL`, else the environment's account app
/// (`https://app.cua.ai`, `https://app.staging.cua.ai`).
pub fn account_app_url() -> String {
    Endpoints::current().account_app_url
}

/// The cua home a non-production environment uses when `CUA_HOME` is not
/// set: `None` for production or when `CUA_HOME` is set.
pub fn env_home() -> Option<PathBuf> {
    if std::env::var_os("CUA_HOME").is_some_and(|v| !v.is_empty()) {
        return None;
    }
    CuaEnv::current().home_subdir()?;
    Some(crate::cua_home())
}

/// Sets `CUA_HOME` to the environment's home when `CUA_ENV` selects a
/// non-production deployment and `CUA_HOME` is unset, so every component
/// (and every child process) that reads `CUA_HOME` directly agrees with
/// [`crate::cua_home`]. Returns the home it set.
///
/// # Safety
///
/// Changes the process environment: call it first in `main`, before any
/// other thread exists.
pub unsafe fn pin_home() -> Option<PathBuf> {
    let home = env_home()?;
    // SAFETY: the caller guarantees no other thread exists.
    unsafe { std::env::set_var("CUA_HOME", &home) };
    Some(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn endpoints(vars: &[(&str, &str)]) -> Endpoints {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Endpoints::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn parses_environment_names() {
        assert_eq!(CuaEnv::parse(""), Some(CuaEnv::Prod));
        assert_eq!(CuaEnv::parse("prod"), Some(CuaEnv::Prod));
        assert_eq!(CuaEnv::parse(" Production "), Some(CuaEnv::Prod));
        assert_eq!(CuaEnv::parse("STAGING"), Some(CuaEnv::Staging));
        assert_eq!(CuaEnv::parse("stage"), None);
        assert_eq!(CuaEnv::try_from_value(None), Ok(CuaEnv::Prod));
        let err = CuaEnv::try_from_value(Some("dev")).unwrap_err();
        assert!(err.contains("CUA_ENV") && err.contains("staging"), "{err}");
    }

    #[test]
    fn unset_is_production_with_todays_endpoints() {
        let e = endpoints(&[]);
        assert_eq!(e.env, CuaEnv::Prod);
        assert_eq!(e.oidc_issuer, "https://auth.cua.ai/realms/cyclops-cs");
        assert_eq!(e.relay_url, "https://relay.cua.ai");
        assert_eq!(e.spaces_api_url, "https://api.cua.ai");
        assert_eq!(e.account_app_url, "https://app.cua.ai");
        assert_eq!(e.spaces_site_url, "https://cua.ai");
        assert_eq!(CuaEnv::Prod.keyring_service(), "ai.cua.session");
        assert_eq!(CuaEnv::Prod.legacy_keyring_service(), "run.cua.ai");
        assert_eq!(CuaEnv::Prod.home_subdir(), None);
        // An unknown value never selects staging.
        assert_eq!(endpoints(&[("CUA_ENV", "stagign")]), e);
    }

    #[test]
    fn staging_flips_every_endpoint() {
        let e = endpoints(&[("CUA_ENV", "staging")]);
        assert_eq!(e.env, CuaEnv::Staging);
        assert_eq!(
            e.oidc_issuer,
            "https://auth.staging.cua.ai/realms/cua-staging"
        );
        assert_eq!(e.relay_url, "https://relay.staging.cua.ai");
        assert_eq!(e.spaces_api_url, "https://api.staging.cua.ai");
        assert_eq!(e.account_app_url, "https://app.staging.cua.ai");
        assert_eq!(e.spaces_site_url, "https://spaces.staging.cua.ai");
        assert_eq!(CuaEnv::Staging.keyring_service(), "ai.cua.session.staging");
        assert_eq!(
            CuaEnv::Staging.legacy_keyring_service(),
            "staging.run.cua.ai"
        );
        assert_eq!(CuaEnv::Staging.home_subdir(), Some("envs/staging"));
    }

    #[test]
    fn per_setting_overrides_win_over_the_environment() {
        let e = endpoints(&[
            ("CUA_ENV", "staging"),
            ("CUA_OIDC_ISSUER", "http://127.0.0.1:8080/realms/dev/"),
            ("CUA_RELAY_URL", " http://127.0.0.1:9000 "),
            ("CUA_SPACES_API_URL", ""),
        ]);
        assert_eq!(e.oidc_issuer, "http://127.0.0.1:8080/realms/dev");
        assert_eq!(e.relay_url, "http://127.0.0.1:9000");
        // Empty means unset.
        assert_eq!(e.spaces_api_url, "https://api.staging.cua.ai");
        assert_eq!(e.spaces_site_url, "https://spaces.staging.cua.ai");
        let p = endpoints(&[("CUA_RELAY_URL", "https://relay.example")]);
        assert_eq!(p.relay_url, "https://relay.example");
        assert_eq!(p.oidc_issuer, "https://auth.cua.ai/realms/cyclops-cs");
    }
}
