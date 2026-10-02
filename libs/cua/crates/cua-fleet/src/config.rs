//! Fleet configuration with the same environment variables and defaults as
//! cua-sandbox's `_config.py`.

use std::fmt;

/// Default Fleet API endpoint (`_DEFAULT_FLEET_BASE_URL`).
pub const DEFAULT_FLEET_BASE_URL: &str = "https://run.cua.ai";
/// Default OAuth token endpoint (`_DEFAULT_TOKEN_URL`).
pub const DEFAULT_TOKEN_URL: &str =
    "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token";
/// Poll interval cua-sandbox uses for pools and claims (ms).
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 2000;
/// Poll limit cua-sandbox uses for pools and claims.
pub const DEFAULT_POLL_LIMIT: u32 = 300;

/// How to reach and authenticate to Fleet.
#[derive(Clone, PartialEq, Eq)]
pub struct FleetConfig {
    /// Fleet API base URL (`CUA_FLEET_BASE_URL`).
    pub base_url: String,
    /// OAuth token URL (`CUA_TOKEN_URL`).
    pub token_url: String,
    /// OAuth client id (`CUA_CLIENT_ID`).
    pub client_id: Option<String>,
    /// OAuth client secret (`CUA_CLIENT_SECRET`).
    pub client_secret: Option<String>,
    /// Static Fleet workload token (`FLEETS_TOKEN`); wins over client
    /// credentials, as in cua-sandbox.
    pub fleet_token: Option<String>,
    /// Pool poll interval.
    pub pool_poll_interval_ms: u64,
    /// Pool poll limit.
    pub pool_poll_limit: u32,
    /// Claim poll interval.
    pub claim_poll_interval_ms: u64,
    /// Claim poll limit.
    pub claim_poll_limit: u32,
}

impl fmt::Debug for FleetConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redact = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("FleetConfig")
            .field("base_url", &self.base_url)
            .field("token_url", &self.token_url)
            .field("client_id", &redact(&self.client_id))
            .field("client_secret", &redact(&self.client_secret))
            .field("fleet_token", &redact(&self.fleet_token))
            .finish_non_exhaustive()
    }
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_FLEET_BASE_URL.into(),
            token_url: DEFAULT_TOKEN_URL.into(),
            client_id: None,
            client_secret: None,
            fleet_token: None,
            pool_poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            pool_poll_limit: DEFAULT_POLL_LIMIT,
            claim_poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            claim_poll_limit: DEFAULT_POLL_LIMIT,
        }
    }
}

impl FleetConfig {
    /// Reads `CUA_FLEET_BASE_URL`, `CUA_TOKEN_URL`, `CUA_CLIENT_ID`,
    /// `CUA_CLIENT_SECRET` and `FLEETS_TOKEN`.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// [`FleetConfig::from_env`] over an arbitrary lookup (for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let non_empty = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Self {
            base_url: non_empty("CUA_FLEET_BASE_URL")
                .unwrap_or_else(|| DEFAULT_FLEET_BASE_URL.into())
                .trim_end_matches('/')
                .to_string(),
            token_url: non_empty("CUA_TOKEN_URL").unwrap_or_else(|| DEFAULT_TOKEN_URL.into()),
            client_id: non_empty("CUA_CLIENT_ID"),
            client_secret: non_empty("CUA_CLIENT_SECRET"),
            fleet_token: non_empty("FLEETS_TOKEN"),
            ..Default::default()
        }
    }

    /// True when a static token or both client credentials are present.
    pub fn has_auth(&self) -> bool {
        self.fleet_token.is_some() || (self.client_id.is_some() && self.client_secret.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn defaults_match_cua_sandbox() {
        let c = FleetConfig::from_lookup(|_| None);
        assert_eq!(c.base_url, "https://run.cua.ai");
        assert_eq!(
            c.token_url,
            "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
        );
        assert!(!c.has_auth());
        assert_eq!((c.claim_poll_interval_ms, c.claim_poll_limit), (2000, 300));
    }

    #[test]
    fn reads_env_and_trims() {
        let env: HashMap<&str, &str> = [
            ("CUA_FLEET_BASE_URL", "https://fleet.example/"),
            ("CUA_CLIENT_ID", "id"),
            ("CUA_CLIENT_SECRET", "secret"),
            ("FLEETS_TOKEN", "  "),
        ]
        .into();
        let c = FleetConfig::from_lookup(|k| env.get(k).map(|v| v.to_string()));
        assert_eq!(c.base_url, "https://fleet.example");
        assert!(c.has_auth());
        assert_eq!(c.fleet_token, None);
        assert!(!format!("{c:?}").contains("secret\""));
    }
}
