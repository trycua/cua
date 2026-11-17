// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

use super::*;
use std::collections::BTreeMap;

fn record(harness: &str) -> AgentRecord {
    serde_json::from_value(json!({
        "name": "local-agent", "harness": harness, "space": "local:dev"
    }))
    .unwrap()
}

fn custom(saved: &ag::keys::AgentKeys, key: &str) {
    saved
        .set_with(
            "custom",
            None,
            key,
            None,
            &BTreeMap::from([
                ("base_url".into(), "http://localhost:8000/v1".into()),
                ("model".into(), "local-model".into()),
                ("wire".into(), "openai-chat".into()),
            ]),
        )
        .unwrap();
}

#[test]
fn saved_custom_endpoint_reaches_persistent_agents_with_and_without_a_key() {
    for key in ["local-test-key", ""] {
        let home = tempfile::tempdir().unwrap();
        let saved = ag::keys::AgentKeys::new(cua_auth::Store::TestKeychain(home.path().into()));
        custom(&saved, key);
        for harness in ["hermes", "opencode", "openclaw"] {
            saved
                .set_use(harness, Some(("CUSTOM_ENDPOINT_API_KEY", None)))
                .unwrap();
            let (opts, missing) = record(harness).run_options(&saved).unwrap();
            assert!(missing.is_empty());
            if harness == "hermes" {
                let yaml = opts
                    .hermes_config
                    .expect("Hermes needs its provider config");
                assert!(yaml.contains("provider: \"custom\""), "{yaml}");
                assert!(
                    yaml.contains("base_url: \"http://localhost:8000/v1\""),
                    "{yaml}"
                );
                assert!(yaml.contains("default: \"local-model\""), "{yaml}");
                if key.is_empty() {
                    assert!(yaml.contains("api_key: \"no-key-required\""), "{yaml}");
                } else {
                    assert!(yaml.contains("${CUSTOM_ENDPOINT_API_KEY}"), "{yaml}");
                    assert!(!yaml.contains(key), "credentials must stay out of config");
                    assert_eq!(opts.env["CUSTOM_ENDPOINT_API_KEY"], key);
                }
            } else {
                let endpoint = opts.endpoint.expect("the agent needs the saved endpoint");
                assert_eq!(endpoint.base_url, "http://localhost:8000/v1");
                assert_eq!(endpoint.model.as_deref(), Some("local-model"));
                assert_eq!(endpoint.wire, Some(ag::Wire::OpenaiChat));
                let expected_key = if key.is_empty() {
                    "no-key-required"
                } else {
                    key
                };
                assert_eq!(opts.env["OPENAI_API_KEY"], expected_key);
            }
        }
    }
}

#[test]
fn explicit_endpoint_does_not_receive_saved_credentials() {
    let home = tempfile::tempdir().unwrap();
    let saved = ag::keys::AgentKeys::new(cua_auth::Store::TestKeychain(home.path().into()));
    custom(&saved, "saved-key");
    for harness in ["hermes", "opencode", "openclaw"] {
        let mut rec = record(harness);
        rec.base_url = Some("http://localhost:9000/v1".into());
        rec.model = Some("explicit-model".into());
        rec.env
            .insert("OPENAI_API_KEY".into(), "explicit-key".into());
        let (opts, _) = rec.run_options(&saved).unwrap();
        let endpoint = opts.endpoint.unwrap();
        assert_eq!(endpoint.base_url, "http://localhost:9000/v1");
        assert_eq!(endpoint.model.as_deref(), Some("explicit-model"));
        assert_eq!(opts.env, rec.env);
        assert!(opts.hermes_config.is_none());
    }
}

#[test]
fn persistent_model_override_keeps_the_saved_endpoint() {
    let home = tempfile::tempdir().unwrap();
    let saved = ag::keys::AgentKeys::new(cua_auth::Store::TestKeychain(home.path().into()));
    custom(&saved, "local-test-key");
    let mut rec = record("opencode");
    rec.model = Some("another-local-model".into());
    rec.effort = Some("high".into());
    let (opts, _) = rec.run_options(&saved).unwrap();
    assert_eq!(
        opts.endpoint.unwrap().model.as_deref(),
        Some("another-local-model")
    );
    assert_eq!(opts.model, rec.model);
    assert_eq!(opts.effort, rec.effort);
}

#[test]
fn credential_refresh_uses_the_mapped_endpoint_key() {
    let home = tempfile::tempdir().unwrap();
    let saved = ag::keys::AgentKeys::new(cua_auth::Store::TestKeychain(home.path().into()));
    for harness in ["opencode", "openclaw"] {
        custom(&saved, "first-key");
        let rec = record(harness);
        let (opts, _) = rec.run_options(&saved).unwrap();
        let started = ag::runs::credential_hash(harness, &opts.env);
        let expected = BTreeMap::from([("OPENAI_API_KEY".into(), "first-key".into())]);
        assert_eq!(started, ag::runs::credential_hash(harness, &expected));
        let (unchanged, _) = rec.run_options(&saved).unwrap();
        assert_eq!(started, ag::runs::credential_hash(harness, &unchanged.env));
        custom(&saved, "second-key");
        let (rotated, _) = rec.run_options(&saved).unwrap();
        assert_ne!(started, ag::runs::credential_hash(harness, &rotated.env));
    }
}
