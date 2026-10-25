// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A cloud's sign-in problems as one plain line with the exact fix, and the
//! guard every cloud call goes through.
//!
//! [`Guarded`] wraps each [`CloudApi`] this home uses:
//!
//! - an error that is a sign-in problem (expired, missing or rejected
//!   credentials, a revoked token) becomes `Your AWS sign-in expired. Sign in
//!   with `aws login`, then run this again.`, with the cloud's raw error kept
//!   after [`cua_sandbox_core::byoc::signin::DETAILS`] (the CLI shows it
//!   with `--verbose`) and logged;
//! - every call that lists, describes, creates, stops or deletes first checks
//!   that the credentials still reach the identity the cloud was connected
//!   with ([`CloudApi::identity`]), at most once a minute per connection. A
//!   default profile, keys in the environment or another Hetzner token that
//!   reach another account are refused: Cua never "fixes" a sign-in by acting
//!   through someone else, and never reads another account's silence as
//!   "deleted".

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_sandbox_core::Error;
use cua_sandbox_core::byoc::signin::{DETAILS, with_details};
use cua_sandbox_core::byoc::{CloudCredentials, CloudKind};

use crate::api::{CloudApi, Identity, Result, Target, Tested};
use crate::model::{Capabilities, Connection, HostType, ProvisionSpec, Resource, Tier};

/// What kind of sign-in problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The sign-in expired (an SSO or login session, a gcloud token).
    Expired,
    /// There is no sign-in (the CLI is signed out, no token).
    Missing,
    /// The cloud no longer accepts the credentials (a revoked token, deleted
    /// keys).
    Rejected,
    /// The credentials reach another account than the one connected.
    WrongAccount,
    /// The Cua sign-in a keyless link uses expired.
    CuaSignIn,
    /// The trust a keyless link uses was removed on the cloud's side.
    TrustRevoked,
}

/// A sign-in problem: what it is and the line people read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignIn {
    /// What kind.
    pub kind: Kind,
    /// One line: what happened and the exact command that fixes it.
    pub line: String,
}

impl SignIn {
    /// "Your AWS sign-in expired. Sign in with `aws login`, then run this
    /// again."
    pub fn expired(title: &str, fix: &str) -> Self {
        SignIn {
            kind: Kind::Expired,
            line: format!("Your {title} sign-in expired. Sign in with `{fix}`, then run this again."),
        }
    }

    /// "You're not signed in to AWS here. Sign in with `aws login`, then run
    /// this again."
    pub fn missing(title: &str, fix: &str) -> Self {
        SignIn {
            kind: Kind::Missing,
            line: format!(
                "You're not signed in to {title} here. Sign in with `{fix}`, then run this again."
            ),
        }
    }

    /// "Hetzner no longer accepts the saved API token (revoked or deleted).
    /// Connect it again with `...`, then run this again."
    pub fn rejected(title: &str, what: &str, fix: &str) -> Self {
        SignIn {
            kind: Kind::Rejected,
            line: format!(
                "{title} no longer accepts {what}. Connect it again with `{fix}`, then run this again."
            ),
        }
    }

    /// The credentials reach `got`, not `want`.
    pub fn wrong_account(title: &str, got: &str, want: &str, how: &str) -> Self {
        SignIn {
            kind: Kind::WrongAccount,
            line: format!(
                "Your {title} sign-in reaches {got}, not {want} that Cua connected. Sign in to {want}{how}, then run this again."
            ),
        }
    }

    /// Whether a sweep, a status or a forced disconnect should treat the
    /// cloud as "cannot be checked" (every kind is).
    pub fn unchecked(&self) -> bool {
        true
    }

    /// Why it could not be checked, a few words for "Cua couldn't check AWS
    /// ... (signed out)".
    pub fn short(&self) -> &'static str {
        match self.kind {
            Kind::Expired => "your sign-in expired",
            Kind::Missing => "signed out",
            Kind::Rejected => "its credentials were revoked",
            Kind::WrongAccount => "signed in to another account",
            Kind::CuaSignIn => "your Cua sign-in expired",
            Kind::TrustRevoked => "the trust was removed in that cloud",
        }
    }
}

/// Whether `raw` (a cloud's error, lowercase or not) contains any of
/// `needles` (compared lowercase).
pub fn mentions(raw: &str, needles: &[&str]) -> bool {
    let raw = raw.to_ascii_lowercase();
    needles.iter().any(|n| raw.contains(&n.to_ascii_lowercase()))
}

/// `e` as its sign-in line when `api` says it is a sign-in problem (the raw
/// error after the details separator, and in the log); otherwise `e`.
pub fn friendly(api: &dyn CloudApi, conn: &Connection, e: Error) -> Error {
    let raw = e.to_string();
    if raw.contains(DETAILS) {
        return e;
    }
    match api.sign_in_help(conn, &raw) {
        Some(s) => {
            tracing::warn!(provider = api.name(), "{} ({raw})", s.line);
            Error::Cloud(with_details(&s.line, strip_kind(&raw)))
        }
        None => e,
    }
}

/// The sign-in problem behind `e`, if it is one (before or after
/// [`friendly`]).
pub fn sign_in_of(api: &dyn CloudApi, conn: &Connection, e: &Error) -> Option<SignIn> {
    let raw = e.to_string();
    let (plain, details) = cua_sandbox_core::byoc::signin::split(&raw);
    let plain = strip_kind(plain);
    let line = |kind| {
        Some(SignIn {
            kind,
            line: plain.to_string(),
        })
    };
    if plain.contains(" sign-in reaches ") {
        return line(Kind::WrongAccount);
    }
    if details.is_some() {
        // Already a sign-in line (from [`friendly`]): its kind by its words.
        return if plain.contains("Your Cua sign-in expired") {
            line(Kind::CuaSignIn)
        } else if plain.contains(" no longer trusts Cua") {
            line(Kind::TrustRevoked)
        } else if plain.contains(" sign-in expired. ") {
            line(Kind::Expired)
        } else if plain.starts_with("You're not signed in to ") {
            line(Kind::Missing)
        } else if plain.contains(" no longer accepts ") {
            line(Kind::Rejected)
        } else {
            api.sign_in_help(conn, details.unwrap_or_default())
        };
    }
    api.sign_in_help(conn, &raw)
}

/// `raw` without the `your cloud: ` kind prefix [`Error::Cloud`] adds.
fn strip_kind(raw: &str) -> &str {
    raw.strip_prefix("your cloud: ").unwrap_or(raw)
}

/// How long a checked identity is trusted before it is checked again.
pub const IDENTITY_TTL: Duration = Duration::from_secs(60);

/// A [`CloudApi`] whose errors read as sign-in lines and whose calls check
/// the identity first (see the module docs).
pub struct Guarded {
    inner: Arc<dyn CloudApi>,
    checked: Mutex<HashMap<String, Instant>>,
}

impl Guarded {
    /// `api`, guarded.
    pub fn wrap(api: Arc<dyn CloudApi>) -> Arc<dyn CloudApi> {
        Arc::new(Guarded {
            inner: api,
            checked: Mutex::new(HashMap::new()),
        })
    }

    fn key(conn: &Connection, want: &str) -> String {
        format!("{}|{}|{}|{want}", conn.profile, conn.project, conn.environment)
    }

    /// Checks `conn`'s credentials still reach the identity it was connected
    /// with: a check younger than [`IDENTITY_TTL`] does for reads; a call
    /// that creates, changes or deletes (`fresh`) always asks.
    async fn verify(&self, conn: &Connection, fresh: bool) -> Result<()> {
        let want = self.inner.expected_identity(conn);
        if want.is_empty() {
            return Ok(());
        }
        let key = Self::key(conn, &want);
        if !fresh
            && self
                .checked
                .lock()
                .unwrap()
                .get(&key)
                .is_some_and(|t| t.elapsed() < IDENTITY_TTL)
        {
            return Ok(());
        }
        let id = self.identity(conn).await?;
        match id.id {
            Some(got) if got != want => Err(Error::Cloud(self.wrong(conn, &got, &want))),
            _ => Ok(()),
        }
    }

    fn wrong(&self, conn: &Connection, got: &str, want: &str) -> String {
        let how = if conn.profile.is_empty() {
            String::new()
        } else {
            format!(" (profile {})", conn.profile)
        };
        SignIn::wrong_account(self.inner.title(), got, want, &how).line
    }

    fn friendly(&self, conn: &Connection, e: Error) -> Error {
        friendly(self.inner.as_ref(), conn, e)
    }
}

#[async_trait]
impl CloudApi for Guarded {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn title(&self) -> &'static str {
        self.inner.title()
    }
    fn tier(&self) -> Tier {
        self.inner.tier()
    }
    fn arches(&self) -> Vec<&'static str> {
        self.inner.arches()
    }
    fn runtimes(&self) -> Vec<cua_sandbox_core::placement::Runtime> {
        self.inner.runtimes()
    }
    fn detect(&self) -> CloudCredentials {
        self.inner.detect()
    }
    fn resolve(&self, target: &Target) -> Result<Connection> {
        self.inner.resolve(target)
    }
    fn stage_secret(&self, target: &Target) {
        self.inner.stage_secret(target)
    }
    fn persist_secret(&self) -> Result<()> {
        self.inner.persist_secret()
    }
    fn forget_secret(&self) {
        self.inner.forget_secret()
    }
    fn kinds(&self, conn: &Connection) -> Vec<CloudKind> {
        self.inner.kinds(conn)
    }

    async fn test(&self, conn: &Connection) -> Tested {
        let mut t = self.inner.test(conn).await;
        for c in t.checks.iter_mut().filter(|c| !c.ok) {
            if let Some(s) = self.inner.sign_in_help(conn, &c.detail) {
                tracing::warn!(provider = self.name(), "{} ({})", s.line, c.detail);
                c.detail = with_details(&s.line, &c.detail);
            }
        }
        t
    }

    async fn provision(&self, conn: &Connection, spec: &ProvisionSpec) -> Result<Vec<Resource>> {
        self.verify(conn, true).await?;
        self.inner
            .provision(conn, spec)
            .await
            .map_err(|e| self.friendly(conn, e))
    }

    async fn describe(
        &self,
        conn: &Connection,
        r: &Resource,
    ) -> Result<Option<(String, BTreeMap<String, String>)>> {
        self.verify(conn, false).await?;
        let found = self
            .inner
            .describe(conn, r)
            .await
            .map_err(|e| self.friendly(conn, e))?;
        // "Not found" is a verdict (gone): only from the right account.
        if found.is_none() {
            self.verify(conn, true).await?;
        }
        Ok(found)
    }

    async fn stop(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.verify(conn, true).await?;
        self.inner
            .stop(conn, r, owner)
            .await
            .map_err(|e| self.friendly(conn, e))
    }

    async fn start(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.verify(conn, true).await?;
        self.inner
            .start(conn, r, owner)
            .await
            .map_err(|e| self.friendly(conn, e))
    }

    async fn delete(&self, conn: &Connection, r: &Resource, owner: &str) -> Result<()> {
        self.verify(conn, true).await?;
        self.inner
            .delete(conn, r, owner)
            .await
            .map_err(|e| self.friendly(conn, e))
    }

    async fn list_owned(&self, conn: &Connection, owner: &str) -> Result<Vec<Resource>> {
        self.verify(conn, false).await?;
        self.inner
            .list_owned(conn, owner)
            .await
            .map_err(|e| self.friendly(conn, e))
    }

    fn capabilities(&self, conn: &Connection) -> Capabilities {
        self.inner.capabilities(conn)
    }

    async fn host_types(&self, conn: &Connection) -> Result<Vec<HostType>> {
        self.inner
            .host_types(conn)
            .await
            .map_err(|e| self.friendly(conn, e))
    }

    async fn boot_log(&self, conn: &Connection, r: &Resource) -> Result<Option<String>> {
        self.inner.boot_log(conn, r).await
    }

    async fn identity(&self, conn: &Connection) -> Result<Identity> {
        let id = self
            .inner
            .identity(conn)
            .await
            .map_err(|e| self.friendly(conn, e))?;
        let want = self.inner.expected_identity(conn);
        if !want.is_empty() && id.id.as_deref().is_none_or(|got| got == want) {
            self.checked
                .lock()
                .unwrap()
                .insert(Self::key(conn, &want), Instant::now());
        }
        Ok(id)
    }

    fn expected_identity(&self, conn: &Connection) -> String {
        self.inner.expected_identity(conn)
    }

    fn sign_in_help(&self, conn: &Connection, error: &str) -> Option<SignIn> {
        self.inner.sign_in_help(conn, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_one_sentence_pair_with_the_command() {
        assert_eq!(
            SignIn::expired("AWS", "aws login").line,
            "Your AWS sign-in expired. Sign in with `aws login`, then run this again."
        );
        assert!(!SignIn::missing("Modal", "modal token new").line.contains('\n'));
        assert!(mentions("ExpiredToken: x", &["expiredtoken"]));
    }
}
