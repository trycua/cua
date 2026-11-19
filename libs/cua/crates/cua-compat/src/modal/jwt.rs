//! The tokens the gateway mints: the command router's bearer JWT
//! (`TaskGetCommandRouterAccess`, refreshed by the client when a call
//! answers `UNAUTHENTICATED`) and the auth token of `AuthTokenGet`
//! (`x-modal-auth-token`). HS256, keyed from the gateway's API key. The
//! client reads only `exp`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct Signer {
    key: [u8; 32],
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Signer {
    /// A signer keyed from `secret` (the gateway's API key).
    pub fn new(secret: &str) -> Signer {
        let mut h = Sha256::new();
        h.update(b"cua-compat-modal-jwt\0");
        h.update(secret.as_bytes());
        Signer {
            key: h.finalize().into(),
        }
    }

    /// A token for `scope` (`"tcr"` or `"api"`) and `subject`, valid for `ttl_s`.
    pub fn mint(&self, scope: &str, subject: &str, ttl_s: u64) -> String {
        self.mint_with(scope, subject, ttl_s, serde_json::Value::Null)
    }

    /// [`Signer::mint`] with extra claims (an object).
    pub fn mint_with(
        &self,
        scope: &str,
        subject: &str,
        ttl_s: u64,
        extra: serde_json::Value,
    ) -> String {
        let head = B64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let now = now_s();
        let mut claims = json!({
            "iss": "cua-compat-modal",
            "scope": scope,
            "sub": subject,
            "iat": now,
            "exp": now + ttl_s,
        });
        if let (Some(c), Some(e)) = (claims.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                c.insert(k.clone(), v.clone());
            }
        }
        let body = B64.encode(claims.to_string());
        let sig = self.sign(&format!("{head}.{body}"));
        format!("{head}.{body}.{sig}")
    }

    fn sign(&self, data: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("hmac key");
        mac.update(data.as_bytes());
        B64.encode(mac.finalize().into_bytes())
    }

    /// The claims of a valid, unexpired token of `scope`.
    pub fn verify(&self, token: &str, scope: &str) -> Result<Value, &'static str> {
        let mut parts = token.split('.');
        let (Some(h), Some(b), Some(s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("malformed token");
        };
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("hmac key");
        mac.update(format!("{h}.{b}").as_bytes());
        let sig = B64.decode(s).map_err(|_| "malformed token")?;
        mac.verify_slice(&sig)
            .map_err(|_| "invalid token signature")?;
        let claims: Value = serde_json::from_slice(&B64.decode(b).map_err(|_| "malformed token")?)
            .map_err(|_| "malformed token")?;
        if claims["scope"] != scope {
            return Err("token has the wrong scope");
        }
        if claims["exp"].as_u64().unwrap_or(0) <= now_s() {
            return Err("token expired");
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_verify_and_expire() {
        let s = Signer::new("k");
        let t = s.mint("tcr", "ta-1", 60);
        let c = s.verify(&t, "tcr").unwrap();
        assert_eq!(c["sub"], "ta-1");
        assert!(s.verify(&t, "api").is_err());
        assert!(Signer::new("other").verify(&t, "tcr").is_err());
        let mut bad = t.clone();
        bad.push('x');
        assert!(s.verify(&bad, "tcr").is_err());
        assert_eq!(
            s.verify(&s.mint("tcr", "x", 0), "tcr"),
            Err("token expired")
        );
    }
}
