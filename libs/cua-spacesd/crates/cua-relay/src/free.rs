// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay without the optional hosted-relay features (the `hosted`
//! Cargo feature off): every account is Free, nothing is metered or
//! refused for allowance, and there are no Teams. These modules stand in
//! for `meter`, `teams` and `org_access` with that behaviour, so the rest
//! of the relay is the same code either way.
//!
//! - `GET /v1/usage` answers 404 "metering is not enabled on this relay"
//!   (clients read that as "no allowance applies").
//! - The Teams routes (`/v1/teams`, `/v1/teams/*`, `/join/team/{code}`)
//!   answer 410 with `{"error": "gone", "detail": "Teams are not
//!   available on this relay"}`.
//! - A pairing code for a team is refused as not found.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};

use crate::server::Relay;

/// What the Teams routes answer.
pub const TEAMS_UNAVAILABLE: &str = "Teams are not available on this relay";

/// The Teams routes: 410, not on this relay.
pub(crate) fn routes() -> Router<Relay> {
    async fn gone() -> Response {
        (
            StatusCode::GONE,
            Json(serde_json::json!({ "error": "gone", "detail": TEAMS_UNAVAILABLE })),
        )
            .into_response()
    }
    Router::new()
        .route("/v1/teams", any(gone))
        .route("/v1/teams/{*rest}", any(gone))
        .route("/join/team/{code}", any(gone))
}

/// The meter, teams and organization access of a relay without them.
pub(crate) fn open() -> (
    Arc<meter::Meter>,
    Arc<teams::Teams>,
    Arc<org_access::OrgAccess>,
) {
    (
        Arc::new(meter::Meter::default()),
        Arc::new(teams::Teams),
        Arc::new(org_access::OrgAccess),
    )
}

/// No allowance metering: no live sessions or hours are counted, nothing
/// is refused and no allowance header is sent.
pub mod meter {
    use std::sync::Arc;
    use std::time::Instant;

    use axum::http::{HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use axum::{Json, Router};

    use crate::oidc::Identity;
    use crate::server::Relay;

    /// Response header the hosted relay sets past an allowance (never set
    /// here).
    pub const ALLOWANCE_HEADER: &str = "x-cua-relay-allowance";

    /// The meter's settings: off.
    #[derive(Debug, Clone)]
    pub struct MeterConfig {
        /// Always false.
        pub enabled: bool,
        /// Unused.
        pub upgrade_url: String,
    }

    /// A meter that counts nothing.
    #[derive(Debug)]
    pub struct Meter {
        config: MeterConfig,
    }

    impl Default for Meter {
        fn default() -> Self {
            Self {
                config: MeterConfig {
                    enabled: false,
                    upgrade_url: String::new(),
                },
            }
        }
    }

    /// Never made: nothing is metered.
    #[derive(Debug)]
    pub struct MeterStream(());

    /// Never made: nothing is refused.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Refusal {
        pub code: &'static str,
        pub plan: String,
        pub used: u32,
        pub limit: u32,
    }

    impl Refusal {
        pub fn message(&self) -> String {
            String::new()
        }
    }

    impl Meter {
        pub fn config(&self) -> &MeterConfig {
            &self.config
        }

        pub fn enter(
            self: &Arc<Self>,
            _who: &Identity,
            _machine: &str,
        ) -> Option<(MeterStream, u8)> {
            None
        }

        pub fn enter_enforced(
            self: &Arc<Self>,
            _who: &Identity,
            _machine: &str,
        ) -> Result<Option<(MeterStream, u8)>, Refusal> {
            Ok(None)
        }

        pub fn enter_mesh_enforced(
            self: &Arc<Self>,
            _who: &Identity,
            _machine: &str,
        ) -> Result<Option<MeterStream>, Refusal> {
            Ok(None)
        }

        pub fn relayed(&self, _user: &str, _machine: &str, _at: Instant) {}

        pub fn mark_live(&self, _user: &str, _machine: &str) {}

        pub fn over_now(&self, _user: &str) -> u8 {
            0
        }

        pub fn persist(&self, _force: bool) {}

        pub fn spawn(self: &Arc<Self>) {}
    }

    /// No allowance header.
    pub fn allowance_header(_over: u8, _upgrade_url: &str) -> Option<HeaderValue> {
        None
    }

    /// `GET /v1/usage`: 404, as on a relay that meters nothing.
    pub(crate) fn routes() -> Router<Relay> {
        async fn usage() -> Response {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "metering is not enabled on this relay" })),
            )
                .into_response()
        }
        Router::new().route("/v1/usage", get(usage))
    }
}

/// No Teams: no machine is reached through a team.
pub mod teams {
    use std::collections::BTreeMap;

    use crate::directory::MachineRecord;
    use crate::oidc::Identity;

    /// Who acts.
    pub struct Actor<'a> {
        pub who: &'a Identity,
        pub device: Option<&'a str>,
    }

    /// A team role (never held here).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TeamRole {
        Member,
    }

    impl TeamRole {
        pub fn manages(self) -> bool {
            false
        }
    }

    /// A team device check (never asked here: no machine has teams).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum DeviceCheck {
        Approved,
        Refused(String),
    }

    /// Teams are not available on this relay.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Unavailable;

    impl std::fmt::Display for Unavailable {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(super::TEAMS_UNAVAILABLE)
        }
    }

    /// No teams.
    #[derive(Debug, Default)]
    pub struct Teams;

    impl Teams {
        pub fn get_for(&self, _id: &str, _account: &str) -> Result<((), TeamRole), Unavailable> {
            Err(Unavailable)
        }

        pub fn add_machine(
            &self,
            _id: &str,
            _actor: &Actor,
            _record: &MachineRecord,
            _host: Option<&MachineRecord>,
            _declared_os: Option<&str>,
        ) -> Result<(), Unavailable> {
            Err(Unavailable)
        }

        pub fn accept_email_invites(&self, _actor: &Actor, _device_name: &str) -> Vec<String> {
            Vec::new()
        }

        pub fn machines_for(&self, _account: &str) -> BTreeMap<String, String> {
            BTreeMap::new()
        }

        pub fn forget_machine(&self, _machine: &str) {}

        pub fn access(
            &self,
            _record: &MachineRecord,
            _host: Option<&MachineRecord>,
            _account: &str,
        ) -> Vec<(String, TeamRole)> {
            Vec::new()
        }

        pub fn device_approved_any(
            &self,
            _teams: &[String],
            _account: &str,
            _device: &str,
        ) -> bool {
            false
        }

        pub fn check_device_any(
            &self,
            _teams: &[String],
            _who: &Identity,
            _device: Option<&str>,
            _machine: &str,
        ) -> (String, DeviceCheck) {
            (
                String::new(),
                DeviceCheck::Refused(super::TEAMS_UNAVAILABLE.to_owned()),
            )
        }

        pub fn record_open(
            &self,
            _team: &str,
            _who: &Identity,
            _device: Option<&str>,
            _machine: &str,
        ) {
        }
    }
}

/// No organizations: access is the relay's own (owner, allowlist, viewers).
pub mod org_access {
    use crate::oidc::OrgClaim;

    /// Where team access comes from: nowhere but the relay's own records.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum TeamsSource {
        #[default]
        File,
        Shadow,
        Org,
    }

    impl TeamsSource {
        pub fn evaluates_orgs(self) -> bool {
            !matches!(self, Self::File)
        }
    }

    /// An organization answer (never asked here).
    #[derive(Debug, Clone, Default)]
    pub struct AccessAnswer {
        pub member: bool,
    }

    /// An organization decision (always "not shared" here).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum OrgDecision {
        Granted { org: String },
        DeviceRefused { org: String, message: String },
        NotShared,
        Unavailable(String),
    }

    /// Why sharing is refused (never here).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ShareRefusal {}

    /// No organizations, no team-only sharing.
    #[derive(Debug, Default)]
    pub struct OrgAccess;

    impl OrgAccess {
        pub fn source(&self) -> TeamsSource {
            TeamsSource::File
        }

        pub fn team_only_sharing(&self) -> bool {
            false
        }

        pub fn cached_share_allowed(&self, _owner: &str, _who: &[String]) -> Option<bool> {
            None
        }

        pub async fn share_check(
            &self,
            _owner: &str,
            _who: &[String],
        ) -> Result<Result<(), ShareRefusal>, String> {
            Ok(Ok(()))
        }

        pub async fn access(
            &self,
            _sub: &str,
            _org: &str,
            _machine: &str,
            _device: Option<&str>,
        ) -> Result<AccessAnswer, String> {
            Ok(AccessAnswer::default())
        }

        pub async fn decide(
            &self,
            _sub: &str,
            _owner: &str,
            _orgs: &[OrgClaim],
            _machine: &str,
            _device: Option<&str>,
            _device_required: bool,
        ) -> OrgDecision {
            OrgDecision::NotShared
        }

        pub async fn org_machines(&self, _org: &str) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }

        pub fn cached_org_roles(
            &self,
            _sub: &str,
            _owner: &str,
            _machine: &str,
        ) -> Vec<(String, bool)> {
            Vec::new()
        }

        pub fn cached_device_approved(&self, _sub: &str, _orgs: &[String], _device: &str) -> bool {
            false
        }

        pub fn shadow(
            &self,
            _what: &str,
            _sub: &str,
            _machine: &str,
            _file: bool,
            _org: &OrgDecision,
        ) {
        }
    }
}

/// No team-only sharing.
pub mod team_sharing {
    use crate::oidc::Identity;

    /// The names a person is known by (unused without team-only sharing).
    pub fn who_keys(who: &Identity) -> Vec<String> {
        vec![who.user.clone()]
    }
}
