//! The helpers that moved to `cua-image` are still reachable at their old
//! `cua_fleet` paths, as the same items, so callers keep compiling while they
//! switch to `cua-image`.

use std::collections::BTreeMap;

#[test]
fn build_helpers_are_cua_images() {
    // Same types: each side is assignable to the other without a conversion.
    let spec: cua_image::build::BuildSpec = cua_fleet::BuildSpec {
        from: "python:3.12-slim".into(),
        ..Default::default()
    };
    let _: cua_fleet::builds::BuildSpec = spec.clone();
    let file: cua_image::build::BuildFile = cua_fleet::BuildFile {
        source: "app.py".into(),
        destination: "/srv/app.py".into(),
    };
    let _: cua_fleet::builds::BuildFile = file;
    let _: cua_image::spec::ImageLayer = cua_fleet::ImageLayer::Run {
        command: "true".into(),
    };
    let _: cua_image::RegistryCredentials = cua_fleet::RegistryCredentials::new("me", "pw");

    // Same constants and functions.
    assert_eq!(
        cua_fleet::builds::DEFAULT_BUILD_TIMEOUT,
        cua_image::build::DEFAULT_BUILD_TIMEOUT
    );
    assert_eq!(
        cua_fleet::builds::BUILD_NAME_PREFIX,
        cua_image::build::BUILD_NAME_PREFIX
    );
    assert_eq!(
        cua_fleet::builds::MAX_BUILD_ENV,
        cua_image::build::MAX_BUILD_ENV
    );
    let recipe = cua_fleet::builds::build_recipe(&spec, "r/p@sha256:ab", vec![], None);
    assert_eq!(
        cua_fleet::builds::build_name(&recipe),
        cua_image::build::build_name(&cua_image::build::build_recipe(
            &spec,
            "r/p@sha256:ab",
            vec![],
            None
        ))
    );
    assert!(
        cua_fleet::builds::local_build_file(&cua_fleet::BuildFile {
            source: "/definitely/not/here".into(),
            destination: "/x".into(),
        })
        .is_err()
    );
}

#[test]
fn sidecar_helpers_are_cua_images() {
    let s: cua_image::sidecar::Sidecar = cua_fleet::Sidecar::new("redis:7-alpine");
    assert_eq!(s.name, cua_fleet::default_sidecar_name("redis:7-alpine"));
    assert_eq!(cua_fleet::MAX_SIDECARS, cua_image::sidecar::MAX_SIDECARS);
    assert_eq!(
        cua_fleet::MAIN_CONTAINER_NAME,
        cua_image::sidecar::MAIN_CONTAINER_NAME
    );
    assert_eq!(
        cua_fleet::RESERVED_SERVICE_NAMES,
        cua_image::sidecar::RESERVED_SERVICE_NAMES
    );
    cua_fleet::validate_sidecars(std::slice::from_ref(&s), &[]).unwrap();
    // A refusal is cua-image's error; Fleet callers get it as their own.
    let dup = [s.clone(), s];
    let e: cua_fleet::Error = cua_fleet::validate_sidecars(&dup, &[]).unwrap_err().into();
    assert!(matches!(e, cua_fleet::Error::InvalidArgument(m) if m.contains("used twice")));
    let services = BTreeMap::from([("sc".to_string(), 80u16)]);
    assert!(cua_fleet::check_reserved_service_names(services.keys(), true).is_err());
}

#[test]
fn canonical_images_follow_cua_image() {
    assert_eq!(
        cua_fleet::canonical_image("linux"),
        cua_image::canonical::canonical_image_or_linux("linux")
    );
    assert_eq!(
        cua_fleet::canonical_image("no-such-os"),
        cua_fleet::canonical_image("linux"),
        "an unknown OS gets Linux"
    );
    assert!(cua_fleet::is_canonical_image("ghcr.io/trycua/linux:24.04"));
    assert!(!cua_fleet::is_canonical_image("python:3.12-slim"));
    assert_eq!(
        cua_fleet::registry_of("ghcr.io/trycua/linux:24.04"),
        cua_image::registry_of("ghcr.io/trycua/linux:24.04")
    );
}

#[test]
fn sidecar_names_use_the_fleet_dns_label_rule() {
    // cua-image cannot depend on the Fleet SDK, so it has its own DNS-label
    // check; it must agree with the one Fleet admits.
    let long = "a".repeat(64);
    let max = "a".repeat(63);
    for name in [
        "redis", "a", "a-b", "-a", "a-", "A", "a_b", "", "9lives", "a.b", &max, &long,
    ] {
        assert_eq!(
            cua_image::spec::is_dns_label(name),
            cua_fleet::sdk::validate_dns_label(name).is_ok(),
            "{name:?}"
        );
    }
}
