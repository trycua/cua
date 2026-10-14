//! Structural checks on the compiled contract: every service is present,
//! every file on disk is compiled, enum zero values follow the policy, and
//! every client-streaming RPC has a registered unary fallback.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use prost::Message;
use prost_types::FileDescriptorSet;

fn descriptor_set() -> FileDescriptorSet {
    FileDescriptorSet::decode(cua_proto::FILE_DESCRIPTOR_SET).expect("descriptor set decodes")
}

/// Fully-qualified method path ("/pkg.Service/Method") -> (client_streaming,
/// server_streaming).
fn methods(fds: &FileDescriptorSet) -> BTreeMap<String, (bool, bool)> {
    let mut out = BTreeMap::new();
    for file in &fds.file {
        for service in &file.service {
            for method in &service.method {
                out.insert(
                    format!("/{}.{}/{}", file.package(), service.name(), method.name()),
                    (method.client_streaming(), method.server_streaming()),
                );
            }
        }
    }
    out
}

const ENV_SERVICES: &[&str] = &[
    "cua.env.v1.SystemService",
    "cua.env.v1.ProcessService",
    "cua.env.v1.FilesystemService",
    "cua.env.v1.ComputerService",
    "cua.env.v1.WindowsService",
    "cua.env.v1.AccessibilityService",
    "cua.env.v1.DriverService",
    "cua.env.v1.StreamService",
    "cua.env.v1.PresenceService",
    "cua.env.v1.TeleportService",
    "cua.env.v1.TunnelService",
    "cua.env.v1.HostSpacesService",
    "cua.env.v1.VolumeService",
];

const DAEMON_SERVICES: &[&str] = &[
    "cua.daemon.v1.SandboxService",
    "cua.daemon.v1.SpaceService",
    "cua.daemon.v1.RuntimeService",
    "cua.daemon.v1.DaemonService",
];

#[test]
fn every_service_is_in_the_descriptor_set() {
    let fds = descriptor_set();
    let found: BTreeSet<String> = fds
        .file
        .iter()
        .flat_map(|f| {
            f.service
                .iter()
                .map(move |s| format!("{}.{}", f.package(), s.name()))
        })
        .collect();
    let expected: BTreeSet<String> = ENV_SERVICES
        .iter()
        .chain(DAEMON_SERVICES)
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        found, expected,
        "service set drifted; update this test and the README crate map"
    );
}

#[cfg(feature = "server")]
#[test]
fn generated_servers_carry_the_contract_names() {
    use cua_proto::daemon::v1 as d;
    use cua_proto::env::v1 as e;
    let generated = [
        e::system_service_server::SERVICE_NAME,
        e::process_service_server::SERVICE_NAME,
        e::filesystem_service_server::SERVICE_NAME,
        e::computer_service_server::SERVICE_NAME,
        e::windows_service_server::SERVICE_NAME,
        e::accessibility_service_server::SERVICE_NAME,
        e::driver_service_server::SERVICE_NAME,
        e::stream_service_server::SERVICE_NAME,
        e::presence_service_server::SERVICE_NAME,
        e::teleport_service_server::SERVICE_NAME,
        e::tunnel_service_server::SERVICE_NAME,
        e::host_spaces_service_server::SERVICE_NAME,
        e::volume_service_server::SERVICE_NAME,
        d::sandbox_service_server::SERVICE_NAME,
        d::space_service_server::SERVICE_NAME,
        d::runtime_service_server::SERVICE_NAME,
        d::daemon_service_server::SERVICE_NAME,
    ];
    let expected: Vec<&str> = ENV_SERVICES
        .iter()
        .chain(DAEMON_SERVICES)
        .copied()
        .collect();
    assert_eq!(generated.to_vec(), expected);
}

fn proto_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto")
}

fn collect_protos(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_protos(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "proto") {
            let rel = path.strip_prefix(root).unwrap();
            out.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn every_proto_file_on_disk_is_compiled() {
    let root = proto_root();
    let mut on_disk = BTreeSet::new();
    collect_protos(&root, &root, &mut on_disk);
    let compiled: BTreeSet<String> = descriptor_set()
        .file
        .iter()
        .map(|f| f.name().to_string())
        .filter(|n| n.starts_with("cua/"))
        .collect();
    assert_eq!(
        on_disk, compiled,
        "add new .proto files to PROTOS in build.rs"
    );
}

#[test]
fn enum_zero_values_are_unspecified() {
    fn check(name: &str, e: &prost_types::EnumDescriptorProto, bad: &mut Vec<String>) {
        let zero = e.value.iter().find(|v| v.number() == 0);
        match zero {
            Some(v) if v.name().ends_with("_UNSPECIFIED") => {}
            _ => bad.push(format!("{name}.{}", e.name())),
        }
    }
    fn walk(prefix: &str, m: &prost_types::DescriptorProto, bad: &mut Vec<String>) {
        let name = format!("{prefix}.{}", m.name());
        for e in &m.enum_type {
            check(&name, e, bad);
        }
        for nested in &m.nested_type {
            walk(&name, nested, bad);
        }
    }
    let mut bad = Vec::new();
    for file in descriptor_set()
        .file
        .iter()
        .filter(|f| f.package().starts_with("cua."))
    {
        for e in &file.enum_type {
            check(file.package(), e, &mut bad);
        }
        for m in &file.message_type {
            walk(file.package(), m, &mut bad);
        }
    }
    assert!(bad.is_empty(), "enums without *_UNSPECIFIED = 0: {bad:?}");
}

#[test]
fn every_client_stream_has_a_unary_fallback() {
    let methods = methods(&descriptor_set());
    let client_streaming: BTreeSet<&str> = methods
        .iter()
        .filter(|(_, (client, _))| *client)
        .map(|(name, _)| name.as_str())
        .collect();
    let registered: BTreeSet<&str> = cua_proto::CLIENT_STREAM_FALLBACKS
        .iter()
        .map(|(s, _)| *s)
        .collect();
    assert_eq!(
        client_streaming, registered,
        "gRPC-Web cannot client-stream: register a unary fallback in CLIENT_STREAM_FALLBACKS"
    );
    for (stream, fallback) in cua_proto::CLIENT_STREAM_FALLBACKS {
        let (client, server) = methods
            .get(*fallback)
            .unwrap_or_else(|| panic!("fallback {fallback} for {stream} does not exist"));
        assert!(!client && !server, "fallback {fallback} must be unary");
    }
}

#[test]
fn no_bidirectional_streams() {
    // Bidi streams cannot cross gRPC-Web or the Fleet gateway at all.
    let bidi: Vec<String> = methods(&descriptor_set())
        .into_iter()
        .filter(|(_, (c, s))| *c && *s)
        .map(|(n, _)| n)
        .collect();
    assert!(
        bidi.is_empty(),
        "bidirectional RPCs are not allowed: {bidi:?}"
    );
}

#[test]
fn descriptor_set_keeps_source_comments() {
    // Docs generators and reflection clients rely on comments being present.
    let fds = descriptor_set();
    let system = fds
        .file
        .iter()
        .find(|f| f.name() == "cua/env/v1/system.proto")
        .unwrap();
    let info = system
        .source_code_info
        .as_ref()
        .expect("source info included");
    assert!(info.location.iter().any(|l| {
        l.leading_comments()
            .contains("Discovery, bootstrap and lifecycle")
    }));
}

/// Pins the field numbers of the audio additions (protocol revision 2) so a
/// renumbering is caught here as well as by `buf breaking`.
#[test]
fn audio_fields_are_pinned() {
    let fds = descriptor_set();
    let field = |file: &str, message: &str, name: &str| -> i32 {
        let f = fds.file.iter().find(|f| f.name() == file).unwrap();
        let m = f.message_type.iter().find(|m| m.name() == message).unwrap();
        m.field
            .iter()
            .find(|x| x.name() == name)
            .unwrap_or_else(|| panic!("{message}.{name} missing"))
            .number()
    };
    let stream = "cua/env/v1/stream.proto";
    assert_eq!(field(stream, "StreamTarget", "audio"), 5);
    assert_eq!(field(stream, "ListTargetsResponse", "audio_sources"), 3);
    assert_eq!(field(stream, "ListTargetsResponse", "audio_codecs"), 4);
    assert_eq!(field(stream, "OpenMediaRequest", "audio"), 10);
    assert_eq!(field(stream, "OpenMediaRequest", "disable_video"), 11);
    assert_eq!(field(stream, "OpenMediaResponse", "audio"), 14);
    assert_eq!(field(stream, "SetPreferencesRequest", "audio_enabled"), 5);
    assert_eq!(
        field(stream, "SetPreferencesRequest", "audio_uplink_muted"),
        11
    );
    assert_eq!(field(stream, "SetPreferencesResponse", "audio_encoding"), 5);
    assert_eq!(
        field("cua/env/v1/system.proto", "InitRequest", "audio_uplink"),
        8
    );
}

#[test]
fn audio_feature_names_are_documented() {
    let fds = descriptor_set();
    let system = fds
        .file
        .iter()
        .find(|f| f.name() == "cua/env/v1/system.proto")
        .unwrap();
    let docs: String = system
        .source_code_info
        .as_ref()
        .unwrap()
        .location
        .iter()
        .map(|l| l.leading_comments())
        .collect();
    for name in ["\"audio.desktop\"", "\"audio.per_app\"", "\"audio.uplink\""] {
        assert!(
            docs.contains(name),
            "capability {name} not documented on Feature"
        );
    }
}
