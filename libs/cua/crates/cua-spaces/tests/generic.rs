//! Spaces on images without cua-spacesd: a Space is any sandbox, its
//! capability set is empty, its declared services answer generic MCP, and
//! spacesd primitives fail at once with `capability_missing`. The MCP
//! servers are `cua_sandbox_core::testing::McpTestServer` (rmcp); Fleet is
//! `FakeFleet`; nothing starts a VM, container or claim.

use async_trait::async_trait;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::placement::On;
use cua_sandbox_core::testing::{McpTestServer, expected_result};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, Sandboxes,
};
use cua_spaces::mcp::McpServer;
use cua_spaces::{Provider, SpaceCreate, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use serde_json::{Map, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PLAIN: &str = "docker.io/library/python:3.12-slim";

fn args(v: serde_json::Value) -> Map<String, serde_json::Value> {
    v.as_object().unwrap().clone()
}

/// Every spacesd primitive on a Space without one fails fast and names
/// the missing capability.
async fn assert_env_primitives_fail_fast(spaces: &Spaces, id: &str) {
    let space = spaces.space(id).await.unwrap();
    assert!(!space.has_spacesd());
    assert!(space.capabilities().features.is_empty());
    let started = Instant::now();
    let e = space
        .bash("uname", Duration::from_secs(5))
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "capability_missing", "{e}");
    assert!(space.spacesd().is_err());
    let server = McpServer::new(spaces.clone());
    let out = server
        .call("space_bash", json!({"space": id, "command": "uname"}))
        .await;
    assert!(out.is_error);
    assert_eq!(
        out.structured.as_ref().unwrap()["error"]["kind"],
        "capability_missing",
        "{out:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "capability errors are immediate, took {:?}",
        started.elapsed()
    );
}

async fn assert_mcp_service_works(spaces: &Spaces, id: &str, service: Option<&str>) {
    let space = spaces.space(id).await.unwrap();
    let (tools, contract) = space.list_tools(service).await.unwrap();
    assert_eq!(tools.len(), 5);
    assert!(contract.starts_with("mcp/20"), "{contract}");
    assert!(tools[0].read_only);
    let r = space
        .call_tool(service, "add", args(json!({"a": 2, "b": 3})), None)
        .await
        .unwrap();
    assert_eq!(r.content[0]["text"], "5");
    assert_eq!(r.structured, Some(json!({"sum": 5})));
    let r = space
        .call_tool(service, "fail", Map::new(), None)
        .await
        .unwrap();
    assert!(r.is_error, "a tool error is a result");
    // Multimodal content is forwarded as the server sent it.
    for (tool, a) in [
        ("image", json!({"bytes": 300_000})),
        ("audio", json!({})),
        ("resources", json!({})),
    ] {
        let r = space
            .call_tool(service, tool, args(a.clone()), None)
            .await
            .unwrap();
        assert_eq!(
            serde_json::Value::Array(r.content.clone()),
            expected_result(tool, a)["content"],
            "{tool}"
        );
    }

    // The same through the Spaces MCP server (`cua daemon mcp`).
    let server = McpServer::new(spaces.clone());
    let mut a = json!({"space": id});
    if let Some(s) = service {
        a["service"] = json!(s);
    }
    let out = server.call("list_tools", a.clone()).await;
    assert!(!out.is_error, "{out:?}");
    let v: serde_json::Value = serde_json::from_str(out.first_text().unwrap()).unwrap();
    assert_eq!(v["service"], service.unwrap_or("mcp"));
    assert_eq!(v["count"], 5);
    assert!(
        v["services"]
            .as_array()
            .unwrap()
            .contains(&json!(v["service"].as_str().unwrap())),
        "{v}"
    );
    a["tool"] = json!("add");
    a["arguments"] = json!({"a": 40, "b": 2});
    let out = server.call("call_tool", a.clone()).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.first_text(), Some("42"));
    a["tool"] = json!("image");
    a["arguments"] = json!({"bytes": 4096});
    let out = server.call("call_tool", a).await;
    assert_eq!(
        out.to_result()["content"],
        expected_result("image", json!({"bytes": 4096}))["content"],
        "the Spaces MCP server forwards image blocks verbatim"
    );
}

#[tokio::test]
async fn add_a_plain_mcp_url_as_a_space() {
    {
        let server = McpTestServer::start("").await.unwrap();
        let reg = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder().home(reg.path()).build();
        let started = Instant::now();
        let info = spaces
            .add(&format!("{}/mcp", server.url), None, Some("tools".into()))
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(info.provider, Provider::Direct);
        assert!(info.features.is_empty());
        assert_eq!(info.spacesd_version, "");
        assert_eq!(info.services, ["mcp"]);
        assert_mcp_service_works(&spaces, &info.id, Some("mcp")).await;
        assert_mcp_service_works(&spaces, &info.id, None).await;
        assert_env_primitives_fail_fast(&spaces, &info.id).await;

        // A fresh registry handle reconnects from the stored service URL.
        let again = Spaces::builder().home(reg.path()).build();
        assert_eq!(again.list().unwrap()[0].services, ["mcp"]);
        assert_mcp_service_works(&again, "tools", Some("mcp")).await;
        again.remove(&info.id).await.unwrap();
    }
}

#[tokio::test]
async fn add_names_the_service_and_refuses_urls_with_nothing_behind_them() {
    let server = McpTestServer::start("/api").await.unwrap();
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(reg.path()).build();
    let info = spaces
        .add_with_service(
            &format!("{}/api/mcp", server.url),
            None,
            None,
            Some("blender".into()),
        )
        .await
        .unwrap();
    assert_eq!(info.services, ["blender"]);
    assert_mcp_service_works(&spaces, &info.id, Some("blender")).await;
    let space = spaces.space(&info.id).await.unwrap();
    assert_eq!(
        space.list_tools(Some("nope")).await.unwrap_err().tag(),
        "not_found"
    );

    // A port nothing listens on is neither a spacesd nor MCP.
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let started = Instant::now();
    let e = spaces
        .add(&format!("http://127.0.0.1:{closed}/mcp"), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "spacesd_not_available", "{e}");
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(spaces.list().unwrap().len(), 1, "nothing registered");
}

/// A local runtime whose guest ports map to loopback listeners.
#[derive(Default)]
struct PortRuntime {
    ports: Mutex<BTreeMap<u16, u16>>,
    started: Mutex<Vec<LocalStartSpec>>,
    running: Mutex<BTreeMap<String, bool>>,
}

impl PortRuntime {
    fn endpoints(&self) -> LocalEndpoints {
        LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: self.ports.lock().unwrap().clone(),
            ..Default::default()
        }
    }
}

#[async_trait]
impl LocalRuntime for PortRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.started.lock().unwrap().push(spec.clone());
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        // Like a real runtime, only the ports the spec asks for.
        let mut ports = self.ports.lock().unwrap().clone();
        ports.retain(|g, _| spec.ports.contains(g));
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ports,
                ..Default::default()
            },
        })
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().insert(name.into(), false);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(true) => Ok(InstanceStatus::Running),
            Some(false) => Ok(InstanceStatus::Stopped),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(self.endpoints())
    }
}

fn local_spaces(rt: &Arc<PortRuntime>, reg: &std::path::Path, state: &std::path::Path) -> Spaces {
    Spaces::builder()
        .home(reg)
        .sandboxes(
            Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state)
                .build(),
        )
        .build()
}

#[tokio::test]
async fn provision_local_plain_image_with_an_mcp_service() {
    {
        let server = McpTestServer::start("").await.unwrap();
        let rt = Arc::new(PortRuntime::default());
        // 3211 would refuse: the image has no spacesd.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        rt.ports
            .lock()
            .unwrap()
            .extend([(8765, server.port()), (3211, closed)]);
        let reg = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let spaces = local_spaces(&rt, reg.path(), state.path());
        let started = Instant::now();
        let info = spaces
            .create(SpaceCreate {
                on: Some(On::Local),
                image: Some(PLAIN.into()),
                name: Some("cua-e2e-plain".into()),
                services: [("mcp".to_string(), 8765u16)].into(),
                command: Some(vec!["python".into(), "/srv/mcp.py".into()]),
                timeout: Some(Duration::from_secs(600)),
                ..Default::default()
            })
            .await
            .unwrap()
            .ready()
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no 600 s wait for 3211: {:?}",
            started.elapsed()
        );
        assert_eq!(info.id, "local:cua-e2e-plain");
        assert!(info.features.is_empty());
        assert_eq!(info.services, ["mcp"]);
        let spec = rt.started.lock().unwrap()[0].clone();
        assert!(spec.ports.contains(&8765));
        // The command reaches the local runtime (container ENTRYPOINT).
        assert_eq!(
            spec.command,
            Some(vec!["python".to_string(), "/srv/mcp.py".to_string()])
        );
        assert_mcp_service_works(&spaces, &info.id, Some("mcp")).await;
        assert_env_primitives_fail_fast(&spaces, &info.id).await;

        // Reconnect from the registry and the sandbox state file.
        let again = local_spaces(&rt, reg.path(), state.path());
        assert_mcp_service_works(&again, &info.id, None).await;
        again.delete(&info.id).await.unwrap();
        assert!(again.list().unwrap().is_empty());
    }
}

#[tokio::test]
async fn an_spacesd_space_also_reaches_its_declared_services() {
    let server = McpTestServer::start("").await.unwrap();
    let env = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(PortRuntime::default());
    rt.ports
        .lock()
        .unwrap()
        .extend([(8765, server.port()), (3211, env.addr.port())]);
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = local_spaces(&rt, reg.path(), state.path());
    let info = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("ghcr.io/trycua/cua-desktop-linux:docker-latest".into()),
            spacesd: Some(true),
            name: Some("cua-e2e-both".into()),
            services: [("tools".to_string(), 8765u16)].into(),
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert!(!info.features.is_empty());
    assert_eq!(info.services, ["tools"]);
    let space = spaces.space(&info.id).await.unwrap();
    assert!(space.has_spacesd());
    assert_eq!(
        space
            .bash("echo env", Duration::from_secs(5))
            .await
            .unwrap()
            .stdout,
        "env\n"
    );
    assert_mcp_service_works(&spaces, &info.id, Some("tools")).await;
    assert!(space.services().contains(&"tools".to_string()));
    spaces.delete(&info.id).await.unwrap();
}

#[tokio::test]
async fn claim_fleet_plain_image_is_ready_without_spacesd() {
    {
        let reg = tempfile::tempdir().unwrap();
        let fleet = FakeFleet::new();
        cua_fleet::testing::set_image_variant(PLAIN, cua_fleet::ImageVariant::Rootfs);
        // The gateway is a loopback server serving the claim's `mcp` service
        // at `/api/svc/<pool>/sbx-<claim>-mcp/mcp`; the pool name comes from
        // the image and workload (command and env, as the template has them).
        let names = Spaces::builder()
            .home(reg.path().join("names"))
            .fleet_namespace("cua-e2e-gen")
            .build();
        let command: Vec<String> = ["python", "/srv/mcp.py"].map(String::from).to_vec();
        let pool = names.fleet_pool_name(
            cua_spaces::contract::inputs::FleetRuntime::Gvisor,
            &cua_spaces::pool_key(
                PLAIN,
                Some(&command),
                &[("MCP_PORT".to_string(), "8765".to_string())].into(),
                &[("mcp".to_string(), 8765u16)].into(),
            ),
        );
        let server = McpTestServer::start(&format!("/api/svc/{pool}/sbx-cua-e2e-gen-claim-mcp"))
            .await
            .unwrap();
        let spaces = Spaces::builder()
            .home(reg.path())
            .fleet(fleet.client_with_base(&server.url))
            .fleet_namespace("cua-e2e-gen")
            .build();
        let started = Instant::now();
        let info = spaces
            .create(SpaceCreate {
                on: Some(On::Cloud),
                image: Some(PLAIN.into()),
                name: Some("cua-e2e-gen-claim".into()),
                command: Some(vec!["python".into(), "/srv/mcp.py".into()]),
                env: [("MCP_PORT".to_string(), "8765".to_string())].into(),
                services: [("mcp".to_string(), 8765u16)].into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .ready()
            .expect("waited");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "no 300 s wait for 3211: {:?}",
            started.elapsed()
        );
        assert!(info.features.is_empty());
        assert_eq!(info.services, ["mcp"]);
        // The pool template: gVisor from the manifest, the command and env
        // (processMode Run), readiness on the declared service, no env port.
        let pools: Vec<_> = fleet
            .all_namespaces()
            .into_iter()
            .filter(|n| n.starts_with("cua-e2e-gen-gvisor-"))
            .collect();
        assert_eq!(pools.len(), 1, "{pools:?}");
        let template = fleet.object("template", &pools[0], &pools[0]).unwrap();
        let t = template.to_string();
        let vm = &template["spec"]["vmTemplate"];
        assert_eq!(vm["env"]["MCP_PORT"], "8765", "{t}");
        assert_eq!(vm["command"][1], "/srv/mcp.py", "{t}");
        assert_eq!(vm["processMode"], "Run", "{t}");
        assert!(!t.contains("3211"), "no spacesd port: {t}");
        assert_mcp_service_works(&spaces, &info.id, Some("mcp")).await;
        assert_env_primitives_fail_fast(&spaces, &info.id).await;
        spaces.delete(&info.id).await.unwrap();
    }
}

#[tokio::test]
async fn fleet_env_and_commands_run_on_both_runtimes() {
    let reg = tempfile::tempdir().unwrap();
    let fleet = FakeFleet::new();
    cua_fleet::testing::set_image_variant(PLAIN, cua_fleet::ImageVariant::Rootfs);
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fleet.client())
        .build();
    let template_of = |prefix: &str| {
        let ns = fleet
            .all_namespaces()
            .into_iter()
            .find(|n| n.contains(prefix))
            .unwrap_or_else(|| panic!("no {prefix} pool"));
        fleet.object("template", &ns, &ns).unwrap()
    };
    // env without a command: the image's entrypoint runs with it.
    // `wait: false`: the fake has no sandbox to wait for; the template is
    // what is checked.
    let pending = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(PLAIN.into()),
            env: [("A".to_string(), "b".to_string())].into(),
            wait: Some(false),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready();
    assert!(pending.is_err(), "wait: false returns the pending claim");
    let vm = template_of("-gvisor-")["spec"]["vmTemplate"].clone();
    assert_eq!(vm["env"]["A"], "b");
    assert_eq!(vm["processMode"], "Run");
    assert!(vm.get("command").is_none_or(|c| c.is_null()), "{vm}");
    // A command on a VM image runs too (processMode Run, cloud-init).
    let image = "ghcr.io/trycua/cua-desktop-linux:latest";
    cua_fleet::testing::set_image_variant(image, cua_fleet::ImageVariant::ContainerDisk);
    let pending = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(image.into()),
            command: Some(vec!["true".into()]),
            wait: Some(false),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready();
    assert!(pending.is_err(), "wait: false returns the pending claim");
    let vm = template_of("-kubevirt-")["spec"]["vmTemplate"].clone();
    assert_eq!(vm["command"], serde_json::json!(["true"]));
    assert_eq!(vm["processMode"], "Run");
}
