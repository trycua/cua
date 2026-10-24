//! The Stable and Local channels must not share a name (CUA-1776).
//!
//! A locally built "Cua Spaces Local" runs beside the released app on one
//! Mac. If any path, launchd label, bundle id, port, URL scheme or Keychain
//! service were the same, the two would overwrite each other's files, take
//! each other's sockets, answer each other's `cua://` links or read each
//! other's sign-in. This collects every name `CuaChannel` owns, for both
//! channels, and asserts that the two sets do not intersect.
//!
//! Modelled on Cua Driver's `testkit_host_state_isolation_test.rs`, which
//! guards the same idea for `cua-driver` and `cua-driver-local`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use cua_home::channel::CuaChannel;
use cua_home::env::CuaEnv;

const ENVS: [CuaEnv; 2] = [CuaEnv::Prod, CuaEnv::Staging];

/// Every file-system location the channel owns.
fn paths(c: CuaChannel) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("app", c.app_path()),
        ("app support", c.app_support_dir()),
        ("cli", c.cli_path()),
        ("cua home", c.home_dir()),
        ("helper spec", c.helper_spec_path()),
        ("volume mount", c.volume_mount_path()),
        ("lume vms", c.lume_dir()),
    ]
}

/// Every launchd label.
fn labels(c: CuaChannel) -> Vec<(&'static str, String)> {
    vec![
        ("app helper", c.helper_label().to_string()),
        ("host runner", c.host_label().to_string()),
        ("lume daemon", c.lume_daemon_label().to_string()),
    ]
}

/// Every other name another program could look a thing up by: bundle id
/// (TCC grants), URL scheme, Keychain services, binary and volume names.
fn identifiers(c: CuaChannel) -> Vec<(String, String)> {
    let mut ids = vec![
        ("bundle id".to_string(), c.bundle_id().to_string()),
        ("app name".to_string(), c.app_name().to_string()),
        (
            "app support dir".to_string(),
            c.app_support_dir_name().to_string(),
        ),
        ("url scheme".to_string(), c.url_scheme().to_string()),
        ("cli name".to_string(), c.cli_name().to_string()),
        ("volume name".to_string(), c.volume_name().to_string()),
        ("lume binary".to_string(), c.lume_bin_name().to_string()),
        ("lume api".to_string(), c.lume_api().to_string()),
        ("telemetry".to_string(), c.telemetry_channel().to_string()),
    ];
    for env in ENVS {
        ids.push((
            format!("keychain service ({})", env.name()),
            c.keychain_service(env).to_string(),
        ));
    }
    ids
}

/// Every port a channel listens on, as `proto:port`. (QUIC and the mesh
/// media port are both UDP.)
fn ports(c: CuaChannel) -> BTreeSet<String> {
    let p = c.ports();
    [
        format!("tcp:{}", p.tcp),
        format!("udp:{}", p.quic),
        format!("udp:{}", p.mesh_udp),
        format!("tcp:{}", c.lume_port()),
    ]
    .into_iter()
    .collect()
}

fn values<K, V: Ord + Clone>(items: &[(K, V)]) -> BTreeSet<V> {
    items.iter().map(|(_, v)| v.clone()).collect()
}

#[test]
fn stable_and_local_paths_never_overlap() {
    let stable = paths(CuaChannel::Stable);
    let local = paths(CuaChannel::Local);
    for (sk, s) in &stable {
        for (lk, l) in &local {
            // Not equal, and neither inside the other: a Local home under
            // `~/.cua` would be wiped or scanned by Stable.
            assert!(
                !s.starts_with(l) && !l.starts_with(s),
                "Stable {sk} {s:?} and Local {lk} {l:?} overlap"
            );
        }
    }
}

#[test]
fn local_paths_stay_out_of_the_system_applications_folder() {
    let app = CuaChannel::Local.app_path();
    assert!(
        !app.starts_with("/Applications"),
        "Local installs to ~/Applications, never over /Applications: {app:?}"
    );
}

#[test]
fn stable_and_local_launchd_labels_never_overlap() {
    let stable = values(&labels(CuaChannel::Stable));
    let local = values(&labels(CuaChannel::Local));
    let both: Vec<_> = stable.intersection(&local).collect();
    assert!(both.is_empty(), "shared launchd labels: {both:?}");
    // Each channel's own labels are distinct from one another.
    assert_eq!(stable.len(), 3);
    assert_eq!(local.len(), 3);
}

#[test]
fn stable_and_local_ports_never_overlap() {
    let stable = ports(CuaChannel::Stable);
    let local = ports(CuaChannel::Local);
    let both: Vec<_> = stable.intersection(&local).collect();
    assert!(both.is_empty(), "shared ports: {both:?}");
}

#[test]
fn stable_and_local_schemes_and_keychain_services_never_overlap() {
    let stable = identifiers(CuaChannel::Stable);
    let local = identifiers(CuaChannel::Local);
    // Kind by kind, so a failure names what is shared.
    for (sk, s) in &stable {
        for (lk, l) in &local {
            assert!(
                s != l,
                "Stable {sk} and Local {lk} are both {s:?}: the two builds would share it"
            );
        }
    }
    // The URL scheme and every Keychain service, spelled out.
    assert_ne!(
        CuaChannel::Stable.url_scheme(),
        CuaChannel::Local.url_scheme()
    );
    let keychain = |c: CuaChannel| -> BTreeSet<&'static str> {
        ENVS.iter().map(|e| c.keychain_service(*e)).collect()
    };
    let (ks, kl) = (keychain(CuaChannel::Stable), keychain(CuaChannel::Local));
    assert_eq!(ks.len(), ENVS.len(), "one Keychain service per environment");
    assert_eq!(kl.len(), ENVS.len(), "one Keychain service per environment");
    assert!(
        ks.is_disjoint(&kl),
        "shared Keychain services: {ks:?} {kl:?}"
    );
}

#[test]
fn no_name_is_shared_across_kinds_either() {
    // A Local label equal to a Stable path, or a Stable scheme equal to a
    // Local volume name, would be a bug of the same kind. Compare every
    // string either channel owns against every string of the other.
    let all = |c: CuaChannel| -> BTreeSet<String> {
        let mut set: BTreeSet<String> = paths(c)
            .into_iter()
            .map(|(_, p)| p.to_string_lossy().into_owned())
            .collect();
        set.extend(values(&labels(c)));
        set.extend(values(&identifiers(c)));
        set.extend(ports(c));
        set
    };
    let (s, l) = (all(CuaChannel::Stable), all(CuaChannel::Local));
    let both: Vec<_> = s.intersection(&l).collect();
    assert!(both.is_empty(), "names both channels use: {both:?}");
}

#[test]
fn local_builds_never_use_production_by_default() {
    assert_eq!(CuaChannel::Local.default_env(), CuaEnv::Staging);
    assert_eq!(CuaChannel::Local.env_from_value(None), Ok(CuaEnv::Staging));
}

#[test]
fn every_local_name_says_local() {
    // A cheap guard that a new Local name was derived, not copied from
    // Stable: every non-numeric Local identifier and label contains
    // "local" (any case). Ports are checked by the overlap test above.
    let l = CuaChannel::Local;
    for (kind, name) in labels(l)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .chain(
            identifiers(l)
                .into_iter()
                // The Lume API is a URL of 127.0.0.1 and a port.
                .filter(|(k, _)| k != "lume api"),
        )
    {
        assert!(
            name.to_ascii_lowercase().contains("local"),
            "Local {kind} {name:?} does not say local"
        );
    }
    for (kind, path) in paths(l) {
        assert!(
            path.to_string_lossy()
                .to_ascii_lowercase()
                .contains("local"),
            "Local {kind} {path:?} does not say local"
        );
    }
}
