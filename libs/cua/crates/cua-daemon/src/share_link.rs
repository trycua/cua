//! Where a share's link points: this machine's entry on the cua.ai relay.

use std::path::Path;
use std::sync::Arc;

use cua_host::{AccountTokens, Host, Machine, RelayClient};
use cua_share::Access;

use crate::runtime::Runtime;
use crate::{Error, Result};

/// The relay account and this machine's relay entry.
pub struct LinkContext {
    pub(crate) client: RelayClient,
    tokens: Arc<dyn AccountTokens>,
    account: cua_spaces::RelayAccount,
    machine: Machine,
    pub(crate) owner_account: String,
    pub(crate) owner_email: Option<String>,
}

/// Link of a share served through `machine_url` (what the relay directory
/// lists for the machine) for machine `machine_id`.
///
/// - Host mode (`https://<id>.<domain>`): each share gets its own origin,
///   `https://<s|p><key>-<id prefix>.<domain>/`, so a shared app's root-relative
///   links work and one share's cookies never reach another.
/// - Path mode (`<relay>/m/<id>`): `<url>/share/<key>/` (and `/share/pub/<key>/`).
pub fn compose_link(machine_url: &str, machine_id: &str, key: &str, access: Access) -> String {
    let kind = if access == Access::Public { 'p' } else { 's' };
    if let Ok(u) = url::Url::parse(machine_url)
        && !u.path().trim_matches('/').starts_with("m/")
        && let Some(host) = u.host_str()
        && let Some(domain) = host.strip_prefix(&format!("{machine_id}."))
    {
        let id_prefix: String = machine_id.chars().take(16).collect();
        let mut out = u.clone();
        let _ = out.set_host(Some(&format!("{kind}{key}-{id_prefix}.{domain}")));
        out.set_path("/");
        return out.to_string();
    }
    let base = machine_url.trim_end_matches('/');
    match access {
        Access::Public => format!("{base}/share/pub/{key}/"),
        _ => format!("{base}/share/{key}/"),
    }
}

impl LinkContext {
    /// Resolves the relay account and this machine's entry, and makes sure
    /// the host forwards `/share/...` to the daemon.
    pub async fn resolve(rt: &Runtime, home: &Path) -> Result<Self> {
        let account = rt.spaces().relay_account().ok_or_else(|| {
            Error::Unsupported(
                "not signed in to cua.ai: run `cua auth login`, then `cua host setup` so this machine can serve links".into(),
            )
        })?;
        let host = Host::new(home);
        let config = host
            .config()
            .map_err(|e| Error::Internal(e.to_string()))?
            .ok_or_else(|| {
                Error::Unsupported(
                    "this machine is not on your cua.ai relay, so a link would only open here. Run `cua host setup` once, then share again".into(),
                )
            })?;
        let machine_id = config.machine_id.clone().filter(|_| config.mode == "relay").ok_or_else(|| {
            Error::Unsupported(
                "this machine is not on your cua.ai relay (direct mode), so a link would only open here. Run `cua host setup`".into(),
            )
        })?;
        host.enable_shares()
            .map_err(|e| Error::Unsupported(e.to_string()))?;
        let tokens = account.tokens.clone();
        let token = tokens
            .access_token()
            .await
            .map_err(|e| Error::Unsupported(format!("not signed in to cua.ai: {e}")))?;
        let client = account
            .client()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        let machine = client
            .machine(&token, &machine_id)
            .await
            .map_err(|e| Error::Internal(format!("the relay does not know this machine: {e}")))?;
        if machine.role != "owner" {
            return Err(Error::Unsupported(
                "only the machine's owner can share links from it".into(),
            ));
        }
        if !machine.online {
            return Err(Error::Unsupported(
                "this machine's host service is not connected to the relay; check `cua host status`".into(),
            ));
        }
        if !machine.sharing {
            return Err(Error::Unsupported(
                "this machine stopped sharing (`cua host start` resumes it)".into(),
            ));
        }
        Ok(Self {
            owner_account: machine.owner.id.clone(),
            owner_email: machine.owner.email.clone().filter(|e| !e.is_empty()),
            client,
            tokens,
            account,
            machine,
        })
    }

    /// The link for a share with `key`.
    pub fn link(&self, key: &str, access: Access) -> String {
        compose_link(&self.machine.url, &self.machine.id, key, access)
    }

    /// A fresh account access token.
    pub async fn access_token(&self) -> Result<String> {
        self.tokens
            .access_token()
            .await
            .map_err(|e| Error::Unsupported(format!("not signed in to cua.ai: {e}")))
    }

    /// Ids of the owner's teams that have this machine as a team machine: a
    /// team share admits their members, and only they can reach the machine
    /// through the relay at all.
    pub async fn owner_teams(&self) -> Result<Vec<String>> {
        let token = self.access_token().await?;
        let api = self
            .account
            .spaces_api()
            .map_err(|e| Error::Unsupported(e.to_string()))?;
        let snap = api
            .teams(&token)
            .await
            .map_err(|e| Error::Unsupported(format!("could not read your teams: {e}")))?;
        if snap.teams.is_empty() {
            return Err(Error::Unsupported(
                "you are not in a team; --team shares a link with your team's members".into(),
            ));
        }
        let mut ids = Vec::new();
        for t in &snap.teams {
            let machines = api
                .org_machines(&token, &t.org.id)
                .await
                .unwrap_or_default();
            if machines.iter().any(|m| m.machine_id == self.machine.id) {
                ids.push(t.org.id.clone());
            }
        }
        if ids.is_empty() {
            let team = snap
                .teams
                .first()
                .map(|t| t.org.label().to_string())
                .unwrap_or_default();
            return Err(Error::Unsupported(format!(
                "this machine is not a team machine, so your team cannot reach it. Add it with: cua team add-machine {team} relay:{}",
                self.machine.id
            )));
        }
        Ok(ids)
    }

    /// Who the audit names.
    pub fn principal(&self) -> String {
        self.owner_email
            .clone()
            .unwrap_or_else(|| format!("account:{}", self.owner_account))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn host_mode_gives_each_share_its_own_origin() {
        let u = compose_link(
            &format!("https://{ID}.m.relay.cua.ai"),
            ID,
            "aaaa1111bbbb2222",
            Access::Private,
        );
        assert_eq!(
            u,
            "https://saaaa1111bbbb2222-0123456789abcdef.m.relay.cua.ai/"
        );
        let token = "t".repeat(40);
        let p = compose_link(
            &format!("https://{ID}.m.relay.cua.ai"),
            ID,
            &token,
            Access::Public,
        );
        assert_eq!(
            p,
            format!("https://p{token}-0123456789abcdef.m.relay.cua.ai/")
        );
        let label = p.trim_start_matches("https://").split('.').next().unwrap();
        assert!(label.len() <= 63, "{label}");
    }

    #[test]
    fn path_mode_keeps_the_machine_path() {
        let u = compose_link(
            &format!("https://relay.example.com/m/{ID}"),
            ID,
            "aaaa",
            Access::Team,
        );
        assert_eq!(u, format!("https://relay.example.com/m/{ID}/share/aaaa/"));
        let p = compose_link(
            &format!("http://127.0.0.1:8080/m/{ID}"),
            ID,
            "tok",
            Access::Public,
        );
        assert_eq!(p, format!("http://127.0.0.1:8080/m/{ID}/share/pub/tok/"));
    }
}
