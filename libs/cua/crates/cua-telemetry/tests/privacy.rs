//! Tier a guarantees: the schema allowlist, PII fuzzing through every
//! builder, every opt-out switch, the first-run notice rule, batching and
//! the offline spool, and the no-network guard. No test here reaches the
//! network: clients use a `MemorySink` or a loopback fake endpoint.

use cua_telemetry::events::{self, *};
use cua_telemetry::schema::{self, Kind};
use cua_telemetry::sink::{HttpSink, MemorySink, SendError, Sink};
use cua_telemetry::{Captured, Telemetry};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    let m: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k| m.get(k).cloned()
}

fn client(home: &Path, env: &[(&str, &str)], sink: Arc<MemorySink>) -> Telemetry {
    Telemetry::builder()
        .env(env_of(env))
        .home(home)
        .sink(sink)
        .product("cli", "1.2.3")
        .foreground()
        .build()
}

/// A client with the notice already acknowledged.
fn ready(home: &Path, env: &[(&str, &str)]) -> (Telemetry, Arc<MemorySink>) {
    let sink = Arc::new(MemorySink::new());
    let t = client(home, env, sink.clone());
    t.acknowledge_notice();
    (t, sink)
}

const PII: &[&str] = &[
    "/Users/alice/Documents/secret-plan.pdf",
    "C:\\Users\\bob\\Desktop\\taxes.xlsx",
    "/home/carol/.ssh/id_ed25519",
    "alice@example.com",
    "bob.smith+cua@corp.example.org",
    "alices-macbook-pro.local",
    "build-host-17.internal.corp",
    "192.168.1.44",
    "2001:db8::8a2e:370:7334",
    "https://mail.example.com/inbox?user=alice",
    "Inbox (3) - alice@example.com - Gmail",
    "Q3 Salary Review.docx - Word",
    "com.acme.SecretInternalApp",
    "my-private-sandbox-name",
    "ghcr.io/acme-corp/internal-image:prod",
    "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    "password123!",
    "rm -rf ~/work && echo done",
    "Please book a flight to Paris for Alice",
];

/// Distinctive fragments of [`PII`] that must never appear in a payload.
const FRAGMENTS: &[&str] = &[
    "alice",
    "bob",
    "carol",
    "example",
    "secret",
    "taxes",
    "macbook",
    "build-host",
    "192.168",
    "2001:db8",
    "mail.",
    "inbox",
    "salary",
    "acme",
    "private",
    "sk-ant",
    "password",
    "rm -rf",
    "paris",
    ".local",
    "internal",
    "users",
    "home/",
    "ssh",
];

/// Every builder, fed `s` wherever it takes text.
fn all_builders(s: &str) -> Vec<Event> {
    let d = Duration::from_millis(1234);
    let info = TeleportInfo::new(
        s,
        s,
        s,
        CallerKind::EmbeddedSdk,
        CallerKindSource::SelfReported,
        s,
    );
    let mut v = vec![
        events::first_run(s),
        events::cli_command(s, Outcome::Error, Some(s), d, true, s),
        events::sandbox_created(
            &SandboxCreate {
                on: s,
                kind: s,
                runtime: s,
                image: s,
                guest_os: s,
                with_overlays: true,
                with_build: false,
            },
            Outcome::Error,
            Some(s),
            d,
        ),
        events::sandbox_deleted(s, Outcome::Ok, Some(s)),
        events::teleport_attempted(&info),
        events::teleport_completed(&info, TeleportOutcome::RequiresCuaApp, d, 3),
        events::keyvault_consent(ConsentDecision::Denied),
        events::stream_stats(&StreamStats {
            transport: s,
            codec: s,
            hw_decode: Some(true),
            avg_fps: 59.0,
            p95_latency_ms: Some(40.0),
            height: 1080,
            duration: d,
        }),
        events::agent_run_completed(s, s, Outcome::Ok, Some(s), d),
        events::bench_run_completed(s, Some(0.42), 12, Outcome::Ok),
        events::daemon_started(s, Outcome::Ok),
        events::daemon_health(d, 3),
        events::spacesd_health(d, 0),
        events::spacesd_session_deliveries(false, 2),
        events::stream_summary(s, s, Some(true), 900, 1080, Some(40.0), d).unwrap(),
        events::app_active(),
        // Unknown experiment ids are dropped (`none`); known ones kept.
        events::app_active_with(&[s, "sharing", s]),
        events::space_create(
            &events::SpaceCreate {
                on: s,
                guest_os: s,
                kind: s,
                last_phase: s,
                stalled: true,
                gpu: true,
            },
            Outcome::Error,
            d,
        ),
        events::space_create_started(s, s, s, true),
        events::volume_setup(
            &events::VolumeSetup {
                surface: s,
                storage: s,
                add_to_finder: true,
                mount_method: s,
            },
            Outcome::Ok,
        ),
        events::host_setup(s, true, true, Outcome::Error, Some(s)),
        events::host_space_provided(s, s, Outcome::Error, Some(s)),
        events::keyvault_action(
            events::KeyvaultAction::Unlock,
            events::KeyvaultMethod::OsKeyStore,
            Outcome::Ok,
        ),
        events::presence_session(Outcome::Ok, 3, d),
    ];
    // Builders that drop unknown input return None: they must for PII.
    for opt in [
        events::api_used(s, Outcome::Ok, Some(s), d),
        events::spaces_feature_used(s),
        events::onboarding_step(s, Outcome::Ok),
        events::onboarding_page(s, "shown", "none"),
        events::onboarding_page("welcome", s, "none"),
        events::space_wizard(s),
        events::persistent_agent(s, Some(s), Outcome::Ok, Some(s)),
        events::share(s, s, Outcome::Ok),
        events::app_update(s, s, s),
        events::device_enroll(s, Outcome::Ok),
        events::experiment(s, "sharing"),
        events::experiment("experiment_on", s),
    ] {
        assert!(opt.is_none(), "PII accepted as a vocabulary value: {s}");
    }
    v.push(events::api_used("sandbox.connect", Outcome::Ok, Some(s), d).unwrap());
    v.push(events::spaces_feature_used("teleport_drop").unwrap());
    v.push(events::onboarding_step("signed_in", Outcome::Ok).unwrap());
    // Known names with caller text wherever else a builder takes it.
    v.push(events::onboarding_page("volume", "completed", s).unwrap());
    v.push(events::space_wizard("cancelled").unwrap());
    v.push(events::persistent_agent("create", Some(s), Outcome::Error, Some(s)).unwrap());
    v.push(events::share("share", s, Outcome::Ok).unwrap());
    v.push(events::app_update("installed", s, s).unwrap());
    v.push(events::device_enroll("rekey", Outcome::Ok).unwrap());
    v.push(events::experiment("experiment_off", "your_cloud").unwrap());
    v
}

/// A client that may send every event (product per event).
fn preview_all(home: &Path, ev: &Event) -> Value {
    let spec = schema::spec(ev.name).unwrap();
    let t = Telemetry::builder()
        .env(env_of(&[]))
        .home(home)
        .sink(Arc::new(MemorySink::new()))
        .product(spec.products[0], "0.1.0")
        .foreground()
        .build();
    t.preview(ev)
        .unwrap_or_else(|e| panic!("{} failed validation: {e}", ev.name))
}

#[test]
fn every_builder_output_validates_against_the_declared_schema() {
    let home = tempfile::tempdir().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for ev in all_builders("linux:24.04") {
        preview_all(home.path(), &ev);
        seen.insert(ev.name);
    }
    // Every declared event has a builder under test.
    for spec in schema::EVENTS {
        assert!(seen.contains(spec.name), "no builder covers {}", spec.name);
    }
}

#[test]
fn schema_allowlist_rejects_any_undeclared_property_or_value() {
    let home = tempfile::tempdir().unwrap();
    let ev = events::sandbox_deleted("local", Outcome::Ok, None);
    let ok = preview_all(home.path(), &ev);
    let props = ok["properties"].as_object().unwrap().clone();
    assert!(schema::validate(ev.name, &props).is_ok());

    for extra in [
        "path",
        "hostname",
        "user",
        "window_title",
        "url",
        "prompt",
        "$ip",
    ] {
        let mut p = props.clone();
        p.insert(extra.into(), Value::from("x"));
        assert_eq!(
            schema::validate(ev.name, &p),
            Err(schema::SchemaError::UndeclaredProperty(extra.into()))
        );
    }
    let mut p = props.clone();
    p.insert("location".into(), Value::from("/Users/alice"));
    assert!(matches!(
        schema::validate(ev.name, &p),
        Err(schema::SchemaError::BadValue(_))
    ));
    let mut p = props.clone();
    p.insert("$geoip_disable".into(), Value::Bool(false));
    assert!(schema::validate(ev.name, &p).is_err());
    let mut p = props;
    p.remove("error_kind");
    assert!(matches!(
        schema::validate(ev.name, &p),
        Err(schema::SchemaError::MissingProperty(_))
    ));
}

#[test]
fn schema_declares_no_free_text_property() {
    let mut names = std::collections::BTreeSet::new();
    for spec in schema::EVENTS {
        assert!(spec.name.starts_with("cua_"), "{}", spec.name);
        assert!(names.insert(spec.name), "duplicate event {}", spec.name);
        assert!(spec.sample_rate > 0.0 && spec.sample_rate <= 1.0);
        let mut props = std::collections::BTreeSet::new();
        for p in schema::COMMON.iter().chain(spec.props) {
            assert!(props.insert(p.name), "{}: duplicate {}", spec.name, p.name);
            assert!(!p.doc.is_empty());
            // Only closed kinds exist; check enum vocabularies hold no
            // path, URL or address-like values.
            if let Kind::Enum(v) | Kind::Set(v) = p.kind {
                for x in v {
                    assert!(
                        !x.contains('/') && !x.contains('@') && !x.contains(' '),
                        "{x}"
                    );
                }
            }
        }
    }
}

#[test]
fn pii_never_reaches_a_payload_through_any_builder() {
    let home = tempfile::tempdir().unwrap();
    for pii in PII {
        for variant in [pii.to_string(), pii.to_uppercase(), format!("  {pii}\n")] {
            for ev in all_builders(&variant) {
                let payload = preview_all(home.path(), &ev);
                let text = payload.to_string().to_lowercase();
                for f in FRAGMENTS {
                    assert!(
                        !text.contains(f),
                        "{}: fragment {f:?} of {pii:?} leaked: {text}",
                        ev.name
                    );
                }
            }
        }
    }
}

#[test]
fn classifiers_keep_catalog_values_and_coarsen_everything_else() {
    assert_eq!(
        events::image_id("ghcr.io/trycua/linux:24.04"),
        "linux:24.04"
    );
    assert_eq!(events::image_id("linux:24.04"), "linux:24.04");
    assert_eq!(
        events::image_id("ghcr.io/trycua/linux:24.04@sha256:abc"),
        "linux:24.04"
    );
    assert_eq!(events::image_id("ghcr.io/acme/app:1"), "custom");
    assert_eq!(events::image_id(""), "none");
    assert_eq!(events::location("direct:10.0.0.5:3211"), "direct");
    assert_eq!(events::location("my-own-provider"), "other");
    assert_eq!(events::runtime("gvisor"), "gvisor");
    assert_eq!(events::runtime("acme-vm"), "provider");
    assert_eq!(events::teleport_app("chrome"), "chrome");
    assert_eq!(events::teleport_app("com.acme.Secret"), "other");
    assert_eq!(events::error_kind("InvalidArgument"), "invalid_argument");
    assert_eq!(events::error_kind("invalid argument: /Users/x"), "other");
    assert_eq!(events::harness("claude-code"), "claude-code");
    assert_eq!(events::taskset("/Users/alice/tasks"), "custom");
    assert_eq!(events::score_bucket(Some(0.5)), "50_74");
    assert_eq!(events::count_bucket(7), "5_9");
}

#[test]
fn self_reported_caller_kind_never_claims_a_signature() {
    for p in schema::SDK_PRODUCTS {
        assert_eq!(CallerKind::self_reported(p), CallerKind::EmbeddedSdk);
    }
    for p in ["cli", "spaces_app", "daemon"] {
        assert_eq!(CallerKind::self_reported(p), CallerKind::Unknown);
    }
}

// ---------------------------------------------------------------------------
// Switches
// ---------------------------------------------------------------------------

fn sends_nothing(home: &Path, env: &[(&str, &str)]) {
    let (t, sink) = ready(home, env);
    assert!(!t.is_enabled(), "{env:?} should disable telemetry");
    assert_eq!(
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None)),
        Captured::Disabled
    );
    t.flush(Duration::from_secs(1));
    assert!(sink.batches().is_empty());
    let dir = home.join("telemetry");
    assert!(
        !dir.join("install_id").exists(),
        "an id was created while off"
    );
    assert!(!dir.join("salt").exists());
    assert!(t.salted_hash("space", "x").is_none());
}

#[test]
fn do_not_track_disables_everything_and_wins_over_cua_telemetry_on() {
    for v in ["1", "true", "yes"] {
        let h = tempfile::tempdir().unwrap();
        sends_nothing(h.path(), &[("DO_NOT_TRACK", v), ("CUA_TELEMETRY", "1")]);
    }
    // DO_NOT_TRACK=0 is not an opt-out.
    let h = tempfile::tempdir().unwrap();
    let (t, _) = ready(h.path(), &[("DO_NOT_TRACK", "0")]);
    assert!(t.is_enabled());
}

#[test]
fn cua_telemetry_zero_disables() {
    for v in ["0", "false", "off", "no", "OFF"] {
        let h = tempfile::tempdir().unwrap();
        sends_nothing(h.path(), &[("CUA_TELEMETRY", v)]);
    }
}

#[test]
fn legacy_switches_disable() {
    let h = tempfile::tempdir().unwrap();
    sends_nothing(h.path(), &[("CUA_TELEMETRY_ENABLED", "false")]);
    let h = tempfile::tempdir().unwrap();
    sends_nothing(h.path(), &[("CUA_TELEMETRY_DISABLED", "1")]);
}

#[test]
fn config_file_switch_disables_and_is_what_the_cli_and_app_write() {
    let h = tempfile::tempdir().unwrap();
    // What `cua config set telemetry off`, `cua telemetry off` and the
    // Spaces app setting write: `[telemetry] enabled = "off"`.
    std::fs::write(
        h.path().join("config.toml"),
        "[default]\non = \"cloud\"\n\n[telemetry]\nenabled = \"off\"\n",
    )
    .unwrap();
    sends_nothing(h.path(), &[]);

    let h = tempfile::tempdir().unwrap();
    std::fs::write(h.path().join("config.toml"), "[default]\non = \"cloud\"\n").unwrap();
    let (t, _) = ready(h.path(), &[]);
    assert!(t.is_enabled());
    t.set_enabled(false).unwrap();
    assert!(!t.is_enabled());
    let text = std::fs::read_to_string(h.path().join("config.toml")).unwrap();
    assert!(
        text.contains("on = \"cloud\""),
        "other settings kept: {text}"
    );
    assert!(text.contains("enabled = \"off\""));
    sends_nothing(h.path(), &[]);
    // The environment still wins over the file.
    let (t, _) = ready(h.path(), &[("CUA_TELEMETRY", "1")]);
    assert!(t.is_enabled());
    t.set_enabled(true).unwrap();
    let (t, _) = ready(h.path(), &[]);
    assert!(t.is_enabled());
}

#[test]
fn ci_defaults_off_and_is_marked_when_explicitly_enabled() {
    for var in ["CI", "GITHUB_ACTIONS", "BUILDKITE", "GITLAB_CI"] {
        let h = tempfile::tempdir().unwrap();
        sends_nothing(h.path(), &[(var, "true")]);
        let (t, _) = ready(h.path(), &[(var, "true")]);
        assert_eq!(t.status().source_kind, "ci");
    }
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[("CI", "true"), ("CUA_TELEMETRY", "1")]);
    assert_eq!(
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None)),
        Captured::Queued
    );
    t.flush(Duration::from_secs(1));
    let evs = sink.events();
    let ev = evs
        .iter()
        .find(|e| e["event"] == "cua_sandbox_deleted")
        .unwrap();
    assert_eq!(ev["properties"]["is_ci"], true);
}

#[test]
fn turning_off_drops_queued_events_and_the_spool() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    sink.set_failing(true);
    t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t.flush(Duration::from_secs(1));
    assert!(t.spooled() > 0);
    t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t.set_enabled(false).unwrap();
    assert_eq!(t.queued(), 0);
    assert_eq!(t.spooled(), 0);
}

// ---------------------------------------------------------------------------
// Notice, identity, batching
// ---------------------------------------------------------------------------

#[test]
fn nothing_is_sent_before_the_first_run_notice_was_shown() {
    let h = tempfile::tempdir().unwrap();
    let sink = Arc::new(MemorySink::new());
    let t = client(h.path(), &[], sink.clone());
    assert!(!t.notice_shown());
    assert_eq!(
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None)),
        Captured::NoticePending
    );
    assert!(
        t.notice_shown(),
        "the notice was shown (stderr) and recorded"
    );
    t.flush(Duration::from_secs(1));
    assert!(sink.batches().is_empty());
    assert!(!h.path().join("telemetry/install_id").exists());

    // The next process sends, plus the one-time first-run event.
    let t = client(
        h.path(),
        &[("CUA_INSTALL_CHANNEL", "homebrew")],
        sink.clone(),
    );
    assert_eq!(
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None)),
        Captured::Queued
    );
    t.flush(Duration::from_secs(1));
    let names: Vec<String> = sink
        .events()
        .iter()
        .map(|e| e["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["cua_first_run", "cua_sandbox_deleted"]);
    assert_eq!(
        sink.events()[0]["properties"]["install_channel"],
        "homebrew"
    );
    // Only once per install.
    let t = client(h.path(), &[], sink.clone());
    t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t.flush(Duration::from_secs(1));
    assert_eq!(
        sink.events()
            .iter()
            .filter(|e| e["event"] == "cua_first_run")
            .count(),
        1
    );
}

/// install.sh / install.ps1 record their channel in the state dir, so the
/// first-run event names it even when the env var is gone; an unknown value
/// never leaves the machine verbatim.
#[test]
fn first_run_reads_the_installer_channel_file() {
    for (recorded, reported) in [
        ("install_script\n", "install_script"),
        ("/home/alice", "unknown"),
    ] {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(h.path().join("telemetry")).unwrap();
        std::fs::write(h.path().join("telemetry/install_channel"), recorded).unwrap();
        let (t, sink) = ready(h.path(), &[]);
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
        t.flush(Duration::from_secs(1));
        let first_run: Vec<Value> = sink
            .events()
            .into_iter()
            .filter(|e| e["event"] == "cua_first_run")
            .collect();
        assert_eq!(first_run.len(), 1);
        assert_eq!(first_run[0]["properties"]["install_channel"], reported);
    }
}

#[test]
fn external_notice_mode_waits_for_the_app_to_acknowledge() {
    let h = tempfile::tempdir().unwrap();
    let sink = Arc::new(MemorySink::new());
    let t = client(h.path(), &[], sink.clone());
    t.set_notice_mode(cua_telemetry::NoticeMode::External);
    assert_eq!(
        t.capture(events::spaces_feature_used("teleport_drop").unwrap()),
        Captured::NoticePending
    );
    assert!(!t.notice_shown());
    t.acknowledge_notice();
    t.set_product("spaces_app", "0.1.0");
    assert_eq!(
        t.capture(events::spaces_feature_used("teleport_drop").unwrap()),
        Captured::Queued
    );
}

#[test]
fn payload_envelope_is_anonymous() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    t.capture(events::sandbox_created(
        &SandboxCreate {
            on: "local",
            kind: "container",
            runtime: "gvisor",
            image: "ghcr.io/trycua/linux:24.04",
            guest_os: "linux",
            ..Default::default()
        },
        Outcome::Ok,
        None,
        Duration::from_secs(7),
    ));
    t.flush(Duration::from_secs(1));
    let body = &sink.batches()[0];
    assert_eq!(body["api_key"], cua_telemetry::sink::POSTHOG_API_KEY);
    let ev = body["batch"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["event"] == "cua_sandbox_created")
        .unwrap();
    let keys: std::collections::BTreeSet<&str> =
        ev.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["distinct_id", "event", "properties", "timestamp", "uuid"]
            .into_iter()
            .collect()
    );
    let id = std::fs::read_to_string(h.path().join("telemetry/install_id")).unwrap();
    assert_eq!(ev["distinct_id"], id.trim());
    assert_eq!(id.trim().len(), 32);
    let p = &ev["properties"];
    assert_eq!(p["$geoip_disable"], true);
    assert_eq!(p["$process_person_profile"], false);
    assert!(p.get("$ip").is_none());
    assert_eq!(p["image"], "linux:24.04");
    assert_eq!(p["duration_bucket"], "5_29s");
    assert_eq!(p["product"], "cli");
    assert_eq!(p["product_version"], "1.2.3");
}

#[test]
fn reset_id_creates_a_new_identity_and_salt() {
    let h = tempfile::tempdir().unwrap();
    let (t, _) = ready(h.path(), &[]);
    let a = t.salted_hash("space", "s1").unwrap();
    assert_eq!(a.len(), 16);
    assert_eq!(
        t.salted_hash("space", "s1").unwrap(),
        a,
        "stable per install"
    );
    let id1 = std::fs::read_to_string(h.path().join("telemetry/install_id")).unwrap();
    let removed = t.reset_id().unwrap();
    assert_eq!(removed.len(), 2);
    let b = t.salted_hash("space", "s1").unwrap();
    let id2 = std::fs::read_to_string(h.path().join("telemetry/install_id")).unwrap();
    assert_ne!(a, b);
    assert_ne!(id1, id2);
    // Another install hashes the same value differently.
    let h2 = tempfile::tempdir().unwrap();
    let (t2, _) = ready(h2.path(), &[]);
    assert_ne!(t2.salted_hash("space", "s1").unwrap(), b);
}

#[test]
fn show_last_prints_exactly_what_was_sent() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    t.capture(events::keyvault_consent(ConsentDecision::Approved));
    let last = t.show_last(10);
    assert!(last.iter().any(|e| e["status"] == "queued"));
    t.flush(Duration::from_secs(1));
    let last = t.show_last(10);
    assert!(!sink.events().is_empty());
    let consent = last
        .iter()
        .find(|e| e["payload"]["event"] == "cua_keyvault_consent")
        .unwrap();
    assert_eq!(consent["status"], "sent");
    let wire = sink
        .events()
        .into_iter()
        .find(|e| e["event"] == "cua_keyvault_consent")
        .unwrap();
    assert_eq!(
        consent["payload"], wire,
        "show-last matches the wire payload"
    );
}

#[test]
fn offline_batches_are_spooled_bounded_and_retried() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    sink.set_failing(true);
    for _ in 0..10 {
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    }
    t.flush(Duration::from_secs(1));
    assert!(sink.batches().is_empty());
    let spooled = t.spooled();
    assert!(spooled >= 10, "spooled {spooled}");
    // A later process delivers the spool.
    sink.set_failing(false);
    let (t2, sink2) = ready(h.path(), &[]);
    t2.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t2.flush(Duration::from_secs(1));
    assert!(sink2.events().len() > spooled);
    assert_eq!(t2.spooled(), 0);
    // Bounded.
    let (t3, sink3) = ready(h.path(), &[]);
    sink3.set_failing(true);
    for _ in 0..600 {
        t3.capture(events::sandbox_deleted("local", Outcome::Ok, None));
        if t3.queued() >= 50 {
            t3.send_once();
        }
    }
    t3.flush(Duration::from_secs(2));
    assert!(t3.spooled() <= cua_telemetry::client::MAX_SPOOL);
}

#[test]
fn sampling_and_rate_limits_bound_volume() {
    let h = tempfile::tempdir().unwrap();
    let (t, _) = ready(h.path(), &[]);
    let mut queued = 0;
    for _ in 0..400 {
        let s = StreamStats {
            transport: "quic",
            codec: "h264",
            ..Default::default()
        };
        if t.capture(events::stream_stats(&s)) == Captured::Queued {
            queued += 1;
        }
        if t.queued() >= 50 {
            t.send_once();
        }
    }
    // Sample rate 0.5 (+ the first-run event path is separate).
    assert!((120..=280).contains(&queued), "queued {queued}");
    let mut limited = false;
    for _ in 0..1000 {
        if t.capture(events::sandbox_deleted("local", Outcome::Ok, None)) == Captured::RateLimited {
            limited = true;
            break;
        }
        if t.queued() >= 50 {
            t.send_once();
        }
    }
    assert!(limited, "per-process hourly cap applies");
}

#[test]
fn capture_never_blocks_on_a_slow_or_dead_endpoint() {
    // A loopback port that accepts but never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tempfile::tempdir().unwrap();
    let t = Telemetry::builder()
        .env(env_of(&[]))
        .home(h.path())
        .sink(Arc::new(HttpSink::new(
            format!("http://{addr}/batch/"),
            true,
        )))
        .product("cli", "1.0.0")
        .build();
    t.acknowledge_notice();
    let start = std::time::Instant::now();
    for _ in 0..20 {
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    }
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "capture blocked"
    );
    let start = std::time::Instant::now();
    t.flush(Duration::from_millis(300));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "flush exceeded its budget"
    );
    drop(listener);
}

// ---------------------------------------------------------------------------
// No network in tests
// ---------------------------------------------------------------------------

#[test]
fn test_runs_forbid_the_network_and_disable_telemetry() {
    // libs/cua/.cargo/config.toml sets these for every cargo-started
    // process; removing them must fail this test.
    assert_eq!(std::env::var("CUA_TELEMETRY").as_deref(), Ok("0"));
    assert_eq!(
        std::env::var("CUA_TELEMETRY_FORBID_NETWORK").as_deref(),
        Ok("1")
    );
    assert!(!cua_telemetry::global().is_enabled());
    assert!(cua_telemetry::config::network_forbidden(
        &cua_telemetry::config::process_env
    ));
}

#[test]
fn forbidden_network_refuses_real_endpoints_without_connecting() {
    let before = cua_telemetry::sink::forbidden_attempts();
    let sink = HttpSink::new(cua_telemetry::sink::DEFAULT_ENDPOINT, true);
    assert_eq!(
        sink.send(&serde_json::json!({"batch": []})),
        Err(SendError::Forbidden)
    );
    assert_eq!(cua_telemetry::sink::forbidden_attempts(), before + 1);
    for url in [
        "https://eu.i.posthog.com/batch/",
        "http://127.evil.example/",
        "http://localhost.example.com/",
    ] {
        assert!(!cua_telemetry::sink::is_loopback_url(url), "{url}");
    }
    for url in [
        "http://127.0.0.1:9/x",
        "http://localhost:1/",
        "http://[::1]:5/",
    ] {
        assert!(cua_telemetry::sink::is_loopback_url(url), "{url}");
    }
}

#[test]
fn http_sink_posts_the_batch_to_a_loopback_fake_endpoint() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        // Bounded read: until the client goes quiet or 1 MiB.
        s.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        while buf.len() < (1 << 20) {
            match s.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
            .unwrap();
        String::from_utf8_lossy(&buf).to_string()
    });
    let h = tempfile::tempdir().unwrap();
    let t = Telemetry::builder()
        .env(env_of(&[]))
        .home(h.path())
        .sink(Arc::new(HttpSink::new(
            format!("http://{addr}/batch/"),
            true,
        )))
        .product("cli", "1.0.0")
        .foreground()
        .build();
    t.acknowledge_notice();
    t.capture(events::keyvault_consent(ConsentDecision::Requested));
    t.flush(Duration::from_secs(5));
    let req = server.join().unwrap();
    assert!(req.starts_with("POST /batch/ "), "{req}");
    assert!(req.contains("cua_keyvault_consent"));
    assert!(req.contains("\"$geoip_disable\":true"), "{req}");
}

#[test]
fn a_switch_flipped_elsewhere_applies_after_refresh_and_clears_the_spool() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    sink.set_failing(true);
    t.capture(events::sandbox_deleted("local", Outcome::Ok, None));
    t.flush(Duration::from_secs(1));
    assert!(t.spooled() > 0);
    // Another process (the CLI's `cua config set telemetry off`, the app).
    std::fs::write(
        h.path().join("config.toml"),
        "[telemetry]\nenabled = \"off\"\n",
    )
    .unwrap();
    t.refresh();
    assert_eq!(
        t.capture(events::sandbox_deleted("local", Outcome::Ok, None)),
        Captured::Disabled
    );
    assert_eq!(t.spooled(), 0);
}

/// The "Telemetry and privacy" pages list every event and every property
/// the schema declares, so nothing is sent that the docs do not name.
#[test]
fn docs_page_names_every_event_and_property() {
    let page = concat!(
        include_str!("../../../../../docs/content/docs/cua-sdk/concepts/telemetry.mdx"),
        include_str!("../../../../../docs/content/docs/cua-sdk/concepts/telemetry-events.mdx"),
    );
    for spec in schema::EVENTS {
        assert!(
            page.contains(&format!("`{}`", spec.name)),
            "docs miss event {}",
            spec.name
        );
        for p in spec.props {
            assert!(
                page.contains(&format!("`{}`", p.name)),
                "docs miss property {} of {}",
                p.name,
                spec.name
            );
        }
    }
    for p in schema::COMMON {
        if !p.name.starts_with('$') {
            assert!(
                page.contains(&format!("`{}`", p.name)),
                "docs miss common {}",
                p.name
            );
        }
    }
    for kind in schema::CALLER_KINDS {
        assert!(
            page.contains(&format!("`{kind}`")),
            "docs miss caller kind {kind}"
        );
    }
    assert!(!page.contains('\u{2014}'), "no em dashes in docs");
}

#[test]
fn activation_builders_keep_vocabulary_and_coarsen_the_rest() {
    let d = Duration::from_secs(47);
    let e = events::space_create(
        &events::SpaceCreate {
            on: "local",
            guest_os: "linux",
            kind: "container",
            last_phase: "pulling",
            stalled: true,
            gpu: false,
        },
        Outcome::Ok,
        d,
    );
    // Success never names a phase or a stall.
    assert_eq!(e.props["failed_phase"], "none");
    assert_eq!(e.props["stalled"], false);
    assert_eq!(e.props["time_bucket"], "40_59s");
    let e = events::space_create(
        &events::SpaceCreate {
            on: "cloud",
            guest_os: "windows",
            kind: "vm",
            last_phase: "none",
            stalled: false,
            gpu: true,
        },
        Outcome::Error,
        Duration::from_secs(4000),
    );
    assert_eq!(e.props["failed_phase"], "other");
    assert_eq!(e.props["time_bucket"], "gte_30m");
    assert_eq!(e.props["location"], "cloud");
    assert_eq!(e.props["gpu"], true);
    // A cancel names the phase it was cancelled in, and is never a stall.
    let c = events::space_create(
        &events::SpaceCreate {
            on: "local",
            guest_os: "macos",
            kind: "vm",
            last_phase: "pulling",
            stalled: true,
            gpu: true,
        },
        Outcome::Cancelled,
        Duration::from_secs(25),
    );
    assert_eq!(c.props["outcome"], "cancelled");
    assert_eq!(c.props["failed_phase"], "pulling");
    assert_eq!(c.props["stalled"], false);
    assert_eq!(c.props["time_bucket"], "20_39s");
    let s = events::space_create_started("cloud", "windows", "vm", true);
    assert_eq!(
        (&s.props["location"], &s.props["gpu"]),
        (&serde_json::json!("cloud"), &serde_json::json!(true))
    );
    let v = events::volume_setup(
        &events::VolumeSetup {
            surface: "onboarding",
            storage: "s3",
            add_to_finder: true,
            mount_method: "FSKit",
        },
        Outcome::Ok,
    );
    assert_eq!(v.props["mount_method"], "fskit");
    assert_eq!(v.props["storage"], "s3");
    assert_eq!(
        events::host_setup("RELAY", false, true, Outcome::Ok, None).props["profile"],
        "spare"
    );
    assert_eq!(
        events::share("unshare", "editor", Outcome::Ok)
            .unwrap()
            .props["role"],
        "none"
    );
    assert_eq!(
        events::share("share", "viewer", Outcome::Ok).unwrap().props["role"],
        "viewer"
    );
    assert_eq!(
        events::persistent_agent("create", Some("hermes"), Outcome::Ok, None)
            .unwrap()
            .props["harness"],
        "hermes"
    );
    assert!(
        events::onboarding_page("welcome", "shown", "/Users/alice")
            .unwrap()
            .props["choice"]
            == "none"
    );
    assert_eq!(Outcome::from_word("cancelled"), Outcome::Cancelled);
    assert_eq!(Outcome::from_word("/Users/alice"), Outcome::Error);
}

/// Retention: one `cua_app_active` per install per UTC day, none while
/// off, and a new one the next day.
#[test]
fn app_active_is_sent_once_per_day() {
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    assert_eq!(t.capture_active_on(20_000), Captured::Queued);
    assert_eq!(t.capture_active_on(20_000), Captured::AlreadyRecorded);
    // Another process the same day.
    let (t2, _) = ready(h.path(), &[]);
    assert_eq!(t2.capture_active_on(20_000), Captured::AlreadyRecorded);
    assert_eq!(t.capture_active_on(20_001), Captured::Queued);
    t.flush(Duration::from_secs(1));
    let active = sink
        .events()
        .iter()
        .filter(|e| e["event"] == "cua_app_active")
        .count();
    assert_eq!(active, 2);
    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[("DO_NOT_TRACK", "1")]);
    assert_eq!(t.capture_active_on(20_000), Captured::Disabled);
    t.flush(Duration::from_secs(1));
    assert!(sink.events().is_empty());
    assert!(!h.path().join("telemetry/active_day").exists());
}

/// `experiments_on`: the Spaces app's experiments, ordered members of a
/// fixed set or `none`, kept for the day's `cua_app_active` whichever
/// product sends it; nothing written while telemetry is off.
#[test]
fn the_active_day_carries_the_experiments_that_are_on() {
    let k = Kind::Set(schema::EXPERIMENTS);
    for ok in [
        "none",
        "sharing",
        "cua_volume+sharing",
        "cua_volume+your_cloud+sharing",
    ] {
        assert!(schema::value_ok(k, &Value::from(ok)), "{ok}");
    }
    for bad in [
        "",
        "sharing+cua_volume",
        "sharing+sharing",
        "cua_volume+",
        "+sharing",
        "none+sharing",
        "/Users/alice",
    ] {
        assert!(!schema::value_ok(k, &Value::from(bad)), "{bad}");
    }
    assert_eq!(
        events::experiments_set(&["sharing", "nope", "cua_volume", "sharing"]),
        "cua_volume+sharing"
    );
    assert_eq!(events::experiments_set(&[]), "none");

    let h = tempfile::tempdir().unwrap();
    let (t, sink) = ready(h.path(), &[]);
    assert_eq!(t.capture_active_on(20_000), Captured::Queued);
    t.set_experiments_on(&["your_cloud", "alice@example.com"]);
    // The next day, another process of the same install.
    let (t2, sink2) = ready(h.path(), &[]);
    assert_eq!(t2.capture_active_on(20_001), Captured::Queued);
    t.flush(Duration::from_secs(1));
    t2.flush(Duration::from_secs(1));
    let sets = |s: &MemorySink| -> Vec<String> {
        s.events()
            .iter()
            .filter(|e| e["event"] == "cua_app_active")
            .map(|e| {
                e["properties"]["experiments_on"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(sets(&sink), ["none"], "the first day had none");
    assert_eq!(sets(&sink2), ["your_cloud"]);
    let h2 = tempfile::tempdir().unwrap();
    let (off, _) = ready(h2.path(), &[("DO_NOT_TRACK", "1")]);
    off.set_experiments_on(&["sharing"]);
    assert!(!h2.path().join("telemetry/experiments_on").exists());
}

#[test]
fn stream_summary_is_bucketed_and_needs_video() {
    let e = events::stream_summary(
        "websocket",
        "h264",
        None,
        300,
        720,
        None,
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(e.props["transport"], "websocket");
    assert_eq!(e.props["fps_bucket"], "30_49");
    assert_eq!(e.props["decode"], "none");
    assert!(e.props.values().all(|v| v.is_string()), "{:?}", e.props);
    assert!(
        events::stream_summary(
            "websocket",
            "h264",
            None,
            0,
            720,
            None,
            Duration::from_secs(10)
        )
        .is_none()
    );
    assert!(
        events::stream_summary("websocket", "h264", None, 5, 720, None, Duration::ZERO).is_none()
    );
}
