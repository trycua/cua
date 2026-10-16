//! Adding a machine by link or QR code.
//!
//! An enrolled device mints a one-time pairing code on the relay
//! ([`RelayClient::create_pairing_code`]) and shows it as a link
//! ([`add_machine_url`]) and QR code. The machine being added runs
//! `cua host setup --pair <code>`, which registers it with the code
//! ([`PairingCodeToken`]) instead of a signed-in account, so it needs no
//! browser sign-in. Codes are single use, expire (30 minutes unless asked
//! otherwise) and can be revoked ([`RelayClient::revoke_pairing_code`]).
//!
//! ```text
//! POST   /v1/pairing-codes        enrolled device  -> PairingCode (with `code`)
//! GET    /v1/pairing-codes        enrolled device  -> {"codes": [PairingCode]}
//! DELETE /v1/pairing-codes/{id}   enrolled device  -> 204
//! POST   /v1/machines             Bearer cpc_<code> -> Registration
//! ```

use crate::relay::{AccountTokens, DEFAULT_RELAY_URL, RelayClient};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// Prefix of a pairing code sent as a bearer token.
pub const PAIRING_TOKEN_PREFIX: &str = "cpc_";
/// Characters of a code.
pub const PAIRING_CODE_LEN: usize = 12;
/// Where an add-machine link opens in production.
pub const ADD_MACHINE_BASE_URL: &str = "https://cua.ai/add";

/// Where an add-machine link opens: `<Spaces site>/add`, the site of
/// `CUA_SPACES_SITE_URL` or `CUA_ENV` ([`ADD_MACHINE_BASE_URL`] in
/// production).
pub fn add_machine_base_url() -> String {
    format!("{}/add", cua_home::env::spaces_site_url())
}

const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A code in canonical form (12 upper-case Crockford base32 characters)
/// from what a person typed, pasted or scanned: any case, with or without
/// dashes and spaces, `I`/`L` read as `1` and `O` as `0`. An add-machine
/// link (`…/add/<code>`) works too. `None` when it cannot be a code.
pub fn normalize_pairing_code(input: &str) -> Option<String> {
    let s = input.trim();
    let s = match s.split_once("/add/") {
        Some((_, rest)) => rest.split(['?', '#', '/']).next().unwrap_or(""),
        None => s,
    };
    let s = s.strip_prefix(PAIRING_TOKEN_PREFIX).unwrap_or(s);
    let mut out = String::with_capacity(PAIRING_CODE_LEN);
    for c in s.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' => continue,
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        };
        if !ALPHABET.contains(c) {
            return None;
        }
        out.push(c);
    }
    (out.len() == PAIRING_CODE_LEN).then_some(out)
}

/// The relay an add-machine link names (`…/add/<code>?relay=<url>`), if any.
pub fn pairing_link_relay(input: &str) -> Option<String> {
    let url = url::Url::parse(input.trim()).ok()?;
    url.query_pairs()
        .find(|(k, _)| k == "relay")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `XXXX-XXXX-XXXX` (input that is not a code is returned as is).
pub fn display_pairing_code(code: &str) -> String {
    match normalize_pairing_code(code) {
        Some(c) => format!("{}-{}-{}", &c[..4], &c[4..8], &c[8..]),
        None => code.to_string(),
    }
}

/// The add-machine link for `code`: `<add_machine_base_url>/<code>`
/// (`https://cua.ai/add/<code>` in production), plus `?relay=<url>`
/// when the machine must join another relay than the production
/// [`DEFAULT_RELAY_URL`] (a staging relay is always named, since the
/// machine being added has no `CUA_ENV`).
pub fn add_machine_url(code: &str, relay: &str) -> String {
    let code = display_pairing_code(code);
    let relay = relay.trim().trim_end_matches('/');
    let base = add_machine_base_url();
    if relay.is_empty() || relay == DEFAULT_RELAY_URL {
        format!("{base}/{code}")
    } else {
        let mut url = url::Url::parse(&format!("{base}/{code}")).expect("static add-machine URL");
        url.query_pairs_mut().append_pair("relay", relay);
        url.to_string()
    }
}

/// The one-line command that installs `cua` and adds the machine:
/// `os` is `windows` for PowerShell, anything else for sh.
pub fn install_command(code: &str, relay: &str, os: &str) -> String {
    let code = display_pairing_code(code);
    let relay = relay.trim().trim_end_matches('/');
    let custom = !relay.is_empty() && relay != DEFAULT_RELAY_URL;
    if os.eq_ignore_ascii_case("windows") {
        let mut cmd =
            format!("& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Pair {code}");
        if custom {
            cmd.push_str(&format!(" -Relay {relay}"));
        }
        cmd
    } else {
        let mut cmd = format!("curl -fsSL https://cua.ai/install.sh | sh -s -- --pair {code}");
        if custom {
            cmd.push_str(&format!(" --relay {relay}"));
        }
        cmd
    }
}

/// A pairing code standing in for the account during `cua host setup`: the
/// relay registers the machine to the account that minted it.
pub struct PairingCodeToken(String);

impl PairingCodeToken {
    /// From what the person typed (see [`normalize_pairing_code`]).
    pub fn parse(input: &str) -> Result<Self> {
        normalize_pairing_code(input).map(Self).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "{:?} is not a pairing code (12 letters and digits, like K7QM-4XPB-2RHD)",
                input.trim()
            ))
        })
    }

    /// The canonical code.
    pub fn code(&self) -> &str {
        &self.0
    }
}

#[async_trait::async_trait]
impl AccountTokens for PairingCodeToken {
    async fn access_token(&self) -> Result<String> {
        Ok(format!("{PAIRING_TOKEN_PREFIX}{}", self.0))
    }
}

/// `POST /v1/pairing-codes` body.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingCodeRequest {
    /// Name for the machine (else the machine's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Team (`team-…`) the machine is added to; the caller must be its owner
    /// or an admin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// Lifetime in seconds (the relay clamps it to 60 s ..= 24 h; 30
    /// minutes when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
}

/// A pairing code of the account.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingCode {
    /// `pc_…`.
    pub id: String,
    /// `XXXX-XXXX-XXXX`: only in the reply that minted it.
    #[serde(default)]
    pub code: Option<String>,
    /// `active`, `used`, `revoked` or `expired`.
    #[serde(default)]
    pub state: String,
    /// Name for the machine.
    #[serde(default)]
    pub name: Option<String>,
    /// Team.
    #[serde(default)]
    pub team: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: u64,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: u64,
    /// When a machine used it.
    #[serde(default)]
    pub used_at: Option<u64>,
    /// The machine it added.
    #[serde(default)]
    pub machine: Option<String>,
}

#[derive(Deserialize)]
struct PairingCodeList {
    #[serde(default)]
    codes: Vec<PairingCode>,
}

/// A code for a team needs Teams on ([`cua_home::experimental`]).
fn check_pairing_team(req: &PairingCodeRequest, teams: bool) -> Result<()> {
    if req.team.is_some() && !teams {
        return Err(Error::InvalidArgument(format!(
            "{} A pairing code adds the machine to your account; leave out the team.",
            cua_home::experimental::TEAMS_UNAVAILABLE
        )));
    }
    Ok(())
}

impl RelayClient {
    /// Mints a one-time pairing code (from an enrolled device: needs
    /// [`RelayClient::with_device_session`]).
    pub async fn create_pairing_code(
        &self,
        account_token: &str,
        req: &PairingCodeRequest,
    ) -> Result<PairingCode> {
        check_pairing_team(req, cua_home::experimental::teams_enabled())?;
        self.send_json(
            reqwest::Method::POST,
            "/v1/pairing-codes",
            Some(req),
            account_token,
        )
        .await
    }

    /// The account's pairing codes, newest first (never the codes).
    pub async fn pairing_codes(&self, account_token: &str) -> Result<Vec<PairingCode>> {
        let list: PairingCodeList = self
            .send_json::<(), _>(
                reqwest::Method::GET,
                "/v1/pairing-codes",
                None,
                account_token,
            )
            .await?;
        Ok(list.codes)
    }

    /// Revokes an unused pairing code (`pc_…`).
    pub async fn revoke_pairing_code(&self, account_token: &str, id: &str) -> Result<()> {
        let valid = id
            .strip_prefix("pc_")
            .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric()));
        if !valid {
            return Err(Error::InvalidArgument(format!(
                "{id:?} is not a pairing code id (pc_…; see `cua host pair --list`)"
            )));
        }
        let _: serde_json::Value = self
            .send_json::<(), _>(
                reqwest::Method::DELETE,
                &format!("/v1/pairing-codes/{id}"),
                None,
                account_token,
            )
            .await?;
        Ok(())
    }
}

impl RelayClient {
    /// Waits (polling every `poll`) until the pairing code `code_id` adds a
    /// machine, then makes that machine a machine of the team `org` on the
    /// Spaces API (`POST /v1/orgs/{org}/machines`): pairing alone never puts
    /// a machine in a team. A plan bought a moment ago is waited for (see
    /// [`crate::spaces_api::SpacesApiClient::finishing_setup`]; `waiting`
    /// is called meanwhile). Gives up when the code is revoked or expires,
    /// or at `until` (Unix seconds). Returns the machine id.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_paired_machine_to_team(
        &self,
        api: &crate::spaces_api::SpacesApiClient,
        token: &str,
        code_id: &str,
        org: &str,
        until: u64,
        poll: std::time::Duration,
        waiting: impl FnMut(std::time::Duration),
    ) -> Result<String> {
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        };
        let (machine, name) = loop {
            let codes = self.pairing_codes(token).await?;
            let c = codes.into_iter().find(|c| c.id == code_id);
            match c {
                Some(c) if c.state == "used" && c.machine.is_some() => {
                    break (c.machine.unwrap_or_default(), c.name.unwrap_or_default());
                }
                Some(c) if c.state == "revoked" || c.state == "expired" => {
                    return Err(Error::InvalidArgument(format!(
                        "pairing code {code_id} was {} before a machine used it",
                        c.state
                    )));
                }
                None => {
                    return Err(Error::NotFound(format!("pairing code {code_id}")));
                }
                Some(_) if now() >= until => {
                    return Err(Error::InvalidArgument(format!(
                        "no machine used pairing code {code_id} in time"
                    )));
                }
                Some(_) => tokio::time::sleep(poll).await,
            }
        };
        if let Err(e) = api
            .finishing_setup_org(
                token,
                org,
                crate::spaces_api::SETUP_RETRY_WINDOW,
                waiting,
                || api.add_org_machine(token, org, &machine, "", &name),
            )
            .await
        {
            return Err(api.explain_plan_error(token, org, e).await);
        }
        Ok(machine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_team_pairing_code_needs_teams_on() {
        let team = PairingCodeRequest {
            team: Some("team-aigis".into()),
            ..Default::default()
        };
        let err = check_pairing_team(&team, false).unwrap_err().to_string();
        assert!(
            err.contains("Teams is turned off (CUA_EXPERIMENTAL_TEAMS=0)."),
            "{err}"
        );
        check_pairing_team(&team, true).unwrap();
        check_pairing_team(&PairingCodeRequest::default(), false).unwrap();
    }

    #[test]
    fn codes_read_the_way_people_type_them() {
        assert_eq!(
            normalize_pairing_code("k7qm-4xpb-2rhd").as_deref(),
            Some("K7QM4XPB2RHD")
        );
        assert_eq!(
            normalize_pairing_code("https://cua.ai/add/K7QM-4XPB-2RHD?relay=x").as_deref(),
            Some("K7QM4XPB2RHD")
        );
        // Links printed by older apps keep working.
        assert_eq!(
            normalize_pairing_code("https://spaces.cua.ai/add/K7QM-4XPB-2RHD?relay=x").as_deref(),
            Some("K7QM4XPB2RHD")
        );
        assert_eq!(
            normalize_pairing_code("cpc_OILO 0000 0000").as_deref(),
            Some("011000000000")
        );
        assert_eq!(normalize_pairing_code("K7QM-4XPB-2RH"), None);
        assert_eq!(normalize_pairing_code("K7QM-4XPB-2RHU"), None);
        match PairingCodeToken::parse("nope") {
            Err(Error::InvalidArgument(m)) => assert!(m.contains("is not a pairing code"), "{m}"),
            other => panic!("expected InvalidArgument, got {:?}", other.map(|t| t.0)),
        }
        assert_eq!(
            pairing_link_relay("https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example")
                .as_deref(),
            Some("https://r.example")
        );
        assert_eq!(pairing_link_relay("K7QM-4XPB-2RHD"), None);
        assert_eq!(display_pairing_code("k7qm4xpb2rhd"), "K7QM-4XPB-2RHD");
    }

    #[test]
    fn links_and_commands_carry_a_custom_relay_only() {
        assert_eq!(
            add_machine_url("k7qm4xpb2rhd", DEFAULT_RELAY_URL),
            "https://cua.ai/add/K7QM-4XPB-2RHD"
        );
        assert_eq!(
            add_machine_url("K7QM4XPB2RHD", "https://relay.example.com/"),
            "https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Frelay.example.com"
        );
        assert_eq!(
            install_command("K7QM4XPB2RHD", DEFAULT_RELAY_URL, "linux"),
            "curl -fsSL https://cua.ai/install.sh | sh -s -- --pair K7QM-4XPB-2RHD"
        );
        assert_eq!(
            install_command("K7QM4XPB2RHD", "https://r.example", "windows"),
            "& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Pair K7QM-4XPB-2RHD -Relay https://r.example"
        );
    }

    #[tokio::test]
    async fn revoking_refuses_ids_that_are_not_code_ids() {
        let client = RelayClient::new("http://127.0.0.1:9").unwrap();
        for id in ["../machines/m_123", "pc_", "pc_a/b", "x"] {
            match client.revoke_pairing_code("t", id).await {
                Err(Error::InvalidArgument(m)) => {
                    assert!(m.contains("not a pairing code id"), "{m}")
                }
                other => panic!("{id}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn the_code_is_the_bearer_for_registration() {
        let t = PairingCodeToken::parse("k7qm-4xpb-2rhd").unwrap();
        assert_eq!(t.access_token().await.unwrap(), "cpc_K7QM4XPB2RHD");
    }
}
