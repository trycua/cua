//! External runtime providers: discovery, the per-call protocol, error
//! mapping, timeouts, image passthrough and routing in front of the
//! built-in local runtime. The provider is a fixture shell script that
//! records each call and answers from files; every test uses its own temp
//! directory and an explicit [`Registry`] (no `PATH`, no `CUA_HOME`).
#![cfg(unix)]

use async_trait::async_trait;
use cua_sandbox_core::external::{ExternalRuntime, Registry, WithExternal};
use cua_sandbox_core::placement::{Kind, Runtime, local_capabilities_with};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, RuntimeResult,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The fixture provider: logs `<verb> <protocol env> <request>` to
/// `calls.log`, then answers `<verb>.json` on stdout, `<verb>.stderr` on
/// stderr, sleeps `<verb>.sleep` seconds (as itself, so a kill stops it)
/// and exits `<verb>.code`.
const FIXTURE: &str = r#"#!/bin/sh
state='STATE'
verb="$1"
req="$(cat)"
printf '%s %s %s\n' "$verb" "$CUA_RUNTIME_PROTOCOL" "$req" >> "$state/calls.log"
[ -f "$state/$verb.stderr" ] && cat "$state/$verb.stderr" >&2
[ -f "$state/$verb.json" ] && cat "$state/$verb.json"
if [ -f "$state/$verb.sleep" ]; then
  echo $$ > "$state/$verb.pid"
  exec sleep "$(cat "$state/$verb.sleep")"
fi
[ -f "$state/$verb.code" ] && exit "$(cat "$state/$verb.code")"
exit 0
"#;

/// One installed fixture provider.
struct Fixture {
    state: PathBuf,
}

impl Fixture {
    /// Installs `cua-runtime-<file_name>` into `dir`, its `info` answering
    /// `manifest`.
    fn install(dir: &Path, file_name: &str, manifest: Value) -> Fixture {
        let state = dir.join(format!("state-{file_name}"));
        std::fs::create_dir_all(&state).unwrap();
        let exe = dir.join(format!("cua-runtime-{file_name}"));
        std::fs::write(&exe, FIXTURE.replace("STATE", &state.display().to_string())).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let f = Fixture { state };
        f.answer("info", manifest);
        f
    }

    fn answer(&self, verb: &str, v: Value) {
        std::fs::write(self.state.join(format!("{verb}.json")), v.to_string()).unwrap();
    }

    fn fail(&self, verb: &str, kind: &str, message: &str, stderr: &str) {
        self.answer(verb, json!({"error": {"kind": kind, "message": message}}));
        std::fs::write(self.state.join(format!("{verb}.code")), "3").unwrap();
        std::fs::write(self.state.join(format!("{verb}.stderr")), stderr).unwrap();
    }

    /// `(verb, protocol env, request)` of every call so far.
    fn calls(&self) -> Vec<(String, String, Value)> {
        std::fs::read_to_string(self.state.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let mut it = l.splitn(3, ' ');
                let verb = it.next().unwrap().to_string();
                let env = it.next().unwrap_or_default().to_string();
                let req = serde_json::from_str(it.next().unwrap_or("{}")).unwrap_or(Value::Null);
                (verb, env, req)
            })
            .collect()
    }

    fn verbs(&self) -> Vec<String> {
        self.calls().into_iter().map(|c| c.0).collect()
    }
}

fn manifest(name: &str) -> Value {
    json!({
        "protocol": 1,
        "name": name,
        "display_name": "macOS VMs",
        "description": "An example runtime provider",
        "kind": "vm",
        "os": ["macos"],
        "arch": ["arm64"],
        "handles_images": true,
        "capabilities": {"suspend": false, "exec": true, "max_cpus": 8},
        "images": [{"reference": "registry.example/acme/macos:15", "title": "macOS 15", "default": true}]
    })
}

fn spec(name: &str, runtime: Option<&str>) -> LocalStartSpec {
    LocalStartSpec {
        name: name.into(),
        image: "registry.example/acme/macos:15".into(),
        os: "macos".into(),
        cpus: 4,
        memory_mb: 8192,
        disk_size_gb: None,
        ports: vec![3211],
        env: BTreeMap::from([("CUA_ENV_TOKEN".into(), "t".into())]),
        command: None,
        ready_timeout: Duration::from_secs(30),
        firmware: None,
        sidecars: vec![],
        registry_credentials: None,
        container_runtime: None,
        kind: None,
        runtime: runtime.map(String::from),
        restrict_network: false,
        headless: false,
        spacesd: None,
        egress: Default::default(),
        gpu: None,
        probes: vec![],
        ephemeral: false,
        warm: None,
        mounts: vec![],
        forkable: false,
    }
}

fn running(name: &str, port: u16, host_port: u16) -> Value {
    json!({
        "name": name,
        "backend": "whatever",
        "status": "running",
        "endpoints": {"host": "10.0.0.7", "ports": {port.to_string(): host_port}}
    })
}

#[test]
fn discovery_finds_providers_and_skips_bad_ones() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Registry::with_dirs(vec![dir.path().to_path_buf()]);
    assert!(reg.providers().is_empty());

    Fixture::install(dir.path(), "acme", manifest("acme"));
    // A built-in word: the built-in wins.
    Fixture::install(dir.path(), "lume", manifest("lume"));
    // Not a valid name.
    Fixture::install(dir.path(), "Bad_Name", manifest("Bad_Name"));
    // A protocol this build does not speak.
    let mut m = manifest("future");
    m["protocol"] = json!(2);
    Fixture::install(dir.path(), "future", m);
    // `info` names another provider.
    Fixture::install(dir.path(), "liar", manifest("acme"));
    // Not executable.
    let plain = dir.path().join("cua-runtime-plain");
    std::fs::write(&plain, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();

    let d = reg.discover();
    let names: Vec<&str> = d.providers.iter().map(|p| p.name()).collect();
    assert_eq!(names, ["acme"]);
    let acme = &d.providers[0];
    assert_eq!(acme.kind, Kind::Vm);
    assert_eq!(acme.manifest.label(), "macOS VMs");
    assert!(acme.manifest.handles_images);
    assert_eq!(acme.manifest.spacesd_port, 3211);
    assert_eq!(acme.manifest.capabilities.max_cpus, Some(8));
    assert_eq!(
        acme.manifest.images[0].reference,
        "registry.example/acme/macos:15"
    );
    let mut skipped: Vec<&str> = d.skipped.iter().map(|s| s.name.as_str()).collect();
    skipped.sort();
    assert_eq!(skipped, ["Bad_Name", "future", "liar", "lume"]);
    let lume = d.skipped.iter().find(|s| s.name == "lume").unwrap();
    assert!(lume.reason.contains("built-in"), "{}", lume.reason);
    assert!(reg.get("lume").is_none());
    assert!(reg.get("plain").is_none());
    assert_eq!(reg.get("ACME").unwrap().name(), "acme");
}

#[test]
fn discovery_is_cached_and_sees_a_new_provider_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Registry::with_dirs(vec![dir.path().to_path_buf()]);
    let acme = Fixture::install(dir.path(), "acme", manifest("acme"));
    assert_eq!(reg.providers().len(), 1);
    assert_eq!(reg.providers().len(), 1);
    assert!(reg.get("acme").is_some());
    // `info` ran once: nothing changed since.
    assert_eq!(acme.verbs(), ["info"]);
    assert_eq!(acme.calls()[0].1, "1", "CUA_RUNTIME_PROTOCOL");

    // Installing another provider changes the directory.
    std::thread::sleep(Duration::from_millis(20));
    Fixture::install(dir.path(), "other", manifest("other"));
    let mut names: Vec<String> = reg
        .providers()
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["acme", "other"]);
    assert_eq!(
        acme.verbs(),
        ["info"],
        "unchanged providers are not asked again"
    );

    // Removing it does too.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::remove_file(dir.path().join("cua-runtime-other")).unwrap();
    assert_eq!(reg.providers().len(), 1);
}

#[test]
fn an_earlier_directory_wins_a_name() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let mut m = manifest("acme");
    m["display_name"] = json!("first");
    Fixture::install(first.path(), "acme", m);
    Fixture::install(second.path(), "acme", manifest("acme"));
    let reg = Registry::with_dirs(vec![first.path().into(), second.path().into()]);
    let p = reg.get("acme").unwrap();
    assert_eq!(p.manifest.display_name, "first");
    assert!(p.path.starts_with(first.path()));
}

#[test]
fn local_capabilities_list_providers_under_their_kind_and_nothing_without() {
    let dir = tempfile::tempdir().unwrap();
    let reg = Registry::with_dirs(vec![dir.path().to_path_buf()]);
    let none = local_capabilities_with(&reg.providers());
    assert_eq!(
        none,
        cua_sandbox_core::placement::builtin_local_capabilities()
    );
    Fixture::install(dir.path(), "acme", manifest("acme"));
    let caps = local_capabilities_with(&reg.providers());
    let acme = Runtime::Other("acme".into());
    assert_eq!(caps.kind_of(&acme), Some(Kind::Vm));
    // After the built-ins: `auto` never picks it.
    assert_eq!(caps.runtimes(Kind::Vm).last(), Some(&acme));
    assert_eq!(caps.runtimes(Kind::Vm).first(), Some(&Runtime::Qemu));
}

fn runtime(dir: &Path, name: &str) -> ExternalRuntime {
    let reg = Registry::with_dirs(vec![dir.to_path_buf()]);
    ExternalRuntime::new(reg.get(name).expect("provider"))
}

#[tokio::test]
async fn start_passes_the_image_through_and_maps_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = manifest("acme");
    m["spacesd_port"] = json!(4000);
    let f = Fixture::install(dir.path(), "acme", m);
    let mut answer = running("s1", 4000, 55000);
    answer["image"] =
        json!({"os": "macos", "arch": "arm64", "spacesd": true, "digest": "sha256:abc"});
    f.answer("start", answer);
    let rt = runtime(dir.path(), "acme");

    let mut s = spec("s1", Some("acme"));
    s.registry_credentials = Some(cua_sandbox_core::RegistryCredentials::new("me", "secret"));
    let (inst, image) = rt.start_resolved(&s).await.unwrap();

    // The persisted runtime_type is the provider's name.
    assert_eq!(inst.backend, "acme");
    assert_eq!(inst.status, InstanceStatus::Running);
    assert_eq!(inst.endpoints.host, "10.0.0.7");
    // The provider's spacesd port is reachable as cua's.
    assert_eq!(inst.endpoints.ports.get(&3211), Some(&55000));
    let image = image.unwrap();
    assert_eq!(image.reference, "registry.example/acme/macos:15");
    assert_eq!(
        image.pinned_ref,
        "registry.example/acme/macos:15@sha256:abc"
    );
    assert_eq!(image.spacesd, Some(true));
    assert_eq!(image.os, "macos");
    assert_eq!(image.variant, "external");

    let (verb, env, req) = f.calls().pop().unwrap();
    assert_eq!((verb.as_str(), env.as_str()), ("start", "1"));
    // Untouched: no resolution, no prefix.
    assert_eq!(req["image"], "registry.example/acme/macos:15");
    assert_eq!(req["ports"], json!([4000]));
    assert_eq!(req["runtime"], "acme");
    assert_eq!(req["kind"], "vm");
    assert_eq!(req["ready_timeout_secs"], 30);
    assert_eq!(req["registry_credentials"]["username"], "me");
    assert_eq!(req["registry_credentials"]["password"], "secret");
    assert_eq!(req["env"]["CUA_ENV_TOKEN"], "t");

    // An unresolved reference arrives with the caller's default OS; the
    // provider gets the one OS it runs.
    let mut linux = spec("s1", Some("acme"));
    linux.os = "linux".into();
    rt.start(&linux).await.unwrap();
    assert_eq!(f.calls().pop().unwrap().2["os"], "macos");

    // Without credentials the field is absent.
    rt.start(&spec("s1", Some("acme"))).await.unwrap();
    assert!(
        f.calls()
            .pop()
            .unwrap()
            .2
            .get("registry_credentials")
            .is_none()
    );
}

#[tokio::test]
async fn the_manifest_limits_are_checked_before_the_provider_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::install(dir.path(), "acme", manifest("acme"));
    let rt = runtime(dir.path(), "acme");
    let mut s = spec("s1", Some("acme"));
    s.cpus = 16;
    assert!(matches!(
        rt.start(&s).await,
        Err(RuntimeError::InvalidPlacement(e)) if e.message.contains("at most 8")
    ));
    let mut s = spec("s1", Some("acme"));
    s.restrict_network = true;
    assert!(matches!(
        rt.start(&s).await,
        Err(RuntimeError::Unsupported { backend, .. }) if backend == "acme"
    ));
    let mut s = spec("s1", Some("acme"));
    s.ports.push(8080);
    assert!(matches!(
        rt.start(&s).await,
        Err(RuntimeError::Unsupported { .. })
    ));
    assert!(matches!(
        rt.suspend("s1").await,
        Err(RuntimeError::Unsupported { .. })
    ));
    assert!(matches!(
        rt.fork("s1", "s2").await,
        Err(RuntimeError::Unsupported { .. })
    ));
    assert_eq!(f.verbs(), ["info"]);
}

#[tokio::test]
async fn errors_map_to_runtime_errors_with_the_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::install(dir.path(), "acme", manifest("acme"));
    let rt = runtime(dir.path(), "acme");

    f.fail("status", "not_found", "no such VM", "");
    assert_eq!(
        rt.status("gone").await,
        Err(RuntimeError::NotFound("gone".into()))
    );
    f.fail("stop", "unsupported", "stopping while booting", "");
    assert_eq!(
        rt.stop("s1").await,
        Err(RuntimeError::Unsupported {
            backend: "acme".into(),
            op: "stopping while booting".into()
        })
    );
    f.fail("start", "invalid", "that image is not for this runtime", "");
    match rt.start(&spec("s1", Some("acme"))).await {
        Err(RuntimeError::InvalidPlacement(e)) => {
            assert!(
                e.message.contains("that image is not for this runtime"),
                "{e}"
            )
        }
        other => panic!("{other:?}"),
    }
    f.fail(
        "delete",
        "unavailable",
        "service is down",
        "line 1\nlast words\n",
    );
    match rt.delete("s1").await {
        Err(RuntimeError::Other(m)) => {
            assert!(
                m.contains("runtime acme is unavailable: service is down"),
                "{m}"
            );
            assert!(m.contains("last words"), "{m}");
        }
        other => panic!("{other:?}"),
    }
    // A crash without a protocol error is `other`, with stderr.
    std::fs::remove_file(f.state.join("endpoints.json")).ok();
    std::fs::write(f.state.join("endpoints.code"), "2").unwrap();
    std::fs::write(f.state.join("endpoints.stderr"), "panic: boom").unwrap();
    match rt.endpoints("s1").await {
        Err(RuntimeError::Other(m)) => assert!(m.contains("panic: boom"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Garbage on success.
    f.answer("list", json!("not a list"));
    assert!(matches!(rt.list().await, Err(RuntimeError::Other(m)) if m.contains("invalid JSON")));
}

#[tokio::test]
async fn a_call_that_outlives_its_timeout_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::install(dir.path(), "acme", manifest("acme"));
    std::fs::write(f.state.join("status.sleep"), "30").unwrap();
    let rt = runtime(dir.path(), "acme").with_call_timeout(Duration::from_millis(500));
    let t = Instant::now();
    match rt.status("s1").await {
        Err(RuntimeError::Other(m)) => assert!(m.contains("timed out"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(10));
    let pid = std::fs::read_to_string(f.state.join("status.pid"))
        .unwrap()
        .trim()
        .to_string();
    // Killed: give the kernel a moment to reap it.
    let mut alive = true;
    for _ in 0..50 {
        alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if !alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!alive, "provider {pid} still running after its timeout");
}

#[tokio::test]
async fn exec_and_status_and_doctor_speak_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::install(dir.path(), "acme", manifest("acme"));
    let rt = runtime(dir.path(), "acme");
    f.answer(
        "exec",
        json!({"exit_code": 3, "stdout_b64": "aGkK", "stderr_b64": ""}),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    assert_eq!(rt.guest_exec("s1", "echo hi", None, tx).await.unwrap(), 3);
    assert_eq!(
        rx.recv().await,
        Some(cua_sandbox_core::GuestOutput::Stdout(b"hi\n".to_vec()))
    );
    let req = f.calls().pop().unwrap().2;
    assert_eq!(req["argv"], json!(["/bin/sh", "-c", "echo hi"]));

    f.answer("status", json!({"status": "stopped"}));
    assert_eq!(rt.status("s1").await.unwrap(), InstanceStatus::Stopped);
    assert_eq!(f.calls().pop().unwrap().2, json!({"name": "s1"}));

    f.answer(
        "doctor",
        json!({"ready": false, "detail": "needs setup", "missing": ["service"]}),
    );
    let d = rt.doctor().await.unwrap();
    assert!(!d.ready);
    assert_eq!(d.missing, ["service"]);
    // No suspend: a Space powers off with stop.
    assert_eq!(
        rt.power_control("acme"),
        Some(cua_sandbox_core::PowerControl::Stop)
    );
}

/// The built-in runtime, recording what reaches it.
#[derive(Default)]
struct Builtin {
    calls: Mutex<Vec<String>>,
}

impl Builtin {
    fn log(&self, c: String) {
        self.calls.lock().unwrap().push(c);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl LocalRuntime for Builtin {
    fn backend(&self) -> String {
        "vmm".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.log(format!("start {}", spec.name));
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "qemu".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints::default(),
        })
    }
    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.log(format!("stop {name}"));
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        self.log("list".into());
        Ok(vec![LocalSummary {
            name: "b1".into(),
            backend: "qemu".into(),
            status: InstanceStatus::Running,
        }])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.log(format!("status {name}"));
        Ok(InstanceStatus::Running)
    }
    async fn status_on(&self, name: &str, runtime_type: &str) -> RuntimeResult<InstanceStatus> {
        self.log(format!("status_on {name} {runtime_type}"));
        Ok(InstanceStatus::Running)
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.log(format!("delete {name}"));
        Ok(())
    }
    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        self.log(format!("endpoints {name}"));
        Ok(LocalEndpoints::default())
    }
}

#[tokio::test]
async fn with_no_provider_every_call_goes_to_the_builtin_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let inner = Arc::new(Builtin::default());
    let rt = WithExternal::new(inner.clone(), Registry::with_dirs(vec![dir.path().into()]));
    rt.start(&spec("b1", None)).await.unwrap();
    // A runtime word no provider has is the built-in runtime's to refuse.
    rt.start(&spec("b2", Some("acme"))).await.unwrap();
    assert_eq!(rt.list().await.unwrap().len(), 1);
    rt.stop("b1").await.unwrap();
    rt.status_on("b1", "qemu").await.unwrap();
    rt.delete("b1").await.unwrap();
    assert_eq!(rt.backend(), "vmm");
    assert_eq!(
        inner.calls(),
        [
            "start b1",
            "start b2",
            "list",
            "stop b1",
            "status_on b1 qemu",
            "delete b1"
        ]
    );
}

#[tokio::test]
async fn calls_route_to_the_provider_that_owns_the_instance() {
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::install(dir.path(), "acme", manifest("acme"));
    f.answer("start", running("ext1", 3211, 50001));
    f.answer(
        "list",
        json!([{"name": "ext1", "backend": "acme", "status": "running"}]),
    );
    f.answer("status", json!({"status": "running"}));
    f.answer(
        "endpoints",
        json!({"host": "10.0.0.7", "ports": {"3211": 50001}}),
    );
    let reg = Registry::with_dirs(vec![dir.path().into()]);
    let inner = Arc::new(Builtin::default());
    let rt = WithExternal::new(inner.clone(), reg.clone());

    let inst = rt.start(&spec("ext1", Some("acme"))).await.unwrap();
    assert_eq!(inst.backend, "acme");
    rt.start(&spec("b1", None)).await.unwrap();
    rt.stop("ext1").await.unwrap();
    assert_eq!(rt.endpoints("ext1").await.unwrap().ports[&3211], 50001);

    let listed = rt.list().await.unwrap();
    let mut names: Vec<(&str, &str)> = listed
        .iter()
        .map(|s| (s.name.as_str(), s.backend.as_str()))
        .collect();
    names.sort();
    assert_eq!(names, [("b1", "qemu"), ("ext1", "acme")]);

    // The persisted runtime_type routes by itself.
    assert_eq!(
        rt.status_on("ext1", "acme").await.unwrap(),
        InstanceStatus::Running
    );
    assert_eq!(
        rt.power_control("acme"),
        Some(cua_sandbox_core::PowerControl::Stop)
    );

    // After a restart (a new router), the provider's listing finds it.
    let fresh = WithExternal::new(inner.clone(), reg.clone());
    fresh.delete("ext1").await.unwrap();
    // A built-in instance the provider does not list stays built-in.
    fresh.stop("b1").await.unwrap();

    assert_eq!(
        f.verbs(),
        [
            "info",
            "start",
            "stop",
            "endpoints",
            "list",
            "status",
            "list",
            "delete",
            "list"
        ]
    );
    assert_eq!(inner.calls(), ["start b1", "list", "stop b1"]);
}
