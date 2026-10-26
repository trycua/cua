//! Space ids: the unified sandbox ref scheme (a Space is a sandbox with
//! extra features), with no infrastructure terms leaking into it.
//!
//! ```text
//! local:<name>
//! cloud:<name>
//! direct:<host:port>
//! relay:<machine-id>
//! ```
//!
//! Legacy spellings still parse (`space://{cloud,local,direct,relay}/…`,
//! `local:<vm>`, a URL), so ids stored by older clients keep working;
//! output always uses the new form.
//! See [`cua_sandbox_core::refs`].

use crate::error::{Error, Result};
use cua_sandbox_core::SandboxRef;
use std::fmt;

/// Which provider backs a Space: the `location` vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// A Cua cloud sandbox (Cua Cloud has ended: it only parses).
    Cloud,
    /// A sandbox on this machine's local runtime.
    Local,
    /// Any reachable spacesd, added by address.
    Direct,
    /// A machine of the signed-in account on a cua-relay
    /// (`<relay>/m/<machine-id>`), listed from the relay's directory.
    Relay,
}

impl Provider {
    /// `cloud`, `local`, `direct` or `relay`.
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Cloud => "cloud",
            Provider::Local => "local",
            Provider::Direct => "direct",
            Provider::Relay => "relay",
        }
    }

    /// Parses a location word.
    pub fn parse(s: &str) -> Option<Self> {
        match cua_sandbox_core::Location::parse(s)? {
            cua_sandbox_core::Location::Cloud => Some(Provider::Cloud),
            cua_sandbox_core::Location::Local => Some(Provider::Local),
            cua_sandbox_core::Location::Direct => Some(Provider::Direct),
            cua_sandbox_core::Location::Relay => Some(Provider::Relay),
            // Contrib sandboxes are not Space ids (yet): attach to their
            // cua-spacesd through the sandbox API.
            cua_sandbox_core::Location::Contrib => None,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parsed Space id.
///
/// Equality and hashing use the canonical form.
#[derive(Clone, Debug)]
pub enum SpaceId {
    /// `cloud:<name>`: a Cua cloud sandbox. Cua Cloud has ended, so using
    /// it fails with [`cua_sandbox_core::CLOUD_ENDED`].
    Cloud {
        /// Sandbox name.
        name: String,
    },
    /// `local:<name>`.
    Local {
        /// Local sandbox name.
        name: String,
    },
    /// `direct:<host:port>`.
    Direct {
        /// `host:port` (IPv6 hosts bracketed).
        authority: String,
    },
    /// `relay:<machine-id>`.
    Relay {
        /// Machine id registered with the relay (`[a-z0-9-]{8,64}`).
        machine_id: String,
    },
}

impl PartialEq for SpaceId {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for SpaceId {}

impl std::hash::Hash for SpaceId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_string().hash(state)
    }
}

impl SpaceId {
    /// Parses a qualified ref or a legacy spelling (never a bare name).
    pub fn parse(input: &str) -> Result<Self> {
        let r = SandboxRef::parse(input)
            .map_err(|_| Error::invalid(format!("not a Space id: {input:?}")))?;
        Self::from_ref(r).ok_or_else(|| Error::invalid(format!("not a Space id: {input:?}")))
    }

    /// The Space id of a qualified sandbox ref (`None` for a bare name).
    pub fn from_ref(r: SandboxRef) -> Option<Self> {
        Some(match r {
            SandboxRef::Cloud { name } => SpaceId::Cloud { name },
            SandboxRef::Local { name } => SpaceId::Local { name },
            SandboxRef::Direct { authority } => SpaceId::Direct { authority },
            SandboxRef::Relay { machine_id } => SpaceId::Relay { machine_id },
            SandboxRef::Contrib { .. } | SandboxRef::Bare { .. } => return None,
        })
    }

    /// The same id as a sandbox ref.
    pub fn to_ref(&self) -> SandboxRef {
        match self.clone() {
            SpaceId::Cloud { name } => SandboxRef::Cloud { name },
            SpaceId::Local { name } => SandboxRef::Local { name },
            SpaceId::Direct { authority } => SandboxRef::Direct { authority },
            SpaceId::Relay { machine_id } => SandboxRef::Relay { machine_id },
        }
    }

    /// The provider.
    pub fn provider(&self) -> Provider {
        match self {
            SpaceId::Cloud { .. } => Provider::Cloud,
            SpaceId::Local { .. } => Provider::Local,
            SpaceId::Direct { .. } => Provider::Direct,
            SpaceId::Relay { .. } => Provider::Relay,
        }
    }

    /// A short human label: the cloud or local name, `host:port` or the
    /// machine id.
    pub fn short_name(&self) -> &str {
        match self {
            SpaceId::Cloud { name } => name,
            SpaceId::Local { name } => name,
            SpaceId::Direct { authority } => authority,
            SpaceId::Relay { machine_id } => machine_id,
        }
    }
}

impl fmt::Display for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider(), self.short_name())
    }
}

impl std::str::FromStr for SpaceId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        SpaceId::parse(s)
    }
}

/// Canonicalizes a stored id (legacy spellings to the new form); anything
/// that is not a Space id comes back unchanged.
pub fn canonical_id(id: &str) -> String {
    SpaceId::parse(id).map_or_else(|_| id.to_string(), |i| i.to_string())
}

/// `host:port`, bracketing an IPv6 literal.
pub(crate) fn authority(host: &str, port: u16) -> String {
    cua_sandbox_core::refs::authority(host, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_provider() {
        for id in [
            "cloud:claim-1",
            "local:space-ab12",
            "direct:10.0.0.5:3211",
            "direct:[::1]:3211",
            "relay:0123abcd4567ef89",
        ] {
            assert_eq!(SpaceId::parse(id).unwrap().to_string(), id);
        }
    }

    #[test]
    fn legacy_ids_parse_to_the_new_form() {
        for (legacy, new) in [
            ("space://cloud/claim-1", "cloud:claim-1"),
            ("space://local/space-ab12", "local:space-ab12"),
            ("space://direct/10.0.0.5:3211", "direct:10.0.0.5:3211"),
            ("space://direct/[::1]:3211", "direct:[::1]:3211"),
            ("space://relay/0123abcd4567ef89", "relay:0123abcd4567ef89"),
            ("local:cua-space-1", "local:cua-space-1"),
        ] {
            assert_eq!(SpaceId::parse(legacy).unwrap().to_string(), new, "{legacy}");
            assert_eq!(canonical_id(legacy), new);
        }
        assert_eq!(
            SpaceId::parse("space://cloud/c").unwrap(),
            SpaceId::parse("cloud:c").unwrap()
        );
    }

    #[test]
    fn a_url_is_a_direct_space_with_the_default_port() {
        assert_eq!(
            SpaceId::parse("http://127.0.0.1").unwrap().to_string(),
            "direct:127.0.0.1:80"
        );
        assert_eq!(
            SpaceId::parse("http://127.0.0.1:4000/")
                .unwrap()
                .to_string(),
            "direct:127.0.0.1:4000"
        );
        assert_eq!(
            SpaceId::parse("https://h.example").unwrap().to_string(),
            "direct:h.example:443"
        );
    }

    #[test]
    fn providers_use_the_location_words() {
        assert_eq!(Provider::Cloud.as_str(), "cloud");
        assert_eq!(Provider::parse("cloud"), Some(Provider::Cloud));
        let p: Provider = serde_json::from_str("\"cloud\"").unwrap();
        assert_eq!(p, Provider::Cloud);
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"cloud\"");
    }

    #[test]
    fn refuses_garbage() {
        for bad in [
            "",
            "space://",
            "space://cloud/a/b",
            "space://moon/x",
            "space://relay/short",
            "space://relay/UPPERCASE1",
            "space://relay/a/b-c-d-e-f",
            "local:",
            "claim-1",
        ] {
            assert!(SpaceId::parse(bad).is_err(), "{bad:?}");
        }
    }
}

/// The host-name suffix of a routed Space's address: `direct:<host
/// machine>.via.cua:<port>` is a Space that host provides, reached through
/// the host's own channel (a loopback forward there) instead of its own
/// relay machine or a LAN address.
pub const ROUTED_SUFFIX: &str = ".via.cua";

/// The address of the Space host `machine` forwards on its loopback `port`.
pub fn routed_authority(machine: &str, port: u16) -> String {
    format!("{machine}{ROUTED_SUFFIX}:{port}")
}

/// `(host machine, port)` of a routed Space's authority.
pub fn routed_host(authority: &str) -> Option<(&str, u16)> {
    let (host, port) = authority.rsplit_once(':')?;
    let machine = host.strip_suffix(ROUTED_SUFFIX)?;
    if machine.is_empty() {
        return None;
    }
    Some((machine, port.parse().ok().filter(|p: &u16| *p > 0)?))
}

#[cfg(test)]
mod routed_tests {
    use super::*;

    #[test]
    fn routed_spaces_round_trip_as_direct_ids() {
        let a = routed_authority("m-4f2a9c", 40123);
        assert_eq!(a, "m-4f2a9c.via.cua:40123");
        assert_eq!(routed_host(&a), Some(("m-4f2a9c", 40123)));
        let id = SpaceId::parse(&format!("direct:{a}")).unwrap();
        assert!(matches!(&id, SpaceId::Direct { authority } if routed_host(authority).is_some()));
        assert_eq!(id.to_string(), format!("direct:{a}"));
        assert_eq!(routed_host("10.0.0.2:3211"), None);
        assert_eq!(routed_host(".via.cua:1"), None);
        assert_eq!(routed_host("m.via.cua:0"), None);
    }
}
