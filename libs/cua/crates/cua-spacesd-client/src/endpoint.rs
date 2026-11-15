//! Where a spacesd lives and how its URL is shaped.
//!
//! Accepted forms:
//! - `http://host:port`, `https://host:port` (optionally with a path prefix),
//! - a bare `host:port` or `host` (plain HTTP, default port 3211),
//! - a relay machine URL `https://relay.example/m/<machine-id>`.

use crate::error::{Error, Result};
use cua_proto::SPACESD_DEFAULT_PORT;
use std::fmt;
use url::Url;

/// How the endpoint is reached. Decides the default transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndpointKind {
    /// The spacesd listener itself (loopback, LAN, VM IP, container port).
    Direct,
    /// Through a `cua-relay` reverse tunnel (`/m/<machine-id>`).
    Relay {
        /// Machine id registered with the relay.
        machine_id: String,
    },
}

/// A parsed spacesd endpoint: an absolute base URL (scheme, authority and
/// an optional path prefix, never with a trailing slash) plus its kind.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    base: Url,
    kind: EndpointKind,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("base", &self.base.as_str())
            .field("kind", &self.kind)
            .finish()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.base.as_str())
    }
}

impl std::str::FromStr for Endpoint {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

impl Endpoint {
    /// Parses any of the accepted forms (see the module docs).
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Error::InvalidEndpoint("empty endpoint".into()));
        }
        let with_scheme = if input.contains("://") {
            input.to_string()
        } else {
            format!("http://{input}")
        };
        let mut url = Url::parse(&with_scheme)
            .map_err(|e| Error::InvalidEndpoint(format!("{input}: {e}")))?;
        match url.scheme() {
            "http" | "https" => {}
            "grpc" | "h2c" => {
                url = Url::parse(&with_scheme.replacen(url.scheme(), "http", 1))
                    .map_err(|e| Error::InvalidEndpoint(format!("{input}: {e}")))?;
            }
            "grpcs" => {
                url = Url::parse(&with_scheme.replacen("grpcs", "https", 1))
                    .map_err(|e| Error::InvalidEndpoint(format!("{input}: {e}")))?;
            }
            other => {
                return Err(Error::InvalidEndpoint(format!(
                    "{input}: unsupported scheme {other:?} (use http or https)"
                )));
            }
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(Error::InvalidEndpoint(format!("{input}: missing host")));
        }
        if !input.contains("://") && url.port().is_none() {
            let _ = url.set_port(Some(SPACESD_DEFAULT_PORT));
        }
        url.set_query(None);
        url.set_fragment(None);
        let trimmed = url.path().trim_end_matches('/').to_string();
        url.set_path(&trimmed);
        let kind = classify(&url);
        Ok(Self { base: url, kind })
    }

    /// Base URL (no trailing slash).
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// Endpoint kind.
    pub fn kind(&self) -> &EndpointKind {
        &self.kind
    }

    /// Path prefix every request path is appended to ("" for a bare host).
    pub fn path_prefix(&self) -> &str {
        match self.base.path() {
            "/" => "",
            p => p,
        }
    }

    /// True for `https`.
    pub fn is_tls(&self) -> bool {
        self.base.scheme() == "https"
    }

    /// Host name or address.
    pub fn host(&self) -> &str {
        self.base.host_str().unwrap_or_default()
    }

    /// Port (explicit or scheme default).
    pub fn port(&self) -> u16 {
        self.base
            .port_or_known_default()
            .unwrap_or(SPACESD_DEFAULT_PORT)
    }

    /// `scheme://authority` without the path prefix.
    pub fn origin(&self) -> String {
        let mut origin = self.base.clone();
        origin.set_path("");
        origin.as_str().trim_end_matches('/').to_string()
    }

    /// Absolute HTTP URL of `path` (which may carry a query) under this
    /// endpoint's prefix.
    pub fn http_url(&self, path: &str) -> String {
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("{}{}{}", self.origin(), self.path_prefix(), path)
    }

    /// Absolute WebSocket URL (`ws`/`wss`) of `path` under this endpoint.
    pub fn ws_url(&self, path: &str) -> String {
        let http = self.http_url(path);
        if let Some(rest) = http.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = http.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            http
        }
    }
}

fn classify(url: &Url) -> EndpointKind {
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    match segments.as_slice() {
        ["m", machine_id, ..] => EndpointKind::Relay {
            machine_id: (*machine_id).to_string(),
        },
        _ => EndpointKind::Direct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_port_defaults_to_http() {
        let e = Endpoint::parse("10.0.0.5:4000").unwrap();
        assert_eq!(e.base_url().as_str(), "http://10.0.0.5:4000/");
        assert_eq!(e.kind(), &EndpointKind::Direct);
        assert_eq!(e.http_url("/health"), "http://10.0.0.5:4000/health");
    }

    #[test]
    fn bare_host_gets_default_port() {
        let e = Endpoint::parse("sandbox.local").unwrap();
        assert_eq!(e.port(), 3211);
        assert_eq!(e.origin(), "http://sandbox.local:3211");
    }

    #[test]
    fn explicit_scheme_keeps_scheme_default_port() {
        let e = Endpoint::parse("https://env.example.com").unwrap();
        assert_eq!(e.port(), 443);
        assert!(e.is_tls());
        assert_eq!(
            e.ws_url("/media?ticket=a"),
            "wss://env.example.com/media?ticket=a"
        );
    }

    #[test]
    fn relay_url_is_classified() {
        let e = Endpoint::parse("https://relay.example/m/abc123").unwrap();
        assert_eq!(
            e.kind(),
            &EndpointKind::Relay {
                machine_id: "abc123".into()
            }
        );
        assert_eq!(e.ws_url("/media"), "wss://relay.example/m/abc123/media");
    }

    #[test]
    fn rejects_garbage() {
        assert!(Endpoint::parse("").is_err());
        assert!(Endpoint::parse("ftp://x").is_err());
    }

    #[test]
    fn ipv6_bare() {
        let e = Endpoint::parse("[::1]:5000").unwrap();
        assert_eq!(e.origin(), "http://[::1]:5000");
    }
}
