// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sealed delivery to a pinned machine key (S1).
//!
//! Keyvault site-login passwords and teleport session bundles cross
//! `relay:` Spaces through `relay.cua.ai`, which terminates TLS and parses
//! every request: without this, it sees those secrets in the clear. This
//! crate seals such a payload end to end, from the sender to the one
//! machine (cua-spacesd) whose public key it was sealed to, so a relay (or
//! anyone else) in the path forwards only ciphertext.
//!
//! Construction (no hand-rolled crypto; every primitive is a vetted
//! RustCrypto / dalek crate): an ephemeral X25519 keypair per delivery,
//! Diffie-Hellman with the recipient's long-term public key, HKDF-SHA256 to
//! derive a one-time symmetric key bound to both public keys, and
//! XChaCha20Poly1305 (a 24-byte random nonce, safe against birthday-bound
//! collisions without a counter) over the plaintext, authenticating the
//! delivery's identity and freshness as associated data: the machine
//! (Space) id, a purpose string (so a Keyvault delivery cannot be replayed
//! as a teleport one or vice versa) and a timestamp. [`ReplayGuard`] rejects
//! a re-sent envelope and a stale one.
//!
//! This is the sealing primitive and the machine keypair / pinning
//! plumbing; wiring it into a specific delivery path (Keyvault site-login,
//! teleport `ImportSession`) is done where that path already lives (see
//! `cua_teleport::send`, `cua_spaces_ext::daemon`, and the matching
//! `cua-spacesd-server` receiver). [`PinStore`] mirrors the pattern
//! `cua_relay::client::AccountLink` already uses to pin the relay's own
//! key: trust-on-first-use, refuse on any later mismatch.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand_core::{OsRng, TryRngCore as _};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

/// Envelope format version.
pub const VERSION: u8 = 1;
/// How stale a `created_at` may be before [`open`] refuses it outright,
/// independent of [`ReplayGuard`].
pub const MAX_AGE_SECS: u64 = 300;
/// Longest plaintext this crate will seal or open in memory: a Keyvault
/// password, or a whole teleport session bundle (cookies, small config
/// files -- typically well under a megabyte; this is generous headroom,
/// not a target size). Both `cua_teleport::upload_bundle` and the guest's
/// unseal read the whole payload into memory to seal or open it (this is a
/// single-shot AEAD, not a streaming one), so a bundle over this bound is
/// sent unsealed (with the usual relay: consent gate) rather than sealed
/// a piece at a time; wire a streaming construction instead of raising
/// this further if that stops being rare.
pub const MAX_PLAINTEXT_LEN: usize = 64 * 1024 * 1024;

/// Errors sealing or opening an envelope.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// `open` on ciphertext that does not decrypt: a tampered envelope, or
    /// the wrong recipient key.
    #[error("the envelope does not decrypt: tampered, or sealed to a different machine key")]
    Tampered,
    /// `space_id` or `purpose` did not match what the envelope was sealed
    /// under (still fails decryption above; this is for callers that check
    /// the AAD before even attempting to decrypt).
    #[error("envelope is for {0:?}, not this delivery")]
    WrongContext(&'static str),
    /// `created_at` is more than [`MAX_AGE_SECS`] in the past or future.
    #[error("envelope is stale or its clock is off")]
    Stale,
    /// The same (space, purpose, nonce) was already opened.
    #[error("envelope already delivered (replay)")]
    Replay,
    /// The plaintext or a field exceeded a bound.
    #[error("{0}")]
    TooLarge(&'static str),
    /// Malformed bytes (not one of this crate's envelopes).
    #[error("malformed sealed envelope: {0}")]
    Malformed(&'static str),
    /// Local storage (keypair or pin file) failed.
    #[error("{0}")]
    Storage(String),
}

/// A machine's long-term X25519 public key, pinned by the sender.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MachinePublicKey(pub [u8; 32]);

impl MachinePublicKey {
    /// Base64url (no padding), for display and config.
    pub fn to_base64url(self) -> String {
        b64().encode(self.0)
    }

    /// Parses a base64url-encoded 32-byte key.
    pub fn from_base64url(s: &str) -> Result<Self, Error> {
        let raw = b64()
            .decode(s.trim())
            .map_err(|_| Error::Malformed("public key is not base64url"))?;
        let arr: [u8; 32] = raw
            .try_into()
            .map_err(|_| Error::Malformed("public key must be 32 bytes"))?;
        Ok(Self(arr))
    }
}

fn b64() -> base64_lite::Base64Url {
    base64_lite::Base64Url
}

/// Minimal base64url (no padding) so this crate does not need the `base64`
/// dependency just for key display.
mod base64_lite {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    pub struct Base64Url;

    impl Base64Url {
        pub fn encode(&self, bytes: impl AsRef<[u8]>) -> String {
            let bytes = bytes.as_ref();
            let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
            for chunk in bytes.chunks(3) {
                let b0 = chunk[0];
                let b1 = *chunk.get(1).unwrap_or(&0);
                let b2 = *chunk.get(2).unwrap_or(&0);
                let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
                out.push(ALPHABET[(n >> 18) as usize & 0x3f] as char);
                out.push(ALPHABET[(n >> 12) as usize & 0x3f] as char);
                if chunk.len() > 1 {
                    out.push(ALPHABET[(n >> 6) as usize & 0x3f] as char);
                }
                if chunk.len() > 2 {
                    out.push(ALPHABET[n as usize & 0x3f] as char);
                }
            }
            out
        }

        pub fn decode(&self, s: &str) -> Result<Vec<u8>, ()> {
            let mut out = Vec::with_capacity(s.len() * 3 / 4);
            let mut buf = 0u32;
            let mut bits = 0u32;
            for c in s.bytes() {
                let v = match c {
                    b'A'..=b'Z' => c - b'A',
                    b'a'..=b'z' => c - b'a' + 26,
                    b'0'..=b'9' => c - b'0' + 52,
                    b'-' => 62,
                    b'_' => 63,
                    _ => return Err(()),
                };
                buf = (buf << 6) | u32::from(v);
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((buf >> bits) as u8);
                }
            }
            Ok(out)
        }
    }
}

/// A machine's long-term X25519 keypair. The guest (cua-spacesd) generates
/// one at first start and keeps it in its own state, 0600; a sender never
/// holds more than the [`MachinePublicKey`] it pinned.
pub struct MachineKeypair {
    secret: StaticSecret,
    public: MachinePublicKey,
}

impl MachineKeypair {
    /// A fresh random keypair.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.try_fill_bytes(&mut bytes).expect("OS RNG");
        let secret = StaticSecret::from(bytes);
        bytes.zeroize();
        let public = MachinePublicKey(PublicKey::from(&secret).to_bytes());
        Self { secret, public }
    }

    /// This machine's public key, to be pinned by a sender.
    pub fn public(&self) -> MachinePublicKey {
        self.public
    }

    /// Loads the keypair at `path`, generating and saving one (0600) on
    /// first use, so a restart keeps the same key senders have pinned.
    pub fn load_or_create(path: &Path) -> Result<Self, Error> {
        match std::fs::read(path) {
            Ok(raw) => {
                let arr: [u8; 32] = raw
                    .try_into()
                    .map_err(|_| Error::Storage(format!("{}: not 32 bytes", path.display())))?;
                let secret = StaticSecret::from(arr);
                let public = MachinePublicKey(PublicKey::from(&secret).to_bytes());
                Ok(Self { secret, public })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let keypair = Self::generate();
                keypair.save(path)?;
                Ok(keypair)
            }
            Err(e) => Err(Error::Storage(format!("{}: {e}", path.display()))),
        }
    }

    fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Storage(format!("{}: {e}", parent.display())))?;
        }
        write_private(path, self.secret.as_bytes())
            .map_err(|e| Error::Storage(format!("{}: {e}", path.display())))
    }
}

/// Writes `bytes` to `path` atomically (temp file + rename) at mode 0600.
/// Refuses (loudly) a write under the user's real `~/.cua` from a test
/// process that forgot to isolate `CUA_HOME`.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    cua_home::guard_write(path)?;
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, bytes)?;
    }
    std::fs::rename(&tmp, path)
}

/// A sender's TOFU pin store: `machine id -> public key`. Mirrors
/// `cua_relay::client::AccountLink`'s pinning of the relay's own key: the
/// first key seen for an id is trusted and persisted; any later key
/// reported for the same id is refused rather than silently replacing the
/// pin (a re-pin needs an explicit, out-of-band confirmation this crate
/// does not itself provide -- see `unpin`).
pub struct PinStore {
    path: Option<std::path::PathBuf>,
    pins: std::sync::Mutex<HashMap<String, [u8; 32]>>,
}

impl PinStore {
    /// An in-memory store.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            pins: std::sync::Mutex::default(),
        }
    }

    /// Loads (or starts) the store persisted at `path`.
    pub fn open(path: std::path::PathBuf) -> Result<Self, Error> {
        let pins = match std::fs::read(&path) {
            Ok(raw) => parse_pins(&raw)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(Error::Storage(format!("{}: {e}", path.display()))),
        };
        Ok(Self {
            path: Some(path),
            pins: std::sync::Mutex::new(pins),
        })
    }

    fn persist(&self, pins: &HashMap<String, [u8; 32]>) -> Result<(), Error> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut lines: Vec<String> = pins
            .iter()
            .map(|(id, key)| format!("{id} {}", b64().encode(key)))
            .collect();
        lines.sort();
        write_private(path, lines.join("\n").as_bytes())
            .map_err(|e| Error::Storage(format!("{}: {e}", path.display())))
    }

    /// The pinned key for `machine_id`, if any.
    pub fn get(&self, machine_id: &str) -> Option<MachinePublicKey> {
        self.pins
            .lock()
            .expect("pins")
            .get(machine_id)
            .copied()
            .map(MachinePublicKey)
    }

    /// Pins `key` for `machine_id`: trust-on-first-use. A different key for
    /// an id already pinned is refused (`Err`, the existing pin unchanged);
    /// the same key again is a harmless no-op.
    pub fn pin(&self, machine_id: &str, key: MachinePublicKey) -> Result<(), Error> {
        let mut pins = self.pins.lock().expect("pins");
        match pins.get(machine_id) {
            Some(existing) if *existing == key.0 => return Ok(()),
            Some(_) => {
                return Err(Error::Storage(format!(
                    "{machine_id} is already pinned to a different key; unpin it first if this is an intentional re-key"
                )));
            }
            None => {}
        }
        pins.insert(machine_id.to_owned(), key.0);
        self.persist(&pins)
    }

    /// Removes a pin (an intentional re-key, or a machine that is gone).
    pub fn unpin(&self, machine_id: &str) -> Result<(), Error> {
        let mut pins = self.pins.lock().expect("pins");
        pins.remove(machine_id);
        self.persist(&pins)
    }

    /// Whether `machine_id` has a pinned key.
    pub fn is_pinned(&self, machine_id: &str) -> bool {
        self.pins.lock().expect("pins").contains_key(machine_id)
    }
}

fn parse_pins(raw: &[u8]) -> Result<HashMap<String, [u8; 32]>, Error> {
    let text =
        std::str::from_utf8(raw).map_err(|_| Error::Storage("pin file is not UTF-8".into()))?;
    let mut pins = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (id, key) = line
            .split_once(' ')
            .ok_or_else(|| Error::Storage("malformed pin line".into()))?;
        let key = MachinePublicKey::from_base64url(key)?;
        pins.insert(id.to_owned(), key.0);
    }
    Ok(pins)
}

/// A sealed payload: only this struct's bytes ([`SealedEnvelope::to_bytes`])
/// are meant to cross an untrusted relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedEnvelope {
    sender_pub: [u8; 32],
    nonce: [u8; 24],
    space_id: String,
    purpose: String,
    created_at: u64,
    ciphertext: Vec<u8>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Associated data binding an envelope to its delivery: the fields a
/// tampered envelope cannot change without the AEAD tag failing, but which
/// this crate leaves visible (a relay may legitimately need the machine id
/// to route; nothing here is a secret).
fn aad(space_id: &str, purpose: &str, created_at: u64, sender_pub: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 2 + space_id.len() + 2 + purpose.len() + 8 + 32);
    out.push(VERSION);
    out.extend_from_slice(&(space_id.len() as u16).to_be_bytes());
    out.extend_from_slice(space_id.as_bytes());
    out.extend_from_slice(&(purpose.len() as u16).to_be_bytes());
    out.extend_from_slice(purpose.as_bytes());
    out.extend_from_slice(&created_at.to_be_bytes());
    out.extend_from_slice(sender_pub);
    out
}

fn derive_key(
    shared: &x25519_dalek::SharedSecret,
    sender_pub: &[u8; 32],
    recipient_pub: &[u8; 32],
) -> Zeroizing<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(b"cua-machine-seal/v1"), shared.as_bytes());
    let mut okm = Zeroizing::new([0u8; 32]);
    let mut info = Vec::with_capacity(64);
    info.extend_from_slice(sender_pub);
    info.extend_from_slice(recipient_pub);
    hk.expand(&info, okm.as_mut()).expect("32-byte okm");
    okm
}

/// Seals `plaintext` to `recipient`, bound to `space_id` and `purpose`
/// (both become associated data the AEAD tag covers, so an envelope for one
/// Space or purpose cannot be replayed as another). A fresh ephemeral
/// keypair is generated for this call alone.
pub fn seal(
    recipient: MachinePublicKey,
    space_id: &str,
    purpose: &str,
    plaintext: &[u8],
) -> Result<SealedEnvelope, Error> {
    if plaintext.len() > MAX_PLAINTEXT_LEN {
        return Err(Error::TooLarge("plaintext"));
    }
    if space_id.len() > u16::MAX as usize || purpose.len() > u16::MAX as usize {
        return Err(Error::TooLarge("space_id or purpose"));
    }
    let mut eph_bytes = [0u8; 32];
    OsRng.try_fill_bytes(&mut eph_bytes).expect("OS RNG");
    let ephemeral = StaticSecret::from(eph_bytes);
    eph_bytes.zeroize();
    let sender_pub = PublicKey::from(&ephemeral).to_bytes();
    let recipient_pub = PublicKey::from(recipient.0);
    let shared = ephemeral.diffie_hellman(&recipient_pub);
    let key = derive_key(&shared, &sender_pub, &recipient.0);

    let mut nonce_bytes = [0u8; 24];
    OsRng.try_fill_bytes(&mut nonce_bytes).expect("OS RNG");
    let created_at = now_secs();
    let cipher = XChaCha20Poly1305::new(&Key::try_from(key.as_ref()).expect("32-byte key"));
    let ad = aad(space_id, purpose, created_at, &sender_pub);
    let ciphertext = cipher
        .encrypt(
            &XNonce::try_from(nonce_bytes.as_slice()).expect("24-byte nonce"),
            Payload {
                msg: plaintext,
                aad: &ad,
            },
        )
        .map_err(|_| Error::Tampered)?;
    Ok(SealedEnvelope {
        sender_pub,
        nonce: nonce_bytes,
        space_id: space_id.to_owned(),
        purpose: purpose.to_owned(),
        created_at,
        ciphertext,
    })
}

/// Opens `envelope` with `recipient`'s secret key, checking it is for
/// `expected_space_id` / `expected_purpose` and not stale
/// ([`MAX_AGE_SECS`]) or a replay ([`ReplayGuard`]) before decrypting.
pub fn open(
    recipient: &MachineKeypair,
    expected_space_id: &str,
    expected_purpose: &str,
    replay: &ReplayGuard,
    envelope: &SealedEnvelope,
) -> Result<Zeroizing<Vec<u8>>, Error> {
    if envelope.space_id != expected_space_id {
        return Err(Error::WrongContext("space_id"));
    }
    if envelope.purpose != expected_purpose {
        return Err(Error::WrongContext("purpose"));
    }
    let now = now_secs();
    if now.abs_diff(envelope.created_at) > MAX_AGE_SECS {
        return Err(Error::Stale);
    }
    if !replay.check_and_record(&envelope.space_id, &envelope.purpose, &envelope.nonce) {
        return Err(Error::Replay);
    }
    let sender_pub = PublicKey::from(envelope.sender_pub);
    let shared = recipient.secret.diffie_hellman(&sender_pub);
    let key = derive_key(&shared, &envelope.sender_pub, &recipient.public.0);
    let cipher = XChaCha20Poly1305::new(&Key::try_from(key.as_ref()).expect("32-byte key"));
    let ad = aad(
        &envelope.space_id,
        &envelope.purpose,
        envelope.created_at,
        &envelope.sender_pub,
    );
    let plaintext = cipher
        .decrypt(
            &XNonce::try_from(envelope.nonce.as_slice()).expect("24-byte nonce"),
            Payload {
                msg: &envelope.ciphertext,
                aad: &ad,
            },
        )
        .map_err(|_| Error::Tampered)?;
    Ok(Zeroizing::new(plaintext))
}

/// Fixed 8-byte prefix on every [`SealedEnvelope::to_bytes`] wire form, so a
/// receiver can tell a sealed envelope apart from a legacy plaintext
/// payload (a teleport bundle's own tar-ish header, a raw password string)
/// by its first 8 bytes alone, without attempting to parse either.
pub const MAGIC: &[u8; 8] = b"CUASEAL1";

/// Well-known `purpose` strings, shared by every sender and the guest's
/// unseal, so both sides always agree on the exact string without
/// duplicating a literal in each crate.
pub mod purpose {
    /// A teleport session bundle delivered through
    /// `TeleportService.ImportSession` (`cua_teleport::send::upload_bundle`
    /// and its unseal in `cua-spacesd-server`).
    pub const TELEPORT_BUNDLE: &str = "teleport.bundle";
    /// A Keyvault site-login password delivered through the driver
    /// (reserved; not wired yet).
    pub const KEYVAULT_SITE_LOGIN: &str = "keyvault.site_login";
}

/// Whether `bytes` starts with [`MAGIC`]: a cheap check before attempting
/// [`SealedEnvelope::from_bytes`].
pub fn looks_sealed(bytes: &[u8]) -> bool {
    bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

impl SealedEnvelope {
    /// Self-describing wire bytes: [`MAGIC`], then version, then
    /// length-prefixed fields. This is what actually crosses the relay; it
    /// contains no plaintext.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            MAGIC.len()
                + 1
                + 32
                + 24
                + 2
                + self.space_id.len()
                + 2
                + self.purpose.len()
                + 8
                + 4
                + self.ciphertext.len(),
        );
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.sender_pub);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.space_id.len() as u16).to_be_bytes());
        out.extend_from_slice(self.space_id.as_bytes());
        out.extend_from_slice(&(self.purpose.len() as u16).to_be_bytes());
        out.extend_from_slice(self.purpose.as_bytes());
        out.extend_from_slice(&self.created_at.to_be_bytes());
        out.extend_from_slice(&(self.ciphertext.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Parses [`Self::to_bytes`]'s format.
    pub fn from_bytes(raw: &[u8]) -> Result<Self, Error> {
        if !looks_sealed(raw) {
            return Err(Error::Malformed("missing sealed-envelope magic"));
        }
        let mut r = Cursor(&raw[MAGIC.len()..]);
        let version = r.u8()?;
        if version != VERSION {
            return Err(Error::Malformed("unknown envelope version"));
        }
        let sender_pub: [u8; 32] = r.array()?;
        let nonce: [u8; 24] = r.array()?;
        let space_id_len = r.u16()? as usize;
        let space_id = r.utf8(space_id_len)?;
        let purpose_len = r.u16()? as usize;
        let purpose = r.utf8(purpose_len)?;
        let created_at = r.u64()?;
        let ct_len = r.u32()? as usize;
        if ct_len > MAX_PLAINTEXT_LEN + 64 {
            return Err(Error::TooLarge("ciphertext"));
        }
        let ciphertext = r.bytes(ct_len)?.to_vec();
        r.end()?;
        Ok(Self {
            sender_pub,
            nonce,
            space_id,
            purpose,
            created_at,
            ciphertext,
        })
    }

    /// The machine (Space) id this was sealed for, without decrypting (the
    /// relay, or a router, may read this -- it is associated data, not
    /// secret).
    pub fn space_id(&self) -> &str {
        &self.space_id
    }

    /// The purpose string this was sealed for.
    pub fn purpose(&self) -> &str {
        &self.purpose
    }
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.0.len() < n {
            return Err(Error::Malformed("truncated"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    fn utf8(&mut self, n: usize) -> Result<String, Error> {
        std::str::from_utf8(self.bytes(n)?)
            .map(str::to_owned)
            .map_err(|_| Error::Malformed("not UTF-8"))
    }

    fn end(&self) -> Result<(), Error> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Malformed("trailing bytes"))
        }
    }
}

/// Rejects a re-sent envelope: keyed by (space_id, purpose, nonce), pruned
/// by [`MAX_AGE_SECS`] (an envelope older than that is already refused by
/// [`open`], so nothing legitimate needs a longer memory than that).
pub struct ReplayGuard {
    seen: std::sync::Mutex<HashMap<ReplayKey, u64>>,
}

/// (space_id, purpose, nonce): what makes a delivery unique for replay
/// purposes.
type ReplayKey = (String, String, [u8; 24]);

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayGuard {
    /// An empty guard.
    pub fn new() -> Self {
        Self {
            seen: std::sync::Mutex::default(),
        }
    }

    /// `true` and records it the first time (space, purpose, nonce) is
    /// seen; `false` (a replay) every time after, until it ages out.
    fn check_and_record(&self, space_id: &str, purpose: &str, nonce: &[u8; 24]) -> bool {
        let now = now_secs();
        let mut seen = self.seen.lock().expect("replay guard");
        seen.retain(|_, at| now.saturating_sub(*at) <= MAX_AGE_SECS);
        let key = (space_id.to_owned(), purpose.to_owned(), *nonce);
        if seen.contains_key(&key) {
            return false;
        }
        seen.insert(key, now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let machine = MachineKeypair::generate();
        let replay = ReplayGuard::new();
        let envelope = seal(
            machine.public(),
            "space-1",
            "keyvault.site_login",
            b"s3cret",
        )
        .unwrap();
        let opened = open(
            &machine,
            "space-1",
            "keyvault.site_login",
            &replay,
            &envelope,
        )
        .unwrap();
        assert_eq!(&opened[..], b"s3cret");
    }

    #[test]
    fn tampering_fails() {
        let machine = MachineKeypair::generate();
        let replay = ReplayGuard::new();
        let mut envelope = seal(
            machine.public(),
            "space-1",
            "teleport.bundle",
            b"bundle bytes",
        )
        .unwrap();
        // Flip a bit in the ciphertext.
        envelope.ciphertext[0] ^= 1;
        assert_eq!(
            open(&machine, "space-1", "teleport.bundle", &replay, &envelope),
            Err(Error::Tampered)
        );
        // Flipping a bit in the sender's ephemeral key (also AAD) fails too.
        let mut envelope2 = seal(
            machine.public(),
            "space-1",
            "teleport.bundle",
            b"bundle bytes",
        )
        .unwrap();
        envelope2.sender_pub[0] ^= 1;
        assert_eq!(
            open(
                &machine,
                "space-1",
                "teleport.bundle",
                &ReplayGuard::new(),
                &envelope2
            ),
            Err(Error::Tampered)
        );
        // A bundle sealed for one Space id does not open as another's.
        let envelope3 = seal(machine.public(), "space-1", "teleport.bundle", b"x").unwrap();
        assert_eq!(
            open(
                &machine,
                "space-2",
                "teleport.bundle",
                &ReplayGuard::new(),
                &envelope3
            ),
            Err(Error::WrongContext("space_id"))
        );
    }

    #[test]
    fn the_wrong_machine_key_fails() {
        let machine = MachineKeypair::generate();
        let other = MachineKeypair::generate();
        let envelope = seal(
            machine.public(),
            "space-1",
            "keyvault.site_login",
            b"s3cret",
        )
        .unwrap();
        assert_eq!(
            open(
                &other,
                "space-1",
                "keyvault.site_login",
                &ReplayGuard::new(),
                &envelope
            ),
            Err(Error::Tampered)
        );
    }

    #[test]
    fn replays_are_rejected() {
        let machine = MachineKeypair::generate();
        let replay = ReplayGuard::new();
        let envelope = seal(
            machine.public(),
            "space-1",
            "keyvault.site_login",
            b"s3cret",
        )
        .unwrap();
        assert!(
            open(
                &machine,
                "space-1",
                "keyvault.site_login",
                &replay,
                &envelope
            )
            .is_ok()
        );
        assert_eq!(
            open(
                &machine,
                "space-1",
                "keyvault.site_login",
                &replay,
                &envelope
            ),
            Err(Error::Replay)
        );
    }

    #[test]
    fn a_stale_envelope_is_refused() {
        let machine = MachineKeypair::generate();
        // created_at is AAD, so an attacker cannot just edit a fresh
        // envelope's timestamp (that fails the tag, see
        // a_mutated_created_at_fails_the_tag below); simulate a genuinely
        // old envelope instead, sealed with a backdated clock.
        let old = seal_at(
            machine.public(),
            "space-1",
            "keyvault.site_login",
            b"s3cret",
            now_secs() - MAX_AGE_SECS - 60,
        );
        assert_eq!(
            open(
                &machine,
                "space-1",
                "keyvault.site_login",
                &ReplayGuard::new(),
                &old
            ),
            Err(Error::Stale)
        );
    }

    /// Test-only: seal with an explicit `created_at` (a real sender always
    /// uses [`seal`], which stamps now).
    fn seal_at(
        recipient: MachinePublicKey,
        space_id: &str,
        purpose: &str,
        plaintext: &[u8],
        created_at: u64,
    ) -> SealedEnvelope {
        let mut eph_bytes = [0u8; 32];
        OsRng.try_fill_bytes(&mut eph_bytes).unwrap();
        let ephemeral = StaticSecret::from(eph_bytes);
        let sender_pub = PublicKey::from(&ephemeral).to_bytes();
        let recipient_pub = PublicKey::from(recipient.0);
        let shared = ephemeral.diffie_hellman(&recipient_pub);
        let key = derive_key(&shared, &sender_pub, &recipient.0);
        let mut nonce_bytes = [0u8; 24];
        OsRng.try_fill_bytes(&mut nonce_bytes).unwrap();
        let cipher = XChaCha20Poly1305::new(&Key::try_from(key.as_ref()).expect("32-byte key"));
        let ad = aad(space_id, purpose, created_at, &sender_pub);
        let ciphertext = cipher
            .encrypt(
                &XNonce::try_from(nonce_bytes.as_slice()).expect("24-byte nonce"),
                Payload {
                    msg: plaintext,
                    aad: &ad,
                },
            )
            .unwrap();
        SealedEnvelope {
            sender_pub,
            nonce: nonce_bytes,
            space_id: space_id.to_owned(),
            purpose: purpose.to_owned(),
            created_at,
            ciphertext,
        }
    }

    #[test]
    fn a_mutated_created_at_fails_the_tag() {
        let machine = MachineKeypair::generate();
        let mut envelope = seal(
            machine.public(),
            "space-1",
            "keyvault.site_login",
            b"s3cret",
        )
        .unwrap();
        // A clearly different value, not just "now" again (which could
        // coincide with the original within the same second and make this
        // assertion vacuous).
        envelope.created_at = envelope.created_at.wrapping_add(1);
        assert_eq!(
            open(
                &machine,
                "space-1",
                "keyvault.site_login",
                &ReplayGuard::new(),
                &envelope
            ),
            Err(Error::Tampered)
        );
    }

    #[test]
    fn relay_visible_bytes_contain_no_plaintext() {
        let machine = MachineKeypair::generate();
        let secret = b"hunter2-this-is-the-real-password";
        let envelope = seal(machine.public(), "space-1", "keyvault.site_login", secret).unwrap();
        let wire = envelope.to_bytes();
        // The exact secret, any byte rotation of it, and its own bytes
        // reversed do not appear in what crosses the relay.
        assert!(!contains_subslice(&wire, secret));
        let mut reversed = secret.to_vec();
        reversed.reverse();
        assert!(!contains_subslice(&wire, &reversed));
        // Sanity: the harness itself can find a needle when one truly is
        // there, so the assertions above are not vacuous.
        let mut haystack = wire.clone();
        haystack.extend_from_slice(secret);
        assert!(contains_subslice(&haystack, secret));
    }

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn the_magic_prefix_tells_sealed_bytes_from_plaintext() {
        let machine = MachineKeypair::generate();
        let envelope = seal(machine.public(), "tp-1", "teleport.bundle", b"x").unwrap();
        let wire = envelope.to_bytes();
        assert!(wire.starts_with(MAGIC));
        assert!(looks_sealed(&wire));
        // A legacy plaintext bundle (or any other payload) does not.
        assert!(!looks_sealed(b"PK\x03\x04 a plain tar-ish bundle header"));
        assert!(!looks_sealed(b"short"));
        assert!(SealedEnvelope::from_bytes(b"not sealed at all").is_err());
    }

    #[test]
    fn wire_round_trip_through_bytes() {
        let machine = MachineKeypair::generate();
        let envelope = seal(
            machine.public(),
            "space-1",
            "teleport.bundle",
            b"bundle bytes",
        )
        .unwrap();
        let wire = envelope.to_bytes();
        let parsed = SealedEnvelope::from_bytes(&wire).unwrap();
        assert_eq!(parsed, envelope);
        let opened = open(
            &machine,
            "space-1",
            "teleport.bundle",
            &ReplayGuard::new(),
            &parsed,
        )
        .unwrap();
        assert_eq!(&opened[..], b"bundle bytes");
    }

    #[test]
    fn keypair_persists_0600_and_pin_store_is_tofu() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("machine.key");
        let a = MachineKeypair::load_or_create(&key_path).unwrap();
        let b = MachineKeypair::load_or_create(&key_path).unwrap();
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let pins = PinStore::open(dir.path().join("pins")).unwrap();
        assert!(!pins.is_pinned("space-1"));
        pins.pin("space-1", a.public()).unwrap();
        assert!(pins.is_pinned("space-1"));
        assert_eq!(pins.get("space-1"), Some(a.public()));
        // Re-pinning the same key is fine (idempotent).
        pins.pin("space-1", a.public()).unwrap();
        // A different key for an already-pinned id is refused.
        let other = MachineKeypair::generate();
        assert!(pins.pin("space-1", other.public()).is_err());
        assert_eq!(pins.get("space-1"), Some(a.public()));
        // Persists.
        let reopened = PinStore::open(dir.path().join("pins")).unwrap();
        assert_eq!(reopened.get("space-1"), Some(a.public()));
        reopened.unpin("space-1").unwrap();
        assert!(!reopened.is_pinned("space-1"));
    }

    #[test]
    fn base64url_round_trips() {
        let machine = MachineKeypair::generate();
        let s = machine.public().to_base64url();
        assert_eq!(
            MachinePublicKey::from_base64url(&s).unwrap(),
            machine.public()
        );
    }
}
