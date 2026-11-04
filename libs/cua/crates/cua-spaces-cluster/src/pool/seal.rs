// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! End-to-end encryption of the pool's data on the relay.
//!
//! The relay stores the pool log, its snapshot and the mailbox between the
//! account's devices. What it needs for fencing and exactly-once (epochs,
//! positions, job ids, the kind of each change, which entry ends a job,
//! which machine an attempt went to, sizes) stays readable; everything a
//! job is and produces (command, environment, input, outputs, results,
//! durable Spaces' addresses and tokens, machines' details, the requests
//! and answers of the mailbox) is sealed on the device before it leaves.
//!
//! - **Pool keys.** Each device of the account has an X25519 key of its
//!   own for the pool ([`PoolSecret`]). It certifies the public half with
//!   its enrolled P-256 device key ([`Member`]): the device id is the hash
//!   of that P-256 key, so the relay cannot swap one device's pool key for
//!   another without the signature failing.
//! - **Data keys.** The account's data is sealed with a 256-bit key of the
//!   current generation ([`Bundle`]), XChaCha20-Poly1305, with the entry's
//!   clear fields as associated data. A new generation is made whenever a
//!   device leaves (revoked, or replaced by a new key); a bundle carries
//!   every generation so far, so a machine that takes over reads the whole
//!   log, and a device that left never gets a key made after it left.
//! - **Keyring.** The bundle is wrapped to each member's pool key with
//!   HPKE (RFC 9180, `DHKEM(X25519, HKDF-SHA256)`, HKDF-SHA256,
//!   ChaCha20-Poly1305) in **auth mode**: the wrap also proves which
//!   member's pool key made it, so a relay cannot hand a device keys of
//!   its own making ([`Keyring`]). The relay stores the keyring and checks
//!   only its clear metadata (who wrapped what to whom, the generation).
//!
//! Every primitive is a vetted crate (`hpke`, `chacha20poly1305`, `ring`);
//! nothing here is hand-rolled.

use std::collections::BTreeMap;

use base64::Engine as _;
use chacha20poly1305::aead::{Aead as _, KeyInit as _, Payload};
use hpke::{Deserializable as _, Kem as _, Serializable as _};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

type Kem = hpke::kem::X25519HkdfSha256;
type Kdf = hpke::kdf::HkdfSha256;
type WrapAead = hpke::aead::ChaCha20Poly1305;

/// The version of sealed data this code writes and reads.
pub const VERSION: u32 = 1;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn b64url() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// Why sealed data could not be opened or made.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SealError {
    /// Not sealed data (or of another version).
    #[error("not sealed pool data: {0}")]
    Malformed(String),
    /// Sealed under a generation this device has no key of.
    #[error("sealed under key generation {0}, which this device does not have")]
    UnknownGeneration(u64),
    /// The data or its clear fields were changed, or the key is wrong.
    #[error("sealed pool data failed to open (changed, or another key)")]
    Tampered,
    /// No wrap of the keyring for this device.
    #[error("the pool's keys are not shared with this device yet")]
    NotShared,
    /// A wrap made by a key that is not a member's.
    #[error("the pool keyring was wrapped by {0}, which is not one of the account's devices")]
    UnknownSender(String),
    /// A bundle that does not continue the one this device had.
    #[error("the pool keyring does not continue the keys this device had: {0}")]
    Inconsistent(String),
}

/// Sealed data as it is stored: generation, nonce and ciphertext (with
/// its tag), and nothing else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// [`VERSION`].
    pub v: u32,
    /// The key generation it is sealed under.
    pub g: u64,
    /// The 24-byte XChaCha20 nonce, base64.
    pub n: String,
    /// The ciphertext and tag, base64.
    pub c: String,
}

impl Envelope {
    /// The envelope `v` is, if it is one (exactly).
    pub fn parse(v: &Value) -> Option<Self> {
        let e: Self = serde_json::from_value(v.clone()).ok()?;
        (e.v == VERSION).then_some(e)
    }

    /// As JSON.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// The bytes of a sealed blob (a snapshot).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The envelope a sealed blob is.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let e: Self = serde_json::from_slice(b).ok()?;
        (e.v == VERSION).then_some(e)
    }
}

/// Associated data: domain-separated, length-prefixed fields.
pub fn aad(domain: &str, fields: &[&str]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    for f in std::iter::once(domain).chain(fields.iter().copied()) {
        out.extend_from_slice(&(f.len() as u32).to_be_bytes());
        out.extend_from_slice(f.as_bytes());
    }
    out
}

/// The account's data keys: every generation so far (the newest seals).
#[derive(Clone, PartialEq, Eq)]
pub struct Bundle {
    account: String,
    generation: u64,
    keys: BTreeMap<u64, [u8; 32]>,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("account", &self.account)
            .field("gen", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        for k in self.keys.values_mut() {
            k.zeroize();
        }
    }
}

#[derive(Serialize, Deserialize)]
struct BundleWire {
    account: String,
    #[serde(rename = "gen")]
    generation: u64,
    keys: BTreeMap<String, String>,
}

fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    rand::rng().fill_bytes(&mut k);
    k
}

impl Bundle {
    /// The first generation of `account`'s keys.
    pub fn first(account: &str) -> Self {
        Self::fresh(account, 1)
    }

    /// A new chain whose only key is generation `gen` (after every device
    /// that held the earlier ones left).
    pub fn fresh(account: &str, generation: u64) -> Self {
        Self {
            account: account.to_string(),
            generation,
            keys: BTreeMap::from([(generation, random_key())]),
        }
    }

    /// The next generation (a new key; the earlier ones are kept to read
    /// what was sealed under them).
    pub fn rotate(&self) -> Self {
        let mut keys = self.keys.clone();
        keys.insert(self.generation + 1, random_key());
        Self {
            account: self.account.clone(),
            generation: self.generation + 1,
            keys,
        }
    }

    /// The account.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// The generation that seals.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether this bundle has every key `older` has, unchanged, and is
    /// not older (a keyring must never take keys away or swap them).
    pub fn continues(&self, older: &Bundle) -> Result<(), SealError> {
        if self.account != older.account {
            return Err(SealError::Inconsistent("another account".into()));
        }
        if self.generation < older.generation {
            return Err(SealError::Inconsistent(format!(
                "generation {} after {}",
                self.generation, older.generation
            )));
        }
        for (g, k) in &older.keys {
            if self.keys.get(g) != Some(k) {
                return Err(SealError::Inconsistent(format!(
                    "the key of generation {g} changed"
                )));
            }
        }
        Ok(())
    }

    /// Seals `plaintext` under the newest key, bound to `aad`.
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Envelope {
        let key = self.keys[&self.generation];
        let cipher = chacha20poly1305::XChaCha20Poly1305::new((&key).into());
        let mut nonce = [0u8; 24];
        rand::rng().fill_bytes(&mut nonce);
        let c = cipher
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("XChaCha20-Poly1305 seals any length this code makes");
        Envelope {
            v: VERSION,
            g: self.generation,
            n: b64().encode(nonce),
            c: b64().encode(c),
        }
    }

    /// Opens `env`, bound to `aad`.
    pub fn open(&self, aad: &[u8], env: &Envelope) -> Result<Vec<u8>, SealError> {
        if env.v != VERSION {
            return Err(SealError::Malformed(format!("version {}", env.v)));
        }
        let key = self
            .keys
            .get(&env.g)
            .ok_or(SealError::UnknownGeneration(env.g))?;
        let nonce: [u8; 24] = b64()
            .decode(&env.n)
            .ok()
            .and_then(|n| n.try_into().ok())
            .ok_or_else(|| SealError::Malformed("nonce".into()))?;
        let c = b64()
            .decode(&env.c)
            .map_err(|_| SealError::Malformed("ciphertext".into()))?;
        chacha20poly1305::XChaCha20Poly1305::new(key.into())
            .decrypt((&nonce).into(), Payload { msg: &c, aad })
            .map_err(|_| SealError::Tampered)
    }

    fn to_wire(&self) -> Vec<u8> {
        let w = BundleWire {
            account: self.account.clone(),
            generation: self.generation,
            keys: self
                .keys
                .iter()
                .map(|(g, k)| (g.to_string(), b64().encode(k)))
                .collect(),
        };
        serde_json::to_vec(&w).unwrap_or_default()
    }

    fn from_wire(b: &[u8]) -> Result<Self, SealError> {
        let w: BundleWire =
            serde_json::from_slice(b).map_err(|e| SealError::Malformed(format!("bundle: {e}")))?;
        let mut keys = BTreeMap::new();
        for (g, k) in w.keys {
            let g: u64 = g
                .parse()
                .map_err(|_| SealError::Malformed("bundle generation".into()))?;
            let k: [u8; 32] = b64()
                .decode(k)
                .ok()
                .and_then(|k| k.try_into().ok())
                .ok_or_else(|| SealError::Malformed("bundle key".into()))?;
            keys.insert(g, k);
        }
        if !keys.contains_key(&w.generation) || keys.keys().any(|g| *g > w.generation || *g == 0) {
            return Err(SealError::Malformed("bundle generations".into()));
        }
        Ok(Self {
            account: w.account,
            generation: w.generation,
            keys,
        })
    }
}

/// This device's pool key (X25519).
#[derive(Clone)]
pub struct PoolSecret {
    sk: <Kem as hpke::Kem>::PrivateKey,
}

impl std::fmt::Debug for PoolSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PoolSecret(public {})", self.public())
    }
}

impl PoolSecret {
    /// A new key.
    pub fn generate() -> Self {
        let (sk, _) = Kem::gen_keypair(&mut rand::rng());
        Self { sk }
    }

    /// The key stored as `bytes` ([`PoolSecret::to_bytes`]).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SealError> {
        <Kem as hpke::Kem>::PrivateKey::from_bytes(bytes)
            .map(|sk| Self { sk })
            .map_err(|_| SealError::Malformed("pool key".into()))
    }

    /// Its 32 bytes (to store it, 0600 or in the OS vault).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.sk.to_bytes().to_vec()
    }

    /// The public half, base64.
    pub fn public(&self) -> String {
        b64().encode(Kem::sk_to_pk(&self.sk).to_bytes())
    }
}

fn public_key(b64key: &str) -> Result<<Kem as hpke::Kem>::PublicKey, SealError> {
    let raw = b64()
        .decode(b64key)
        .map_err(|_| SealError::Malformed("pool public key".into()))?;
    <Kem as hpke::Kem>::PublicKey::from_bytes(&raw)
        .map_err(|_| SealError::Malformed("pool public key".into()))
}

/// What a device signs (with its P-256 device key) to certify its pool key.
pub fn member_message(account: &str, device: &str, pool_key: &str) -> String {
    format!("cua-pool-key/v1\n{account}\n{device}\n{pool_key}")
}

/// The device id of an uncompressed P-256 public key (base64url): what the
/// relay derives (`dev_` and the first 24 hex digits of its SHA-256).
pub fn device_id(public_key_b64url: &str) -> Option<String> {
    use sha2::Digest as _;
    let raw = b64url().decode(public_key_b64url.trim()).ok()?;
    if raw.len() != 65 || raw[0] != 4 {
        return None;
    }
    Some(format!(
        "dev_{}",
        &hex::encode(sha2::Sha256::digest(&raw))[..24]
    ))
}

/// One of the account's devices that may hold the pool's keys: its id, its
/// P-256 device key, its pool key and the device key's signature of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// `dev_…`.
    pub device: String,
    /// Its P-256 device key (uncompressed, base64url).
    pub public_key: String,
    /// Its pool key (X25519, base64).
    pub pool_key: String,
    /// ECDSA P-256 SHA-256 (fixed, base64url) of [`member_message`].
    pub sig: String,
    /// What the device calls itself (display only).
    #[serde(default)]
    pub name: String,
}

impl Member {
    /// Whether the device id is its P-256 key's and the key signed the
    /// pool key for `account`.
    pub fn verify(&self, account: &str) -> bool {
        if device_id(&self.public_key).as_deref() != Some(self.device.as_str()) {
            return false;
        }
        let Ok(pk) = b64url().decode(self.public_key.trim()) else {
            return false;
        };
        let Ok(sig) = b64url().decode(self.sig.trim()) else {
            return false;
        };
        public_key(&self.pool_key).is_ok()
            && ring::signature::UnparsedPublicKey::new(
                &ring::signature::ECDSA_P256_SHA256_FIXED,
                pk,
            )
            .verify(
                member_message(account, &self.device, &self.pool_key).as_bytes(),
                &sig,
            )
            .is_ok()
    }
}

/// The bundle wrapped to one member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wrap {
    /// The recipient device.
    pub device: String,
    /// Its pool key the wrap is for.
    pub key: String,
    /// The device that made the wrap (its pool key authenticates it).
    pub from: String,
    /// HPKE encapsulated key, base64.
    pub enc: String,
    /// HPKE ciphertext of the bundle, base64.
    pub ct: String,
}

/// The account's keyring as the relay stores it: the generation and the
/// bundle wrapped to each member.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyring {
    /// The newest generation.
    #[serde(rename = "gen")]
    pub generation: u64,
    /// The wraps.
    pub wraps: Vec<Wrap>,
}

fn wrap_info(account: &str, generation: u64, to: &str, from: &str) -> Vec<u8> {
    aad(
        "cua-pool-keyring/v1",
        &[account, &generation.to_string(), to, from],
    )
}

/// This device as a member: its id and pool key.
pub struct Me<'a> {
    /// `dev_…`.
    pub device: &'a str,
    /// Its pool key.
    pub secret: &'a PoolSecret,
}

impl Keyring {
    /// A keyring of `bundle` wrapped to every one of `members` (by `me`,
    /// which must be one of them).
    pub fn wrap(bundle: &Bundle, me: &Me<'_>, members: &[Member]) -> Result<Self, SealError> {
        let mut k = Self {
            generation: bundle.generation,
            wraps: vec![],
        };
        k.add(bundle, me, members)?;
        Ok(k)
    }

    /// Adds a wrap of `bundle` for each of `members` that has none.
    pub fn add(
        &mut self,
        bundle: &Bundle,
        me: &Me<'_>,
        members: &[Member],
    ) -> Result<(), SealError> {
        let sender_pk = Kem::sk_to_pk(&me.secret.sk);
        let mode = hpke::OpModeS::Auth((me.secret.sk.clone(), sender_pk));
        let plain = bundle.to_wire();
        for m in members {
            if self.has(&m.device, &m.pool_key) {
                continue;
            }
            let pk = public_key(&m.pool_key)?;
            let info = wrap_info(&bundle.account, bundle.generation, &m.device, me.device);
            let (enc, ct) = hpke::single_shot_seal::<WrapAead, Kdf, Kem, _>(
                &mode,
                &pk,
                &info,
                &plain,
                b"",
                &mut rand::rng(),
            )
            .map_err(|e| SealError::Malformed(format!("wrapping: {e}")))?;
            self.wraps.push(Wrap {
                device: m.device.clone(),
                key: m.pool_key.clone(),
                from: me.device.to_string(),
                enc: b64().encode(enc.to_bytes()),
                ct: b64().encode(ct),
            });
        }
        Ok(())
    }

    /// Whether `device` has a wrap for `pool_key`.
    pub fn has(&self, device: &str, pool_key: &str) -> bool {
        self.wraps
            .iter()
            .any(|w| w.device == device && w.key == pool_key)
    }

    /// The wraps whose recipient is not one of `members` with that pool
    /// key: a device that left. Then a new generation is due.
    pub fn stale(&self, members: &[Member]) -> bool {
        self.wraps.iter().any(|w| {
            !members
                .iter()
                .any(|m| m.device == w.device && m.pool_key == w.key)
        })
    }

    /// Opens this device's wrap: the bundle, checked to come from one of
    /// `members` (verified) and to be of `account`.
    pub fn open(
        &self,
        account: &str,
        me: &Me<'_>,
        members: &[Member],
    ) -> Result<Bundle, SealError> {
        let mine = me.secret.public();
        let w = self
            .wraps
            .iter()
            .find(|w| w.device == me.device && w.key == mine)
            .ok_or(SealError::NotShared)?;
        let sender = members
            .iter()
            .find(|m| m.device == w.from && m.verify(account))
            .ok_or_else(|| SealError::UnknownSender(w.from.clone()))?;
        let spk = public_key(&sender.pool_key)?;
        let enc_raw = b64()
            .decode(&w.enc)
            .map_err(|_| SealError::Malformed("wrap".into()))?;
        let enc = <Kem as hpke::Kem>::EncappedKey::from_bytes(&enc_raw)
            .map_err(|_| SealError::Malformed("wrap".into()))?;
        let ct = b64()
            .decode(&w.ct)
            .map_err(|_| SealError::Malformed("wrap".into()))?;
        let info = wrap_info(account, self.generation, me.device, &w.from);
        let plain = hpke::single_shot_open::<WrapAead, Kdf, Kem>(
            &hpke::OpModeR::Auth(spk),
            &me.secret.sk,
            &enc,
            &info,
            &ct,
            b"",
        )
        .map_err(|_| SealError::Tampered)?;
        let b = Bundle::from_wire(&plain)?;
        if b.account != account || b.generation != self.generation {
            return Err(SealError::Inconsistent(
                "the wrapped keys are of another account or generation".into(),
            ));
        }
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) struct Dev {
        pub key: ring::signature::EcdsaKeyPair,
        pub secret: PoolSecret,
    }

    impl Dev {
        pub fn new() -> Self {
            let rng = ring::rand::SystemRandom::new();
            let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
            let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
            Self {
                key: ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap(),
                secret: PoolSecret::generate(),
            }
        }
        fn public(&self) -> String {
            use ring::signature::KeyPair as _;
            b64url().encode(self.key.public_key().as_ref())
        }
        pub fn id(&self) -> String {
            device_id(&self.public()).unwrap()
        }
        pub fn member(&self, account: &str) -> Member {
            let rng = ring::rand::SystemRandom::new();
            let pool_key = self.secret.public();
            let sig = self
                .key
                .sign(
                    &rng,
                    member_message(account, &self.id(), &pool_key).as_bytes(),
                )
                .unwrap();
            Member {
                device: self.id(),
                public_key: self.public(),
                pool_key,
                sig: b64url().encode(sig.as_ref()),
                name: String::new(),
            }
        }
    }

    #[test]
    fn a_keyring_opens_only_for_its_members_and_from_a_member() {
        let (a, b, c) = (Dev::new(), Dev::new(), Dev::new());
        let acct = "ada";
        let members = vec![a.member(acct), b.member(acct)];
        assert!(members.iter().all(|m| m.verify(acct)));
        assert!(!members[0].verify("eve"));
        let bundle = Bundle::first(acct);
        let ida = a.id();
        let me_a = Me {
            device: &ida,
            secret: &a.secret,
        };
        let ring = Keyring::wrap(&bundle, &me_a, &members).unwrap();
        let idb = b.id();
        let me_b = Me {
            device: &idb,
            secret: &b.secret,
        };
        assert_eq!(ring.open(acct, &me_b, &members).unwrap(), bundle);
        // Not a recipient.
        let idc = c.id();
        let me_c = Me {
            device: &idc,
            secret: &c.secret,
        };
        assert_eq!(ring.open(acct, &me_c, &members), Err(SealError::NotShared));
        // A wrap whose sender is not a member is refused.
        let only_b = vec![b.member(acct)];
        assert!(matches!(
            ring.open(acct, &me_b, &only_b),
            Err(SealError::UnknownSender(_))
        ));
        // A relay that claims another sender cannot make the wrap open.
        let mut forged = ring.clone();
        for w in &mut forged.wraps {
            w.from = idc.clone();
        }
        let with_c = vec![a.member(acct), b.member(acct), c.member(acct)];
        assert_eq!(forged.open(acct, &me_b, &with_c), Err(SealError::Tampered));
        // Nor reuse it for another account.
        assert!(ring.open("eve", &me_b, &members).is_err());
    }

    #[test]
    fn sealed_data_is_bound_to_its_clear_fields_and_generation() {
        let b1 = Bundle::first("ada");
        let aad1 = aad("log", &["1", "submit"]);
        let env = b1.seal(&aad1, b"echo secret");
        assert_eq!(b1.open(&aad1, &env).unwrap(), b"echo secret");
        assert_eq!(
            b1.open(&aad("log", &["2", "submit"]), &env),
            Err(SealError::Tampered)
        );
        let b2 = b1.rotate();
        b2.continues(&b1).unwrap();
        assert_eq!(b2.open(&aad1, &env).unwrap(), b"echo secret");
        let env2 = b2.seal(&aad1, b"x");
        assert_eq!(env2.g, 2);
        assert_eq!(b1.open(&aad1, &env2), Err(SealError::UnknownGeneration(2)));
        assert!(b1.continues(&b2).is_err());
        assert!(Bundle::first("ada").continues(&b1).is_err());
        let v = env.to_value();
        assert_eq!(Envelope::parse(&v), Some(env.clone()));
        let mut extra = v.clone();
        extra["plain"] = "leak".into();
        assert_eq!(Envelope::parse(&extra), None);
    }
}
