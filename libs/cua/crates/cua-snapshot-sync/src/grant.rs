//! Read grants: `csg1.<payload>.<mac>`, where the payload is the base64url
//! JSON `{"d": <manifest digest>, "e": <expiry, Unix seconds>}` and the mac
//! its HMAC-SHA256 under the exporting host's grant key
//! ([`crate::store::Store::grant_key`]). A grant lets its holder read the
//! files of that one manifest until it expires, and nothing else.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::{Error, Result};

const PREFIX: &str = "csg1";
/// The longest a grant may last.
pub const MAX_TTL_SECS: u64 = 86_400;
/// The default lifetime.
pub const DEFAULT_TTL_SECS: u64 = 3_600;

#[derive(Serialize, Deserialize)]
struct Payload {
    d: String,
    e: u64,
}

fn mac(key: &[u8], payload: &str) -> Hmac<Sha256> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    m.update(PREFIX.as_bytes());
    m.update(b".");
    m.update(payload.as_bytes());
    m
}

/// A grant to read the files of manifest `digest` until `now + ttl`.
pub fn issue(key: &[u8], digest: &str, ttl_secs: u64, now: u64) -> String {
    let ttl = if ttl_secs == 0 {
        DEFAULT_TTL_SECS
    } else {
        ttl_secs.min(MAX_TTL_SECS)
    };
    let payload = B64.encode(
        serde_json::to_vec(&Payload {
            d: digest.to_string(),
            e: now + ttl,
        })
        .expect("a grant payload serializes"),
    );
    let tag = B64.encode(mac(key, &payload).finalize().into_bytes());
    format!("{PREFIX}.{payload}.{tag}")
}

/// The manifest digest `grant` is for, when it is valid now.
pub fn verify(key: &[u8], grant: &str, now: u64) -> Result<String> {
    let mut parts = grant.trim().split('.');
    let (Some(PREFIX), Some(payload), Some(tag), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Error::Denied("not a snapshot grant".into()));
    };
    let tag = B64
        .decode(tag)
        .map_err(|_| Error::Denied("not a snapshot grant".into()))?;
    mac(key, payload)
        .verify_slice(&tag)
        .map_err(|_| Error::Denied("the snapshot grant is not valid on this host".into()))?;
    let p: Payload = serde_json::from_slice(
        &B64.decode(payload)
            .map_err(|_| Error::Denied("not a snapshot grant".into()))?,
    )
    .map_err(|_| Error::Denied("not a snapshot grant".into()))?;
    if p.e <= now {
        return Err(Error::Denied("the snapshot grant expired".into()));
    }
    Ok(p.d)
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_reads_one_manifest_until_it_expires() {
        let key = [7u8; 32];
        let g = issue(&key, "abc", 60, 1000);
        assert_eq!(verify(&key, &g, 1000).unwrap(), "abc");
        assert_eq!(verify(&key, &g, 1059).unwrap(), "abc");
        assert!(
            verify(&key, &g, 1060)
                .unwrap_err()
                .to_string()
                .contains("expired")
        );
        // Another host's key.
        assert!(
            verify(&[8u8; 32], &g, 1000)
                .unwrap_err()
                .to_string()
                .contains("not valid on this host")
        );
        // A changed payload (another digest, a later expiry) fails the MAC.
        let parts: Vec<&str> = g.split('.').collect();
        let forged_payload = B64.encode(br#"{"d":"other","e":999999}"#);
        let forged = format!("{}.{}.{}", parts[0], forged_payload, parts[2]);
        assert!(matches!(verify(&key, &forged, 1000), Err(Error::Denied(_))));
        for junk in ["", "csg1", "csg1.a.b.c", "x.y.z", "csg1.!!.??"] {
            assert!(
                matches!(verify(&key, junk, 1000), Err(Error::Denied(_))),
                "{junk}"
            );
        }
        // Lifetimes are clamped.
        let long = issue(&key, "abc", 10 * MAX_TTL_SECS, 0);
        assert!(verify(&key, &long, MAX_TTL_SECS).is_err());
        let default = issue(&key, "abc", 0, 0);
        assert!(verify(&key, &default, DEFAULT_TTL_SECS - 1).is_ok());
    }
}
