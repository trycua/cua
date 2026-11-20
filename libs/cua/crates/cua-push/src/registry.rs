// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The account's phones and preferences.
//!
//! One JSON file, written atomically (a temporary file, then a rename).
//! A token the operating system gave a phone is a credential for pushing to
//! it, so it is only ever returned to the provider, never listed back: a
//! listing shows an id, the name and when it registered.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::event::EventKind;

/// Most phones one account registers.
pub const MAX_PER_ACCOUNT: usize = 10;
/// Most registrations an account holds that no enrolled device made (made
/// while the relay lets unenrolled devices through, or with enrollment off).
/// They cannot replace a registration an enrolled device made.
pub const MAX_UNBOUND: usize = 3;
/// Longest device token accepted, in characters.
pub const TOKEN_MAX: usize = 512;
/// Shortest.
pub const TOKEN_MIN: usize = 32;

/// What the phone runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    /// Reserved: no Android provider is built yet.
    Android,
}

/// The two switches a phone has: what needs the person, and what ended.
pub const CATEGORIES: [&str; 2] = ["needs_you", "done"];

/// A registered phone.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    /// The phone's device id (`dev_...`, the one it enrolled with): a
    /// phone has one registration, and registering again replaces it.
    pub id: String,
    pub platform: Platform,
    /// The token the operating system gave the app.
    pub token: String,
    /// The app's bundle id (the APNs topic it must match).
    #[serde(default)]
    pub app: String,
    /// A development build (APNs sandbox).
    #[serde(default)]
    pub sandbox: bool,
    /// The phone's name, for the person ("Dillon's iPhone").
    #[serde(default)]
    pub name: String,
    /// Which alerts this phone takes: `needs_you` (an approval waits, an
    /// agent is blocked) and `done` (a task finished or failed).
    #[serde(default = "all_categories")]
    pub categories: Vec<String>,
    /// The language of the alerts' words.
    #[serde(default)]
    pub locale: String,
    /// Made by an enrolled device (its session proved the id). Absent in
    /// files from before: not bound.
    #[serde(default)]
    pub bound: bool,
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("id", &self.id)
            .field("platform", &self.platform)
            .field("token", &"<redacted>")
            .field("app", &self.app)
            .field("sandbox", &self.sandbox)
            .field("name", &self.name)
            .field("categories", &self.categories)
            .field("locale", &self.locale)
            .field("bound", &self.bound)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

fn all_categories() -> Vec<String> {
    CATEGORIES.iter().map(|c| c.to_string()).collect()
}

impl Registration {
    /// Whether this phone takes alerts of `kind`.
    pub fn takes(&self, kind: EventKind) -> bool {
        let c = match kind {
            EventKind::ApprovalNeeded | EventKind::AgentBlocked => "needs_you",
            EventKind::TaskFinished | EventKind::TaskFailed => "done",
        };
        self.categories.iter().any(|x| x == c)
    }
}

/// What a listing shows: no token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationView {
    pub id: String,
    pub platform: Platform,
    pub app: String,
    pub sandbox: bool,
    pub name: String,
    pub categories: Vec<String>,
    pub created_at: u64,
}

impl From<&Registration> for RegistrationView {
    fn from(r: &Registration) -> Self {
        RegistrationView {
            id: r.id.clone(),
            platform: r.platform,
            app: r.app.clone(),
            sandbox: r.sandbox,
            name: r.name.clone(),
            categories: r.categories.clone(),
            created_at: r.created_at,
        }
    }
}

/// Which alerts the account wants. All on by default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    /// The master switch: off sends nothing.
    pub enabled: bool,
    pub approval_needed: bool,
    pub task_finished: bool,
    pub task_failed: bool,
    pub agent_blocked: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            enabled: true,
            approval_needed: true,
            task_finished: true,
            task_failed: true,
            agent_blocked: true,
        }
    }
}

impl Preferences {
    /// Whether an alert of `kind` is wanted.
    pub fn wants(&self, kind: EventKind) -> bool {
        self.enabled
            && match kind {
                EventKind::ApprovalNeeded => self.approval_needed,
                EventKind::TaskFinished => self.task_finished,
                EventKind::TaskFailed => self.task_failed,
                EventKind::AgentBlocked => self.agent_blocked,
            }
    }
}

/// A partial change: only what is present is set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferencesPatch {
    pub enabled: Option<bool>,
    pub approval_needed: Option<bool>,
    pub task_finished: Option<bool>,
    pub task_failed: Option<bool>,
    pub agent_blocked: Option<bool>,
}

impl Preferences {
    pub fn apply(&mut self, p: &PreferencesPatch) {
        if let Some(v) = p.enabled {
            self.enabled = v;
        }
        if let Some(v) = p.approval_needed {
            self.approval_needed = v;
        }
        if let Some(v) = p.task_finished {
            self.task_finished = v;
        }
        if let Some(v) = p.task_failed {
            self.task_failed = v;
        }
        if let Some(v) = p.agent_blocked {
            self.agent_blocked = v;
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AccountPush {
    #[serde(default)]
    phones: Vec<Registration>,
    #[serde(default)]
    prefs: Option<Preferences>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    accounts: BTreeMap<String, AccountPush>,
}

/// Why a change was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Limit(String),
    #[error("no such registration")]
    NotFound,
    #[error("could not save: {0}")]
    Storage(String),
}

/// The registry. In memory without a path.
pub struct Registry {
    path: Option<PathBuf>,
    state: Mutex<State>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// What a refused registration without an enrolled device says.
pub const UNBOUND_REFUSED: &str =
    "this phone cannot be registered now: enroll it, or remove a registration";

/// A device id as the phone makes it (`dev_...`): letters, digits, `_`, `-`.
fn valid_device(d: &str) -> bool {
    (8..=64).contains(&d.len())
        && d.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A device token as the operating system gives it: hex for APNs, a long
/// token string for others. Printable, no spaces, a sane length.
fn valid_token(t: &str) -> bool {
    (TOKEN_MIN..=TOKEN_MAX).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:.=/+".contains(&b))
}

impl Registry {
    pub fn in_memory() -> Registry {
        Registry {
            path: None,
            state: Mutex::default(),
        }
    }

    /// Loads `path` (missing: empty). A file that does not parse is an
    /// error, never silently replaced.
    pub fn open(path: impl Into<PathBuf>) -> Result<Registry, RegistryError> {
        let path = path.into();
        let state = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| RegistryError::Storage(format!("{}: {e}", path.display())))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(RegistryError::Storage(format!("{}: {e}", path.display()))),
        };
        Ok(Registry {
            path: Some(path),
            state: Mutex::new(state),
        })
    }

    fn save(&self, state: &State) -> Result<(), RegistryError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let io = |e: std::io::Error| RegistryError::Storage(e.to_string());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("tmp");
        let text =
            serde_json::to_vec_pretty(state).map_err(|e| RegistryError::Storage(e.to_string()))?;
        {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp).map_err(io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(io)?;
            }
            f.write_all(&text).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// Registers the phone `device` for `account`, replacing its earlier
    /// registration. A token another phone or account held moves here (the
    /// phone changed hands, or its owner signed in as someone else).
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &self,
        account: &str,
        device: &str,
        platform: Platform,
        token: &str,
        app: &str,
        sandbox: bool,
        name: &str,
        categories: Option<&[String]>,
        locale: &str,
        bound: bool,
    ) -> Result<Registration, RegistryError> {
        let token = token.trim();
        if !valid_token(token) {
            return Err(RegistryError::Invalid(
                "token: the device token the system gave the app".into(),
            ));
        }
        if !valid_device(device) {
            return Err(RegistryError::Invalid(
                "device: the phone's device id".into(),
            ));
        }
        if app.chars().count() > 128 || name.chars().count() > 80 || locale.chars().count() > 35 {
            return Err(RegistryError::Invalid(
                "app, name or locale is too long".into(),
            ));
        }
        let categories: Vec<String> = match categories {
            None => all_categories(),
            Some(c) => {
                let mut out: Vec<String> = vec![];
                for x in c {
                    if !CATEGORIES.contains(&x.as_str()) {
                        return Err(RegistryError::Invalid(format!(
                            "categories: {x:?} is not one of {CATEGORIES:?}"
                        )));
                    }
                    if !out.contains(x) {
                        out.push(x.clone());
                    }
                }
                out
            }
        };
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        let mut st = self.state.lock().expect("push registry");
        let now = now_ms();
        let mut created_at = now;
        // Without an enrolled device behind the call, nothing a device
        // registered is replaced, and only a few registrations are held
        // (one answer for both refusals: nothing says which it was).
        if !bound {
            let mine = st.accounts.get(account);
            let replaces_bound = mine.is_some_and(|a| {
                a.phones
                    .iter()
                    .any(|p| p.bound && (p.id == device || p.token == token))
            });
            let unbound = mine.map_or(0, |a| {
                a.phones
                    .iter()
                    .filter(|p| !p.bound && p.id != device && p.token != token)
                    .count()
            });
            if replaces_bound || unbound >= MAX_UNBOUND {
                return Err(RegistryError::Limit(UNBOUND_REFUSED.into()));
            }
        }
        for (acct, a) in st.accounts.iter_mut() {
            a.phones.retain(|p| {
                let same_phone = acct == account && p.id == device;
                if same_phone {
                    created_at = p.created_at;
                }
                !(same_phone || p.token == token)
            });
        }
        let mine = st.accounts.entry(account.to_string()).or_default();
        if mine.phones.len() >= MAX_PER_ACCOUNT {
            return Err(RegistryError::Limit(format!(
                "at most {MAX_PER_ACCOUNT} phones per account: remove one first"
            )));
        }
        let reg = Registration {
            id: device.to_string(),
            platform,
            token: token.to_string(),
            app: app.trim().to_string(),
            sandbox,
            name: name.trim().to_string(),
            categories,
            locale: locale.trim().to_string(),
            bound,
            created_at,
            updated_at: now,
        };
        mine.phones.push(reg.clone());
        self.save(&st)?;
        Ok(reg)
    }

    /// Who holds `token`, if anyone: `(account, device id)`. A push token
    /// is a secret of one physical phone; this is how a registration of the
    /// same token under another account is noticed.
    pub fn holder(&self, token: &str) -> Option<(String, String)> {
        let st = self.state.lock().expect("push registry");
        st.accounts.iter().find_map(|(acct, a)| {
            a.phones
                .iter()
                .find(|p| p.token == token)
                .map(|p| (acct.clone(), p.id.clone()))
        })
    }

    /// Removes registration `id` of `account`.
    pub fn unregister(&self, account: &str, id: &str) -> Result<(), RegistryError> {
        let mut st = self.state.lock().expect("push registry");
        let a = st
            .accounts
            .get_mut(account)
            .ok_or(RegistryError::NotFound)?;
        let before = a.phones.len();
        a.phones.retain(|p| p.id != id);
        if a.phones.len() == before {
            return Err(RegistryError::NotFound);
        }
        self.save(&st)
    }

    /// Removes the registration with `token` of `account`, if any.
    pub fn unregister_token(&self, account: &str, token: &str) -> Result<bool, RegistryError> {
        let mut st = self.state.lock().expect("push registry");
        let Some(a) = st.accounts.get_mut(account) else {
            return Ok(false);
        };
        let before = a.phones.len();
        a.phones.retain(|p| p.token != token);
        let removed = a.phones.len() != before;
        if removed {
            self.save(&st)?;
        }
        Ok(removed)
    }

    /// Every registration of `account`, tokens included (for providers).
    pub fn phones(&self, account: &str) -> Vec<Registration> {
        let st = self.state.lock().expect("push registry");
        st.accounts
            .get(account)
            .map(|a| a.phones.clone())
            .unwrap_or_default()
    }

    /// What a listing shows.
    pub fn list(&self, account: &str) -> Vec<RegistrationView> {
        self.phones(account)
            .iter()
            .map(RegistrationView::from)
            .collect()
    }

    pub fn preferences(&self, account: &str) -> Preferences {
        let st = self.state.lock().expect("push registry");
        st.accounts
            .get(account)
            .and_then(|a| a.prefs.clone())
            .unwrap_or_default()
    }

    pub fn set_preferences(
        &self,
        account: &str,
        patch: &PreferencesPatch,
    ) -> Result<Preferences, RegistryError> {
        let mut st = self.state.lock().expect("push registry");
        let a = st.accounts.entry(account.to_string()).or_default();
        let mut p = a.prefs.clone().unwrap_or_default();
        p.apply(patch);
        a.prefs = Some(p.clone());
        self.save(&st)?;
        Ok(p)
    }

    /// Forgets everything about `account` (account erasure).
    pub fn erase(&self, account: &str) -> Result<(), RegistryError> {
        let mut st = self.state.lock().expect("push registry");
        if st.accounts.remove(account).is_some() {
            self.save(&st)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn dev(n: u8) -> String {
        format!("dev_{n:08}")
    }

    fn reg(r: &Registry, acct: &str, n: u8, token: u8) -> Result<Registration, RegistryError> {
        r.register(
            acct,
            &dev(n),
            Platform::Ios,
            &tok(token),
            "ai.cua.spaces.mobile",
            false,
            "iPhone",
            None,
            "en",
            true,
        )
    }

    #[test]
    fn register_list_and_unregister() {
        let r = Registry::in_memory();
        let a = reg(&r, "acct", 1, 1).unwrap();
        assert_eq!(a.id, dev(1));
        assert_eq!(a.categories, ["needs_you", "done"]);
        // The same phone again replaces its registration (a new token).
        let again = r
            .register(
                "acct",
                &dev(1),
                Platform::Ios,
                &tok(9),
                "ai.cua.spaces.mobile",
                true,
                "New name",
                Some(&["done".to_string()]),
                "en",
                true,
            )
            .unwrap();
        assert_eq!(again.created_at, a.created_at);
        assert!(again.sandbox);
        assert_eq!(r.list("acct").len(), 1);
        assert_eq!(r.phones("acct")[0].token, tok(9));
        let json = serde_json::to_string(&r.list("acct")).unwrap();
        assert!(!json.contains(&tok(9)), "a listing never shows the token");
        r.unregister("acct", &dev(1)).unwrap();
        assert_eq!(r.unregister("acct", &dev(1)), Err(RegistryError::NotFound));
        assert!(r.phones("acct").is_empty());
    }

    #[test]
    fn a_token_moves_to_the_phone_that_registers_it_last() {
        let r = Registry::in_memory();
        reg(&r, "a", 1, 1).unwrap();
        reg(&r, "b", 2, 1).unwrap();
        assert!(r.phones("a").is_empty());
        assert_eq!(r.phones("b").len(), 1);
        // Two devices of one account never share a token either.
        reg(&r, "b", 3, 1).unwrap();
        assert_eq!(r.phones("b").len(), 1);
        assert_eq!(r.phones("b")[0].id, dev(3));
    }

    #[test]
    fn bad_input_and_the_per_account_limit_are_refused() {
        let r = Registry::in_memory();
        for bad in [
            "",
            "short",
            &"z y".repeat(20),
            &"a".repeat(TOKEN_MAX + 1),
            "tok\nen0123456789012345678901234567",
        ] {
            assert!(
                r.register(
                    "a",
                    &dev(1),
                    Platform::Ios,
                    bad,
                    "",
                    false,
                    "",
                    None,
                    "",
                    true
                )
                .is_err(),
                "{bad:?}"
            );
        }
        assert!(
            r.register(
                "a",
                "x",
                Platform::Ios,
                &tok(1),
                "",
                false,
                "",
                None,
                "",
                true
            )
            .is_err()
        );
        assert!(
            r.register(
                "a",
                "../../etc/passwd",
                Platform::Ios,
                &tok(1),
                "",
                false,
                "",
                None,
                "",
                true
            )
            .is_err()
        );
        assert!(
            r.register(
                "a",
                &dev(1),
                Platform::Ios,
                &tok(1),
                "",
                false,
                "",
                Some(&["nope".to_string()]),
                "",
                true
            )
            .is_err()
        );
        for n in 0..MAX_PER_ACCOUNT as u8 {
            reg(&r, "a", n + 10, n).unwrap();
        }
        assert!(matches!(
            reg(&r, "a", 200, 200),
            Err(RegistryError::Limit(_))
        ));
        // Replacing one that exists is still fine at the limit.
        reg(&r, "a", 10, 77).unwrap();
    }

    #[test]
    fn a_phone_takes_the_categories_it_chose() {
        let r = Registry::in_memory();
        let done_only = r
            .register(
                "a",
                &dev(1),
                Platform::Ios,
                &tok(1),
                "",
                false,
                "",
                Some(&["done".to_string()]),
                "",
                true,
            )
            .unwrap();
        assert!(done_only.takes(EventKind::TaskFinished) && done_only.takes(EventKind::TaskFailed));
        assert!(
            !done_only.takes(EventKind::ApprovalNeeded)
                && !done_only.takes(EventKind::AgentBlocked)
        );
        let none = r
            .register(
                "a",
                &dev(2),
                Platform::Ios,
                &tok(2),
                "",
                false,
                "",
                Some(&[]),
                "",
                true,
            )
            .unwrap();
        assert!(EventKind::ALL.iter().all(|k| !none.takes(*k)));
    }

    #[test]
    fn preferences_default_on_and_patch() {
        let r = Registry::in_memory();
        assert!(r.preferences("a").wants(EventKind::TaskFailed));
        let p = r
            .set_preferences(
                "a",
                &PreferencesPatch {
                    task_finished: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!p.wants(EventKind::TaskFinished));
        assert!(p.wants(EventKind::ApprovalNeeded));
        let off = r
            .set_preferences(
                "a",
                &PreferencesPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(EventKind::ALL.iter().all(|k| !off.wants(*k)));
    }

    #[test]
    fn the_file_survives_a_restart_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("push.json");
        {
            let r = Registry::open(&path).unwrap();
            reg(&r, "a", 3, 3).unwrap();
            r.set_preferences(
                "a",
                &PreferencesPatch {
                    agent_blocked: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let r = Registry::open(&path).unwrap();
        assert_eq!(r.phones("a").len(), 1);
        assert!(!r.preferences("a").agent_blocked);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // A file that does not parse is refused, not replaced.
        std::fs::write(&path, "{nope").unwrap();
        assert!(Registry::open(&path).is_err());
    }

    #[test]
    fn erasing_an_account_forgets_its_phones() {
        let r = Registry::in_memory();
        reg(&r, "a", 1, 1).unwrap();
        r.erase("a").unwrap();
        assert!(r.phones("a").is_empty());
    }

    #[test]
    fn without_an_enrolled_device_few_registrations_are_held_and_none_replaced() {
        let r = Registry::in_memory();
        let unbound = |dev_n: u8, token: u8| {
            r.register(
                "a",
                &dev(dev_n),
                Platform::Ios,
                &tok(token),
                "",
                false,
                "",
                None,
                "",
                false,
            )
        };
        for n in 0..MAX_UNBOUND as u8 {
            unbound(n, n).unwrap();
        }
        // One more is refused, with the one answer.
        let refused = unbound(50, 50).unwrap_err();
        assert_eq!(refused, RegistryError::Limit(UNBOUND_REFUSED.into()));
        // Replacing an unbound one is fine; the cap of the account is unaffected.
        unbound(0, 90).unwrap();
        // A device's own registration cannot be replaced from outside (by its
        // id or its token), and gives the same answer.
        reg(&r, "a", 80, 80).unwrap();
        assert_eq!(unbound(80, 81).unwrap_err(), refused);
        assert_eq!(unbound(81, 80).unwrap_err(), refused);
        assert_eq!(
            r.phones("a")
                .iter()
                .find(|p| p.id == dev(80))
                .unwrap()
                .token,
            tok(80)
        );
        // The whole account still stops at its cap.
        let r = Registry::in_memory();
        for n in 0..MAX_PER_ACCOUNT as u8 {
            reg(&r, "b", n, n).unwrap();
        }
        assert!(matches!(reg(&r, "b", 99, 99), Err(RegistryError::Limit(_))));
        assert!(
            r.register(
                "b",
                &dev(98),
                Platform::Ios,
                &tok(98),
                "",
                false,
                "",
                None,
                "",
                false
            )
            .is_err()
        );
    }
}
