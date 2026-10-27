use super::*;
use crate::egress::BACKENDS;
use proptest::prelude::*;
use std::net::{Ipv4Addr, Ipv6Addr};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn net(s: &str) -> Net {
    Net::parse(s).unwrap()
}

/// A runtime's facts: QEMU-like aliases, a LAN host address, one sibling in
/// the tree and one Space of another tree.
fn ctx() -> NetworkContext {
    NetworkContext {
        resolvers: vec![ip("10.0.2.3")],
        host_aliases: vec![ip("10.0.2.2"), ip("192.168.64.1")],
        host_addrs: vec![ip("192.168.1.20"), ip("203.0.113.7")],
        tree: vec![net("172.30.0.3/32")],
        other_trees: vec![net("172.30.1.0/24")],
        daemon_ports: vec![3200, 8443],
    }
}

fn secure(s: &NetworkSettings) -> Rules {
    compile_mode(s, &ctx(), Mode::Secure).unwrap()
}

fn localhost(ports: &[u16]) -> NetworkSettings {
    NetworkSettings {
        preset: NetworkPreset::Localhost,
        localhost_ports: ports.to_vec(),
        ..Default::default()
    }
}

fn custom(base: NetworkPreset, allow: &[&str], deny: &[&str]) -> NetworkSettings {
    NetworkSettings {
        preset: NetworkPreset::Custom,
        custom_base: base,
        custom: EgressPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        },
        ..Default::default()
    }
}

#[test]
fn internet_reaches_the_internet_and_its_tree_only() {
    let r = secure(&NetworkSettings::internet());
    for ok in ["1.1.1.1", "8.8.8.8", "2606:4700::1111", "172.30.0.3"] {
        assert!(r.allows(ip(ok)), "{ok}");
    }
    for blocked in [
        "169.254.169.254",
        "169.254.170.2",
        "fd00:ec2::254",
        "168.63.129.16",
        "100.100.100.200",
        "10.0.2.2",
        "192.168.64.1",
        "192.168.1.20",
        "203.0.113.7",
        "127.0.0.1",
        "10.1.2.3",
        "172.16.5.5",
        "192.168.0.1",
        "100.64.0.1",
        "100.101.102.103",
        "169.254.1.1",
        "224.0.0.251",
        "255.255.255.255",
        "0.0.0.0",
        "::1",
        "fe80::1",
        "fd12::1",
        "ff02::1",
        "64:ff9b::a00:1",
        "172.30.1.9",
        // IPv4-mapped forms are judged as IPv4.
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::ffff:169.254.169.254",
    ] {
        assert!(!r.allows(ip(blocked)), "{blocked}");
    }
    assert!(r.allows(ip("::ffff:1.1.1.1")));
    // The resolver on 53 only.
    assert!(r.allows_port(ip("10.0.2.3"), Proto::Udp, 53));
    assert!(r.allows_port(ip("10.0.2.3"), Proto::Tcp, 53));
    assert!(!r.allows_port(ip("10.0.2.3"), Proto::Tcp, 80));
    assert!(!r.allows(ip("10.0.2.3")));
    // The reason ids.
    let why = |s: &str| r.decide(ip(s), None, None).map(|r| r.reason);
    assert_eq!(why("169.254.169.254"), Some(Reason::PlatformMetadata));
    assert_eq!(why("10.0.2.2"), Some(Reason::DefaultHostLoopback));
    assert_eq!(why("203.0.113.7"), Some(Reason::DefaultHost));
    assert_eq!(why("192.168.0.1"), Some(Reason::DefaultLan));
    assert_eq!(why("127.0.0.1"), Some(Reason::DefaultReserved));
    assert_eq!(why("172.30.1.9"), Some(Reason::PlatformOtherTree));
    assert_eq!(why("172.30.0.3"), Some(Reason::Tree));
    assert_eq!(why("1.1.1.1"), None);
    assert_eq!(
        r.decide(ip("10.0.2.2"), Some(Proto::Tcp), Some(3200))
            .map(|r| r.reason),
        Some(Reason::PlatformDaemon)
    );
}

#[test]
fn platform_denies_beat_every_preset_and_user_rule() {
    let wide = custom(
        NetworkPreset::Internet,
        &["169.254.169.254", "172.30.1.0/24", "all", "10.0.2.2"],
        &[],
    );
    for s in [
        wide,
        NetworkSettings::preset(NetworkPreset::Lan),
        NetworkSettings::preset(NetworkPreset::Unprotected),
        NetworkSettings {
            preset: NetworkPreset::Localhost,
            all_localhost_ports: true,
            ..Default::default()
        },
    ] {
        let r = secure(&s);
        for blocked in [
            "169.254.169.254",
            "fd00:ec2::254",
            "168.63.129.16",
            "172.30.1.4",
        ] {
            assert!(!r.allows(ip(blocked)), "{s:?} {blocked}");
        }
        for host in ["10.0.2.2", "192.168.64.1", "192.168.1.20"] {
            assert!(
                !r.allows_port(ip(host), Proto::Tcp, 3200),
                "the daemon: {s:?} {host}"
            );
        }
    }
}

#[test]
fn localhost_opens_the_host_aliases_on_its_ports_only() {
    let r = secure(&localhost(&[5432, 3000]));
    assert!(r.allows_port(ip("10.0.2.2"), Proto::Tcp, 5432));
    assert!(r.allows_port(ip("192.168.64.1"), Proto::Tcp, 3000));
    assert!(!r.allows_port(ip("10.0.2.2"), Proto::Tcp, 22));
    assert!(!r.allows(ip("10.0.2.2")));
    // Not the host's other addresses, not the LAN.
    assert!(!r.allows_port(ip("192.168.1.20"), Proto::Tcp, 5432));
    assert!(!r.allows(ip("192.168.0.9")));
    // All ports, said explicitly: still not the daemon's.
    let all = secure(&NetworkSettings {
        preset: NetworkPreset::Localhost,
        all_localhost_ports: true,
        ..Default::default()
    });
    assert!(all.allows_port(ip("10.0.2.2"), Proto::Tcp, 22));
    assert!(!all.allows_port(ip("10.0.2.2"), Proto::Tcp, 8443));
    // Empty ports are refused unless all.
    let e = compile_mode(&localhost(&[]), &ctx(), Mode::Secure).unwrap_err();
    assert!(e.0.contains("needs the ports"), "{e}");
    let e = compile_mode(&localhost(&[0]), &ctx(), Mode::Secure).unwrap_err();
    assert!(e.0.contains("port 0"), "{e}");
}

#[test]
fn lan_opens_private_networks_but_not_the_host() {
    let r = secure(&NetworkSettings::preset(NetworkPreset::Lan));
    for ok in [
        "192.168.1.30",
        "10.9.9.9",
        "172.16.0.1",
        "169.254.10.10",
        "fd00::5",
        "fe80::9",
    ] {
        assert!(r.allows(ip(ok)), "{ok}");
    }
    for blocked in [
        "10.0.2.2",
        "192.168.64.1",
        "192.168.1.20",
        "127.0.0.1",
        "100.64.1.1",
        "169.254.169.254",
    ] {
        assert!(!r.allows(ip(blocked)), "{blocked}");
    }
}

#[test]
fn none_reaches_nothing_and_custom_from_none_only_its_list() {
    let r = secure(&NetworkSettings::none());
    for blocked in ["1.1.1.1", "2606:4700::1111", "172.30.0.3"] {
        assert!(!r.allows(ip(blocked)), "{blocked}");
    }
    assert!(
        !r.allows_port(ip("10.0.2.3"), Proto::Udp, 53),
        "no DNS either"
    );
    let r = secure(&custom(NetworkPreset::None, &["1.1.1.1"], &[]));
    assert!(r.allows(ip("1.1.1.1")));
    assert!(!r.allows(ip("8.8.8.8")));
    assert!(r.allows_port(ip("10.0.2.3"), Proto::Udp, 53), "DNS works");
    assert_eq!(
        r.decide(ip("8.8.8.8"), None, None).map(|r| r.reason),
        Some(Reason::DefaultNone)
    );
    // `none` takes no lists.
    let mut s = NetworkSettings::none();
    s.custom.allow.push("1.1.1.1".into());
    assert!(s.validate().is_err());
}

#[test]
fn a_custom_allow_lifts_lan_but_never_the_host() {
    let r = secure(&custom(
        NetworkPreset::Internet,
        &["192.168.1.0/24", "10.0.2.0/24"],
        &["8.8.8.8"],
    ));
    assert!(r.allows(ip("192.168.1.30")));
    assert!(r.allows(ip("10.0.2.50")));
    assert!(!r.allows(ip("8.8.8.8")));
    assert!(r.allows(ip("8.8.4.4")));
    // The host stays closed: its alias and its own address on that LAN.
    assert!(!r.allows(ip("10.0.2.2")));
    assert!(!r.allows(ip("192.168.1.20")));
    // An allow is an exception to a deny, as before.
    let r = secure(&custom(NetworkPreset::Internet, &["1.1.1.1"], &["all"]));
    assert!(r.allows(ip("1.1.1.1")) && !r.allows(ip("1.0.0.1")));
}

#[test]
fn unprotected_keeps_only_the_platform_denies() {
    let r = secure(&NetworkSettings::preset(NetworkPreset::Unprotected));
    for ok in [
        "10.0.2.2",
        "192.168.1.20",
        "127.0.0.1",
        "192.168.0.1",
        "1.1.1.1",
    ] {
        assert!(r.allows(ip(ok)), "{ok}");
    }
    assert!(!r.allows(ip("169.254.169.254")));
    assert!(!r.allows(ip("172.30.1.1")));
    // The host stays open, but never the daemon on it.
    let local: Vec<&Rule> = r.rules.iter().filter(|r| r.host_local).collect();
    assert_eq!(local.len(), 1, "{local:?}");
    assert_eq!(local[0].reason, Reason::PlatformDaemon);
}

#[test]
fn every_host_address_is_closed_after_the_resolver_and_localhost() {
    let r = secure(&localhost(&[5432]));
    let pos = |f: &dyn Fn(&Rule) -> bool| r.rules.iter().position(f).unwrap();
    let local_drop = pos(&|r| r.host_local && r.reason == Reason::DefaultHost);
    assert!(pos(&|r| r.reason == Reason::Resolver) < local_drop);
    assert!(pos(&|r| r.reason == Reason::PresetLocalhost) < local_drop);
    assert!(local_drop < pos(&|r| r.reason == Reason::Tree));
    assert!(local_drop < pos(&|r| r.reason == Reason::DefaultReserved));
    let daemon = pos(&|r| r.host_local && r.reason == Reason::PlatformDaemon);
    assert!(daemon < pos(&|r| r.reason == Reason::PresetLocalhost));
}

#[test]
fn the_revert_switch_restores_the_old_open_default() {
    assert_eq!(Mode::from_value(None), Mode::Secure);
    assert_eq!(Mode::from_value(Some("1")), Mode::Secure);
    for off in ["0", "false", "OFF", " no "] {
        assert_eq!(Mode::from_value(Some(off)), Mode::Legacy, "{off}");
    }
    let r = compile_mode(&NetworkSettings::internet(), &ctx(), Mode::Legacy).unwrap();
    assert!(r.is_empty());
    let r = compile_mode(&NetworkSettings::none(), &ctx(), Mode::Legacy).unwrap();
    assert!(!r.allows(ip("1.1.1.1")));
    let p = plan(
        &NetworkSettings::internet(),
        network_support(BackendKind::Vz, "macos", Placement::Local),
        &ctx(),
        Mode::Legacy,
    )
    .unwrap();
    assert_eq!(p, NetworkPlan::Open { note: None });
}

#[test]
fn migration_maps_old_spaces() {
    let empty = EgressPolicy::default();
    assert_eq!(
        NetworkSettings::migrate(&empty, false),
        NetworkSettings::internet()
    );
    assert_eq!(
        NetworkSettings::migrate(&empty, true),
        NetworkSettings::none()
    );
    let old = EgressPolicy {
        allow: vec!["192.168.1.0/24".into(), "1.1.1.1".into()],
        deny: vec!["all".into()],
    };
    let m = NetworkSettings::migrate(&old, false);
    assert_eq!(m.preset, NetworkPreset::Custom);
    assert_eq!(m.custom_base, NetworkPreset::Internet);
    assert_eq!(m.custom, old);
    // The allow that lifted a default deny is labelled.
    assert_eq!(
        m.widenings(),
        ["Allows your local network (192.168.1.0/24)"]
    );
    let r = secure(&m);
    assert!(r.allows(ip("192.168.1.9")) && r.allows(ip("1.1.1.1")));
    assert!(!r.allows(ip("8.8.8.8")));
    // A stored meta file from before: no `network` key means migrate.
    let json = r#"{"preset":"custom","custom":{"allow":["1.1.1.1"],"deny":["all"]}}"#;
    let back: NetworkSettings = serde_json::from_str(json).unwrap();
    assert_eq!(back.custom_base, NetworkPreset::Internet);
    assert_eq!(back.preset, NetworkPreset::Custom);
}

#[test]
fn settings_json_is_small_and_stable() {
    assert_eq!(
        serde_json::to_string(&NetworkSettings::internet()).unwrap(),
        r#"{"preset":"internet","custom_base":"internet"}"#
    );
    assert_eq!(
        serde_json::to_string(&localhost(&[5432])).unwrap(),
        r#"{"preset":"localhost","localhost_ports":[5432],"custom_base":"internet"}"#
    );
    let back: NetworkSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(back, NetworkSettings::internet());
    for p in NetworkPreset::ALL {
        assert_eq!(NetworkPreset::parse(p.as_str()).unwrap(), p);
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            format!("\"{}\"", p.as_str())
        );
        assert!(!p.label().is_empty());
    }
    assert_eq!(
        NetworkPreset::parse("Default").unwrap(),
        NetworkPreset::Internet
    );
    assert!(NetworkPreset::parse("open").is_err());
}

#[test]
fn rules_json_is_the_stable_contract() {
    let s = localhost(&[5432]);
    let r = compile_mode(
        &s,
        &NetworkContext {
            resolvers: vec![ip("10.0.2.3")],
            host_aliases: vec![ip("10.0.2.2")],
            ..Default::default()
        },
        Mode::Secure,
    )
    .unwrap();
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(
        json["rules"][0],
        serde_json::json!({"net": "169.254.169.254/32", "accept": false, "reason": "platform:metadata"})
    );
    assert_eq!(
        json["rules"][5],
        serde_json::json!({"net": "10.0.2.3/32", "ports": [53], "accept": true, "reason": "allow:resolver"})
    );
    assert_eq!(
        json["rules"][6],
        serde_json::json!({"net": "10.0.2.2/32", "ports": [5432], "accept": true, "reason": "preset:localhost"})
    );
    let back: Rules = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, r);
    // Strict: unknown fields, other versions and host bits are refused.
    let mut bad = json.clone();
    bad["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Rules>(bad).is_err());
    let mut bad = json.clone();
    bad["version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<Rules>(bad).is_err());
    let mut bad = json.clone();
    bad["rules"][0]["why"] = serde_json::json!("x");
    assert!(serde_json::from_value::<Rules>(bad).is_err());
    for bad_net in ["10.0.0.1/8", "10.0.0.1", "all", "example.com/32"] {
        let mut bad = json.clone();
        bad["rules"][0]["net"] = serde_json::json!(bad_net);
        assert!(serde_json::from_value::<Rules>(bad).is_err(), "{bad_net}");
    }
    let mut tcp = json;
    tcp["rules"][0]["proto"] = serde_json::json!("tcp");
    assert_eq!(
        serde_json::from_value::<Rules>(tcp).unwrap().rules[0].proto,
        Some(Proto::Tcp)
    );
}

#[test]
fn reason_ids_are_unique_and_round_trip() {
    let mut ids: Vec<&str> = Reason::ALL.iter().map(|r| r.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), Reason::ALL.len());
    for r in Reason::ALL {
        assert_eq!(Reason::from_code(r.code()), Some(r));
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            format!("\"{}\"", r.as_str())
        );
    }
    assert_eq!(Reason::from_code(0), None);
    // Exhaustive: a new reason fails to build until it is in ALL.
    let listed = |r: Reason| match r {
        Reason::PlatformMetadata
        | Reason::PlatformDaemon
        | Reason::PlatformOtherTree
        | Reason::Resolver
        | Reason::Tree
        | Reason::UserAllow
        | Reason::UserDeny
        | Reason::PresetLocalhost
        | Reason::DefaultHostLoopback
        | Reason::DefaultHost
        | Reason::DefaultLan
        | Reason::DefaultReserved
        | Reason::DefaultNone => Reason::ALL.contains(&r),
    };
    assert!(Reason::ALL.into_iter().all(listed));
}

/// The support table, cell by cell: every runtime × host OS × placement.
/// A new runtime or placement does not build until [`network_support`]
/// answers for it, and fails here until this table says what it should be.
#[test]
fn golden_support_table() {
    use NetworkSupport as S;
    let short = |s: S| match s {
        S::Enforced { .. } => "E",
        S::NoNetworkOnly { .. } => "N",
        S::Refuse { .. } => "R",
    };
    // (kind, linux, macos, windows) on this machine or a pool host.
    let local = [
        (BackendKind::Lume, "R", "N", "R"),
        (BackendKind::Qemu, "E", "N", "N"),
        (BackendKind::QemuDocker, "R", "R", "R"),
        (BackendKind::Container, "E", "E", "E"),
        (BackendKind::Managed, "E", "E", "E"),
        (BackendKind::Firecracker, "E", "R", "R"),
        (BackendKind::Containerization, "R", "R", "R"),
        (BackendKind::Vz, "R", "N", "R"),
    ];
    assert_eq!(local.len(), BACKENDS.len());
    for (kind, l, m, w) in local {
        for placement in Placement::ALL {
            let want = match placement {
                Placement::Local | Placement::PoolHost => [l, m, w],
                Placement::Provider | Placement::Overflow => ["R", "R", "R"],
            };
            for (os, want) in ["linux", "macos", "windows"].into_iter().zip(want) {
                let got = network_support(kind, os, placement);
                assert_eq!(short(got), want, "{kind} on {os} at {}", placement.as_str());
                match got {
                    S::Enforced { point } => assert!(point.len() > 20),
                    S::NoNetworkOnly { why } | S::Refuse { why } => {
                        assert!(why.len() > 20, "{kind} {os}: say why");
                        assert!(!why.contains("TODO"), "{kind} {os}: finished wording");
                    }
                }
            }
        }
    }
    let md = support_table_markdown();
    assert!(
        md.contains(
            "| container | this machine or one of yours | Enforced | Enforced | Enforced |"
        ),
        "{md}"
    );
    assert!(
        md.contains("| vz | this machine or one of yours | Refused | No network only | Refused |"),
        "{md}"
    );
}

#[test]
fn plan_fails_closed() {
    let c = ctx();
    let local = |k, os| network_support(k, os, Placement::Local);
    let internet = NetworkSettings::internet();
    // Enforced: the compiled rules.
    assert_eq!(
        plan(
            &internet,
            local(BackendKind::Container, "linux"),
            &c,
            Mode::Secure
        )
        .unwrap(),
        NetworkPlan::Enforce(secure(&internet))
    );
    // Can't enforce: no network, with the note.
    assert_eq!(
        plan(&internet, local(BackendKind::Vz, "macos"), &c, Mode::Secure).unwrap(),
        NetworkPlan::NoNetwork {
            note: Some(NO_NETWORK_NOTE)
        }
    );
    // Asked for none: no note.
    assert_eq!(
        plan(
            &NetworkSettings::none(),
            local(BackendKind::Lume, "macos"),
            &c,
            Mode::Secure
        )
        .unwrap(),
        NetworkPlan::NoNetwork { note: None }
    );
    // Refused, in plain words.
    match plan(
        &internet,
        local(BackendKind::Containerization, "macos"),
        &c,
        Mode::Secure,
    )
    .unwrap()
    {
        NetworkPlan::Refuse(why) => assert!(why.contains("No protection"), "{why}"),
        other => panic!("{other:?}"),
    }
    match plan(
        &internet,
        network_support(BackendKind::Container, "linux", Placement::Overflow),
        &c,
        Mode::Secure,
    )
    .unwrap()
    {
        NetworkPlan::Refuse(why) => assert!(why.contains("cloud account"), "{why}"),
        other => panic!("{other:?}"),
    }
    // Unprotected opens where the runtime can't enforce, and says so.
    assert_eq!(
        plan(
            &NetworkSettings::preset(NetworkPreset::Unprotected),
            local(BackendKind::Vz, "macos"),
            &c,
            Mode::Secure
        )
        .unwrap(),
        NetworkPlan::Open {
            note: Some(UNPROTECTED_NOTE)
        }
    );
}

// ----------------------------------------------------------- properties

const PROBES: [&str; 22] = [
    "1.1.1.1",
    "8.8.8.8",
    "2606:4700::1111",
    "169.254.169.254",
    "fd00:ec2::254",
    "10.0.2.2",
    "10.0.2.3",
    "10.0.2.50",
    "192.168.64.1",
    "192.168.1.20",
    "192.168.1.30",
    "203.0.113.7",
    "203.0.113.8",
    "127.0.0.1",
    "172.30.0.3",
    "172.30.1.9",
    "100.64.0.1",
    "fd00::5",
    "fe80::1",
    "::1",
    "::ffff:10.0.2.2",
    "224.0.0.251",
];

const PORTS: [u16; 5] = [53, 22, 3000, 3200, 5432];

fn entry() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "all",
        "1.1.1.1",
        "8.8.8.0/24",
        "10.0.0.0/8",
        "10.0.2.0/24",
        "10.0.2.2",
        "192.168.1.0/24",
        "192.168.64.1",
        "203.0.113.7",
        "127.0.0.1",
        "169.254.169.254",
        "172.30.1.0/24",
        "fd00::/8",
        "::/0",
        "0.0.0.0/0",
    ])
    .prop_map(str::to_string)
}

fn settings() -> impl Strategy<Value = NetworkSettings> {
    (
        prop::sample::select(NetworkPreset::ALL.to_vec()),
        prop::collection::vec(prop::sample::select(vec![22u16, 3000, 5432, 3200]), 0..3),
        any::<bool>(),
        prop::collection::vec(entry(), 0..3),
        prop::collection::vec(entry(), 0..3),
        prop::sample::select(vec![NetworkPreset::Internet, NetworkPreset::None]),
    )
        .prop_map(|(preset, ports, all, allow, deny, base)| {
            let mut s = NetworkSettings {
                preset,
                custom_base: base,
                custom: EgressPolicy { allow, deny },
                ..Default::default()
            };
            match preset {
                NetworkPreset::Localhost => {
                    s.localhost_ports = ports;
                    s.all_localhost_ports = all || s.localhost_ports.is_empty();
                    s.localhost_ports.dedup();
                }
                NetworkPreset::None => s.custom = EgressPolicy::default(),
                _ => {}
            }
            s
        })
}

fn every_support() -> Vec<NetworkSupport> {
    let mut out = Vec::new();
    for k in BACKENDS {
        for os in ["linux", "macos", "windows"] {
            for p in Placement::ALL {
                let s = network_support(k, os, p);
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// What a Space can reach once started under `plan` (`None`: it did not
/// start).
fn reaches(plan: &NetworkPlan, ip: IpAddr, proto: Proto, port: u16) -> Option<bool> {
    match plan {
        NetworkPlan::Enforce(r) => Some(r.allows_port(ip, proto, port)),
        NetworkPlan::NoNetwork { .. } => Some(false),
        NetworkPlan::Open { .. } => Some(true),
        NetworkPlan::Refuse(_) => None,
    }
}

proptest! {
    /// No start is less strict than asked, unless the user chose
    /// `unprotected`: wherever the plan lets the Space reach something, the
    /// settings allow it.
    #[test]
    fn no_start_is_less_strict_than_asked(s in settings()) {
        let c = ctx();
        let asked = compile_mode(&s, &c, Mode::Secure).unwrap();
        for support in every_support() {
            let p = plan(&s, support, &c, Mode::Secure).unwrap();
            if let NetworkPlan::Open { .. } = p {
                prop_assert_eq!(s.preset, NetworkPreset::Unprotected);
                continue;
            }
            for probe in PROBES {
                for port in PORTS {
                    for proto in [Proto::Tcp, Proto::Udp] {
                        let ip = ip(probe);
                        if reaches(&p, ip, proto, port) == Some(true) {
                            prop_assert!(asked.allows_port(ip, proto, port),
                                "{:?} {:?} reaches {} {}", s, support, probe, port);
                        }
                    }
                }
            }
        }
        // And whatever is asked, the platform denies hold.
        for probe in ["169.254.169.254", "fd00:ec2::254", "172.30.1.9"] {
            prop_assert!(!asked.allows_port(ip(probe), Proto::Tcp, 80));
        }
        for host in ["10.0.2.2", "192.168.1.20"] {
            prop_assert!(!asked.allows_port(ip(host), Proto::Tcp, 3200));
        }
        // Only localhost (or no protection) reaches the host.
        if !matches!(s.preset, NetworkPreset::Localhost | NetworkPreset::Unprotected) {
            for host in ["10.0.2.2", "192.168.64.1", "192.168.1.20", "203.0.113.7", "::ffff:10.0.2.2"] {
                for port in PORTS {
                    prop_assert!(!asked.allows_port(ip(host), Proto::Tcp, port), "{:?} {}", s, host);
                }
            }
        }
    }

    /// A child's settings, intersected with its parent's, reach nothing
    /// either of them would not.
    #[test]
    fn a_child_never_reaches_more_than_its_parent(child in settings(), parent in settings()) {
        let c = ctx();
        let both = child.intersect(&parent);
        prop_assert!(both.validate().is_ok(), "{:?}", both);
        let r = compile_mode(&both, &c, Mode::Secure).unwrap();
        let rc = compile_mode(&child, &c, Mode::Secure).unwrap();
        let rp = compile_mode(&parent, &c, Mode::Secure).unwrap();
        for probe in PROBES {
            for port in PORTS {
                let ip = ip(probe);
                if r.allows_port(ip, Proto::Tcp, port) {
                    prop_assert!(rc.allows_port(ip, Proto::Tcp, port) && rp.allows_port(ip, Proto::Tcp, port),
                        "{:?} ∩ {:?} = {:?} reaches {} {}", child, parent, both, probe, port);
                }
            }
        }
    }
}

#[test]
fn intersections_keep_what_both_allow() {
    let both = localhost(&[5432]).intersect(&localhost(&[5432, 3000]));
    assert_eq!(both, localhost(&[5432]));
    let both = localhost(&[5432]).intersect(&NetworkSettings::preset(NetworkPreset::Lan));
    assert_eq!(both, NetworkSettings::internet());
    let both = NetworkSettings::internet().intersect(&custom(
        NetworkPreset::Internet,
        &["1.1.1.1"],
        &["all"],
    ));
    let r = secure(&both);
    assert!(r.allows(ip("1.1.1.1")) && !r.allows(ip("8.8.8.8")));
    let both =
        NetworkSettings::preset(NetworkPreset::Unprotected).intersect(&NetworkSettings::internet());
    assert_eq!(both, NetworkSettings::internet());
    let both = NetworkSettings::internet().intersect(&NetworkSettings::none());
    assert_eq!(both, NetworkSettings::none());
    // A child's LAN allow under an internet-only parent is dropped.
    let both = custom(NetworkPreset::Internet, &["192.168.1.0/24"], &[])
        .intersect(&NetworkSettings::internet());
    assert!(both.custom.allow.is_empty(), "{both:?}");
}

#[test]
fn net_helpers() {
    assert!(net("10.0.0.0/8").covers(&net("10.1.0.0/16")));
    assert!(!net("10.1.0.0/16").covers(&net("10.0.0.0/8")));
    assert!(net("10.1.0.0/16").overlaps(&net("10.0.0.0/8")));
    assert!(!net("10.0.0.0/8").overlaps(&net("::/0")));
    assert_eq!(
        Net::host(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        net("127.0.0.1/32")
    );
    assert_eq!(Net::host(IpAddr::V6(Ipv6Addr::LOCALHOST)), net("::1/128"));
    assert!(Net::parse("all").is_err());
}

#[test]
fn vms_forward_listed_localhost_ports_only() {
    if Mode::from_env() == Mode::Legacy {
        return;
    }
    assert_eq!(vm_localhost_ports(None).unwrap(), Vec::<u16>::new());
    assert_eq!(
        vm_localhost_ports(Some(&NetworkSettings::internet())).unwrap(),
        Vec::<u16>::new()
    );
    let mut s = localhost(&[5432, 3000, 5432]);
    assert_eq!(vm_localhost_ports(Some(&s)).unwrap(), [3000, 5432]);
    s.all_localhost_ports = true;
    let e = vm_localhost_ports(Some(&s)).unwrap_err();
    assert!(e.0.contains("list them"), "{e}");
    assert_eq!(pasta_out_spec(&[]), "none");
    assert_eq!(pasta_out_spec(&[3000, 5432]), "3000,5432");
}
