// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! RSA public keys as JWKs: the `n`/`e` of a key, its RFC 7638 thumbprint
//! (the `kid`), and the DER and PEM forms they arrive in (a PKCS#1
//! `RSAPublicKey` from ring, a `SubjectPublicKeyInfo` from KMS
//! `GetPublicKey` or a `PUBLIC KEY` PEM file).

use base64::Engine as _;
use serde::Serialize;

pub(crate) fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// An RSA public key: big-endian modulus and exponent, no leading zeros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaPublicKey {
    /// Modulus.
    pub n: Vec<u8>,
    /// Public exponent.
    pub e: Vec<u8>,
}

/// One entry of the JWKS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Jwk {
    /// `RSA`.
    pub kty: String,
    /// `sig`.
    #[serde(rename = "use")]
    pub use_: String,
    /// `RS256`.
    pub alg: String,
    /// RFC 7638 SHA-256 thumbprint.
    pub kid: String,
    /// Modulus, base64url.
    pub n: String,
    /// Exponent, base64url.
    pub e: String,
}

impl RsaPublicKey {
    /// Builds a key, stripping leading zero bytes.
    pub fn new(n: &[u8], e: &[u8]) -> Self {
        let strip = |b: &[u8]| {
            let first = b.iter().position(|x| *x != 0).unwrap_or(b.len());
            b[first..].to_vec()
        };
        Self {
            n: strip(n),
            e: strip(e),
        }
    }

    /// Modulus size in bits.
    pub fn bits(&self) -> usize {
        match self.n.first() {
            Some(top) => self.n.len() * 8 - top.leading_zeros() as usize,
            None => 0,
        }
    }

    /// The RFC 7638 thumbprint: base64url SHA-256 of the canonical JSON
    /// `{"e":…,"kty":"RSA","n":…}` (required members, lexical order, no
    /// whitespace).
    pub fn thumbprint(&self) -> String {
        let canonical = format!(
            r#"{{"e":"{}","kty":"RSA","n":"{}"}}"#,
            b64().encode(&self.e),
            b64().encode(&self.n)
        );
        b64().encode(ring::digest::digest(&ring::digest::SHA256, canonical.as_bytes()).as_ref())
    }

    /// The JWKS entry (`kid` = thumbprint).
    pub fn jwk(&self) -> Jwk {
        Jwk {
            kty: "RSA".into(),
            use_: "sig".into(),
            alg: "RS256".into(),
            kid: self.thumbprint(),
            n: b64().encode(&self.n),
            e: b64().encode(&self.e),
        }
    }

    /// Parses a PKCS#1 `RSAPublicKey` (`SEQUENCE { n INTEGER, e INTEGER }`).
    pub fn from_pkcs1_der(der: &[u8]) -> Result<Self, String> {
        let (body, rest) = der_take(der, 0x30)?;
        if !rest.is_empty() {
            return Err("trailing bytes after RSAPublicKey".into());
        }
        let (n, body) = der_take(body, 0x02)?;
        let (e, body) = der_take(body, 0x02)?;
        if !body.is_empty() {
            return Err("trailing bytes in RSAPublicKey".into());
        }
        let key = Self::new(n, e);
        if key.bits() < 2048 {
            return Err(format!(
                "RSA key is {} bits; 2048 or more needed",
                key.bits()
            ));
        }
        Ok(key)
    }

    /// Parses a `SubjectPublicKeyInfo` holding an RSA key (KMS
    /// `GetPublicKey`, a `PUBLIC KEY` PEM).
    pub fn from_spki_der(der: &[u8]) -> Result<Self, String> {
        // rsaEncryption, 1.2.840.113549.1.1.1
        const RSA_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
        let (spki, rest) = der_take(der, 0x30)?;
        if !rest.is_empty() {
            return Err("trailing bytes after SubjectPublicKeyInfo".into());
        }
        let (alg, spki) = der_take(spki, 0x30)?;
        let (oid, _) = der_take(alg, 0x06)?;
        if oid != RSA_OID {
            return Err("not an RSA public key".into());
        }
        let (bits, _) = der_take(spki, 0x03)?;
        match bits.split_first() {
            Some((0, key)) => Self::from_pkcs1_der(key),
            _ => Err("bad BIT STRING in SubjectPublicKeyInfo".into()),
        }
    }
}

/// Takes one DER element of tag `tag`: (contents, rest).
fn der_take(input: &[u8], tag: u8) -> Result<(&[u8], &[u8]), String> {
    let (&got, input) = input.split_first().ok_or("truncated DER")?;
    if got != tag {
        return Err(format!("DER: expected tag {tag:#04x}, got {got:#04x}"));
    }
    let (&len0, mut input) = input.split_first().ok_or("truncated DER")?;
    let len = if len0 < 0x80 {
        len0 as usize
    } else {
        let count = (len0 & 0x7f) as usize;
        if count == 0 || count > 4 || input.len() < count {
            return Err("bad DER length".into());
        }
        let mut len = 0usize;
        for b in &input[..count] {
            len = (len << 8) | *b as usize;
        }
        input = &input[count..];
        len
    };
    if input.len() < len {
        return Err("truncated DER".into());
    }
    Ok(input.split_at(len))
}

/// The DER inside a PEM block labelled `label` (e.g. `PRIVATE KEY`), or
/// the input itself when it is not PEM.
pub fn pem_or_der(input: &[u8], label: &str) -> Result<Vec<u8>, String> {
    let Ok(text) = std::str::from_utf8(input) else {
        return Ok(input.to_vec());
    };
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let Some(start) = text.find(&begin) else {
        if text.contains("-----BEGIN ") {
            return Err(format!("expected a PEM {label} block"));
        }
        return Ok(input.to_vec());
    };
    let body = &text[start + begin.len()..];
    let stop = body.find(&end).ok_or("unterminated PEM block")?;
    let b64: String = body[..stop]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| "PEM body is not base64".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7638_example_thumbprint() {
        // RFC 7638 section 3.1.
        let n = b64().decode("0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw").unwrap();
        let e = b64().decode("AQAB").unwrap();
        assert_eq!(
            RsaPublicKey::new(&n, &e).thumbprint(),
            "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs"
        );
    }

    #[test]
    fn spki_wraps_pkcs1() {
        let n = [vec![0x00, 0xc3], vec![0x55; 255]].concat();
        let pkcs1 = der_seq(&[der_int(&n), der_int(&[1, 0, 1])].concat());
        let alg = der_seq(
            &[
                vec![
                    0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
                ],
                vec![0x05, 0x00],
            ]
            .concat(),
        );
        let bits = [vec![0x03], der_len(pkcs1.len() + 1), vec![0], pkcs1.clone()].concat();
        let spki = der_seq(&[alg, bits].concat());
        let a = RsaPublicKey::from_pkcs1_der(&pkcs1).unwrap();
        let b = RsaPublicKey::from_spki_der(&spki).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.bits(), 2048);
        assert_eq!(a.e, vec![1, 0, 1]);
        // Too small a key is refused.
        let small = der_seq(&[der_int(&[0x00, 0xc3, 0x01]), der_int(&[1, 0, 1])].concat());
        assert!(RsaPublicKey::from_pkcs1_der(&small).is_err());
    }

    fn der_len(len: usize) -> Vec<u8> {
        if len < 0x80 {
            vec![len as u8]
        } else if len < 0x100 {
            vec![0x81, len as u8]
        } else {
            vec![0x82, (len >> 8) as u8, len as u8]
        }
    }

    fn der_seq(body: &[u8]) -> Vec<u8> {
        [vec![0x30], der_len(body.len()), body.to_vec()].concat()
    }

    fn der_int(body: &[u8]) -> Vec<u8> {
        [vec![0x02], der_len(body.len()), body.to_vec()].concat()
    }

    #[test]
    fn pem_is_unwrapped_and_der_passes_through() {
        let pem = b"-----BEGIN PUBLIC KEY-----\nAAEC\n-----END PUBLIC KEY-----\n";
        assert_eq!(pem_or_der(pem, "PUBLIC KEY").unwrap(), vec![0, 1, 2]);
        assert_eq!(
            pem_or_der(&[0x30, 0x00], "PUBLIC KEY").unwrap(),
            vec![0x30, 0]
        );
        assert!(pem_or_der(pem, "PRIVATE KEY").is_err());
    }
}
