// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `join` account mode: accepts relay-signed principal assertions
//! (`x-cua-relay-assertion`) as credentials, checked against the relay keys
//! learned on the join handshake (or pinned), this machine's id and the host
//! policy (owner, allowlist, sharing switch) written by `cua host setup`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    /// Identify tailnet callers by their tailnet login (`cua host setup
    /// --tailnet`; see [`crate::tailnet`]): the owner and share lists then
    /// name tailnet users.
    #[serde(default)]
    pub tailnet_identity: bool,
    /// Forward `/share/*` to the cua daemon's share gateway
    /// ([`HostPolicy::shares_gateway`]). Off by default: a host that never
    /// asked for shares answers 404 there.
    #[serde(default)]
    pub shares: bool,
    /// Unix socket of the daemon's share gateway (`<cua home>/share/gateway.sock`).
    #[serde(default)]
    pub shares_gateway: Option<PathBuf>,
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
            tailnet_identity: false,
            shares: false,
            shares_gateway: None,
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
            return Err("this computer stopped sharing".into());
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
            "account {} is not allowed on this computer",
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
/// from the client's own `x-cua-principal-bin`; identity never does.
///
/// A caller with edit access (`editor`: the owner, or an account the machine
/// is shared with to edit) may present itself as an agent acting for its
/// account (CUA-1750): the id is `<account sub>/<client's agent id>` and the
/// name `<agent label> (<account name>)`, so the agent is a participant of
/// its own and always attributed to the verified account. A view-only
/// caller is always a human. Access itself is decided before this, by
/// [`HostPolicy::authorize`]; the kind grants nothing.
pub fn principal_for(
    claims: &AssertionClaims,
    client: Option<&Principal>,
    editor: bool,
) -> Principal {
    let user = claims
        .name
        .clone()
        .or_else(|| claims.email.clone())
        .unwrap_or_else(|| claims.sub.clone());
    let agent = client.filter(|p| editor && p.kind == PrincipalKind::Agent as i32);
    let (id, display_name, kind) = match agent {
        Some(p) => {
            let suffix: String =
                p.id.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
                    .take(64)
                    .collect();
            let suffix = if suffix.is_empty() {
                "agent".into()
            } else {
                suffix
            };
            let label: String = p
                .display_name
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(40)
                .collect();
            let label = if label.is_empty() {
                "Agent".into()
            } else {
                label
            };
            (
                format!("{}/{suffix}", claims.sub),
                format!("{label} ({user})"),
                PrincipalKind::Agent as i32,
            )
        }
        None => (claims.sub.clone(), user, PrincipalKind::Human as i32),
    };
    let color = client
        .map(|p| p.color.clone())
        .filter(|c| c.len() == 7 && c.starts_with('#'))
        .unwrap_or_else(|| color_for(&id));
    Principal {
        id,
        display_name,
        color,
        kind,
    }
}

struct CachedPolicy {
    /// The file's bytes the policy was parsed from.
    raw: Option<Vec<u8>>,
    policy: HostPolicy,
}

/// The authenticator installed on the server in `join` mode.
pub struct RelayAssertionAuth {
    machine_id: String,
    link: Arc<AccountLink>,
    policy_file: Option<PathBuf>,
    cache: Mutex<CachedPolicy>,
    /// The phones kill switch and the log of what phones did. `None`: this
    /// machine does not answer phones at all.
    phone: Option<Arc<crate::phone_control::PhoneControl>>,
    /// The mobile experiment as last read, and when.
    mobile: Mutex<(std::time::Instant, bool)>,
}

impl RelayAssertionAuth {
    /// Creates the authenticator.
    pub fn new(machine_id: String, link: Arc<AccountLink>, policy_file: Option<PathBuf>) -> Self {
        Self {
            machine_id,
            link,
            policy_file,
            cache: Mutex::new(CachedPolicy {
                raw: None,
                policy: HostPolicy::default(),
            }),
            phone: None,
            mobile: Mutex::new((std::time::Instant::now(), false)),
        }
    }

    /// Lets this machine answer phones, with the kill switch and the log kept
    /// in `phone`. Still off until the `mobile` experiment is on.
    pub fn with_phone(mut self, phone: Arc<crate::phone_control::PhoneControl>) -> Self {
        self.phone = Some(phone);
        // The first call reads the switch.
        self.mobile = Mutex::new((
            std::time::Instant::now() - std::time::Duration::from_secs(60),
            false,
        ));
        self
    }

    /// Whether the mobile experiment is on, read at most once a second.
    fn mobile_on(&self, policy: &HostPolicy) -> bool {
        let mut m = self.mobile.lock().expect("mobile switch");
        if m.0.elapsed() >= std::time::Duration::from_secs(1) {
            let home = policy
                .spaces_daemon
                .as_ref()
                .filter(|d| !d.cua_home.is_empty())
                .map(|d| std::path::PathBuf::from(&d.cua_home));
            *m = (
                std::time::Instant::now(),
                crate::phone_control::mobile_enabled(home.as_deref()),
            );
        }
        m.1
    }

    /// What a verified assertion says about a phone: `Ok(None)` for a
    /// computer, the phone, or the reason this machine does not answer it.
    fn phone_of(
        &self,
        claims: &AssertionClaims,
        policy: &HostPolicy,
    ) -> Result<Option<crate::auth::PhoneCaller>, String> {
        match claims.dcls.as_deref() {
            None => Ok(None),
            Some(cua_relay::phone::CLASS_MOBILE) => {
                let Some(store) = &self.phone else {
                    return Err("this computer does not answer phones".into());
                };
                if !self.mobile_on(policy) {
                    return Err("this computer does not answer phones: the mobile experiment is off (`cua experiments enable mobile`)".into());
                }
                if store.phones_off() {
                    return Err("phones are turned off on this computer".into());
                }
                let device = claims
                    .dev
                    .clone()
                    .filter(|d| !d.is_empty())
                    .ok_or("a phone's call does not say which phone it is")?;
                Ok(Some(crate::auth::PhoneCaller { device }))
            }
            // A kind this machine does not know is a kind it does not serve.
            Some(_) => Err("this computer does not know this kind of phone".into()),
        }
    }

    /// The current policy, re-read when the file changed. A missing file is
    /// the default policy; an unreadable or invalid one refuses everyone
    /// (fail closed).
    pub fn policy(&self) -> Result<HostPolicy, String> {
        let Some(path) = &self.policy_file else {
            return Ok(HostPolicy::default());
        };
        // Compared by content, not by modification time and size: a rewrite
        // of the same size within one timestamp tick (an `allow` swapped for
        // `"sharing":false`) must not keep the old policy. The file is small.
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HostPolicy::default()),
            Err(e) => return Err(format!("host policy {}: {e}", path.display())),
        };
        let mut cache = self.cache.lock().expect("policy cache");
        if cache.raw.as_deref() != Some(raw.as_slice()) {
            let policy: HostPolicy = serde_json::from_slice(&raw)
                .map_err(|e| format!("host policy {}: {e}", path.display()))?;
            cache.policy = policy;
            cache.raw = Some(raw);
        }
        Ok(cache.policy.clone())
    }
}

impl ExternalAuthenticator for RelayAssertionAuth {
    fn share_gateway(&self) -> Result<Option<PathBuf>, String> {
        let policy = self.policy()?;
        if !policy.sharing {
            return Err("this machine stopped sharing".into());
        }
        Ok(policy.shares.then_some(policy.shares_gateway).flatten())
    }

    fn phone_refused(&self, caller: &crate::auth::RelayCaller, path: &str) {
        if let (Some(store), Some(phone)) = (&self.phone, &caller.phone) {
            store.note_refused(
                &caller.account,
                &phone.device,
                path.rsplit('/').next().unwrap_or(""),
            );
        }
    }

    fn phone_used(&self, caller: &crate::auth::RelayCaller, scope: crate::phone_control::Scope) {
        if let (Some(store), Some(phone)) = (&self.phone, &caller.phone) {
            store.note_use(&caller.account, &phone.device, scope);
        }
    }

    fn phone_still_allowed(
        &self,
        _caller: &crate::auth::RelayCaller,
        path: &str,
    ) -> Result<(), String> {
        let Some(store) = &self.phone else {
            return Err("this computer does not answer phones".into());
        };
        let policy = self.policy()?;
        if !policy.sharing {
            return Err("this computer stopped sharing".into());
        }
        if crate::auth::desktop_path(path) && !policy.share_desktop {
            return Err("this computer stopped sharing its desktop".into());
        }
        if !self.mobile_on(&policy) {
            return Err(
                "this computer does not answer phones: the mobile experiment is off".into(),
            );
        }
        if store.phones_off() {
            return Err("phones are turned off on this computer".into());
        }
        Ok(())
    }

    fn authenticate(&self, headers: &http::HeaderMap) -> Option<Result<ExternalGrant, String>> {
        let token = headers.get(ASSERTION_HEADER)?.to_str().ok()?;
        Some((|| {
            // The relay's keys, or the in-process key this machine's own
            // mesh endpoint signs with for peers the relay's map vouched for.
            let claims = match self.link.keys.verify(token, &self.machine_id) {
                Ok(claims) => claims,
                Err(e) => match crate::mesh_host::verify_local(token, &self.machine_id) {
                    Some(Ok(claims)) => claims,
                    _ => return Err(e),
                },
            };
            let policy = self.policy()?;
            let phone = self.phone_of(&claims, &policy)?;
            if claims.role == cua_relay::assertion::THREAD_ROLE {
                let mut grant = thread_grant(&claims, &policy, headers)?;
                if let Some(a) = grant.account.as_mut() {
                    a.phone = phone;
                }
                return Ok(grant);
            }
            let access = policy.authorize(&claims, self.link.owner().as_deref())?;
            let client = crate::auth::principal_from_headers(headers);
            let owner = policy
                .owner
                .as_deref()
                .or(self.link.owner().as_deref())
                .is_some_and(|o| o == claims.acct);
            Ok(ExternalGrant {
                principal: principal_for(&claims, client.as_ref(), access == Access::Full),
                view_only: access == Access::ViewOnly,
                host_only: !policy.share_desktop,
                account: Some(crate::auth::RelayCaller {
                    account: claims.acct.clone(),
                    email: claims.email.clone(),
                    name: claims.name.clone(),
                    role: if owner { "owner" } else { "shared" }.into(),
                    issued_at: claims.iat,
                    teams: claims.teams.clone(),
                    space: None,
                    phone,
                    device: claims.dev.clone(),
                }),
            })
        })())
    }
}

/// A thread grant: the relay vouches that the Space `claims.space`, which
/// this machine provides, is shared with the account (`claims.space_role`).
/// It reaches only that Space's thread ([`crate::auth::THREAD_GRANT_GRPC_METHODS`];
/// the daemon checks the Space again). Honoured only where this machine
/// trusts the relay's sharing (`trust_relay_allowlist`) and still shares.
fn thread_grant(
    claims: &AssertionClaims,
    policy: &HostPolicy,
    headers: &http::HeaderMap,
) -> Result<ExternalGrant, String> {
    use crate::auth::THREAD_GRANT_REASON_PREFIX as P;
    if !policy.sharing {
        return Err(format!("{P}this computer stopped sharing"));
    }
    if !policy.trust_relay_allowlist {
        return Err(format!(
            "{P}this computer takes no Space thread shares from the relay (it trusts only its \
             own allowlist)"
        ));
    }
    let space = claims
        .space
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{P}it names no Space"))?;
    let role = match claims.space_role.as_deref() {
        Some("viewer") => "viewer",
        Some("shared") => "shared",
        _ => return Err(format!("{P}it names no role")),
    };
    let client = crate::auth::principal_from_headers(headers);
    Ok(ExternalGrant {
        // A thread grant that may post ("shared") may present its own agent;
        // a viewer is always a person (CUA-1750's rule for view-only callers).
        principal: principal_for(claims, client.as_ref(), role == "shared"),
        view_only: false,
        host_only: true,
        account: Some(crate::auth::RelayCaller {
            account: claims.acct.clone(),
            email: claims.email.clone(),
            name: claims.name.clone(),
            role: role.into(),
            issued_at: claims.iat,
            teams: claims.teams.clone(),
            space: Some(space.to_owned()),
            phone: None,
            device: None,
        }),
    })
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
            teams: Vec::new(),
            space: None,
            space_role: None,
            dev: None,
            dcls: None,
        }
    }

    fn agent(id: &str, name: &str) -> Principal {
        Principal {
            id: id.into(),
            display_name: name.into(),
            color: String::new(),
            kind: PrincipalKind::Agent as i32,
        }
    }

    /// CUA-1750: an editor's agent is an agent attributed to its account; a
    /// viewer's never is; the client never picks the account part.
    #[test]
    fn editors_may_act_as_their_own_agents() {
        let bob = claims("bob", "shared", Some("bob@x.io"));
        let p = principal_for(
            &bob,
            Some(&agent("agent:claude-code-42", "Claude Code")),
            true,
        );
        assert_eq!(p.kind, PrincipalKind::Agent as i32);
        assert_eq!(p.id, "bob/agent:claude-code-42");
        assert_eq!(p.display_name, "Claude Code (User bob)");

        // View-only: always the human account.
        let p = principal_for(&bob, Some(&agent("agent:x", "X")), false);
        assert_eq!(p.kind, PrincipalKind::Human as i32);
        assert_eq!(p.id, "bob");
        assert_eq!(p.display_name, "User bob");

        // The client can't smuggle another identity or markup into the id.
        let p = principal_for(&bob, Some(&agent("../ada/ <b>\n", "")), true);
        assert_eq!(p.id, "bob/adab");
        assert_eq!(p.display_name, "Agent (User bob)");
        let p = principal_for(&bob, Some(&agent("", &"n".repeat(500))), true);
        assert_eq!(p.id, "bob/agent");
        assert_eq!(p.display_name.len(), 40 + " (User bob)".len());

        // A human principal stays the account, owner or not.
        let human = Principal {
            id: "spoofed".into(),
            kind: PrincipalKind::Human as i32,
            ..Principal::default()
        };
        let p = principal_for(&claims("ada", "owner", None), Some(&human), true);
        assert_eq!(
            (p.id.as_str(), p.kind),
            ("ada", PrincipalKind::Human as i32)
        );
        assert_eq!(principal_for(&bob, None, true).id, "bob");
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
    fn teams_reach_the_caller_and_the_policy_names_the_share_gateway() {
        let key = RelayKey::generate();
        let link = Arc::new(AccountLink::default());
        link.keys.set_jwks_json(&key.jwks().to_string()).unwrap();
        *link.owner.write().unwrap() = Some("ada".into());
        let dir = tempfile::tempdir().unwrap();
        let policy = dir.path().join("host.json");
        std::fs::write(&policy, r#"{"owner":"ada","allow":["bob"]}"#).unwrap();
        let auth = RelayAssertionAuth::new("machine-0001".into(), link, Some(policy.clone()));
        // Shares are off until the policy turns them on and names a socket.
        assert_eq!(auth.share_gateway(), Ok(None));
        std::fs::write(&policy, r#"{"owner":"ada","shares":true}"#).unwrap();
        assert_eq!(auth.share_gateway(), Ok(None), "on, but no gateway");
        std::fs::write(
            &policy,
            r#"{"owner":"ada","allow":["bob"],"shares":true,"shares_gateway":"/run/g.sock"}"#,
        )
        .unwrap();
        assert_eq!(auth.share_gateway(), Ok(Some(PathBuf::from("/run/g.sock"))));
        let mut headers = http::HeaderMap::new();
        let mut c = claims("bob", "shared", None);
        c.teams = vec!["team-a".into(), "team-b".into()];
        headers.insert(ASSERTION_HEADER, key.sign(&c).parse().unwrap());
        let g = auth.authenticate(&headers).unwrap().unwrap();
        let who = g.account.unwrap();
        assert_eq!(
            (who.role.as_str(), who.teams),
            ("shared", vec!["team-a".to_string(), "team-b".to_string()])
        );
        // Stopped sharing: no gateway, whatever else the policy says.
        std::fs::write(
            &policy,
            r#"{"owner":"ada","sharing":false,"shares":true,"shares_gateway":"/run/g.sock"}"#,
        )
        .unwrap();
        assert!(auth.share_gateway().is_err());
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

    fn phone_claims(acct: &str, device: &str) -> AssertionClaims {
        let mut c = claims(acct, "owner", None);
        c.dev = Some(device.into());
        c.dcls = Some(cua_relay::phone::CLASS_MOBILE.into());
        c
    }

    struct PhoneRig {
        key: RelayKey,
        auth: RelayAssertionAuth,
        store: Arc<crate::phone_control::PhoneControl>,
        home: tempfile::TempDir,
        _dir: tempfile::TempDir,
    }

    /// A machine of `ada` whose policy names a cua home; `on` writes that
    /// home's `mobile` switch; `phones` lets it answer phones at all.
    fn phone_rig(on: bool, phones: bool) -> PhoneRig {
        let key = RelayKey::generate();
        let link = Arc::new(AccountLink::default());
        link.keys.set_jwks_json(&key.jwks().to_string()).unwrap();
        *link.owner.write().unwrap() = Some("ada".into());
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        if on {
            std::fs::create_dir_all(home.path().join("experiments")).unwrap();
            std::fs::write(home.path().join("experiments/mobile"), "on\n").unwrap();
        }
        let policy = dir.path().join("host.json");
        std::fs::write(
            &policy,
            serde_json::json!({"owner": "ada", "spaces_daemon": {"socket": "/x", "cua_home": home.path()}})
                .to_string(),
        )
        .unwrap();
        let store = Arc::new(crate::phone_control::PhoneControl::in_data_dir(dir.path()));
        let mut auth = RelayAssertionAuth::new("machine-0001".into(), link, Some(policy));
        if phones {
            auth = auth.with_phone(store.clone());
        }
        PhoneRig {
            key,
            auth,
            store,
            home,
            _dir: dir,
        }
    }

    fn call(rig: &PhoneRig, c: &AssertionClaims) -> Result<ExternalGrant, String> {
        let mut headers = http::HeaderMap::new();
        headers.insert(ASSERTION_HEADER, rig.key.sign(c).parse().unwrap());
        rig.auth.authenticate(&headers).unwrap()
    }

    /// Nothing about phones works until the owner turned the experiment on,
    /// and a machine started without phone support never answers one.
    #[test]
    fn a_phone_is_answered_only_while_the_mobile_experiment_is_on() {
        if std::env::var_os("CUA_EXPERIMENTS").is_some() {
            return; // the environment would decide, not the switch under test
        }
        let off = phone_rig(false, true);
        let e = call(&off, &phone_claims("ada", "dev_1")).unwrap_err();
        assert!(e.contains("the mobile experiment is off"), "{e}");
        // A computer is untouched by it.
        assert!(call(&off, &claims("ada", "owner", None)).is_ok());

        let none = phone_rig(true, false);
        let e = call(&none, &phone_claims("ada", "dev_1")).unwrap_err();
        assert_eq!(e, "this computer does not answer phones");

        let on = phone_rig(true, true);
        let g = call(&on, &phone_claims("ada", "dev_1")).unwrap();
        let p = g.account.as_ref().unwrap().phone.clone().unwrap();
        assert_eq!(p.device, "dev_1");
        // A phone without a device, and a kind nobody knows, are refused.
        let mut nameless = phone_claims("ada", "");
        nameless.dev = None;
        assert_eq!(
            call(&on, &nameless).unwrap_err(),
            "a phone's call does not say which phone it is"
        );
        let mut odd = phone_claims("ada", "dev_1");
        odd.dcls = Some("watch".into());
        assert!(call(&on, &odd)
            .unwrap_err()
            .contains("does not know this kind"));
        // A computer carries no phone state.
        let g = call(&on, &claims("ada", "owner", None)).unwrap();
        assert!(g.account.unwrap().phone.is_none());
    }

    /// The kill switch is read on every call: while it is on, no phone is
    /// answered, and turning it off brings them back at the next call.
    #[test]
    fn the_phones_switch_is_read_on_every_call() {
        if std::env::var_os("CUA_EXPERIMENTS").is_some() {
            return;
        }
        let rig = phone_rig(true, true);
        let c = phone_claims("ada", "dev_1");
        assert!(call(&rig, &c).is_ok());
        rig.store.set_off(true).unwrap();
        assert_eq!(
            call(&rig, &c).unwrap_err(),
            "phones are turned off on this computer"
        );
        // A computer is untouched by it.
        assert!(call(&rig, &claims("ada", "owner", None)).is_ok());
        rig.store.set_off(false).unwrap();
        assert!(call(&rig, &c).is_ok());
        // Another account's phone is not answered on this computer at all.
        let mut bob = phone_claims("bob", "dev_1");
        bob.role = "owner".into();
        assert!(call(&rig, &bob).is_err(), "bob is not on this machine");
        let _ = &rig.home;
    }

    /// A phone's running call is allowed only while this computer still
    /// shares, still shares its desktop (for the picture and the input),
    /// still answers phones.
    #[test]
    fn a_running_phone_call_stops_when_the_computer_stops_sharing_its_desktop() {
        if std::env::var_os("CUA_EXPERIMENTS").is_some() {
            return;
        }
        let rig = phone_rig(true, true);
        let caller = call(&rig, &phone_claims("ada", "dev_1"))
            .unwrap()
            .account
            .unwrap();
        let open = "/cua.env.v1.StreamService/OpenMedia";
        let thread = "/cua.env.v1.HostThreadService/CallThreadTool";
        assert!(rig.auth.phone_still_allowed(&caller, open).is_ok());
        let write = |v: serde_json::Value| {
            std::fs::write(
                rig._dir.path().join("host.json"),
                serde_json::json!({"owner": "ada", "spaces_daemon": {"socket": "/x", "cua_home": rig.home.path()}})
                    .as_object()
                    .cloned()
                    .map(|mut m| {
                        m.extend(v.as_object().unwrap().clone());
                        serde_json::Value::Object(m)
                    })
                    .unwrap()
                    .to_string(),
            )
            .unwrap();
        };
        write(serde_json::json!({"share_desktop": false}));
        assert_eq!(
            rig.auth.phone_still_allowed(&caller, open).unwrap_err(),
            "this computer stopped sharing its desktop"
        );
        assert!(
            rig.auth.phone_still_allowed(&caller, thread).is_ok(),
            "the thread of a Space it provides goes on"
        );
        write(serde_json::json!({"sharing": false}));
        assert_eq!(
            rig.auth.phone_still_allowed(&caller, thread).unwrap_err(),
            "this computer stopped sharing"
        );
        write(serde_json::json!({}));
        rig.store.set_off(true).unwrap();
        assert_eq!(
            rig.auth.phone_still_allowed(&caller, thread).unwrap_err(),
            "phones are turned off on this computer"
        );
    }
}
