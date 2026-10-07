// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Firefox's saved-login crypto (NSS "secret decoder ring"), pure.
//!
//! Firefox keeps logins in `logins.json`: `encryptedUsername` and
//! `encryptedPassword` are base64 of a DER
//! `SEQUENCE { OCTET STRING keyId, SEQUENCE { OID cipher, OCTET STRING iv },
//! OCTET STRING ciphertext }` (NSS `lib/softoken/sdr.c`), under a master key that
//! `key4.db` (SQLite) holds, itself encrypted with a key derived from the
//! master password (empty by default):
//!
//! * `metaData` row `password`: `item1` = global salt, `item2` = DER
//!   `SEQUENCE { SEQUENCE { OID pbe, params }, OCTET STRING ciphertext }`, which
//!   decrypts to `password-check` (a wrong password fails here);
//! * `nssPrivate.a11` = the same shape wrapping the master key, `a102` = key id.
//!
//! Two PBE schemes (NSS `lib/softoken/lowpbe.c`): the legacy
//! `pbeWithSha1AndTripleDES-CBC` (1.2.840.113549.1.12.5.1.3: SHA-1 PBKDF1 then
//! the HMAC-SHA1 extension `nsspkcs5_PFXPBE`, 3DES-CBC, IV = last 8 bytes) and
//! PBES2 (1.2.840.113549.1.5.13: PBKDF2-HMAC-SHA256 over
//! `SHA1(globalSalt || password)`, AES-256-CBC, IV = `04 0e` + 14 bytes).
//!
//! Nothing here touches a file: callers hand it the bytes.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha1::{Digest, Sha1};
use zeroize::Zeroizing;

use crate::error::TeleportError;

const OID_PBE_3DES: &[u8] = &[
    0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x0c, 0x05, 0x01, 0x03,
];
const OID_PBES2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0d];
const OID_DES_EDE3_CBC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x03, 0x07];
const OID_AES256_CBC: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x01, 0x2a];
const PASSWORD_CHECK: &[u8] = b"password-check";

fn err(m: &str) -> TeleportError {
    TeleportError::Provider(format!("Firefox key4.db: {m}"))
}

// ---- a minimal DER reader and writer ----------------------------------------

struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    /// The next element: its tag and content.
    fn next(&mut self) -> Result<(u8, &'a [u8]), TeleportError> {
        let b = self.0;
        let (&tag, rest) = b.split_first().ok_or_else(|| err("truncated DER"))?;
        let (&l0, rest) = rest.split_first().ok_or_else(|| err("truncated DER"))?;
        let (len, rest) = if l0 < 0x80 {
            (l0 as usize, rest)
        } else {
            let n = (l0 & 0x7f) as usize;
            if n == 0 || n > 4 || rest.len() < n {
                return Err(err("bad DER length"));
            }
            let len = rest[..n].iter().fold(0usize, |a, &x| (a << 8) | x as usize);
            (len, &rest[n..])
        };
        if rest.len() < len {
            return Err(err("truncated DER"));
        }
        self.0 = &rest[len..];
        Ok((tag, &rest[..len]))
    }

    fn expect(&mut self, tag: u8) -> Result<&'a [u8], TeleportError> {
        let (t, c) = self.next()?;
        if t != tag {
            return Err(err("unexpected DER structure"));
        }
        Ok(c)
    }
}

fn der_int(c: &[u8]) -> u32 {
    c.iter().fold(0u32, |a, &x| (a << 8) | x as u32)
}

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if content.len() < 0x80 {
        out.push(content.len() as u8);
    } else if content.len() < 0x100 {
        out.extend([0x81, content.len() as u8]);
    } else {
        out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
    }
    out.extend_from_slice(content);
    out
}

// ---- key derivation ---------------------------------------------------------

type HmacSha1 = Hmac<Sha1>;

/// Legacy NSS PBE: PBKDF1 (SHA-1, `iter` rounds) then the HMAC-SHA1 extension
/// until `need` bytes exist.
fn legacy_derive(password: &[u8], salt: &[u8], iter: u32, need: usize) -> Vec<u8> {
    let mut h = Sha1::digest([password, salt].concat()).to_vec();
    for _ in 1..iter {
        h = Sha1::digest(&h).to_vec();
    }
    if need <= 20 {
        return h;
    }
    let state_len = salt.len().max(20);
    let mut state = salt.to_vec();
    state.resize(state_len, 0);
    let mut out = Vec::new();
    while out.len() < need {
        let mut m = <HmacSha1 as Mac>::new_from_slice(&h).expect("HMAC takes any key length");
        m.update(&state);
        m.update(salt);
        out.extend_from_slice(&m.finalize().into_bytes());
        let mut m = <HmacSha1 as Mac>::new_from_slice(&h).expect("HMAC takes any key length");
        m.update(&state);
        state = m.finalize().into_bytes().to_vec();
    }
    out
}

/// Decrypts a `SEQUENCE { SEQUENCE { OID, params }, OCTET STRING ct }` blob
/// (key4.db `item2`, `a11`) with the password-derived key.
fn decrypt_pbe_blob(
    global_salt: &[u8],
    password: &[u8],
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
    let mut top = Der(Der(blob).expect(0x30)?);
    let mut alg = Der(top.expect(0x30)?);
    let ct = top.expect(0x04)?;
    let oid = alg.expect(0x06)?;
    let params = alg.expect(0x30)?;
    let hp = Sha1::digest([global_salt, password].concat()).to_vec();
    if oid == OID_PBE_3DES {
        let mut p = Der(params);
        let salt = p.expect(0x04)?;
        let iter = der_int(p.expect(0x02)?);
        let k = legacy_derive(&hp, salt, iter.max(1), 32);
        decrypt_3des(&k[..24], &k[k.len().min(40) - 8..], ct)
    } else if oid == OID_PBES2 {
        let mut p = Der(params);
        let mut kdf = Der(p.expect(0x30)?);
        kdf.expect(0x06)?;
        let mut kp = Der(kdf.expect(0x30)?);
        let salt = kp.expect(0x04)?;
        let iter = der_int(kp.expect(0x02)?);
        let mut enc = Der(p.expect(0x30)?);
        enc.expect(0x06)?;
        let iv = enc.expect(0x04)?;
        let mut key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(&hp, salt, iter.max(1), &mut key);
        decrypt_aes256(&key, &sdr_iv(iv), ct)
    } else {
        Err(err("an unknown password-based encryption scheme"))
    }
}

/// NSS keeps a 14-byte IV as its DER (`04 0e` + bytes): that 16-byte string is
/// the CBC IV.
fn sdr_iv(iv: &[u8]) -> Vec<u8> {
    if iv.len() == 14 {
        let mut v = vec![0x04, 0x0e];
        v.extend_from_slice(iv);
        v
    } else {
        iv.to_vec()
    }
}

fn decrypt_3des(key: &[u8], iv: &[u8], ct: &[u8]) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
    type D = cbc::Decryptor<des::TdesEde3>;
    let mut buf = ct.to_vec();
    let pt = D::new_from_slices(key, iv)
        .map_err(|_| err("a bad 3DES key or IV"))?
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| err("wrong password or damaged data"))?;
    Ok(Zeroizing::new(pt.to_vec()))
}

fn decrypt_aes256(key: &[u8], iv: &[u8], ct: &[u8]) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
    type D = cbc::Decryptor<aes::Aes256>;
    let mut buf = ct.to_vec();
    let pt = D::new_from_slices(key, iv)
        .map_err(|_| err("a bad AES key or IV"))?
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| err("wrong password or damaged data"))?;
    Ok(Zeroizing::new(pt.to_vec()))
}

/// The login master key from `key4.db`: `global_salt` (`metaData.item1`),
/// `item2` (`metaData.item2`), `a11` (`nssPrivate.a11`) and the master password
/// (empty unless the user set one; a wrong one is an error here).
pub fn master_key(
    global_salt: &[u8],
    item2: &[u8],
    a11: &[u8],
    password: &[u8],
) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
    let check = decrypt_pbe_blob(global_salt, password, item2)
        .map_err(|_| err("a master password is set (or the key database is damaged)"))?;
    if !check.starts_with(PASSWORD_CHECK) {
        return Err(err("a master password is set"));
    }
    let key = decrypt_pbe_blob(global_salt, password, a11)?;
    // 3DES keys are 24 bytes; an AES-256 key 32. PKCS#7 padding is gone.
    Ok(key)
}

/// Decrypts one `encryptedUsername`/`encryptedPassword` (base64) with the
/// master key.
pub fn decrypt_sdr(master: &[u8], b64: &str) -> Result<Zeroizing<Vec<u8>>, TeleportError> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| err("a login field is not base64"))?;
    let mut top = Der(Der(&raw).expect(0x30)?);
    top.expect(0x04)?; // key id
    let mut alg = Der(top.expect(0x30)?);
    let ct = top.expect(0x04)?;
    let oid = alg.expect(0x06)?;
    let iv = alg.expect(0x04)?;
    if oid == OID_DES_EDE3_CBC {
        decrypt_3des(
            master.get(..24).ok_or_else(|| err("a short master key"))?,
            iv,
            ct,
        )
    } else if oid == OID_AES256_CBC {
        decrypt_aes256(
            master.get(..32).ok_or_else(|| err("a short master key"))?,
            iv,
            ct,
        )
    } else {
        Err(err("an unknown login cipher"))
    }
}

/// Encrypts `plain` as an SDR blob (base64) the way Firefox does for new logins
/// under a 3DES master key; `iv` is 8 random bytes supplied by the caller.
pub fn encrypt_sdr(
    master: &[u8],
    key_id: &[u8],
    iv: &[u8; 8],
    plain: &[u8],
) -> Result<String, TeleportError> {
    type E = cbc::Encryptor<des::TdesEde3>;
    let key = master
        .get(..24)
        .ok_or_else(|| err("only 3DES login keys can be written"))?;
    let ct = E::new_from_slices(key, iv)
        .map_err(|_| err("a bad 3DES key"))?
        .encrypt_padded_vec_mut::<Pkcs7>(plain);
    let alg = [tlv(0x06, OID_DES_EDE3_CBC), tlv(0x04, iv)].concat();
    let body = [tlv(0x04, key_id), tlv(0x30, &alg), tlv(0x04, &ct)].concat();
    Ok(base64::engine::general_purpose::STANDARD.encode(tlv(0x30, &body)))
}

/// Builders for fixtures and tests: a `key4.db`'s three blobs for a 3DES master
/// key, in the legacy or the PBES2 scheme, built from the same specification
/// the readers parse (no real Firefox profile is read to make them).
pub mod testing {
    use super::*;

    /// `(global_salt, item2, a11)` for `master` (24 bytes) under `password`.
    pub fn key4_blobs(
        master: &[u8; 24],
        password: &[u8],
        pbes2: bool,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let global_salt = vec![0x11u8; 20];
        let entry_salt = vec![0x22u8; 20];
        let hp = Sha1::digest([&global_salt[..], password].concat()).to_vec();
        let seal = |plain: &[u8]| -> Vec<u8> {
            if pbes2 {
                let mut key = [0u8; 32];
                pbkdf2::pbkdf2_hmac::<sha2::Sha256>(&hp, &entry_salt, 10_000, &mut key);
                let iv14 = [0x33u8; 14];
                let iv = sdr_iv(&iv14);
                type E = cbc::Encryptor<aes::Aes256>;
                let ct = E::new_from_slices(&key, &iv)
                    .unwrap()
                    .encrypt_padded_vec_mut::<Pkcs7>(plain);
                let kdf_params = [
                    tlv(0x04, &entry_salt),
                    tlv(0x02, &[0x27, 0x10]),
                    tlv(0x02, &[0x20]),
                ]
                .concat();
                let kdf = [
                    tlv(
                        0x06,
                        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0c],
                    ),
                    tlv(0x30, &kdf_params),
                ]
                .concat();
                let enc = [tlv(0x06, OID_AES256_CBC), tlv(0x04, &iv14)].concat();
                let params = [tlv(0x30, &kdf), tlv(0x30, &enc)].concat();
                let alg = [tlv(0x06, OID_PBES2), tlv(0x30, &params)].concat();
                tlv(0x30, &[tlv(0x30, &alg), tlv(0x04, &ct)].concat())
            } else {
                let k = legacy_derive(&hp, &entry_salt, 1, 32);
                type E = cbc::Encryptor<des::TdesEde3>;
                let ct = E::new_from_slices(&k[..24], &k[k.len().min(40) - 8..])
                    .unwrap()
                    .encrypt_padded_vec_mut::<Pkcs7>(plain);
                let params = [tlv(0x04, &entry_salt), tlv(0x02, &[1])].concat();
                let alg = [tlv(0x06, OID_PBE_3DES), tlv(0x30, &params)].concat();
                tlv(0x30, &[tlv(0x30, &alg), tlv(0x04, &ct)].concat())
            }
        };
        (global_salt.clone(), seal(PASSWORD_CHECK), seal(master))
    }

    /// A login field as Firefox stores it (3DES master key, fixed IV).
    pub fn sealed_field(master: &[u8; 24], key_id: &[u8], plain: &[u8]) -> String {
        encrypt_sdr(master, key_id, &[0x44u8; 8], plain).expect("a 24-byte key seals")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: [u8; 24] = *b"0123456789abcdef01234567";

    #[test]
    fn both_pbe_schemes_unwrap_the_master_key_and_a_wrong_password_is_refused() {
        for pbes2 in [false, true] {
            let (salt, item2, a11) = testing::key4_blobs(&MASTER, b"", pbes2);
            let key = master_key(&salt, &item2, &a11, b"").unwrap();
            assert_eq!(&key[..24], &MASTER, "pbes2={pbes2}");
            assert!(
                master_key(&salt, &item2, &a11, b"hunter2").is_err(),
                "pbes2={pbes2}"
            );
            let (salt, item2, a11) = testing::key4_blobs(&MASTER, b"hunter2", pbes2);
            assert!(master_key(&salt, &item2, &a11, b"").is_err());
            assert!(master_key(&salt, &item2, &a11, b"hunter2").is_ok());
        }
    }

    #[test]
    fn a_login_field_round_trips_under_the_master_key() {
        let sealed = testing::sealed_field(
            &MASTER,
            &[0xf8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
            b"s3cret-pw",
        );
        assert_eq!(&*decrypt_sdr(&MASTER, &sealed).unwrap(), b"s3cret-pw");
        assert!(decrypt_sdr(&[9u8; 24], &sealed).is_err());
        assert!(decrypt_sdr(&MASTER, "bm90IGRlcg==").is_err());
    }

    /// The legacy derivation follows NSS: with `iter` 1 it is the well-known
    /// `k1 = HMAC(chp, pes||salt)`, `tk = HMAC(chp, pes)`, `k2 = HMAC(chp, tk||salt)`
    /// sequence (firepwd); here its first block is recomputed independently.
    #[test]
    fn the_legacy_derivation_matches_the_nss_hmac_extension() {
        let hp = [7u8; 20];
        let salt = [5u8; 20];
        let chp = Sha1::digest([&hp[..], &salt[..]].concat());
        let mut pes = salt.to_vec();
        pes.resize(20, 0);
        let mut m = <HmacSha1 as Mac>::new_from_slice(&chp).unwrap();
        m.update(&pes);
        m.update(&salt);
        let k1 = m.finalize().into_bytes();
        let got = legacy_derive(&hp, &salt, 1, 32);
        assert_eq!(&got[..20], k1.as_slice());
        assert_eq!(got.len(), 40);
    }
}
