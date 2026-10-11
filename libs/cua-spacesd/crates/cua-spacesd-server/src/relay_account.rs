// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `join` account mode: accepts relay-signed principal assertions
//! (`x-cua-relay-assertion`) as credentials, checked against the relay keys
//! learned on the join handshake (or pinned), this machine's id and the host
//! policy (owner, allowlist, sharing switch) written by `cua host setup`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::auth::{ExternalAuthenticator, ExternalGrant};
use cua_proto::env::v1::{Principal, PrincipalKind};
use cua_relay::assertion::{AssertionClaims, ASSERTION_HEADER};
use cua_relay::client::AccountLink;
use serde::Deserialize;

/// The host policy file (`~/.cua/host/host.json` → `policy` section is the
/// whole file for the driver; unknown fields are ignored).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HostPolicy {
    /// Owning account id. When absent, the relay-reported owner is used.
    #[serde(default)]
    pub owner: Option<String>,
    /// Owner email (informational; also accepted as an allow entry).
    #[serde(default)]
    pub owner_email: Option<String>,
    /// Account ids or emails allowed besides the owner.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Account ids or emails that may only watch (presence and a view-only
    /// stream), whatever the relay says. An entry here wins over `allow`.
    #[serde(default)]
    pub viewers: Vec<String>,
    /// Also accept accounts the relay says the owner shared with (the
    /// relay-side allowlist, managed from any signed-in device).
    #[serde(default = "yes")]
    pub trust_relay_allowlist: bool,
    /// False: refuse every relayed caller ("stop sharing").
    #[serde(default = "yes")]
    pub sharing: bool,
    /// False: this machine's desktop is not a Space. Relayed callers reach
    /// only the capabilities probe and `HostSpacesService`.
    #[serde(default = "yes")]
    pub share_desktop: bool,
    /// Accept `HostSpacesService` calls (create Spaces on this machine).
    #[serde(default)]
    pub provide_spaces: bool,
    /// Where this machine's cua daemon listens, which creates the Spaces.
    #[serde(default)]
    pub spaces_daemon: Option<SpacesDaemon>,
    /// A host in direct mode (`cua host setup --direct ...
    /// --provide-spaces`): Spaces are provided without the relay. `None`
    /// in relay mode.
    #[serde(default)]
    pub direct: Option<DirectHosting>,
}

/// How a host in direct mode provides Spaces.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct DirectHosting {
    /// The direct listener (`ip:port`).
    #[serde(default)]
    pub listen: String,
    /// Take host calls from any address, not only loopback, Tailscale and
    /// private LAN addresses ([`crate::peer::is_private_address`]).
    #[serde(default)]
    pub allow_any_address: bool,
}

/// How the driver reaches (and starts) the host's cua daemon.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct SpacesDaemon {
    /// Unix socket of the daemon.
    pub socket: String,
    /// The cua home it runs with.
    #[serde(default)]
    pub cua_home: String,
    /// The `cua` CLI that starts it.
    #[serde(default)]
    pub cua_bin: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for HostPolicy {
    fn default() -> Self {
        Self {
            owner: None,
            owner_email: None,
            allow: Vec::new(),
            viewers: Vec::new(),
            trust_relay_allowlist: true,
            sharing: true,
            share_desktop: true,
            provide_spaces: false,
            spaces_daemon: None,
            direct: None,
        }
    }
}

/// What a verified caller may do on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The owner, or an account shared as an editor.
    Full,
    /// An account shared to watch only.
    ViewOnly,
}

impl HostPolicy {
    /// What `claims` may do on this machine. `relay_owner` is the owner the
    /// relay reported on join (used when the policy names none).
    ///
    /// A viewer entry (here, or the relay's `viewer` role) always wins over
    /// an editor entry for the same account: sharing to watch must never
    /// turn into input because a second list also names the account.
    pub fn authorize(
        &self,
        claims: &AssertionClaims,
        relay_owner: Option<&str>,
    ) -> Result<Access, String> {
        if !self.sharing {
            return Err("this machine stopped sharing".into());
        }
        let owner = self.owner.as_deref().or(relay_owner);
        if owner.is_some_and(|o| o == claims.acct) {
            return Ok(Access::Full);
        }
        let email = claims.email.as_deref().unwrap_or("");
        let listed = |list: &[String]| {
            list.iter().any(|a| {
                a == &claims.acct
                    || a == &claims.sub
                    || (!email.is_empty() && a.eq_ignore_ascii_case(email))
            })
        };
        let trusted = self.trust_relay_allowlist && owner.is_some();
        if listed(&self.viewers) || (trusted && claims.role == "viewer") {
            return Ok(Access::ViewOnly);
        }
        if listed(&self.allow) || (trusted && claims.role == "shared") {
            return Ok(Access::Full);
        }
        Err(format!(
            "account {} is not allowed on this machine",
            claims.acct
        ))
    }
}

/// Stable `#RRGGBB` for a user id.
pub fn color_for(id: &str) -> String {
    const PALETTE: &[&str] = &[
        "#E5484D", "#F76B15", "#FFC53D", "#46A758", "#12A594", "#0090FF", "#6E56CF", "#D6409F",
    ];
    let hash = id.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    PALETTE[(hash % PALETTE.len() as u64) as usize].to_owned()
}

/// Maps verified claims to the presence principal. Color and kind may come
/// from the client's own `x-cua-principal-bin`; identity never does. Only
/// the owner may present itself as an agent: an account the Space was
/// shared with is always a human (its input is a human-origin session).
pub fn principal_for(claims: &AssertionClaims, client: Option<&Principal>) -> Principal {
    let display_name = claims
        .name
        .clone()
        .or_else(|| claims.email.clone())
        .unwrap_or_else(|| claims.sub.clone());
    let color = client
        .map(|p| p.color.clone())
        .filter(|c| c.len() == 7 && c.starts_with('#'))
        .unwrap_or_else(|| color_for(&claims.sub));
    let kind = client
        .map(|p| p.kind)
        .filter(|k| *k == PrincipalKind::Agent as i32 && claims.role == "owner")
        .unwrap_or(PrincipalKind::Human as i32);
    Principal {
        id: claims.sub.clone(),
        display_name,
        color,
        kind,
    }
}

struct CachedPolicy {
    modified: Option<(SystemTime, u64)>,
    policy: HostPolicy,
}

/// The authenticator installed on the server in `join` mode.
pub struct RelayAssertionAuth {
    machine_id: String,
    link: Arc<AccountLink>,
    policy_file: Option<PathBuf>,
    cache: Mutex<CachedPolicy>,
}

impl RelayAssertionAuth {
    /// Creates the authenticator.
    pub fn new(machine_id: String, link: Arc<AccountLink>, policy_file: Option<PathBuf>) -> Self {
        Self {
            machine_id,
            link,
            policy_file,
            cache: Mutex::new(CachedPolicy {
                modified: None,
                policy: HostPolicy::default(),
            }),
        }
    }

    /// The current policy, re-read when the file changed. A missing file is
    /// the default policy; an unreadable or invalid one refuses everyone
    /// (fail closed).
    pub fn policy(&self) -> Result<HostPolicy, String> {
        let Some(path) = &self.policy_file else {
            return Ok(HostPolicy::default());
        };
        let modified = match std::fs::metadata(path) {
            Ok(meta) => meta.modified().ok().map(|m| (m, meta.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HostPolicy::default()),
            Err(e) => return Err(format!("host policy {}: {e}", path.display())),
        };
        let mut cache = self.cache.lock().expect("policy cache");
        if cache.modified.is_none() || cache.modified != modified {
            let raw =
                std::fs::read(path).map_err(|e| format!("host policy {}: {e}", path.display()))?;
            cache.policy = serde_json::from_slice(&raw)
                .map_err(|e| format!("host policy {}: {e}", path.display()))?;
            cache.modified = modified;
        }
        Ok(cache.policy.clone())
    }
}

impl ExternalAuthenticator for RelayAssertionAuth {
    fn authenticate(&self, headers: &http::HeaderMap) -> Option<Result<ExternalGrant, String>> {
        let token = headers.get(ASSERTION_HEADER)?.to_str().ok()?;
        Some((|| {
            let claims = self.link.keys.verify(token, &self.machine_id)?;
            let policy = self.policy()?;
            let access = policy.authorize(&claims, self.link.owner().as_deref())?;
            let client = crate::auth::principal_from_headers(headers);
            let owner = policy
                .owner
                .as_deref()
                .or(self.link.owner().as_deref())
                .is_some_and(|o| o == claims.acct);
            Ok(ExternalGrant {
                principal: principal_for(&claims, client.as_ref()),
                view_only: access == Access::ViewOnly,
                host_only: !policy.share_desktop,
                account: Some(crate::auth::RelayCaller {
                    account: claims.acct.clone(),
                    email: claims.email.clone(),
                    name: claims.name.clone(),
                    role: if owner { "owner" } else { "shared" }.into(),
                }),
            })
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_relay::assertion::{now_secs, RelayKey};

    fn claims(acct: &str, role: &str, email: Option<&str>) -> AssertionClaims {
        let now = now_secs();
        AssertionClaims {
            iss: "http://relay".into(),
            aud: "machine-0001".into(),
            sub: acct.into(),
            acct: acct.into(),
            email: email.map(str::to_owned),
            name: Some(format!("User {acct}")),
            mid: "machine-0001".into(),
            role: role.into(),
            scope: "env".into(),
            iat: now,
            exp: now + 60,
            jti: "j".into(),
        }
    }

    #[test]
    fn policy_owner_allowlist_relay_shares_and_sharing_switch() {
        let mut p = HostPolicy {
            owner: Some("ada".into()),
            ..HostPolicy::default()
        };
        assert_eq!(
            p.authorize(&claims("ada", "owner", None), None),
            Ok(Access::Full)
        );
        assert_eq!(
            p.authorize(&claims("bob", "shared", None), None),
            Ok(Access::Full)
        );
        // The relay's viewer role is view-only, and so is a local viewer
        // entry even when the relay says shared.
        assert_eq!(
            p.authorize(&claims("bob", "viewer", None), None),
            Ok(Access::ViewOnly)
        );
        let mut local = p.clone();
        local.viewers = vec!["bob".into()];
        local.allow = vec!["bob".into()];
        assert_eq!(
            local.authorize(&claims("bob", "shared", None), None),
            Ok(Access::ViewOnly)
        );
        assert!(p.authorize(&claims("carol", "none", None), None).is_err());
        p.trust_relay_allowlist = false;
        assert!(p.authorize(&claims("bob", "shared", None), None).is_err());
        p.allow = vec!["BOB@example.com".into()];
        assert!(p
            .authorize(&claims("bob", "shared", Some("bob@example.com")), None)
            .is_ok());
        // A relay "owner" role for another account is not trusted.
        assert!(p.authorize(&claims("eve", "owner", None), None).is_err());
        p.sharing = false;
        assert!(p.authorize(&claims("ada", "owner", None), None).is_err());
        // No owner in the policy: the relay-reported owner is used.
        let d = HostPolicy::default();
        assert!(d
            .authorize(&claims("ada", "owner", None), Some("ada"))
            .is_ok());
        assert!(d
            .authorize(&claims("eve", "owner", None), Some("ada"))
            .is_err());
    }

    #[test]
    fn verifies_assertions_and_reloads_policy() {
        let key = RelayKey::generate();
        let link = Arc::new(AccountLink::default());
        link.keys.set_jwks_json(&key.jwks().to_string()).unwrap();
        *link.owner.write().unwrap() = Some("ada".into());
        let dir = tempfile::tempdir().unwrap();
        let policy = dir.path().join("host.json");
        let auth = RelayAssertionAuth::new("machine-0001".into(), link, Some(policy.clone()));
        let mut headers = http::HeaderMap::new();
        assert!(
            auth.authenticate(&headers).is_none(),
            "no assertion: fall back to the token"
        );
        headers.insert(
            ASSERTION_HEADER,
            key.sign(&claims("ada", "owner", None)).parse().unwrap(),
        );
        let g = auth.authenticate(&headers).unwrap().unwrap();
        assert!(!g.view_only);
        let p = g.principal;
        assert_eq!(p.id, "ada");
        assert_eq!(p.display_name, "User ada");
        assert!(p.color.starts_with('#'));
        // Stop sharing through the policy file.
        std::fs::write(&policy, r#"{"owner":"ada","sharing":false}"#).unwrap();
        assert!(auth.authenticate(&headers).unwrap().is_err());
        // Assertions for another machine or from another key are refused.
        std::fs::write(&policy, r#"{"owner":"ada"}"#).unwrap();
        let mut other = claims("ada", "owner", None);
        other.aud = "machine-0002".into();
        other.mid = "machine-0002".into();
        headers.insert(ASSERTION_HEADER, key.sign(&other).parse().unwrap());
        assert!(auth.authenticate(&headers).unwrap().is_err());
        headers.insert(
            ASSERTION_HEADER,
            RelayKey::generate()
                .sign(&claims("ada", "owner", None))
                .parse()
                .unwrap(),
        );
        assert!(auth.authenticate(&headers).unwrap().is_err());
        // Invalid policy fails closed.
        std::fs::write(&policy, "{not json").unwrap();
        headers.insert(
            ASSERTION_HEADER,
            key.sign(&claims("ada", "owner", None)).parse().unwrap(),
        );
        assert!(auth.authenticate(&headers).unwrap().is_err());
    }
}
