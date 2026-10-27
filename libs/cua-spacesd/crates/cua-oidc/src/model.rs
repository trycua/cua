// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Connections (one linked cloud of an account), their limits, and audit
//! records. A connection holds identifiers and limits only, never a usable
//! secret: an AWS role ARN or an Azure client id only lets someone ask the
//! cloud to trust a Cua-signed token for this account. A `sealed`
//! connection (Hetzner, Modal) carries its API token as ciphertext that
//! only the account's devices can open.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cloud {
    /// Amazon Web Services.
    Aws,
    /// Google Cloud.
    Gcp,
    /// Microsoft Azure.
    Azure,
    /// Hetzner Cloud.
    Hetzner,
    /// Modal.
    Modal,
}

impl Cloud {
    /// The lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aws => "aws",
            Self::Gcp => "gcp",
            Self::Azure => "azure",
            Self::Hetzner => "hetzner",
            Self::Modal => "modal",
        }
    }

    /// How this cloud is reached: OIDC federation where the cloud supports
    /// it, else a sealed API token.
    pub fn auth(self) -> Auth {
        match self {
            Self::Aws | Self::Gcp | Self::Azure => Auth::Oidc,
            Self::Hetzner | Self::Modal => Auth::Sealed,
        }
    }

    /// The identifiers a connection to this cloud may record, and which one
    /// names it (unique per account among live connections).
    pub fn identifier_keys(self) -> (&'static [&'static str], &'static str) {
        match self {
            Self::Aws => (&["account_id", "role_arn"], "account_id"),
            Self::Gcp => (
                &["project_number", "pool", "provider", "service_account"],
                "project_number",
            ),
            Self::Azure => (
                &[
                    "tenant_id",
                    "client_id",
                    "subscription_id",
                    "resource_group",
                ],
                "subscription_id",
            ),
            Self::Hetzner => (&["project"], "project"),
            Self::Modal => (&["workspace"], "workspace"),
        }
    }

    /// Whether `aud` is an audience this cloud's token exchange accepts:
    /// `sts.amazonaws.com` (AWS), the workload identity provider's full
    /// resource name (GCP), `api://AzureADTokenExchange` (Azure).
    pub fn allows_audience(self, aud: &str) -> bool {
        match self {
            Self::Aws => aud == "sts.amazonaws.com",
            Self::Azure => aud == "api://AzureADTokenExchange",
            Self::Gcp => gcp_provider_audience(aud),
            Self::Hetzner | Self::Modal => false,
        }
    }
}

/// `//iam.googleapis.com/projects/{number}/locations/global/workloadIdentityPools/{pool}/providers/{provider}`.
fn gcp_provider_audience(aud: &str) -> bool {
    let Some(rest) = aud.strip_prefix("//iam.googleapis.com/projects/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let id = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    matches!(
        parts.as_slice(),
        [project, "locations", "global", "workloadIdentityPools", pool, "providers", provider]
            if !project.is_empty() && project.bytes().all(|b| b.is_ascii_digit()) && id(pool) && id(provider)
    )
}

/// How the device gets cloud credentials for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    /// A token from this issuer, exchanged with the cloud.
    Oidc,
    /// An API token sealed to the account's devices.
    Sealed,
}

/// Where a connection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Created by an agent; waits for a person to approve it.
    Pending,
    /// Usable.
    Connected,
    /// Disconnected (terminal; no more tokens).
    Revoked,
}

impl Status {
    /// The lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connected => "connected",
            Self::Revoked => "revoked",
        }
    }
}

/// What a token is for (`cua_purpose`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    /// Create machines.
    Create,
    /// Stop, start, delete, sweep.
    Manage,
    /// Test the trust setup.
    Check,
    /// Read the cloud's own billing data.
    Cost,
}

impl Purpose {
    /// The lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Manage => "manage",
            Self::Check => "check",
            Self::Cost => "cost",
        }
    }
}

/// The largest machine a connection may create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaxSize {
    /// vCPUs.
    pub vcpu: u32,
    /// Memory in GiB.
    pub memory_gib: u32,
}

/// A connection's limits. `None` is "not set".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    /// Monthly budget in US dollars (needed before the first `create`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd_month: Option<f64>,
    /// Largest machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<MaxSize>,
    /// Hours a machine may live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_hours: Option<u32>,
}

impl Limits {
    /// Checks the values are sane.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(b) = self.budget_usd_month {
            if !b.is_finite() || !(0.0..=1_000_000.0).contains(&b) {
                return Err("budget_usd_month must be between 0 and 1000000".into());
            }
        }
        if let Some(m) = self.max_size {
            if m.vcpu == 0 || m.vcpu > 1024 || m.memory_gib == 0 || m.memory_gib > 32_768 {
                return Err("max_size must be 1-1024 vCPU and 1-32768 GiB".into());
            }
        }
        if let Some(t) = self.ttl_hours {
            if t == 0 || t > 24 * 366 {
                return Err("ttl_hours must be between 1 and 8784".into());
            }
        }
        Ok(())
    }

    /// Splits a change into what may apply now and what raises a limit
    /// (and so needs a person's approval when an agent asks for it).
    /// Setting a limit that was unset counts as raising it, except
    /// `max_size` and `ttl_hours`, whose unset means "no limit".
    pub fn split_raise(&self, current: &Limits) -> (Limits, Limits) {
        let mut now = Limits::default();
        let mut raise = Limits::default();
        if let Some(b) = self.budget_usd_month {
            match current.budget_usd_month {
                Some(c) if b <= c => now.budget_usd_month = Some(b),
                _ => raise.budget_usd_month = Some(b),
            }
        }
        if let Some(m) = self.max_size {
            match current.max_size {
                Some(c) if m.vcpu > c.vcpu || m.memory_gib > c.memory_gib => {
                    raise.max_size = Some(m)
                }
                _ => now.max_size = Some(m),
            }
        }
        if let Some(t) = self.ttl_hours {
            match current.ttl_hours {
                Some(c) if t > c => raise.ttl_hours = Some(t),
                _ => now.ttl_hours = Some(t),
            }
        }
        (now, raise)
    }

    /// Applies the fields `change` sets.
    pub fn apply(&mut self, change: &Limits) {
        if change.budget_usd_month.is_some() {
            self.budget_usd_month = change.budget_usd_month;
        }
        if change.max_size.is_some() {
            self.max_size = change.max_size;
        }
        if change.ttl_hours.is_some() {
            self.ttl_hours = change.ttl_hours;
        }
    }

    /// Whether no field is set.
    pub fn is_empty(&self) -> bool {
        self.budget_usd_month.is_none() && self.max_size.is_none() && self.ttl_hours.is_none()
    }
}

/// Who created a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedBy {
    /// Keycloak user (`sub`).
    pub user: String,
    /// `dev_…`.
    pub device: String,
    /// Agent or routine name, when an agent asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// Who approved a connection or a limit raise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedBy {
    /// Keycloak user (`sub`).
    pub user: String,
    /// `dev_…`.
    pub device: String,
    /// Unix seconds.
    pub at: u64,
}

/// A sealed API token: ciphertext only the account's devices open, and the
/// pool key generation it was sealed under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedToken {
    /// Opaque ciphertext, base64 (standard alphabet) on the wire.
    #[serde(with = "b64_bytes")]
    pub ciphertext: Vec<u8>,
    /// Pool data key generation.
    pub generation: u64,
}

mod b64_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
}

/// Largest sealed ciphertext accepted.
pub const MAX_SEALED_BYTES: usize = 16 * 1024;

/// One linked cloud of an account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    /// `conn_…`.
    pub id: String,
    /// The issuer account id (`acct`).
    pub account: String,
    /// Cloud.
    pub cloud: Cloud,
    /// `oidc` or `sealed` (fixed by the cloud).
    pub auth: Auth,
    /// The cloud's identifiers (see [`Cloud::identifier_keys`]).
    pub identifiers: BTreeMap<String, String>,
    /// Default region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Limits in force.
    pub limits: Limits,
    /// Limits an agent raised, waiting for approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_limits: Option<Limits>,
    /// The agent whose raise waits in `pending_limits`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_agent: Option<String>,
    /// Status.
    pub status: Status,
    /// Creator.
    pub created_by: CreatedBy,
    /// Last approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<ApprovedBy>,
    /// Sealed API token (`sealed` connections).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_token: Option<SealedToken>,
    /// Unix seconds.
    pub created_at: u64,
    /// Unix seconds.
    pub updated_at: u64,
    /// Bumped by every change (optimistic concurrency).
    pub version: u64,
}

impl Connection {
    /// The identifier that names this connection.
    pub fn primary_id(&self) -> &str {
        let (_, primary) = self.cloud.identifier_keys();
        self.identifiers
            .get(primary)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Whether `aud` is allowed for this connection: the cloud's audience,
    /// and for Google Cloud the connection's own provider when recorded.
    pub fn allows_audience(&self, aud: &str) -> bool {
        if !self.cloud.allows_audience(aud) {
            return false;
        }
        if self.cloud == Cloud::Gcp {
            if let (Some(number), Some(pool), Some(provider)) = (
                self.identifiers.get("project_number"),
                self.identifiers.get("pool"),
                self.identifiers.get("provider"),
            ) {
                return aud
                    == format!(
                        "//iam.googleapis.com/projects/{number}/locations/global/workloadIdentityPools/{pool}/providers/{provider}"
                    );
            }
        }
        true
    }
}

/// Checks a cloud's identifiers: known keys, the naming one present,
/// printable values of sane length.
pub fn validate_identifiers(
    cloud: Cloud,
    identifiers: &BTreeMap<String, String>,
) -> Result<(), String> {
    let (allowed, primary) = cloud.identifier_keys();
    for (k, v) in identifiers {
        if !allowed.contains(&k.as_str()) {
            return Err(format!(
                "{} connections take identifiers {}; not {k:?}",
                cloud.as_str(),
                allowed.join(", ")
            ));
        }
        if v.is_empty() || v.len() > 512 || v.chars().any(|c| c.is_control()) {
            return Err(format!(
                "identifier {k:?} must be 1-512 printable characters"
            ));
        }
    }
    if !identifiers.contains_key(primary) {
        return Err(format!(
            "{} connections need the identifier {primary:?}",
            cloud.as_str()
        ));
    }
    Ok(())
}

/// Checks an agent name: 1-128 printable characters.
pub fn valid_agent(agent: &str) -> bool {
    !agent.is_empty() && agent.len() <= 128 && !agent.chars().any(|c| c.is_control())
}

/// One append-only audit row: an issuance, a refusal, or a connection
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Unix seconds.
    pub at: u64,
    /// Issuer account id.
    pub account: String,
    /// `token_issued`, `token_refused`, `connection_created`,
    /// `connection_approved`, `limits_changed`, `limits_requested`,
    /// `connection_revoked`, `connection_deleted`, `request_refused`.
    pub kind: String,
    /// Connection id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// `dev_…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Agent name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Keycloak user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Token purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Token audience.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// Token id of an issued token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Error code of a refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl AuditRecord {
    /// A row of `kind` for `account`, now.
    pub fn new(account: &str, kind: &str) -> Self {
        Self {
            at: crate::now_secs(),
            account: account.to_owned(),
            kind: kind.to_owned(),
            connection: None,
            device: None,
            agent: None,
            user: None,
            purpose: None,
            audience: None,
            jti: None,
            code: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audiences_per_cloud() {
        assert!(Cloud::Aws.allows_audience("sts.amazonaws.com"));
        assert!(!Cloud::Aws.allows_audience("api://AzureADTokenExchange"));
        assert!(Cloud::Azure.allows_audience("api://AzureADTokenExchange"));
        assert!(!Cloud::Azure.allows_audience("sts.amazonaws.com"));
        let gcp = "//iam.googleapis.com/projects/123456/locations/global/workloadIdentityPools/cua/providers/cua";
        assert!(Cloud::Gcp.allows_audience(gcp));
        assert!(!Cloud::Aws.allows_audience(gcp));
        for bad in [
            "//iam.googleapis.com/projects/abc/locations/global/workloadIdentityPools/cua/providers/cua",
            "//iam.googleapis.com/projects/1/locations/us/workloadIdentityPools/cua/providers/cua",
            "//iam.googleapis.com/projects/1/locations/global/workloadIdentityPools/cua/providers/cua/x",
            "//iam.googleapis.com/projects/1/locations/global/workloadIdentityPools//providers/cua",
            "https://iam.googleapis.com/projects/1/locations/global/workloadIdentityPools/cua/providers/cua",
            "sts.amazonaws.com",
        ] {
            assert!(!Cloud::Gcp.allows_audience(bad), "{bad}");
        }
        assert!(!Cloud::Hetzner.allows_audience("sts.amazonaws.com"));
    }

    #[test]
    fn raising_limits_is_split_out() {
        let current = Limits {
            budget_usd_month: Some(100.0),
            max_size: Some(MaxSize {
                vcpu: 4,
                memory_gib: 16,
            }),
            ttl_hours: Some(4),
        };
        let lower = Limits {
            budget_usd_month: Some(50.0),
            max_size: Some(MaxSize {
                vcpu: 2,
                memory_gib: 8,
            }),
            ttl_hours: Some(2),
        };
        let (now, raise) = lower.split_raise(&current);
        assert_eq!(now, lower);
        assert!(raise.is_empty());
        let higher = Limits {
            budget_usd_month: Some(500.0),
            max_size: Some(MaxSize {
                vcpu: 8,
                memory_gib: 16,
            }),
            ttl_hours: Some(3),
        };
        let (now, raise) = higher.split_raise(&current);
        assert_eq!(now.ttl_hours, Some(3));
        assert_eq!(raise.budget_usd_month, Some(500.0));
        assert!(raise.max_size.is_some());
        // Setting a budget where none was set is a raise.
        let (now, raise) = Limits {
            budget_usd_month: Some(1.0),
            ..Limits::default()
        }
        .split_raise(&Limits::default());
        assert!(now.is_empty());
        assert_eq!(raise.budget_usd_month, Some(1.0));
    }

    #[test]
    fn identifiers_are_checked_per_cloud() {
        let ids = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };
        assert!(validate_identifiers(Cloud::Aws, &ids(&[("account_id", "123456789012")])).is_ok());
        assert!(validate_identifiers(Cloud::Aws, &ids(&[("role_arn", "x")])).is_err());
        assert!(validate_identifiers(
            Cloud::Aws,
            &ids(&[("account_id", "1"), ("api_token", "secret")])
        )
        .is_err());
        assert!(validate_identifiers(Cloud::Hetzner, &ids(&[("project", "p")])).is_ok());
    }
}
