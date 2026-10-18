//! Sandbox references: one scheme for sandboxes and Spaces (a Space is a
//! sandbox with extra features).
//!
//! ```text
//! local:<name>          a sandbox on this machine
//! cloud:<name>          a Cua cloud (Fleet) sandbox; pool and namespace are
//!                       resolved internally and never shown
//! direct:<host:port>    a machine reached by address (IPv6 bracketed)
//! relay:<machine-id>    a machine of the account, reached via the cua.ai relay
//! <provider>:<name>     a sandbox on a contrib provider (`e2b:<name>`,
//!                       `daytona:<name>`, ...; see `CONTRIB_LOCATIONS`)
//! <name>                a bare name, searched across every location
//! ```
//!
//! A bare name must be unique across locations; [`resolve`] fails with
//! [`Error::AmbiguousSandbox`] (listing the qualified candidates) when it is
//! not. Output always uses the qualified form, and ids round-trip through
//! [`SandboxRef::parse`].
//!
//! Legacy spellings still parse: `space://{fleet,local,direct,relay}/…`,
//! `fleet:<namespace>:<claim>`, and `http(s)://host[:port]` (direct).

use crate::{Error, Result};
use std::fmt;

/// The spacesd port a `direct:<host>` without a port means.
const DEFAULT_DIRECT_PORT: u16 = 3211;

/// Where a sandbox runs: the `location` vocabulary of every surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Location {
    /// This machine's local runtime (a container, QEMU or Lume).
    Local,
    /// Cua cloud (Fleet).
    Cloud,
    /// A machine reached by address.
    Direct,
    /// A machine of the account, reached through the cua.ai relay.
    Relay,
    /// A third-party platform (a contrib provider); the ref names which.
    Contrib,
}

impl Location {
    /// Every location, in display order.
    pub const ALL: [Location; 4] = [
        Location::Local,
        Location::Cloud,
        Location::Direct,
        Location::Relay,
    ];

    /// `local`, `cloud`, `direct` or `relay`.
    pub fn as_str(self) -> &'static str {
        match self {
            Location::Local => "local",
            Location::Cloud => "cloud",
            Location::Direct => "direct",
            Location::Relay => "relay",
            Location::Contrib => "contrib",
        }
    }

    /// Parses a location word; `fleet` is the legacy spelling of `cloud`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Location::Local),
            "cloud" | "fleet" => Some(Location::Cloud),
            "direct" => Some(Location::Direct),
            "relay" => Some(Location::Relay),
            _ => None,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Location {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Location::parse(s).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "unknown location {s:?} (local, cloud, direct or relay)"
            ))
        })
    }
}

/// A parsed sandbox reference.
///
/// Equality, hashing and [`Display`](fmt::Display) use the canonical form:
/// the namespace hint a legacy cloud spelling carries is never shown and
/// never compared.
#[derive(Clone, Debug)]
pub enum SandboxRef {
    /// `local:<name>`.
    Local {
        /// Local sandbox name.
        name: String,
    },
    /// `cloud:<name>`: the claim name, unique within the account.
    Cloud {
        /// Claim name.
        name: String,
        /// Where a legacy id (`fleet:<ns>:<claim>`,
        /// `space://fleet/<ns>/<claim>`) said the claim lives. A lookup
        /// hint only.
        namespace: Option<String>,
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
    /// `<provider>:<name>`: a sandbox on a contrib provider.
    Contrib {
        /// The provider's location word (`e2b`).
        provider: String,
        /// Sandbox name.
        name: String,
    },
    /// An unqualified name, searched across every location.
    Bare {
        /// Name.
        name: String,
    },
}

impl PartialEq for SandboxRef {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for SandboxRef {}

impl std::hash::Hash for SandboxRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_string().hash(state)
    }
}

/// Relay machine ids are DNS-label safe: `[a-z0-9-]{8,64}`.
pub fn valid_machine_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains(':')
        && !name.chars().any(char::is_whitespace)
}

/// `host:port`, bracketing an IPv6 literal.
pub fn authority(host: &str, port: u16) -> String {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Normalizes `host[:port]` (IPv6 bracketed, or a bare IPv6 literal) to
/// `host:port`; the port defaults to spacesd's 3211.
fn normalize_authority(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('/');
    if s.is_empty() || s.contains('/') || s.chars().any(char::is_whitespace) {
        return None;
    }
    // A bare IPv6 literal (`::1`) has no port.
    if !s.starts_with('[') && s.matches(':').count() > 1 {
        return s
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|_| authority(s, DEFAULT_DIRECT_PORT));
    }
    let url = url::Url::parse(&format!("http://{s}")).ok()?;
    let host = url.host_str()?;
    if url.path() != "/" || url.query().is_some() || !url.username().is_empty() {
        return None;
    }
    Some(authority(host, url.port().unwrap_or(DEFAULT_DIRECT_PORT)))
}

impl SandboxRef {
    /// Parses a qualified ref, a legacy spelling, or a bare name.
    pub fn parse(input: &str) -> Result<Self> {
        let s = input.trim();
        let bad =
            |why: &str| Error::InvalidArgument(format!("not a sandbox ref: {input:?} ({why})"));
        if s.is_empty() {
            return Err(bad("empty"));
        }
        if let Some(rest) = s.strip_prefix("space://") {
            let (kind, tail) = rest
                .split_once('/')
                .ok_or_else(|| bad("space://<location>/<id>"))?;
            let tail = tail.trim_end_matches('/');
            return match kind {
                "fleet" | "cloud" => match tail.split_once('/') {
                    Some((namespace, claim)) if !namespace.is_empty() && valid_name(claim) => {
                        Ok(SandboxRef::Cloud {
                            name: claim.into(),
                            namespace: Some(namespace.into()),
                        })
                    }
                    None if valid_name(tail) => Ok(SandboxRef::Cloud {
                        name: tail.into(),
                        namespace: None,
                    }),
                    _ => Err(bad("space://fleet/<namespace>/<claim>")),
                },
                "local" if valid_name(tail) => Ok(SandboxRef::Local { name: tail.into() }),
                "direct" => normalize_authority(tail)
                    .map(|authority| SandboxRef::Direct { authority })
                    .ok_or_else(|| bad("space://direct/<host:port>")),
                "relay" if valid_machine_id(tail) => Ok(SandboxRef::Relay {
                    machine_id: tail.into(),
                }),
                "relay" => Err(bad("relay machine ids are [a-z0-9-]{8,64}")),
                _ => Err(bad("unknown location")),
            };
        }
        if s.starts_with("http://") || s.starts_with("https://") {
            let url = url::Url::parse(s).map_err(|_| bad("bad URL"))?;
            let host = url.host_str().ok_or_else(|| bad("URL without a host"))?;
            let port = url.port_or_known_default().unwrap_or(DEFAULT_DIRECT_PORT);
            return Ok(SandboxRef::Direct {
                authority: authority(host, port),
            });
        }
        let Some((kind, rest)) = s.split_once(':') else {
            return if valid_name(s) {
                Ok(SandboxRef::Bare { name: s.into() })
            } else {
                Err(bad("names have no '/' or spaces"))
            };
        };
        match kind {
            "local" if valid_name(rest) => Ok(SandboxRef::Local { name: rest.into() }),
            "local" => Err(bad("local:<name>")),
            "cloud" if valid_name(rest) => Ok(SandboxRef::Cloud {
                name: rest.into(),
                namespace: None,
            }),
            "cloud" => Err(bad("cloud:<name>")),
            // Legacy: `fleet:<namespace>:<claim>` (and `fleet:<claim>`).
            "fleet" => match rest.split_once(':') {
                Some((namespace, claim)) if !namespace.is_empty() && valid_name(claim) => {
                    Ok(SandboxRef::Cloud {
                        name: claim.into(),
                        namespace: Some(namespace.into()),
                    })
                }
                None if valid_name(rest) => Ok(SandboxRef::Cloud {
                    name: rest.into(),
                    namespace: None,
                }),
                _ => Err(bad("fleet:<namespace>:<claim>")),
            },
            "direct" | "url" => normalize_authority(rest)
                .map(|authority| SandboxRef::Direct { authority })
                .ok_or_else(|| bad("direct:<host:port>")),
            "relay" if valid_machine_id(rest) => Ok(SandboxRef::Relay {
                machine_id: rest.into(),
            }),
            "relay" => Err(bad("relay machine ids are [a-z0-9-]{8,64}")),
            p if crate::provider::is_provider_location(p) && valid_name(rest) => {
                Ok(SandboxRef::Contrib {
                    provider: p.into(),
                    name: rest.into(),
                })
            }
            p if crate::provider::is_provider_location(p) => Err(bad("<provider>:<name>")),
            _ => Err(bad(
                "prefix with local:, cloud:, direct:, relay: or a contrib provider (e2b:, ...)",
            )),
        }
    }

    /// A qualified ref for `name` at `location` (`direct`: `name` is
    /// `host[:port]`).
    pub fn qualified(location: Location, name: &str) -> Result<Self> {
        SandboxRef::parse(&format!("{location}:{name}"))
    }

    /// The location; `None` for a bare name.
    pub fn location(&self) -> Option<Location> {
        match self {
            SandboxRef::Local { .. } => Some(Location::Local),
            SandboxRef::Cloud { .. } => Some(Location::Cloud),
            SandboxRef::Direct { .. } => Some(Location::Direct),
            SandboxRef::Relay { .. } => Some(Location::Relay),
            SandboxRef::Contrib { .. } => Some(Location::Contrib),
            SandboxRef::Bare { .. } => None,
        }
    }

    /// The location word shown in the ref: `local`, `cloud`, `direct`,
    /// `relay` or the contrib provider (`e2b`); `None` for a bare name.
    pub fn location_word(&self) -> Option<&str> {
        match self {
            SandboxRef::Contrib { provider, .. } => Some(provider),
            other => other.location().map(Location::as_str),
        }
    }

    /// Whether the ref names a location.
    pub fn is_qualified(&self) -> bool {
        !matches!(self, SandboxRef::Bare { .. })
    }

    /// The part after the location: the name, `host:port` or machine id.
    pub fn name(&self) -> &str {
        match self {
            SandboxRef::Local { name }
            | SandboxRef::Cloud { name, .. }
            | SandboxRef::Contrib { name, .. }
            | SandboxRef::Bare { name } => name,
            SandboxRef::Direct { authority } => authority,
            SandboxRef::Relay { machine_id } => machine_id,
        }
    }

    /// The legacy namespace hint of a cloud ref.
    pub fn namespace_hint(&self) -> Option<&str> {
        match self {
            SandboxRef::Cloud { namespace, .. } => namespace.as_deref(),
            _ => None,
        }
    }

    /// Narrows a bare name to `location` (`--local`, `--cloud`, `local=`);
    /// a qualified ref at another location is an error.
    pub fn narrow(self, location: Option<Location>) -> Result<Self> {
        let Some(loc) = location else {
            return Ok(self);
        };
        match self.location() {
            None => SandboxRef::qualified(loc, self.name()),
            Some(l) if l == loc => Ok(self),
            Some(l) => Err(Error::InvalidArgument(format!(
                "{self} is a {l} sandbox, not {loc}"
            ))),
        }
    }
}

impl fmt::Display for SandboxRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SandboxRef::Bare { name } => f.write_str(name),
            other => write!(
                f,
                "{}:{}",
                other.location_word().unwrap_or_default(),
                other.name()
            ),
        }
    }
}

impl std::str::FromStr for SandboxRef {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        SandboxRef::parse(s)
    }
}

/// Canonicalizes any accepted spelling (legacy ids included); bare names
/// come back unchanged.
pub fn canonical(input: &str) -> Result<String> {
    Ok(SandboxRef::parse(input)?.to_string())
}

/// The message of [`Error::AmbiguousSandbox`].
pub fn ambiguous_message(name: &str, candidates: &[SandboxRef]) -> String {
    format!(
        "{name:?} names {} sandboxes; use one of: {}",
        candidates.len(),
        candidates
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Resolves `wanted` among `known` (every sandbox the caller can see, as
/// qualified refs). A qualified ref resolves to the known entry equal to
/// it (keeping that entry's hints), or to itself when none is known; a
/// bare name must match the name of exactly one known ref.
pub fn resolve(
    wanted: &SandboxRef,
    known: impl IntoIterator<Item = SandboxRef>,
) -> Result<SandboxRef> {
    resolve_named(wanted, known.into_iter().map(|k| (k.name().to_string(), k)))
}

/// [`resolve`] over `(display name, ref)` pairs: a bare name also matches
/// an entry's display name (a remembered `direct:<host:port>` connection
/// is found by the name it was saved under).
pub fn resolve_named(
    wanted: &SandboxRef,
    known: impl IntoIterator<Item = (String, SandboxRef)>,
) -> Result<SandboxRef> {
    let mut matches: Vec<SandboxRef> = Vec::new();
    for (label, k) in known {
        let hit = match wanted {
            SandboxRef::Bare { name } => k.is_qualified() && (k.name() == name || label == *name),
            w => *w == k,
        };
        if hit && !matches.contains(&k) {
            matches.push(k);
        }
    }
    match (wanted, matches.len()) {
        (_, 1) => Ok(matches.remove(0)),
        (SandboxRef::Bare { name }, 0) => Err(Error::NotFound(name.clone())),
        (SandboxRef::Bare { name }, _) => {
            matches.sort_by_key(|m| (m.location(), m.to_string()));
            Err(Error::AmbiguousSandbox {
                message: ambiguous_message(name, &matches),
                name: name.clone(),
                candidates: matches.iter().map(ToString::to_string).collect(),
            })
        }
        (w, _) => Ok(w.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> SandboxRef {
        SandboxRef::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn qualified_refs_round_trip() {
        for s in [
            "local:box",
            "cloud:sb-12ab34cd",
            "direct:10.0.0.5:3211",
            "direct:[::1]:3211",
            "direct:h.example:443",
            "relay:0123abcd4567ef89",
            "box",
        ] {
            assert_eq!(p(s).to_string(), s);
            assert_eq!(p(&p(s).to_string()), p(s));
        }
    }

    #[test]
    fn legacy_spellings_parse_to_the_new_form() {
        for (legacy, new) in [
            ("space://fleet/cua-spaces-x/claim-1", "cloud:claim-1"),
            ("space://local/space-ab12", "local:space-ab12"),
            ("space://direct/10.0.0.5:3211", "direct:10.0.0.5:3211"),
            ("space://direct/[::1]:3211", "direct:[::1]:3211"),
            ("space://relay/0123abcd4567ef89", "relay:0123abcd4567ef89"),
            ("fleet:ns:claim-1", "cloud:claim-1"),
            ("fleet:claim-1", "cloud:claim-1"),
            ("local:cua-space-1", "local:cua-space-1"),
            ("url:127.0.0.1:3211", "direct:127.0.0.1:3211"),
            ("http://127.0.0.1", "direct:127.0.0.1:80"),
            ("http://127.0.0.1:4000/", "direct:127.0.0.1:4000"),
            ("https://h.example", "direct:h.example:443"),
            ("direct:h", "direct:h:3211"),
            ("direct:::1", "direct:[::1]:3211"),
        ] {
            assert_eq!(p(legacy).to_string(), new, "{legacy}");
        }
        assert_eq!(
            p("space://fleet/ns/claim-1").namespace_hint(),
            Some("ns"),
            "the namespace stays a lookup hint"
        );
        assert_eq!(p("fleet:ns:c"), p("cloud:c"), "hints never compare");
    }

    #[test]
    fn refuses_garbage() {
        for bad in [
            "",
            "space://",
            "space://fleet/ns/a/b",
            "space://moon/x",
            "space://relay/short",
            "relay:UPPERCASE1",
            "relay:short",
            "local:",
            "cloud:",
            "cloud:a/b",
            "direct:",
            "direct:h:p0rt",
            "moon:x",
            "a b",
            "a/b",
        ] {
            assert!(SandboxRef::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn locations_parse_with_the_legacy_word() {
        for l in Location::ALL {
            assert_eq!(Location::parse(l.as_str()), Some(l));
        }
        assert_eq!(Location::parse("fleet"), Some(Location::Cloud));
        assert_eq!(Location::parse("moon"), None);
    }

    #[test]
    fn bare_names_resolve_uniquely_or_fail_typed() {
        let known = || vec![p("local:box"), p("cloud:box"), p("cloud:other")];
        let err = resolve(&p("box"), known()).unwrap_err();
        let Error::AmbiguousSandbox {
            name, candidates, ..
        } = &err
        else {
            panic!("{err:?}")
        };
        assert_eq!(name, "box");
        assert_eq!(candidates, &["local:box", "cloud:box"]);
        assert!(err.to_string().contains("local:box, cloud:box"), "{err}");
        assert_eq!(resolve(&p("other"), known()).unwrap(), p("cloud:other"));
        assert!(matches!(
            resolve(&p("nope"), known()),
            Err(Error::NotFound(_))
        ));
        // Narrowing (`--local`, `--cloud`, `local=`) picks one.
        let local = p("box").narrow(Some(Location::Local)).unwrap();
        assert_eq!(resolve(&local, known()).unwrap(), p("local:box"));
        let cloud = p("box").narrow(Some(Location::Cloud)).unwrap();
        assert_eq!(resolve(&cloud, known()).unwrap(), p("cloud:box"));
        assert!(p("cloud:box").narrow(Some(Location::Local)).is_err());
        // A qualified ref that nobody lists still resolves to itself (a
        // live lookup decides whether it exists).
        assert_eq!(resolve(&p("direct:h:1"), known()).unwrap(), p("direct:h:1"));
        // A display name finds its entry (a remembered direct address).
        assert_eq!(
            resolve_named(&p("dev"), vec![("dev".into(), p("direct:h:1"))]).unwrap(),
            p("direct:h:1")
        );
        // The known entry wins, with its hints.
        let hinted = p("fleet:ns:other");
        assert_eq!(
            resolve(&p("other"), vec![hinted]).unwrap().namespace_hint(),
            Some("ns")
        );
    }
}
