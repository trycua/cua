// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's device key against the Rust the relay and the CLI use (`cua_host::device`): the same id, signatures the
//! relay's own verifier accepts, and the messages the relay checks. `fixtures/device-messages.json` is the one list of
//! message formats the TypeScript client (`src/data/live/relay.ts`) also reads.

use cua_host::device::{
    device_id_of, member_message, register_message, session_message, verify_device_signature,
};
use cua_spaces_mobile_core::{call, device};
use serde_json::Value;

fn run(method: &str, args: Value) -> Value {
    serde_json::from_str(&call(method, &args.to_string()).unwrap()).unwrap()
}

#[test]
fn a_generated_key_has_the_relays_id_and_signs_what_the_relay_verifies() {
    let pkcs8 = run("device.generate", serde_json::json!({}))["pkcs8"]
        .as_str()
        .unwrap()
        .to_string();
    let info = run("device.info", serde_json::json!({ "pkcs8": pkcs8 }));
    let public = info["publicKey"].as_str().unwrap();
    assert_eq!(info["id"].as_str().unwrap(), device_id_of(public).unwrap());
    for msg in [
        register_message(info["id"].as_str().unwrap(), 1_700_000_000),
        session_message(info["id"].as_str().unwrap(), 1_700_000_001),
    ] {
        let sig = run(
            "device.sign",
            serde_json::json!({ "pkcs8": pkcs8, "message": msg }),
        )["signature"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            verify_device_signature(public, &msg, &sig),
            "the relay's verifier refused {msg:?}"
        );
        assert!(!verify_device_signature(public, &format!("{msg}x"), &sig));
    }
}

#[test]
fn a_cua_host_key_gives_the_same_id_and_public_key_through_the_phones_functions() {
    let (key, pkcs8) = cua_host::device::DeviceKey::generate().unwrap();
    let b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&pkcs8)
    };
    let (public, id) = device::info(&b64).unwrap();
    assert_eq!(public, key.public_key());
    assert_eq!(id, key.id());
    let sig = device::sign(&b64, "hello").unwrap();
    assert!(verify_device_signature(&public, "hello", &sig));
}

#[test]
fn the_message_formats_are_the_ones_in_the_shared_file() {
    let shared: Value =
        serde_json::from_str(include_str!("../fixtures/device-messages.json")).unwrap();
    assert_eq!(
        register_message("dev_abc", 7),
        shared["register"]
            .as_str()
            .unwrap()
            .replace("{device}", "dev_abc")
            .replace("{ts}", "7")
    );
    assert_eq!(
        session_message("dev_abc", 7),
        shared["session"]
            .as_str()
            .unwrap()
            .replace("{device}", "dev_abc")
            .replace("{ts}", "7")
    );
    assert_eq!(
        member_message("m1", 7),
        shared["member"]
            .as_str()
            .unwrap()
            .replace("{member}", "m1")
            .replace("{ts}", "7")
    );
}
