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

/// The four (channel, environment) pairs a Mac can run: Stable and Local,
/// each on prod and on staging.
fn pairs() -> Vec<(CuaChannel, CuaEnv)> {
    CuaChannel::ALL
        .into_iter()
        .flat_map(|c| ENVS.map(|e| (c, e)))
        .collect()
}

/// What a cua home holds that two processes must never share: the daemon's
/// socket and files, the credentials file, the Keyvault, the host's tokens
/// and spacesd's data.
const HOME_ENTRIES: [&str; 7] = [
    "cua.sock",
    "daemon.json",
    "credentials.json",
    "keyvault.sock",
    "keyvault",
    "host",
    "spacesd",
];

/// Every file-system location the channel owns.
fn paths(c: CuaChannel) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("app", c.app_path()),
        ("app support", c.app_support_dir()),
        ("cli", c.cli_path()),
        ("cua home", c.home_dir()),
        ("cua home (prod)", c.env_home_dir(CuaEnv::Prod)),
        ("cua home (staging)", c.env_home_dir(CuaEnv::Staging)),
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
        // What the Spaces app writes into a coding agent's config (Claude
        // Code, Codex, ...): the MCP server entry and the skill folders.
        (
            "agent mcp server".to_string(),
            c.mcp_server_name().to_string(),
        ),
    ];
    for skill in [
        "cua-spaces",
        "cua-volume",
        "cua-sandboxes",
        "cua-driver",
        "gui-automation",
    ] {
        ids.push((format!("agent skill {skill}"), c.skill_install_name(skill)));
    }
    for env in ENVS {
        ids.push((
            format!("keychain service ({})", env.name()),
            c.keychain_service(env).to_string(),
        ));
        // The service an older build used, which this channel moves from.
        if let Some(legacy) = c.legacy_keychain_service(env) {
            ids.push((
                format!("legacy keychain service ({})", env.name()),
                legacy.to_string(),
            ));
        }
    }
    ids
}

/// Every Keychain service a process of the channel reads, writes, moves from
/// or deletes, for either environment.
fn keychain_services_touched(c: CuaChannel) -> BTreeSet<&'static str> {
    ENVS.iter()
        .flat_map(|e| [Some(c.keychain_service(*e)), c.legacy_keychain_service(*e)])
        .flatten()
        .collect()
}

/// Every port a channel listens on, as `proto:port`. The mesh media port
/// is left out: it is a port of the mesh's own in-process network, not a
/// socket of the operating system, so two channels cannot collide on it,
/// and it is 3212 for both by design (guest Spaces and older cua-spacesd
/// builds fix it there; see `Ports::mesh_udp`).
fn ports(c: CuaChannel) -> BTreeSet<String> {
    let p = c.ports();
    [
        format!("tcp:{}", p.tcp),
        format!("udp:{}", p.quic),
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

/// The one port the channels do share, on purpose: the mesh media UDP port
/// lives on the mesh's own network (not the OS), and the peers dial 3212.
#[test]
fn the_mesh_media_port_is_shared_by_design() {
    let (s, l) = (CuaChannel::Stable.ports(), CuaChannel::Local.ports());
    assert_eq!((s.mesh_udp, l.mesh_udp), (3212, 3212));
    assert!(s.tcp != l.tcp && s.quic != l.quic, "the OS ports differ");
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
fn every_channel_and_environment_has_a_keychain_service_of_its_own() {
    // The session and the device key live under it: a Local build on
    // staging must not read, replace or sign out a Stable app on staging.
    let services: BTreeSet<&str> = pairs()
        .into_iter()
        .map(|(c, e)| c.keychain_service(e))
        .collect();
    assert_eq!(
        services.len(),
        4,
        "a Keychain service is shared: {services:?}"
    );

    // Nothing Stable touches (its own services and the legacy ones it moves
    // from and deletes) is a Local service, in either environment.
    let stable = keychain_services_touched(CuaChannel::Stable);
    let local = keychain_services_touched(CuaChannel::Local);
    assert!(
        stable.is_disjoint(&local),
        "Keychain services both channels touch: {stable:?} {local:?}"
    );
    // Local has no legacy items, so it touches exactly its own two.
    assert_eq!(local.len(), ENVS.len());
    assert_eq!(
        local,
        BTreeSet::from(["ai.cua.session.local", "ai.cua.session.local.staging"])
    );
    // Stable's are what its sign-in has always used, plus the legacy pair.
    assert_eq!(
        stable,
        BTreeSet::from([
            "ai.cua.session",
            "ai.cua.session.staging",
            "run.cua.ai",
            "staging.run.cua.ai",
        ])
    );
}

#[test]
fn every_channel_and_environment_has_a_home_of_its_own() {
    // The home holds the daemon, its sockets, the registry, the Keyvault,
    // the credentials file and spacesd's data: two builds on one home would
    // take each other's daemon and read each other's sign-in.
    let mut seen: Vec<(String, PathBuf)> = Vec::new();
    for (c, e) in pairs() {
        let home = c.env_home_dir(e);
        let what = format!("{} on {}", c.name(), e.name());
        seen.push((format!("{what}: the home"), home.clone()));
        for entry in HOME_ENTRIES {
            seen.push((format!("{what}: {entry}"), home.join(entry)));
        }
    }
    let unique: BTreeSet<&PathBuf> = seen.iter().map(|(_, p)| p).collect();
    assert_eq!(
        unique.len(),
        seen.len(),
        "two pairs share a path: {seen:#?}"
    );

    // Across channels, no home or file in it is inside the other's: a Local
    // home under `~/.cua` would be wiped or scanned by Stable, and the other
    // way round. (Within a channel the second environment sits below the
    // first, as Stable's staging always has.)
    for (sc, se) in pairs()
        .into_iter()
        .filter(|(c, _)| *c == CuaChannel::Stable)
    {
        for (lc, le) in pairs().into_iter().filter(|(c, _)| *c == CuaChannel::Local) {
            let (s, l) = (sc.env_home_dir(se), lc.env_home_dir(le));
            assert!(
                !s.starts_with(&l) && !l.starts_with(&s),
                "stable on {} {s:?} and local on {} {l:?} overlap",
                se.name(),
                le.name()
            );
        }
    }
}

#[test]
fn local_prod_never_uses_the_stable_or_the_local_staging_home() {
    let local_prod = CuaChannel::Local.env_home_dir(CuaEnv::Prod);
    for (c, e) in pairs() {
        if (c, e) == (CuaChannel::Local, CuaEnv::Prod) {
            continue;
        }
        assert_ne!(
            c.env_home_dir(e),
            local_prod,
            "{} on {}",
            c.name(),
            e.name()
        );
    }
    // It is the opt-in's own folder below Local's, mirroring how Stable
    // keeps its staging below `~/.cua`.
    assert_eq!(
        local_prod,
        CuaChannel::Local.home_dir().join("envs").join("prod")
    );
    assert_eq!(
        CuaChannel::Stable.env_home_dir(CuaEnv::Staging),
        CuaChannel::Stable.home_dir().join("envs").join("staging")
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
