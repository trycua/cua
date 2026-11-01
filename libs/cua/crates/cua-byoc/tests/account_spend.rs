// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A linked cloud's budget is the account's: each device reports what it
//! spent there to Cua's issuer, which adds the devices up, and a create on
//! any device is checked against that total. Stopping and deleting always
//! work. An issuer without spend reports (older), or one that can't be
//! reached, never blocks anything: this device's own figure decides.
#![cfg(feature = "federation")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_byoc::federation::{
    IssuerClient, IssuerError, IssuerLimits, MintedToken, ServerConnection, TokenRequest,
};
use cua_byoc::model::{Auth, Tier};
use cua_byoc::testing::FakeProvider;
use cua_byoc::{RelayAccess, Target};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::byoc::CloudManager as _;
use cua_sandbox_core::{CreateOptions, ProviderKind, Sandboxes};

/// How the fake issuer answers spend reports.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Adds this device's report to what the other devices spent.
    Adds,
    /// An older issuer: no spend route (404), the connection is listed.
    Older,
    /// Can't be reached.
    Offline,
}

struct Issuer {
    mode: Mutex<Mode>,
    /// What the account's other devices spent this month.
    others: Mutex<f64>,
    /// This device's reports: (connection, month, usd).
    reports: Mutex<Vec<(String, String, f64)>>,
}

impl Issuer {
    fn new(mode: Mode, others: f64) -> Arc<Self> {
        Arc::new(Issuer {
            mode: Mutex::new(mode),
            others: Mutex::new(others),
            reports: Mutex::default(),
        })
    }

    fn mine(&self) -> f64 {
        self.reports.lock().unwrap().last().map_or(0.0, |r| r.2)
    }

    fn total(&self) -> f64 {
        *self.others.lock().unwrap() + self.mine()
    }
}

#[async_trait]
impl IssuerClient for Issuer {
    async fn token(&self, _req: &TokenRequest) -> Result<MintedToken, IssuerError> {
        Err(IssuerError::Other("no tokens here".into()))
    }

    async fn connections(&self) -> Result<Vec<ServerConnection>, IssuerError> {
        if *self.mode.lock().unwrap() == Mode::Offline {
            return Err(IssuerError::Unreachable("offline".into()));
        }
        Ok(vec![ServerConnection {
            id: "conn_1".into(),
            cloud: "aws".into(),
            auth: "oidc".into(),
            status: "connected".into(),
            limits: IssuerLimits {
                budget_usd_month: Some(10.0),
                ..Default::default()
            },
            spent_month_usd: (*self.mode.lock().unwrap() == Mode::Adds).then(|| self.total()),
            ..Default::default()
        }])
    }

    async fn report_spend(&self, id: &str, month: &str, usd: f64) -> Result<f64, IssuerError> {
        match *self.mode.lock().unwrap() {
            Mode::Offline => return Err(IssuerError::Unreachable("offline".into())),
            Mode::Older => {
                self.reports
                    .lock()
                    .unwrap()
                    .push((id.into(), month.into(), usd));
                return Err(IssuerError::NotFound(String::new()));
            }
            Mode::Adds => {}
        }
        self.reports
            .lock()
            .unwrap()
            .push((id.into(), month.into(), usd));
        Ok(self.total())
    }
}

struct World {
    dir: tempfile::TempDir,
    _relay: Arc<FakeRelay>,
    sandboxes: Sandboxes,
    fake: Arc<FakeProvider>,
}

/// One device with AWS linked through the issuer (budget $10 a month).
async fn world(issuer: Arc<Issuer>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let relay = Arc::new(FakeRelay::start().await);
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let fake = FakeProvider::new("aws", Tier::Vm);
    let joined = relay.clone();
    fake.on_boot(Arc::new(move |spec, _| {
        joined.set_online(&spec.tags[cua_byoc::model::tag::MACHINE], true, "0.1.0");
    }));
    let access = RelayAccess::new(
        relay.url.clone(),
        Arc::new(cua_host::relay::StaticToken("acct-token".into())),
    );
    let (providers, clouds) = cua_byoc::clouds(dir.path(), vec![fake.clone()], access);
    clouds.set_issuer(issuer);
    let mut b = Sandboxes::builder().state_dir(dir.path().join("sandboxes"));
    for p in providers {
        b = b.provider(p);
    }
    let sandboxes = b.clouds(clouds.clone()).build();
    clouds
        .connect(
            &Target {
                provider: "aws".into(),
                region: Some("us-west-2".into()),
                ..Default::default()
            },
            false,
            Some(2),
        )
        .await
        .unwrap();
    // Linked through the issuer, as `cua cloud connect aws --oidc` leaves it.
    let store = cua_byoc::Store::new(dir.path());
    let mut conn = store.connection("aws").unwrap().unwrap();
    conn.auth = Auth::aws("conn_1", "arn:aws:iam::123456789012:role/cua-spaces");
    conn.budget_usd_month = Some(10.0);
    store.connect(conn).unwrap();
    World {
        dir,
        _relay: relay,
        sandboxes,
        fake,
    }
}

const IMAGE: &str = "registry.invalid/trycua/linux:24.04";

fn create(name: &str) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Contrib, IMAGE);
    o.contrib = Some("aws".into());
    o.name = Some(name.into());
    o
}

/// Waits for the background reports to land.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

#[tokio::test]
async fn the_accounts_devices_together_reach_the_budget_and_no_device_creates_more() {
    let issuer = Issuer::new(Mode::Adds, 0.0);
    let w = world(issuer.clone()).await;

    // Under budget: this device creates, and reports what it spends.
    w.sandboxes.create(create("first")).await.unwrap();
    settle().await;
    let reports = issuer.reports.lock().unwrap().clone();
    assert!(
        !reports.is_empty(),
        "the create reported this device's spend"
    );
    let month = reports[0].1.clone();
    assert_eq!(month.len(), 7, "{month}");
    assert!(reports.iter().all(|r| r.0 == "conn_1"));

    // Two other devices spent $6 and $5 this month: the account is over.
    *issuer.others.lock().unwrap() = 11.0;
    let before = w.fake.calls().len();
    let err = w.sandboxes.create(create("second")).await.unwrap_err();
    assert!(err.to_string().contains("this month is reached"), "{err}");
    assert_eq!(w.fake.calls().len(), before, "no cloud call");

    // cloud_status shows the account's month, not only this device's.
    let clouds = w.sandboxes.clouds().unwrap();
    let status = clouds.status(Some("aws")).await.unwrap();
    let aws = status.providers.iter().find(|p| p.name == "aws").unwrap();
    assert!(aws.spent_month >= 11.0, "{}", aws.spent_month);

    // Deleting always works, and reports again.
    let n = issuer.reports.lock().unwrap().len();
    w.sandboxes.delete("first").await.unwrap();
    assert!(w.fake.calls().iter().any(|c| c.starts_with("delete fake-")));
    settle().await;
    assert!(issuer.reports.lock().unwrap().len() >= n);
    drop(w.dir);
}

#[tokio::test]
async fn an_older_issuer_or_no_issuer_never_blocks_a_create() {
    // An older issuer: the route is missing; it is asked once, then this
    // device's own figure decides.
    let issuer = Issuer::new(Mode::Older, 1000.0);
    let w = world(issuer.clone()).await;
    w.sandboxes.create(create("a")).await.unwrap();
    w.sandboxes.create(create("b")).await.unwrap();
    settle().await;
    assert_eq!(issuer.reports.lock().unwrap().len(), 1, "asked once");

    // Offline: the create goes on with what this device knows.
    let issuer = Issuer::new(Mode::Offline, 1000.0);
    let w = world(issuer.clone()).await;
    w.sandboxes.create(create("c")).await.unwrap();
    let clouds = w.sandboxes.clouds().unwrap();
    let status = clouds.status(Some("aws")).await.unwrap();
    let aws = status.providers.iter().find(|p| p.name == "aws").unwrap();
    assert!(aws.spent_month < 10.0, "{}", aws.spent_month);
}

#[tokio::test]
async fn a_cli_link_never_reports() {
    let issuer = Issuer::new(Mode::Adds, 1000.0);
    let w = world(issuer.clone()).await;
    let store = cua_byoc::Store::new(w.dir.path());
    let mut conn = store.connection("aws").unwrap().unwrap();
    conn.auth = Auth::Cli;
    store.connect(conn).unwrap();
    w.sandboxes.create(create("plain")).await.unwrap();
    settle().await;
    assert!(issuer.reports.lock().unwrap().is_empty());
}
