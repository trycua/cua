// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! External runtime providers in the New Space wizard. The registration is
//! process-wide, so these tests live in their own test binary and the ones
//! that register run as one sequence.

use cua_spaces_app_core::dispatch;
use cua_spaces_app_core::experiments::Experiments;
use cua_spaces_app_core::model::{Location, Runtime, SpaceKind, SpaceOs};
use cua_spaces_app_core::wizard::{self, LocalEngine, WizardAction, WizardEnv};
use serde_json::json;

/// A doctor report with one installed provider, one ignored executable,
/// and one entry whose name is a built-in engine's.
fn report(ready: bool) -> String {
    json!({
        "backends": [],
        "external": [
            {
                "name": "acme",
                "display_name": "macOS VMs",
                "kind": "vm",
                "path": "/usr/local/bin/cua-runtime-acme",
                "os": ["macos"],
                "arch": ["arm64"],
                "images": [
                    {"reference": "acme/other", "title": "Other", "default": false},
                    {"reference": "acme/default", "title": "Default", "default": true}
                ],
                "ready": ready,
                "detail": if ready { "up" } else { "not set up" },
                "missing": if ready { json!([]) } else { json!(["service"]) }
            },
            {"name": "broken", "path": "/usr/local/bin/cua-runtime-broken", "ignored": "info failed"},
            {"name": "qemu", "display_name": "x", "kind": "vm", "os": ["linux"], "ready": true}
        ]
    })
    .to_string()
}

fn env(backends: &[&str]) -> WizardEnv {
    WizardEnv {
        default_location: Location::Local,
        local_available: true,
        local_reason: None,
        local_backends: Some(backends.iter().map(|b| b.to_string()).collect()),
        local_details: None,
        max_cpus: 8,
        host_arch: Some("arm64".into()),
        lume_source: None,
        linux_source: None,
        storage: None,
        clouds: vec![],
        hosts: vec![],
        experiments: Experiments::default(),
        gpus: None,
        name_seed: None,
    }
}

#[test]
fn the_doctor_report_names_the_installed_providers() {
    let list = wizard::external_runtimes_from_doctor(&report(true));
    assert_eq!(list.len(), 1, "ignored and built-in names are skipped");
    let acme = &list[0];
    assert_eq!(acme.name, "acme");
    assert_eq!(acme.label, "macOS VMs");
    assert_eq!(acme.kind, SpaceKind::Vm);
    assert_eq!(acme.os, ["macos"]);
    assert_eq!(acme.arch, ["arm64"]);
    assert!(acme.ready);
    assert_eq!(acme.images.len(), 2);

    // No provider, or nothing usable: none.
    for text in [
        r#"{"backends": []}"#,
        r#"{"external": "nope"}"#,
        "[]",
        "not json",
        "",
    ] {
        assert!(
            wizard::external_runtimes_from_doctor(text).is_empty(),
            "{text}"
        );
    }
    // A micro VM is a virtual machine; a provider with no label shows its name.
    let micro = wizard::external_runtimes_from_doctor(
        r#"{"external":[{"name":"acme","kind":"microvm","ready":false}]}"#,
    );
    assert_eq!(micro[0].kind, SpaceKind::Vm);
    assert_eq!(micro[0].label, "acme");
    let unknown_kind = wizard::external_runtimes_from_doctor(
        r#"{"external":[{"name":"acme","kind":"toaster","ready":true}]}"#,
    );
    assert!(unknown_kind.is_empty());
}

#[test]
fn local_engine_words_round_trip() {
    for (word, engine) in [
        ("container", LocalEngine::Container),
        ("qemu", LocalEngine::Qemu),
        ("lume", LocalEngine::Lume),
        ("acme", LocalEngine::External("acme".into())),
    ] {
        let json = format!("\"{word}\"");
        assert_eq!(serde_json::from_str::<LocalEngine>(&json).unwrap(), engine);
        assert_eq!(serde_json::to_string(&engine).unwrap(), json);
    }
    assert!(serde_json::from_str::<LocalEngine>("\"Not A Word\"").is_err());
}

/// Registering, using and removing a provider, in order (the registration
/// is process-wide).
#[test]
fn a_registered_provider_joins_the_wizard_and_leaves_it_again() {
    let groups_before = wizard::picker_groups();
    let images_before = wizard::picker_images();
    let suggestions_before = wizard::image_suggestions("");
    let linux = images_before
        .iter()
        .find(|i| i.os == SpaceOs::Linux)
        .unwrap()
        .clone();
    let linux_runtimes = wizard::runtime_options(&linux, Location::Local);
    assert!(wizard::find_image("acme/default").is_none());

    // Register through the JSON entry point, as the wasm shell would.
    let listed = dispatch::call_value(
        "wizard.setExternalRuntimes",
        json!({ "reportJson": report(true) }),
    )
    .unwrap();
    assert_eq!(listed[0]["name"], "acme");
    assert_eq!(listed[0]["label"], "macOS VMs");
    assert_eq!(wizard::external_runtimes().len(), 1);

    // Its group comes after the image file's.
    let groups = wizard::picker_groups();
    assert_eq!(&groups[..groups_before.len()], &groups_before[..]);
    let group = groups.last().unwrap();
    assert_eq!(group.id, "runtime:acme");
    assert_eq!(group.label, "macOS VMs");
    assert_eq!(
        group
            .images
            .iter()
            .map(|i| i.image_ref.as_str())
            .collect::<Vec<_>>(),
        ["acme/default", "acme/other"],
        "the default image first"
    );
    let image = wizard::find_image("acme/default").unwrap();
    assert_eq!(image.os, SpaceOs::Macos);
    assert_eq!(image.variant, SpaceKind::Vm);
    assert_eq!(image.name, "Default");
    assert_eq!(image.local, Some(LocalEngine::External("acme".into())));
    assert_eq!(image.arch, ["arm64"]);
    assert!(
        wizard::image_suggestions("acme")
            .iter()
            .any(|g| g.id == "runtime:acme")
    );
    assert_eq!(wizard::validate_image_ref("acme/default"), None);

    // It runs only on this Mac, only on the provider.
    assert!(wizard::can_place(&image, Location::Local));
    for on in [Location::Yours, Location::Host] {
        assert!(!wizard::can_place(&image, on), "{on:?}");
    }
    assert_eq!(
        wizard::runtime_options(&image, Location::Local),
        [Runtime::Other("acme".into())]
    );
    assert_eq!(Runtime::Other("acme".into()).label(), "macOS VMs");
    assert_eq!(
        wizard::runtime_options(&linux, Location::Local),
        linux_runtimes
    );
    let kinds = wizard::kind_options(&image, Location::Local);
    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0].kind, SpaceKind::Vm);
    assert!(wizard::local_runtime_ready(&image, &["acme".into()]));
    assert!(!wizard::local_runtime_ready(
        &image,
        &["lume".into(), "docker".into()]
    ));

    // The wizard with it chosen.
    let ready = env(&["docker", "acme"]);
    let state = wizard::reduce(
        &wizard::initial(&ready),
        &WizardAction::ChooseImage {
            image_ref: "acme/default".into(),
        },
        &ready,
    );
    let v = wizard::view(&state, &ready);
    assert_eq!(v.image.image_ref, "acme/default");
    assert_eq!(v.runtime, Runtime::Other("acme".into()));
    assert_eq!(v.runtimes.len(), 1);
    assert_eq!(v.runtimes[0].value, "acme");
    assert_eq!(v.runtimes[0].label, "macOS VMs");
    assert!(!v.runtime_enabled);
    assert!(v.gpu.is_none());
    assert_eq!(v.placement_error, None);
    assert!(v.can_continue);
    assert!(
        v.summary
            .iter()
            .any(|f| f.label == "Runtime" && f.value == "macOS VMs on this Mac")
    );
    assert!(
        v.placements
            .iter()
            .any(|p| p.id == "local" && p.enabled && p.detail == "macOS VMs on this Mac")
    );
    let args = wizard::create_args(&v.plan);
    assert_eq!(args.image, "acme/default");
    assert_eq!(args.on, "local");
    assert_eq!(args.kind, SpaceKind::Vm);
    assert_eq!(args.runtime, Runtime::Other("acme".into()));
    let wire = serde_json::to_value(&args).unwrap();
    assert_eq!(wire["runtime"], "acme");
    assert_eq!(wire["kind"], "vm");

    // Not ready: Continue waits, and the provider says why.
    wizard::set_external_runtimes(wizard::external_runtimes_from_doctor(&report(false)));
    let not_ready = env(&["docker"]);
    let v = wizard::view(&state, &not_ready);
    assert!(!v.can_continue);
    assert_eq!(
        v.placement_error.as_deref(),
        Some("macOS VMs is not ready on this Mac: not set up.")
    );
    assert_eq!(v.placement_hint, None);

    // Removed again: everything is as before.
    wizard::set_external_runtimes(vec![]);
    assert!(wizard::external_runtimes().is_empty());
    assert_eq!(wizard::picker_groups(), groups_before);
    assert_eq!(wizard::picker_images(), images_before);
    assert_eq!(wizard::image_suggestions(""), suggestions_before);
    assert!(wizard::find_image("acme/default").is_none());
    assert_eq!(Runtime::Other("acme".into()).label(), "acme");
}
