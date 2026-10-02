//! Host flow against the fake relay and a fake service manager (temp
//! homes; nothing is installed).

use crate::preflight::SessionProbe;
use crate::service::FakeServiceManager;
use crate::testing::FakeRelay;
use crate::*;
use std::path::Path;
use std::sync::Arc;

struct Fixture {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    driver: std::path::PathBuf,
    manager: Arc<FakeServiceManager>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".cua");
    let driver = dir.path().join("bundled-cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    Fixture {
        home,
        driver,
        manager: Arc::new(FakeServiceManager::default()),
        _dir: dir,
    }
}

impl Fixture {
    fn host(&self) -> Host {
        Host::new(&self.home).with_service_manager(self.manager.clone())
    }
    fn relay_opts(&self, url: &str) -> SetupOptions {
        let mut o = SetupOptions::relay(url);
        o.name = Some("mini".into());
        o.allow = vec!["friend@example.com".into()];
        o.driver_bin = Some(self.driver.clone());
        o
    }
}

#[tokio::test]
async fn relay_setup_registers_writes_policy_and_installs_the_service() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let f = fixture();
    let host = f.host();
    assert!(!host.status().await.unwrap().configured);

    let status = host
        .setup(f.relay_opts(&relay.url), &StaticToken("acct-token".into()))
        .await
        .unwrap();
    assert!(status.configured);
    assert_eq!(status.mode.as_deref(), Some("relay"));
    assert_eq!(status.name.as_deref(), Some("mini"));
    assert!(status.sharing);
    assert!(status.service.installed && status.service.running);
    assert_eq!(status.online, Some(false));
    assert!(status.error.is_none(), "{:?}", status.error);
    let id = status.machine_id.clone().unwrap();

    // Relay side: owned by the account, allowlist applied.
    let m = relay.machine(&id).unwrap();
    assert_eq!(m.owner.id, "user-1");
    assert_eq!(m.allow, vec!["friend@example.com".to_string()]);

    // Local side: machine token (0600), policy, id file, driver installed.
    let p = host.paths();
    assert_eq!(
        std::fs::read_to_string(p.machine_token()).unwrap().trim(),
        relay.machine_token(&id).unwrap()
    );
    let policy = host.policy().unwrap().unwrap();
    assert_eq!(policy.owner, "user-1");
    assert_eq!(policy.owner_email.as_deref(), Some("ada@example.com"));
    assert!(policy.sharing && policy.trust_relay_allowlist);
    assert_eq!(std::fs::read_to_string(&p.machine_id).unwrap().trim(), id);
    assert!(p.driver_bin().is_file());
    assert!(p.relay_jwks().is_file());

    // The service runs `join` with the machine token and policy files.
    let spec = f.manager.spec.lock().unwrap().clone().unwrap();
    assert_eq!(spec.program, p.driver_bin());
    let args = spec.args.join(" ");
    assert!(
        args.starts_with(&format!("join --relay {}", relay.url)),
        "{args}"
    );
    assert!(args.contains(&format!(
        "--relay-token-file {}",
        p.machine_token().display()
    )));
    assert!(args.contains(&format!("--host-policy {}", p.policy().display())));
    assert!(
        args.contains(&format!("--relay-jwks {}", p.relay_jwks().display())),
        "{args}"
    );
    assert!(args.contains(&format!("--machine-id-file {}", p.machine_id.display())));
    assert!(args.contains(&format!("--token-file {}", p.env_token().display())));

    // Presence: the relay reports who is connected.
    relay.set_online(&id, true, "0.1.0");
    relay.add_client(
        &id,
        ConnectedClient {
            id: "user-2".into(),
            email: Some("friend@example.com".into()),
            name: None,
            streams: 3,
            since: 1,
        },
    );
    let status = host.status().await.unwrap();
    assert_eq!(status.online, Some(true));
    assert_eq!(status.clients.len(), 1);

    // Stop sharing: relay cuts clients, policy refuses, service keeps running.
    let status = host.stop_sharing().await.unwrap();
    assert!(!status.sharing);
    assert!(status.clients.is_empty());
    assert!(!relay.machine(&id).unwrap().sharing);
    assert!(!host.policy().unwrap().unwrap().sharing);
    assert!(status.service.running);

    let status = host.start_sharing().await.unwrap();
    assert!(status.sharing && relay.machine(&id).unwrap().sharing);
    assert!(host.policy().unwrap().unwrap().sharing);

    // Rename + allowlist.
    let status = host
        .update(
            Some("studio".into()),
            Some(vec!["user-3".into()]),
            &StaticToken("acct-token".into()),
        )
        .await
        .unwrap();
    assert_eq!(status.name.as_deref(), Some("studio"));
    assert_eq!(
        relay.machine(&id).unwrap().allow,
        vec!["user-3".to_string()]
    );
    assert_eq!(
        host.policy().unwrap().unwrap().allow,
        vec!["user-3".to_string()]
    );

    // Re-running setup keeps the machine id and rotates the token.
    let old_token = relay.machine_token(&id).unwrap();
    let again = host
        .setup(f.relay_opts(&relay.url), &StaticToken("acct-token".into()))
        .await
        .unwrap();
    assert_eq!(again.machine_id.as_deref(), Some(id.as_str()));
    assert_ne!(relay.machine_token(&id).unwrap(), old_token);

    // Remove: relay forgets it, service uninstalled, host dir gone, id kept.
    host.remove().await.unwrap();
    assert!(relay.machine(&id).is_none());
    assert!(!p.dir.exists());
    assert!(p.machine_id.exists());
    assert!(!host.status().await.unwrap().configured);
    let calls = f.manager.calls.lock().unwrap().clone();
    assert_eq!(calls.last().map(String::as_str), Some("uninstall"));
}

#[tokio::test]
async fn relay_setup_needs_a_signed_in_account() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", None);
    let f = fixture();
    let err = f
        .host()
        .setup(f.relay_opts(&relay.url), &NoAccount)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unauthenticated(_)), "{err}");
    let err = f
        .host()
        .setup(f.relay_opts(&relay.url), &StaticToken("wrong".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unauthenticated(_)), "{err}");
    assert!(
        f.manager.calls.lock().unwrap().is_empty(),
        "nothing installed"
    );
}

#[tokio::test]
async fn a_machine_id_owned_by_someone_else_conflicts() {
    let relay = FakeRelay::start().await;
    relay.add_account("a", "user-a", None);
    relay.add_account("b", "user-b", None);
    let f = fixture();
    f.host()
        .setup(f.relay_opts(&relay.url), &StaticToken("a".into()))
        .await
        .unwrap();
    let err = f
        .host()
        .setup(f.relay_opts(&relay.url), &StaticToken("b".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Conflict(_)), "{err}");
}

#[tokio::test]
async fn direct_setup_serves_with_a_local_token_and_stop_stops_the_service() {
    let f = fixture();
    let host = f.host();
    let mut opts = SetupOptions::direct("10.1.2.3:3211".parse().unwrap());
    opts.driver_bin = Some(f.driver.clone());
    let status = host.setup(opts, &NoAccount).await.unwrap();
    assert_eq!(status.mode.as_deref(), Some("direct"));
    assert_eq!(status.direct_url.as_deref(), Some("http://10.1.2.3:3211"));
    let token_path = status.env_token_path.clone().unwrap();
    assert_eq!(token_path, host.paths().env_token().to_string_lossy());
    assert!(std::fs::read_to_string(&token_path).unwrap().trim().len() >= 32);
    assert!(status.sharing && status.online.is_none() && status.error.is_none());
    let spec = f.manager.spec.lock().unwrap().clone().unwrap();
    assert_eq!(&spec.args[..3], ["serve", "--listen", "10.1.2.3:3211"]);

    let stopped = host.stop_sharing().await.unwrap();
    assert!(!stopped.sharing && !stopped.service.running);
    let started = host.start_sharing().await.unwrap();
    assert!(started.sharing && started.service.running);
    // Names and allowlists are relay concepts.
    assert!(
        host.update(Some("x".into()), None, &NoAccount)
            .await
            .is_err()
    );
    host.remove().await.unwrap();
    assert!(!host.paths().dir.exists());
}

#[tokio::test]
async fn relay_directory_lists_owned_and_shared_machines() {
    let relay = FakeRelay::start().await;
    relay.add_account("owner", "user-1", Some("ada@example.com"));
    relay.add_account("friend", "user-2", Some("friend@example.com"));
    relay.add_account("stranger", "user-3", None);
    let f = fixture();
    let status = f
        .host()
        .setup(f.relay_opts(&relay.url), &StaticToken("owner".into()))
        .await
        .unwrap();
    let id = status.machine_id.unwrap();
    let client = RelayClient::new(&relay.url).unwrap();
    let mine = client.machines("owner").await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].role, "owner");
    assert_eq!(mine[0].url, format!("{}/m/{id}", relay.url));
    let shared = client.machines("friend").await.unwrap();
    assert_eq!(shared[0].role, "shared");
    assert!(shared[0].allow.is_empty(), "allowlist is owner-only");
    assert!(client.machines("stranger").await.unwrap().is_empty());
    assert!(matches!(
        client.machine("stranger", &id).await.unwrap_err(),
        Error::NotFound(_)
    ));
    assert!(matches!(
        client.stop_sharing("friend", &id).await.unwrap_err(),
        Error::PermissionDenied(_)
    ));
}

#[tokio::test]
async fn remove_reports_a_refused_unregister_after_the_local_cleanup() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let f = fixture();
    let host = f.host();
    let status = host
        .setup(f.relay_opts(&relay.url), &StaticToken("acct-token".into()))
        .await
        .unwrap();
    let id = status.machine_id.clone().unwrap();
    let p = host.paths();
    // A token the relay no longer accepts (revoked, or rotated elsewhere).
    std::fs::write(p.machine_token(), "not-the-machine-token").unwrap();

    let err = host.remove().await.unwrap_err();
    assert!(
        matches!(err, Error::Relay(ref m) if m.contains("removed locally")),
        "{err}"
    );
    // Removed here, but the relay still lists it: that is what the error says.
    assert!(!p.dir.exists());
    assert!(relay.machine(&id).is_some());
}

#[tokio::test]
async fn status_shows_recent_access_and_who_is_connected_in_direct_mode() {
    let f = fixture();
    let host = f.host();
    let mut opts = SetupOptions::direct("10.1.2.3:3211".parse().unwrap());
    opts.driver_bin = Some(f.driver.clone());
    let status = host.setup(opts, &NoAccount).await.unwrap();
    assert!(status.clients.is_empty() && status.recent_access.is_empty());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    std::fs::create_dir_all(host.paths().access_log().parent().unwrap()).unwrap();
    crate::access::write_log(
        &host.paths().access_log(),
        &[
            (
                now - 3_600_000,
                "viewer",
                "Bob (viewer:bob)",
                "DesktopService",
            ),
            (now - 5_000, "token", "token", "ProcessService"),
        ],
    );
    let status = host.status().await.unwrap();
    assert_eq!(status.access_log_error, None);
    assert_eq!(status.recent_access.len(), 2);
    assert_eq!(status.recent_access[0].what, "ProcessService");
    // Only the recent caller counts as connected now.
    assert_eq!(status.clients.len(), 1, "{:?}", status.clients);
    assert_eq!(status.clients[0].name.as_deref(), Some("token"));

    // A tampered log is flagged, not shown as clean.
    let text = std::fs::read_to_string(host.paths().access_log()).unwrap();
    std::fs::write(host.paths().access_log(), text.replace("Bob", "Eve")).unwrap();
    assert!(host.status().await.unwrap().access_log_error.is_some());
}

/// Every file under `dir`, recursively.
fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Hosting is least-privilege: the account session is used for the
/// registration call only; the host keeps its machine token, never an
/// account token, refresh token or client-device key, and the service it
/// installs gets none either.
#[tokio::test]
async fn setup_leaves_only_the_machine_credential_on_the_host() {
    let relay = FakeRelay::start().await;
    let account_token = "acct-SECRET-session-token";
    relay.add_account(account_token, "user-1", Some("ada@example.com"));
    let f = fixture();
    let host = f.host();
    let status = host
        .setup(f.relay_opts(&relay.url), &StaticToken(account_token.into()))
        .await
        .unwrap();
    let id = status.machine_id.unwrap();
    for file in files_under(&f.home) {
        let bytes = std::fs::read(&file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(account_token),
            "{} holds the account token",
            file.display()
        );
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            !["credentials.json", "session.json", "device-key"].contains(&name.as_str()),
            "setup wrote {}",
            file.display()
        );
    }
    let spec = f.manager.spec.lock().unwrap().clone().unwrap();
    let rendered = format!("{:?} {:?}", spec.args, spec.env);
    assert!(!rendered.contains(account_token), "{rendered}");
    assert!(spec.args.iter().any(|a| a.ends_with("machine-token")));
    // Hosting never enrolls this machine as a client device.
    assert_eq!(relay.device_count("user-1"), 0);

    // With the stored machine token the host reaches only its own record.
    let token = std::fs::read_to_string(host.paths().machine_token()).unwrap();
    let client = RelayClient::new(&relay.url).unwrap();
    assert!(client.machine(token.trim(), &id).await.is_ok());
    assert!(client.machines(token.trim()).await.is_err());
    assert!(
        client
            .patch(
                token.trim(),
                &id,
                &MachinePatch {
                    allow: Some(vec!["mallory@example.com".into()]),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );

    // Re-running setup rotates the machine token without leaving the
    // session behind either.
    host.setup(f.relay_opts(&relay.url), &StaticToken(account_token.into()))
        .await
        .unwrap();
    let rotated = std::fs::read_to_string(host.paths().machine_token()).unwrap();
    assert_ne!(rotated, token);
    assert_eq!(relay.machine_token(&id).as_deref(), Some(rotated.trim()));
}

/// A device on its own machine (named after the device).
fn device(relay: &FakeRelay, token: &str, name: &str) -> (DeviceAuth, Arc<MemoryKeySlot>) {
    let slot = Arc::new(MemoryKeySlot::default());
    let auth = DeviceAuth::new(
        &relay.url,
        Arc::new(StaticToken(token.into())),
        slot.clone(),
        name,
    )
    .unwrap()
    .with_machine_id(Some(format!("machine-of-{name}")));
    (auth, slot)
}

/// A client device enrolls with a second factor (a fresh sign-in, else an
/// approval by code from an enrolled device), then proves its key per session; the relay's machine API
/// needs that session, not just the account token.
#[tokio::test]
async fn devices_enroll_with_a_second_factor_and_gate_the_machine_api() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct", "user-1", Some("ada@example.com"));
    relay.require_devices(true);
    let client = RelayClient::new(&relay.url).unwrap();
    // The account token alone no longer lists machines.
    assert!(matches!(
        client.machines("acct").await,
        Err(Error::PermissionDenied(_))
    ));

    let (laptop, laptop_slot) = device(&relay, "acct", "laptop");
    assert!(laptop.try_session().await.is_none());
    // Without a fresh sign-in the first device waits for an approval.
    let pending = laptop.enroll().await.unwrap();
    assert_eq!(pending.device.state, DeviceState::Pending);
    assert!(pending.code.is_some());
    assert!(laptop_slot.load().unwrap().is_some());
    relay.fresh_sign_in("user-1");
    let enrolled = laptop.enroll().await.unwrap();
    assert_eq!(enrolled.device.state, DeviceState::Enrolled);
    let session = laptop.session().await.unwrap();
    // Cached until it nears expiry.
    assert_eq!(laptop.session().await.unwrap(), session);
    let machines = client
        .clone()
        .with_device_session(Some(session.clone()))
        .machines("acct")
        .await;
    assert!(machines.is_ok(), "{machines:?}");

    // A second device on a long-lived session shows a code; the laptop
    // approves it.
    relay.stale_sign_in("user-1");
    let (phone, _) = device(&relay, "acct", "phone");
    let r = phone.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Pending);
    assert!(phone.session().await.is_err());
    let approved = laptop.approve(r.code.as_deref(), None).await.unwrap();
    assert_eq!(approved.state, DeviceState::Enrolled);
    assert!(phone.session().await.is_ok());
    // A pending device cannot approve anything.
    let (tablet, _) = device(&relay, "acct", "tablet");
    let t = tablet.enroll().await.unwrap();
    assert!(tablet.approve(t.code.as_deref(), None).await.is_err());

    let list = laptop.devices().await.unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.iter().any(|d| d.current && d.name == "laptop"));
    let phone_id = phone.device_id().unwrap().unwrap();
    assert_eq!(
        laptop.rename(&phone_id, "work phone").await.unwrap().name,
        "work phone"
    );
    // Revoking this device deletes its key.
    let laptop_id = laptop.device_id().unwrap().unwrap();
    phone.revoke(&laptop_id).await.unwrap();
    assert_eq!(relay.device_state(&laptop_id), Some(DeviceState::Revoked));
    laptop.reset_session().await;
    assert!(laptop.session().await.is_err());
    let phone_session = phone.session().await.unwrap();
    phone.revoke(&phone_id).await.unwrap();
    assert!(phone.key().unwrap().is_none());
    assert!(
        client
            .clone()
            .with_device_session(Some(phone_session))
            .machines("acct")
            .await
            .is_err()
    );
    let kinds: Vec<_> = tablet
        .audit(100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&"device_revoked".to_string()), "{kinds:?}");
}

/// Regression test for relay.cua.ai locking out already-enrolled devices
/// right after the S9 hardening redeploy (cloud#7993): a relay restart
/// forgets every open device session at once (they live only in the
/// running process; see `cua_relay::devices::DeviceStore`), but a session
/// opened moments before still looks locally unexpired to `DeviceAuth`, so
/// it gets sent again and the relay refuses it ("do this from an enrolled
/// device" / the relay's `NOT_ENROLLED`). With the grace period that used
/// to follow a restart now zero (S3: `CUA_RELAY_DEVICE_GRACE_DAYS=0` by
/// default), that refusal used to reach the caller as a hard error for up
/// to the session's TTL, on a device the relay's own records still show as
/// enrolled. `DeviceAuth` must notice this specific refusal, forget the
/// stale session, and retry once with a freshly-signed one -- recovering
/// with no error ever reaching the caller.
#[tokio::test]
async fn a_relay_restart_forgets_sessions_but_the_client_recovers_transparently() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct", "user-1", Some("ada@example.com"));
    relay.require_devices(true);
    relay.fresh_sign_in("user-1");

    let (laptop, _) = device(&relay, "acct", "laptop");
    let enrolled = laptop.enroll().await.unwrap();
    assert_eq!(enrolled.device.state, DeviceState::Enrolled);
    let session_before = laptop.session().await.unwrap();

    // A second device, enrolled the way the incident's device was: a
    // one-time code approved from the already-enrolled laptop.
    relay.stale_sign_in("user-1");
    let (phone, _) = device(&relay, "acct", "phone");
    let pending = phone.enroll().await.unwrap();
    assert_eq!(pending.device.state, DeviceState::Pending);
    let code = pending.code.unwrap();
    let approved = laptop
        .approve(Some(&code), None)
        .await
        .expect("the laptop approves the phone's code");
    assert_eq!(approved.state, DeviceState::Enrolled);
    let phone_session_before = phone.session().await.unwrap();

    // Simulate the production redeploy: the relay process restarts.
    // Devices, machines and audit history (all persisted) are untouched;
    // only the in-memory session map is gone.
    relay.forget_sessions();

    // Both devices' cached sessions still look locally valid (nowhere near
    // their claimed expiry) -- this is exactly the gap a relay restart
    // opens.
    assert_eq!(laptop.session().await.unwrap(), session_before);
    assert_eq!(phone.session().await.unwrap(), phone_session_before);

    // An operation that hard-requires a live session (`approve`/`rename`/
    // `revoke`, never covered by the grace period) must still succeed: the
    // first attempt's session is refused, and `DeviceAuth` retries with a
    // fresh one before this call ever returns to its caller.
    let phone_id = phone.device_id().unwrap().unwrap();
    let renamed = laptop
        .rename(&phone_id, "work phone")
        .await
        .expect("a stale post-restart session must not surface as a hard error");
    assert_eq!(renamed.name, "work phone");

    // The retry replaced the stale session with a genuinely fresh one.
    let session_after = laptop.session().await.unwrap();
    assert_ne!(session_after, session_before);

    // The same self-healing covers the device whose session was rejected,
    // not just the caller's.
    relay.forget_sessions();
    assert_eq!(phone.session().await.unwrap(), phone_session_before);
    let revoked = phone
        .revoke(&phone_id)
        .await
        .expect("revoke must self-heal a stale post-restart session too");
    assert_eq!(revoked.state, DeviceState::Revoked);
}

/// The fake relay audits machine access as the real one does: by device,
/// at most once per interval, and flags access without an enrolled device.
/// Devices report their platform, and a revoked key is replaced on enroll.
#[tokio::test]
async fn machine_access_is_audited_by_device_and_platform_is_reported() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct", "user-1", None);
    relay.set_enforce_after(1_900_000_000);
    let client = RelayClient::new(&relay.url).unwrap();
    client
        .register(
            "acct",
            &crate::relay::RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    let (laptop, _) = device(&relay, "acct", "laptop");
    let enrolled = laptop.enroll().await.unwrap();
    assert_eq!(enrolled.device.platform, crate::device_platform());
    let listing = laptop.listing().await.unwrap();
    assert_eq!(listing.enforce_after, 1_900_000_000);
    assert!(listing.devices[0].enrolled_until.is_some());
    let session = laptop.session().await.unwrap();
    let http = reqwest::Client::new();
    let open = |session: Option<&str>| {
        let mut r = http
            .get(format!("{}/m/0123abcd4567ef89/spacesd.v1/Info", relay.url))
            .bearer_auth("acct");
        if let Some(s) = session {
            r = r.header(crate::relay::DEVICE_SESSION_HEADER, s);
        }
        r.send()
    };
    open(Some(&session)).await.unwrap();
    open(Some(&session)).await.unwrap();
    open(None).await.unwrap();
    let access: Vec<_> = laptop
        .audit(100)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "machine_access")
        .collect();
    assert_eq!(access.len(), 2, "{access:?}");
    assert_eq!(access[0].device, laptop.device_id().unwrap());
    assert_eq!(access[0].machine.as_deref(), Some("0123abcd4567ef89"));
    assert_eq!(access[1].subject.as_deref(), Some("unenrolled device"));
    // Revoked by another device: enrolling again makes a new key instead
    // of failing on the revoked one.
    let (phone, _) = device(&relay, "acct", "phone");
    phone.enroll().await.unwrap();
    let old = phone.device_id().unwrap().unwrap();
    laptop.revoke(&old).await.unwrap();
    let again = phone.enroll().await.unwrap();
    assert_ne!(again.device.id, old);
    assert_eq!(again.device.state, DeviceState::Pending);
    assert!(again.code.is_some());
}

#[test]
fn file_key_slot_is_owner_only_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let slot = FileKeySlot(dir.path().join("keys/device-key"));
    assert!(slot.load().unwrap().is_none());
    let (key, pkcs8) = DeviceKey::generate().unwrap();
    slot.save(&pkcs8).unwrap();
    let loaded = DeviceKey::from_pkcs8(&slot.load().unwrap().unwrap()).unwrap();
    assert_eq!(loaded.id(), key.id());
    assert!(key.id().starts_with("dev_") && key.id().len() == 28);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&slot.0).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(slot.clear().unwrap());
    assert!(!slot.clear().unwrap());
}

/// A spare machine: its desktop is not a Space (the driver runs without
/// its desktop services), it provides Spaces, and the policy names the
/// daemon that creates them. Settings change later with `configure`, and
/// every change is in the Spaces audit.
#[tokio::test]
async fn a_spare_machine_provides_spaces_without_its_desktop() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let f = fixture();
    let host = f.host();
    let mut opts = f.relay_opts(&relay.url).profile(HostProfile::Spare);
    opts.cua_bin = Some("/opt/cua/bin/cua".into());
    let status = host
        .setup(opts, &StaticToken("acct-token".into()))
        .await
        .unwrap();
    assert!(!status.share_desktop && status.provide_spaces);
    assert_eq!(status.max_spaces, provided::DEFAULT_MAX_SPACES);
    assert_eq!(status.max_macos_vms, provided::MACOS_VM_LICENSE_LIMIT);
    let spec = f.manager.spec.lock().unwrap().clone().unwrap();
    for flag in ["--no-desktop", "--no-driver", "--no-mcp"] {
        assert!(
            spec.args.iter().any(|a| a == flag),
            "{flag}: {:?}",
            spec.args
        );
    }
    let policy = host.policy().unwrap().unwrap();
    assert!(!policy.share_desktop && policy.provide_spaces);
    let daemon = policy.spaces_daemon.clone().unwrap();
    assert_eq!(daemon.socket, f.home.join("cua.sock").to_string_lossy());
    assert_eq!(daemon.cua_home, f.home.to_string_lossy());
    assert_eq!(daemon.cua_bin.as_deref(), Some("/opt/cua/bin/cua"));
    assert_eq!(status.spaces_audit[0].action, "config");
    assert!(
        status.spaces_audit[0]
            .detail
            .contains("share_desktop=off provide_spaces=on")
    );

    // Sharing the desktop again restarts the service with its desktop.
    let calls = f.manager.calls.lock().unwrap().len();
    let status = host
        .configure(HostSettingsChange {
            share_desktop: Some(true),
            max_spaces: Some(6),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(status.share_desktop && status.provide_spaces);
    assert_eq!(status.max_spaces, 6);
    let spec = f.manager.spec.lock().unwrap().clone().unwrap();
    assert!(!spec.args.iter().any(|a| a == "--no-desktop"));
    assert!(f.manager.calls.lock().unwrap().len() > calls, "reinstalled");
    assert!(host.policy().unwrap().unwrap().share_desktop);
    assert_eq!(status.spaces_audit.len(), 2);

    // Apple's license caps macOS VMs at two; nothing at all is refused.
    let e = host
        .configure(HostSettingsChange {
            max_macos_vms: Some(3),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("Apple"), "{e}");
    let e = host
        .configure(HostSettingsChange {
            share_desktop: Some(false),
            provide_spaces: Some(false),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
}

fn spec_args(f: &Fixture) -> Vec<String> {
    f.manager.spec.lock().unwrap().clone().unwrap().args
}

/// A direct host provides Spaces without the relay: the env token holder
/// owns the policy (four Spaces, two macOS VMs), the driver serves
/// HostSpacesService from it, setup prints how a laptop adds the host, and
/// a public listen address needs --allow-any-address.
#[tokio::test]
async fn direct_mode_hosts_spaces_with_a_local_policy() {
    let f = fixture();
    let host = f.host();
    // A spare machine reached by its Tailscale address.
    let mut opts =
        SetupOptions::direct("100.101.102.103:3211".parse().unwrap()).profile(HostProfile::Spare);
    opts.driver_bin = Some(f.driver.clone());
    opts.name = Some("Mac mini (spare)".into());
    let status = host.setup(opts.clone(), &NoAccount).await.unwrap();
    assert_eq!(status.mode.as_deref(), Some("direct"));
    assert!(!status.share_desktop && status.provide_spaces);
    assert_eq!((status.max_spaces, status.max_macos_vms), (4, 2));
    let policy = host.policy().unwrap().unwrap();
    assert_eq!(policy.owner, "local");
    assert!(policy.provide_spaces && !policy.share_desktop);
    assert!(!policy.trust_relay_allowlist);
    assert!(policy.spaces_daemon.is_some());
    let direct = policy.direct.clone().unwrap();
    assert_eq!(direct.listen, "100.101.102.103:3211");
    assert!(!direct.allow_any_address);
    let args = spec_args(&f);
    assert_eq!(&args[..3], ["serve", "--listen", "100.101.102.103:3211"]);
    let i = args
        .iter()
        .position(|a| a == "--direct-host-policy")
        .expect("the driver serves HostSpacesService");
    assert_eq!(args[i + 1], host.paths().policy().to_string_lossy());
    assert!(args.iter().any(|a| a == "--no-desktop"), "{args:?}");
    // The command for the laptop: the address, the host and its token.
    let token = std::fs::read_to_string(host.paths().env_token()).unwrap();
    assert_eq!(
        host.pairing_command().unwrap().unwrap(),
        format!(
            "cua spaces add 100.101.102.103:3211 --host --name 'Mac mini (spare)' --token {}",
            token.trim()
        )
    );

    // Never a public address silently.
    let mut public = opts.clone();
    public.mode = HostMode::Direct {
        listen: "203.0.113.7:3211".parse().unwrap(),
    };
    let e = host.setup(public.clone(), &NoAccount).await.unwrap_err();
    assert!(
        matches!(e, Error::InvalidArgument(ref m) if m.contains("--allow-any-address")),
        "{e}"
    );
    public.allow_any_address = true;
    host.setup(public, &NoAccount).await.unwrap();
    assert!(
        host.policy()
            .unwrap()
            .unwrap()
            .direct
            .unwrap()
            .allow_any_address
    );
}

/// A direct desktop host runs its driver as before; turning Spaces on
/// writes the policy flag and restarts the driver with it.
#[tokio::test]
async fn a_direct_desktop_host_starts_providing_spaces() {
    let f = fixture();
    let host = f.host();
    let mut opts = SetupOptions::direct("10.1.2.3:3211".parse().unwrap());
    opts.driver_bin = Some(f.driver.clone());
    let status = host.setup(opts, &NoAccount).await.unwrap();
    assert!(status.share_desktop && !status.provide_spaces);
    assert!(!spec_args(&f).iter().any(|a| a == "--direct-host-policy"));
    // Relay hosts have no pairing command; this one does.
    assert!(host.pairing_command().unwrap().is_some());

    let status = host
        .configure(HostSettingsChange {
            provide_spaces: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(status.provide_spaces && status.share_desktop);
    assert!(spec_args(&f).iter().any(|a| a == "--direct-host-policy"));
    assert!(host.policy().unwrap().unwrap().provide_spaces);
    // The desktop off as well is a spare machine; both off is refused.
    let status = host
        .configure(HostSettingsChange {
            share_desktop: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!status.share_desktop && status.provide_spaces);
    let e = host
        .configure(HostSettingsChange {
            provide_spaces: Some(false),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(e, Error::InvalidArgument(_)), "{e}");
}

/// A relay registration can name the host a Space belongs to; only an
/// account that can use that host may.
#[tokio::test]
async fn a_space_registers_under_its_host() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("eve-token", "eve", None);
    let client = RelayClient::new(&relay.url).unwrap();
    let host = client
        .register(
            "ada-token",
            &crate::relay::RegisterRequest {
                id: "hostmachine1".into(),
                name: "Mac mini (spare)".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    let child = client
        .register(
            "ada-token",
            &crate::relay::RegisterRequest {
                id: "space-0001".into(),
                name: "space-0001".into(),
                allow: vec![],
                host: Some(host.machine.id.clone()),
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(child.machine.host.as_deref(), Some("hostmachine1"));
    let listed = client.machines("ada-token").await.unwrap();
    assert!(
        listed
            .iter()
            .any(|m| m.id == "space-0001" && m.host.as_deref() == Some("hostmachine1"))
    );
    // Not Eve's host.
    let e = client
        .register(
            "eve-token",
            &crate::relay::RegisterRequest {
                id: "space-0002".into(),
                name: "x".into(),
                allow: vec![],
                host: Some("hostmachine1".into()),
                meta: Default::default(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::PermissionDenied(_)), "{e}");
}

/// A slot that also sees another build's copy of the key (the signed and
/// the unsigned app on one Mac).
#[derive(Default)]
struct TwoCopySlot {
    own: std::sync::Mutex<Option<Vec<u8>>>,
    other: std::sync::Mutex<Option<Vec<u8>>>,
}

impl KeySlot for TwoCopySlot {
    fn load(&self) -> crate::Result<Option<Vec<u8>>> {
        Ok(self.own.lock().unwrap().clone())
    }
    fn save(&self, pkcs8: &[u8]) -> crate::Result<()> {
        *self.own.lock().unwrap() = Some(pkcs8.to_vec());
        Ok(())
    }
    fn clear(&self) -> crate::Result<bool> {
        Ok(self.own.lock().unwrap().take().is_some())
    }
    fn alternate(&self) -> crate::Result<Option<Vec<u8>>> {
        Ok(self.other.lock().unwrap().clone())
    }
    fn adopt_alternate(&self) -> crate::Result<()> {
        let other = self.other.lock().unwrap().take();
        *self.own.lock().unwrap() = other;
        Ok(())
    }
    fn drop_alternate(&self) -> crate::Result<()> {
        *self.other.lock().unwrap() = None;
        Ok(())
    }
}

fn on_machine(relay: &FakeRelay, slot: Arc<dyn KeySlot>, name: &str, machine: &str) -> DeviceAuth {
    DeviceAuth::new(&relay.url, Arc::new(StaticToken("acct".into())), slot, name)
        .unwrap()
        .with_machine_id(Some(machine.into()))
}

/// Signing in again is enough: a fresh sign-in enrolls another device at
/// once, and a new key of the same machine replaces the old record. An
/// older relay still gets the approval code.
#[tokio::test]
async fn a_fresh_sign_in_enrolls_and_re_keys_this_machine() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct", "user-1", Some("ada@example.com"));
    relay.require_devices(true);
    relay.fresh_sign_in("user-1");
    let unsigned = on_machine(&relay, Arc::new(MemoryKeySlot::default()), "Mac", "mac-1");
    assert_eq!(
        unsigned.enroll().await.unwrap().device.state,
        DeviceState::Enrolled
    );
    let old = unsigned.device_id().unwrap().unwrap();

    // The signed build keeps its own key. On a long-lived session it waits.
    relay.stale_sign_in("user-1");
    let signed = on_machine(&relay, Arc::new(MemoryKeySlot::default()), "Mac", "mac-1");
    let r = signed.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Pending);
    assert!(r.code.is_some());
    assert!(r.superseded.is_empty());
    // After a fresh sign-in it enrolls, replacing the old key's record.
    relay.fresh_sign_in("user-1");
    let r = signed.enroll_after_sign_in().await.unwrap().unwrap();
    assert_eq!(r.device.state, DeviceState::Enrolled);
    assert_eq!(r.superseded, vec![old.clone()]);
    // The name the relay knows stays.
    assert_eq!(r.device.name, "Mac");
    assert_eq!(relay.superseded_by(&old), Some(r.device.id.clone()));
    let list = signed.devices().await.unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    assert!(unsigned.session().await.is_err());

    // A device that never enrolled creates nothing on sign-in.
    let (fresh_device, slot) = device(&relay, "acct", "new");
    assert!(fresh_device.enroll_after_sign_in().await.unwrap().is_none());
    assert!(slot.load().unwrap().is_none());

    // An older relay enrolls only an account's first device by sign-in:
    // the next one still gets its approval code.
    relay.legacy_enrollment(true);
    let (tablet, _) = device(&relay, "acct", "tablet");
    let r = tablet.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Pending);
    assert!(r.code.is_some());
    // Approving it by id (what `cua devices approve dev_…` sends) works
    // there too, and an unknown code says how to approve instead.
    let tablet_id = tablet.device_id().unwrap().unwrap();
    let e = signed.approve(Some("ZZZZ-ZZZZ"), None).await.unwrap_err();
    assert!(e.to_string().contains("approve by id"), "{e}");
    let approved = signed.approve(Some(&tablet_id), None).await.unwrap();
    assert_eq!(approved.state, DeviceState::Enrolled);
}

/// Two builds on one machine that each made a key end up with one: the key
/// the relay enrolled wins, the other's record is retired.
#[tokio::test]
async fn two_keys_on_one_machine_converge_on_the_enrolled_one() {
    for legacy in [false, true] {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        relay.require_devices(true);
        relay.legacy_enrollment(legacy);
        // The unsigned build enrolled its key (now the signed build's
        // alternate: the file it left).
        relay.fresh_sign_in("user-1");
        let unsigned_slot = Arc::new(MemoryKeySlot::default());
        let unsigned = on_machine(&relay, unsigned_slot.clone(), "Mac", "mac-1");
        unsigned.enroll().await.unwrap();
        let enrolled_id = unsigned.device_id().unwrap().unwrap();
        // The signed build made its own key, pending on a stale session.
        relay.stale_sign_in("user-1");
        let slot = Arc::new(TwoCopySlot::default());
        let signed = on_machine(&relay, slot.clone(), "Mac", "mac-1");
        let r = signed.enroll().await.unwrap();
        assert_eq!(r.device.state, DeviceState::Pending);
        let orphan = r.device.id.clone();
        // Now it sees the unsigned build's key too.
        *slot.other.lock().unwrap() = unsigned_slot.load().unwrap();
        let r = signed.enroll().await.unwrap();
        assert_eq!(r.device.state, DeviceState::Enrolled, "legacy={legacy}");
        assert_eq!(r.device.id, enrolled_id);
        assert_eq!(
            signed.device_id().unwrap().as_deref(),
            Some(enrolled_id.as_str())
        );
        assert!(slot.other.lock().unwrap().is_none());
        // Its own pending key is retired, so nobody is asked to approve it.
        assert_eq!(relay.device_state(&orphan), Some(DeviceState::Revoked));
        assert!(signed.session().await.is_ok());
    }

    // The other way round: this build's key is enrolled, the leftover
    // copy is stale. The leftover's record is retired and the copy dropped.
    let relay = FakeRelay::start().await;
    relay.add_account("acct", "user-1", Some("ada@example.com"));
    relay.require_devices(true);
    relay.legacy_enrollment(true);
    relay.fresh_sign_in("user-1");
    let slot = Arc::new(TwoCopySlot::default());
    let this = on_machine(&relay, slot.clone(), "Mac", "mac-1");
    this.enroll().await.unwrap();
    relay.stale_sign_in("user-1");
    let leftover_slot = Arc::new(MemoryKeySlot::default());
    let leftover = on_machine(&relay, leftover_slot.clone(), "Mac", "mac-1");
    let pending = leftover.enroll().await.unwrap();
    *slot.other.lock().unwrap() = leftover_slot.load().unwrap();
    let r = this.enroll().await.unwrap();
    assert_eq!(r.device.state, DeviceState::Enrolled);
    assert_ne!(r.device.id, pending.device.id);
    assert_eq!(
        relay.device_state(&pending.device.id),
        Some(DeviceState::Revoked)
    );
    assert!(slot.other.lock().unwrap().is_none());
}

/// Machine metadata (what the registering client says: a Space in the
/// owner's own cloud, its provider and place) comes back in the listing.
#[tokio::test]
async fn machine_metadata_round_trips() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", None);
    let client = RelayClient::new(&relay.url).unwrap();
    let meta = std::collections::BTreeMap::from([
        ("cua.cloud.provider".to_string(), "aws".to_string()),
        (
            "cua.cloud.place".to_string(),
            "AWS \u{b7} us-west-2".to_string(),
        ),
    ]);
    let reg = client
        .register(
            "acct-token",
            &crate::relay::RegisterRequest {
                id: "cloud-00000000000000aa".into(),
                name: "box".into(),
                allow: vec![],
                host: None,
                meta: meta.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(reg.machine.meta, meta);
    let listed = client.machines("acct-token").await.unwrap();
    assert_eq!(listed[0].meta, meta);
    // A machine without it (and a relay that predates it) has none.
    let plain: crate::relay::Machine =
        serde_json::from_value(serde_json::json!({"id": "x", "name": "y"})).unwrap();
    assert!(plain.meta.is_empty());
}

// --------------------------------------------------------- macOS preflight

/// Wraps a [`FakeServiceManager`] to report [`RunnerKind::Launchd`]: the
/// fixture's own manager always reports `Process` so every test above
/// (none of them macOS-specific) never runs the preflight. These tests
/// need to look like the real macOS runner to exercise it.
struct AsLaunchd(Arc<FakeServiceManager>);

impl ServiceManager for AsLaunchd {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Launchd
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.0.install(spec)
    }
    fn start(&self) -> Result<()> {
        self.0.start()
    }
    fn stop(&self) -> Result<()> {
        self.0.stop()
    }
    fn uninstall(&self) -> Result<()> {
        self.0.uninstall()
    }
    fn state(&self) -> ServiceState {
        self.0.state()
    }
}

/// A [`SessionProbe`] that just answers whether a GUI session exists.
struct FakeProbe {
    gui: bool,
}

impl SessionProbe for FakeProbe {
    fn gui_session(&self, _uid: u32) -> bool {
        self.gui
    }
    fn console_user(&self) -> Option<String> {
        None
    }
    fn permission_status(&self, _driver_bin: &Path) -> Option<(bool, bool)> {
        None
    }
}

/// Setting up a machine over ssh with nobody logged in at the console must
/// not reach `launchctl bootstrap` (it would fail there, often with no
/// useful message): the preflight stops it first, before the service is
/// ever installed or started.
#[tokio::test]
async fn launchd_setup_refuses_without_a_gui_session() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let f = fixture();
    let manager = Arc::new(FakeServiceManager::default());
    let host = Host::new(&f.home)
        .with_service_manager(Arc::new(AsLaunchd(manager.clone())))
        .with_preflight_probe(Arc::new(FakeProbe { gui: false }));
    let err = host
        .setup(f.relay_opts(&relay.url), &StaticToken("acct-token".into()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no GUI"), "{err}");
    assert!(
        manager.calls.lock().unwrap().is_empty(),
        "install/start must not run without a GUI session"
    );
    // Nothing was left half set up for `cua host status` to trip over.
    assert!(!host.status().await.unwrap().configured);
}

/// With a GUI session present, the launchd runner installs and starts as
/// usual: the preflight only stops the broken case.
#[tokio::test]
async fn launchd_setup_proceeds_with_a_gui_session() {
    let relay = FakeRelay::start().await;
    relay.add_account("acct-token", "user-1", Some("ada@example.com"));
    let f = fixture();
    let manager = Arc::new(FakeServiceManager::default());
    let host = Host::new(&f.home)
        .with_service_manager(Arc::new(AsLaunchd(manager.clone())))
        .with_preflight_probe(Arc::new(FakeProbe { gui: true }));
    let status = host
        .setup(f.relay_opts(&relay.url), &StaticToken("acct-token".into()))
        .await
        .unwrap();
    assert!(status.configured);
    assert!(status.service.installed && status.service.running);
    assert_eq!(
        manager.calls.lock().unwrap().as_slice(),
        ["install".to_string(), "start".to_string()]
    );
}
