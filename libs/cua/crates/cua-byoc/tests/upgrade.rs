// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Upgrading a cua home that a released cua wrote: its cloud links, the
//! resources it recorded, the pool's budget and hosts, the Hetzner token in
//! the Cua credential store and the audit log keep working with this cua,
//! with no step by the person. And a home this cua wrote keeps what an older
//! cua would drop (`state.rs`, `links.json`).
//!
//! The homes under `tests/fixtures/upgrade/` were written by released cua
//! builds against fake clouds (see the README there). With
//! `CUA_UPGRADE_OLD_CUA=/path/to/an/older/cua` the round trip test also runs
//! that binary on a home this cua wrote.
#![cfg(all(feature = "aws", feature = "hetzner"))]

#[path = "support/fake_aws.rs"]
mod fake_aws;
#[path = "support/fake_hetzner.rs"]
mod fake_hetzner;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cua_byoc::aws::Aws;
use cua_byoc::hetzner::{CuaCredentialStore, Hetzner};
use cua_byoc::model::{Auth, Connection, MaxSize, Resource};
use cua_byoc::state::{FEDERATED_PROFILE, Link};
use cua_byoc::{CloudApi, Clouds, Store};
use cua_sandbox_core::byoc::CloudManager;
use serde_json::{Value, json};

/// The released homes: (version, directory).
fn fixtures() -> Vec<(&'static str, PathBuf)> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade");
    ["v0.7.0", "v0.8.0"]
        .into_iter()
        .map(|v| (v, base.join(v).join("home")))
        .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), &dest).unwrap();
        }
    }
}

/// A copy of the released home `dir`.
fn seed(dir: &Path) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir(dir, t.path());
    t
}

fn raw(home: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(home.join("cloud/state.json")).unwrap()).unwrap()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn every_released_link_reads_as_a_cli_link_with_its_names_and_ttl() {
    for (v, dir) in fixtures() {
        let home = seed(&dir);
        let old = raw(home.path());
        let store = Store::new(home.path());
        let conns = store.connections().unwrap();
        let old_conns = old["connections"].as_array().unwrap();
        assert!(!old_conns.is_empty(), "{v}: the fixture has links");
        assert_eq!(conns.len(), old_conns.len(), "{v}");
        for (c, o) in conns.iter().zip(old_conns) {
            assert_eq!(c.auth, Auth::Cli, "{v} {}", c.provider);
            assert_eq!((c.budget_usd_month, c.max_size), (None, None), "{v}");
            let back = serde_json::to_value(c).unwrap();
            for k in keys(o) {
                assert_eq!(back[&k], o[&k], "{v} {} {k}", c.provider);
            }
            // No limit a person never set: a large spend refuses nothing.
            cua_byoc::limits::check_new_machine(c, "x", Some(64), Some(256 * 1024), 10.0, 1e9)
                .unwrap_or_else(|e| panic!("{v} {}: {e}", c.provider));
        }
        assert_eq!(
            store.owner_id().unwrap(),
            old["owner_id"].as_str().unwrap(),
            "{v}"
        );
        let ids: Vec<String> = store
            .resources()
            .unwrap()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        let old_ids: Vec<String> = old["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, old_ids, "{v}");
    }
}

#[test]
fn this_cua_writes_a_released_home_back_with_the_same_keys() {
    for (v, dir) in fixtures() {
        let home = seed(&dir);
        let old = raw(home.path());
        let store = Store::new(home.path());
        // A write of every link and record as it is.
        for c in store.connections().unwrap() {
            store.connect(c).unwrap();
        }
        for r in store.resources().unwrap() {
            store.record(r).unwrap();
        }
        let new = raw(home.path());
        assert_eq!(keys(&new), keys(&old), "{v}");
        let sorted = |v: &Value, k: &str| {
            let mut a = v[k].as_array().unwrap().clone();
            a.sort_by_key(|x| x.to_string());
            a
        };
        assert_eq!(
            sorted(&new, "connections"),
            sorted(&old, "connections"),
            "{v}"
        );
        assert_eq!(sorted(&new, "resources"), sorted(&old, "resources"), "{v}");
        // Nothing for links.json: a released link has no new keys.
        assert!(!store.links_path().exists(), "{v}");
    }
}

/// Drift guard: every key a link has today is one a released cua knows
/// (it keeps it when it writes the file back) or one `links.json` keeps.
/// A new `Connection` field must be added to [`Link`] (and restored in
/// `state.rs`), or a released cua would drop it.
#[test]
fn every_new_link_key_is_kept_in_links_json() {
    let full = Connection {
        provider: "aws".into(),
        profile: "p".into(),
        region: "r".into(),
        zone: "z".into(),
        project: "x".into(),
        environment: "e".into(),
        account: "a".into(),
        ttl_hours: 1,
        connected_at: "t".into(),
        auth: Auth::aws("conn_1", "arn"),
        budget_usd_month: Some(1.0),
        max_size: Some(MaxSize {
            vcpu: 1,
            mem_gib: 1,
        }),
    };
    let link = Link {
        account: "a".into(),
        connected_at: "t".into(),
        profile: "p".into(),
        auth: full.auth.clone(),
        budget_usd_month: full.budget_usd_month,
        max_size: full.max_size,
    };
    let link_keys = keys(&serde_json::to_value(&link).unwrap());
    let now = keys(&serde_json::to_value(&full).unwrap());
    for (v, dir) in fixtures() {
        let old = raw(&dir);
        let mut known = BTreeSet::new();
        for c in old["connections"].as_array().unwrap() {
            known.extend(keys(c));
        }
        let lost: Vec<&String> = now
            .iter()
            .filter(|k| !known.contains(*k) && !link_keys.contains(*k))
            .collect();
        assert!(
            lost.is_empty(),
            "cua {v} would drop {lost:?} from a link: keep them in links.json (state.rs Link)"
        );
        // The file's own keys: `spent` is the one links.json keeps.
        let top: BTreeSet<String> = ["owner_id", "connections", "resources", "spent"]
            .into_iter()
            .map(String::from)
            .collect();
        let old_top = keys(&old);
        assert!(old_top.is_subset(&top), "{v}: {old_top:?}");
    }
    // And a released resource record has every key a record has today
    // (records are written back whole by every cua).
    for (v, dir) in fixtures() {
        let old = raw(&dir);
        let records = old["resources"].as_array().unwrap();
        if records.is_empty() {
            continue;
        }
        let mut known = BTreeSet::new();
        for r in records {
            known.extend(keys(r));
        }
        let mut r = Resource {
            provider: "aws".into(),
            id: "i-1".into(),
            resource_type: "instance".into(),
            name: "n".into(),
            region: "r".into(),
            zone: "z".into(),
            project: "p".into(),
            sandbox: "aws:s".into(),
            machine: "m".into(),
            created: 1,
            expires: 2,
            state: "running".into(),
            ..Default::default()
        };
        r.tags.insert("cua-managed".into(), "true".into());
        r.extra.insert("usd_per_hour".into(), "1".into());
        let now = keys(&serde_json::to_value(&r).unwrap());
        let new_keys: Vec<&String> = now.difference(&known).collect();
        // A new record key would be lost by an older cua's write: keep it
        // in links.json too.
        assert!(
            new_keys.is_empty(),
            "cua {v} would drop {new_keys:?} from a record"
        );
    }
}

/// Puts what `home` records into the fake clouds, as they were when the
/// released cua recorded them.
fn seed_clouds(home: &Path, aws: &fake_aws::S, hz: &fake_hetzner::S) {
    let state = raw(home);
    for r in state["resources"].as_array().unwrap() {
        let tags: std::collections::BTreeMap<String, String> =
            serde_json::from_value(r["tags"].clone()).unwrap_or_default();
        let id = r["id"].as_str().unwrap().to_string();
        let name = r["name"].as_str().unwrap_or_default().to_string();
        match (r["provider"].as_str().unwrap(), r["type"].as_str().unwrap()) {
            ("aws", "instance") => {
                aws.lock()
                    .unwrap()
                    .instances
                    .insert(id, ("running".into(), tags));
            }
            ("aws", "security_group") => {
                aws.lock()
                    .unwrap()
                    .groups
                    .insert(id, (name, "vpc-default".into(), tags));
            }
            ("hetzner", "instance") => {
                let n: u64 = id.parse().unwrap();
                hz.lock().unwrap().servers.insert(
                    n,
                    json!({"id": n, "name": name, "status": "running",
                        "created": "2026-10-21T05:00:00+00:00", "labels": tags,
                        "server_type": {"name": r["extra"]["machine_type"].as_str().unwrap_or("cx23")},
                        "location": {"name": r["region"]},
                        "public_net": {"ipv4": {"ip": "203.0.113.9"}}}),
                );
            }
            ("hetzner", "firewall") => {
                let n: u64 = id.parse().unwrap();
                hz.lock().unwrap().firewalls.insert(
                    n,
                    json!({"id": n, "name": name, "labels": tags, "rules": []}),
                );
            }
            (p, t) => panic!("the fixture records a {p} {t}: seed it in the fake"),
        }
    }
}

/// The fake clouds and this cua's manager over `home`.
async fn manage(home: &Path) -> (Clouds, Arc<Store>, fake_aws::S, fake_hetzner::S) {
    let (aws_url, aws_s) = fake_aws::fake().await;
    let (hz_url, hz_s) = fake_hetzner::serve().await;
    seed_clouds(home, &aws_s, &hz_s);
    let aws = Arc::new(Aws::with_endpoint(&aws_url, "AKIDFAKE", "fake"));
    // The token the released cua saved in the Cua credential store (the
    // 0600 file of this home: CUA_HOME and CUA_CREDENTIAL_STORE=file).
    let hz = Arc::new(Hetzner::with_endpoint(
        &hz_url,
        Arc::new(CuaCredentialStore),
        None,
    ));
    let store = Arc::new(Store::new(home));
    let clouds = Clouds::new(
        home,
        vec![aws as Arc<dyn CloudApi>, hz as Arc<dyn CloudApi>],
        store.clone(),
    );
    (clouds, store, aws_s, hz_s)
}

/// One test for every step that reads the process environment (the
/// credential store follows CUA_HOME).
#[tokio::test(flavor = "multi_thread")]
async fn a_released_home_is_managed_by_this_cua_with_no_step_by_the_person() {
    for (v, dir) in fixtures() {
        let home = seed(&dir);
        // SAFETY: the only test in this binary that reads the environment.
        unsafe {
            std::env::set_var("CUA_HOME", home.path());
            std::env::set_var("CUA_CREDENTIAL_STORE", "file");
        }
        let (clouds, store, aws_s, hz_s) = manage(home.path()).await;
        let old = raw(home.path());
        let providers: Vec<String> = old["connections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["provider"].as_str().unwrap().to_string())
            .collect();

        // Status: every released link is connected, nothing else.
        let report = clouds.status(None).await.unwrap();
        for p in &providers {
            let row = report
                .providers
                .iter()
                .find(|r| &r.name == p)
                .unwrap_or_else(|| panic!("{v}: {p} listed"));
            assert!(row.connected, "{v}: {p} connected: {row:?}");
        }

        // The pool's budget and switches carry over as they were set.
        let policy_file = home.path().join("elastic/policy.json");
        if policy_file.exists() {
            let on_disk: Value =
                serde_json::from_slice(&std::fs::read(&policy_file).unwrap()).unwrap();
            let loaded =
                serde_json::to_value(cua_spaces::elastic::load_policy(home.path())).unwrap();
            for (k, val) in on_disk.as_object().unwrap() {
                assert_eq!(&loaded[k], val, "{v}: elastic policy {k}");
            }
            assert_ne!(
                loaded,
                serde_json::to_value(cua_spaces::elastic::ElasticPolicy::default()).unwrap(),
                "{v}: the fixture set a budget"
            );
        }

        // The sweep finds every recorded resource (listing only).
        let swept = clouds.sweep(None, true, true).await.unwrap();
        let found: BTreeSet<String> = swept
            .resources
            .iter()
            .map(|i| i.resource.id.clone())
            .collect();
        for r in store.resources().unwrap() {
            assert!(
                found.contains(&r.id),
                "{v}: sweep finds {} ({found:?})",
                r.id
            );
        }

        // Disconnect --delete removes everything Cua made, then the link.
        let audit_before = store.audit_entries(1000).unwrap().len();
        for p in &providers {
            clouds
                .disconnect(p, true)
                .await
                .unwrap_or_else(|e| panic!("{v}: disconnect {p}: {e}"));
        }
        assert!(store.connections().unwrap().is_empty(), "{v}");
        // Every cloud resource is gone (a Space's relay machine stays
        // recorded until the relay, absent here, removes it).
        let left: Vec<Resource> = store
            .resources()
            .unwrap()
            .into_iter()
            .filter(|r| r.resource_type != "relay_machine")
            .collect();
        assert!(left.is_empty(), "{v}: still recorded {left:?}");
        assert!(
            aws_s
                .lock()
                .unwrap()
                .instances
                .values()
                .all(|(state, _)| state == "terminated"),
            "{v}"
        );
        let hz = hz_s.lock().unwrap();
        assert!(hz.servers.is_empty() && hz.firewalls.is_empty(), "{v}");
        if providers.iter().any(|p| p == "hetzner") {
            // The released token signed every call.
            assert!(
                !hz.tokens.is_empty()
                    && hz
                        .tokens
                        .iter()
                        .all(|t| t == "Bearer hz-fake-token-0123456789"),
                "{v}: {:?}",
                hz.tokens
            );
        }
        drop(hz);
        // The released audit log is read and appended to.
        let audit = store.audit_entries(1000).unwrap();
        assert!(audit.len() > audit_before, "{v}");
        assert_eq!(audit.last().unwrap().action, "disconnect", "{v}");
    }
}

/// Runs a released cua (`CUA_UPGRADE_OLD_CUA`) in `home` (its HOME; the
/// cua home is `home/.cua`) against the fake AWS at `aws_url`, with a
/// default AWS profile and the profile `upgrade-fake`, both fake keys.
fn old_cua(old: &std::ffi::OsStr, home: &Path, aws_url: &str, args: &[&str]) -> (bool, String) {
    std::fs::write(
        home.join("aws-config"),
        "[default]\nregion = us-west-2\n\n[profile upgrade-fake]\nregion = us-west-2\n",
    )
    .unwrap();
    std::fs::write(
        home.join("aws-credentials"),
        "[default]\naws_access_key_id = AKIDDEFAULT\naws_secret_access_key = fake\n\n\
         [upgrade-fake]\naws_access_key_id = AKIDFAKE\naws_secret_access_key = fake\n",
    )
    .unwrap();
    let out = std::process::Command::new(old)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .env("CUA_HOME", home.join(".cua"))
        .env("CUA_CREDENTIAL_STORE", "file")
        .env("DO_NOT_TRACK", "1")
        .env("CUA_TELEMETRY", "0")
        .env("AWS_ENDPOINT_URL", aws_url)
        .env("AWS_CONFIG_FILE", home.join("aws-config"))
        .env("AWS_SHARED_CREDENTIALS_FILE", home.join("aws-credentials"))
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("$ cua {}\n{text}", args.join(" "));
    (out.status.success(), text)
}

fn oidc_aws() -> Connection {
    Connection {
        provider: "aws".into(),
        region: "us-west-2".into(),
        account: "123456789012".into(),
        connected_at: "2026-10-24T10:00:00Z".into(),
        ttl_hours: 4,
        auth: Auth::aws("conn_1", "arn:aws:iam::123456789012:role/cua-spaces"),
        budget_usd_month: Some(200.0),
        max_size: Some(MaxSize {
            vcpu: 4,
            mem_gib: 16,
        }),
        ..Default::default()
    }
}

/// A released cua reads a home with a federated AWS link without a parse
/// error, and refuses that link instead of reaching AWS with the default
/// profile.
#[tokio::test(flavor = "multi_thread")]
async fn a_released_cua_refuses_a_federated_link_instead_of_using_the_default_profile() {
    let Some(old) = std::env::var_os("CUA_UPGRADE_OLD_CUA") else {
        eprintln!("skipped: set CUA_UPGRADE_OLD_CUA=/path/to/a/released/cua");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    Store::new(&home.path().join(".cua"))
        .connect(oidc_aws())
        .unwrap();
    let (aws_url, aws_s) = fake_aws::fake().await;
    let (_, text) = old_cua(
        &old,
        home.path(),
        &aws_url,
        &["cloud", "status", "--embedded"],
    );
    assert!(!text.contains("state.json:"), "a parse error: {text}");
    // A sweep uses the stored link (`cloud test` checks the default
    // profile whatever is linked, so it proves nothing here).
    let (ok, text) = old_cua(
        &old,
        home.path(),
        &aws_url,
        &["cloud", "sweep", "--embedded"],
    );
    assert!(text.contains(FEDERATED_PROFILE), "{ok} {text}");
    assert!(
        aws_s.lock().unwrap().calls.is_empty(),
        "the released cua reached AWS: {:?}",
        aws_s.lock().unwrap().calls
    );
    // And the link is still whole for this cua.
    assert_eq!(
        Store::new(&home.path().join(".cua"))
            .connection("aws")
            .unwrap(),
        Some(oidc_aws())
    );
}

/// A released cua writes the file back with only the keys it knows; this
/// cua puts the rest back from links.json.
#[tokio::test(flavor = "multi_thread")]
async fn a_home_this_cua_wrote_survives_a_released_cua_writing_it() {
    let Some(old) = std::env::var_os("CUA_UPGRADE_OLD_CUA") else {
        eprintln!("skipped: set CUA_UPGRADE_OLD_CUA=/path/to/a/released/cua");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let cua_home = home.path().join(".cua");
    let store = Store::new(&cua_home);
    let sealed = Connection {
        provider: "hetzner".into(),
        region: "fsn1".into(),
        account: "my-project".into(),
        connected_at: "2026-10-24T11:00:00Z".into(),
        auth: Auth::Sealed {
            conn_id: "conn_2".into(),
        },
        budget_usd_month: Some(50.0),
        ..Default::default()
    };
    store.connect(sealed.clone()).unwrap();
    store.connect(oidc_aws()).unwrap();
    let (aws_url, _aws) = fake_aws::fake().await;
    // The released cua links AWS with a profile of its own: it writes the
    // whole file back.
    let (ok, text) = old_cua(
        &old,
        home.path(),
        &aws_url,
        &[
            "cloud",
            "connect",
            "aws",
            "--embedded",
            "--profile",
            "upgrade-fake",
            "--region",
            "us-west-2",
        ],
    );
    assert!(ok, "{text}");
    let on_disk = std::fs::read_to_string(cua_home.join("cloud/state.json")).unwrap();
    assert!(
        !on_disk.contains("\"auth\"") && !on_disk.contains("budget_usd_month"),
        "the released cua kept the new keys (nothing to restore): {on_disk}"
    );
    let back = Store::new(&cua_home);
    // The Hetzner link it did not touch is whole again...
    assert_eq!(back.connection("hetzner").unwrap(), Some(sealed));
    // ...and the AWS link it made is its own (a CLI link, profile kept).
    let aws = back.connection("aws").unwrap().unwrap();
    assert_eq!(
        (aws.auth, aws.profile.as_str()),
        (Auth::Cli, "upgrade-fake")
    );
    assert_eq!(aws.budget_usd_month, None);
}

/// Serves the fake clouds until the file `CUA_UPGRADE_SERVE_FILE` names is
/// removed (it holds their URLs): how the fixtures were captured from
/// released cua binaries (see `tests/fixtures/upgrade/README.md`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "serves fake clouds for a capture by hand (CUA_UPGRADE_SERVE_FILE)"]
async fn serve_fakes_for_a_capture() {
    let Some(file) = std::env::var_os("CUA_UPGRADE_SERVE_FILE") else {
        return;
    };
    let (aws, aws_s) = fake_aws::fake().await;
    let (hz, _) = fake_hetzner::serve().await;
    std::fs::write(&file, format!("AWS_ENDPOINT_URL={aws}\nHETZNER_API={hz}\n")).unwrap();
    while Path::new(&file).exists() {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let calls: Vec<String> = aws_s
        .lock()
        .unwrap()
        .calls
        .iter()
        .map(|c| c["Action"].clone())
        .collect();
    eprintln!("aws calls: {calls:?}");
}
