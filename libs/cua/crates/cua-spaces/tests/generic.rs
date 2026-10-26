//! Spaces on images without cua-spacesd: a Space is any sandbox, its
//! capability set is empty, its declared services answer generic MCP, and
//! spacesd primitives fail at once with `capability_missing`. The MCP
//! servers are `cua_sandbox_core::testing::McpTestServer` (rmcp); nothing
//! starts a VM or container.

use async_trait::async_trait;
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

fn labelled(name: &str, labels: &[(&str, &str)]) -> SpaceCreate {
    SpaceCreate {
        on: Some(On::Local),
        image: Some(PLAIN.into()),
        name: Some(name.into()),
        services: [("mcp".to_string(), 8765u16)].into(),
        timeout: Some(Duration::from_secs(30)),
        labels: labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn labels_are_kept_with_the_space_and_leave_with_it() {
    let server = McpTestServer::start("").await.unwrap();
    let rt = Arc::new(PortRuntime::default());
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
    let want: BTreeMap<String, String> = [("team", "research"), ("run", "7")]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let info = spaces
        .create(labelled("lab-a", &[("team", "research"), ("run", "7")]))
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert_eq!(info.labels, want, "the create returns them");
    let plain = spaces
        .create(labelled("lab-b", &[]))
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert!(plain.labels.is_empty());

    // Listed, and read back by a new handle on the same home.
    let again = local_spaces(&rt, reg.path(), state.path());
    let listed = again.list().unwrap();
    let a = listed.iter().find(|i| i.id == "local:lab-a").unwrap();
    assert_eq!(a.labels, want);
    assert!(
        listed
            .iter()
            .find(|i| i.id == "local:lab-b")
            .unwrap()
            .labels
            .is_empty()
    );
    // They are in the JSON the CLI and MCP print.
    let json = serde_json::to_value(a).unwrap();
    assert_eq!(json["labels"]["team"], "research");
    assert!(
        serde_json::to_value(&listed[1].labels)
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty()
    );

    // `list_spaces` filters on them.
    let mcp = McpServer::new(spaces.clone());
    let rows = |out: cua_spaces::mcp::ToolOutcome| {
        assert!(!out.is_error, "{:?}", out.content);
        serde_json::from_str::<serde_json::Value>(out.content[0]["text"].as_str().unwrap()).unwrap()
    };
    let all = rows(mcp.call("list_spaces", json!({})).await);
    assert_eq!(all.as_array().unwrap().len(), 2);
    let hit = rows(
        mcp.call("list_spaces", json!({"labels": {"team": "research"}}))
            .await,
    );
    assert_eq!(hit.as_array().unwrap().len(), 1);
    assert_eq!(hit[0]["id"], "local:lab-a");
    assert_eq!(hit[0]["labels"]["run"], "7");
    let none = rows(
        mcp.call("list_spaces", json!({"labels": {"team": "other"}}))
            .await,
    );
    assert!(none.as_array().unwrap().is_empty());

    // Deleting forgets them: the same name made again starts clean.
    again.delete("local:lab-a").await.unwrap();
    let fresh = again
        .create(labelled("lab-a", &[]))
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert!(fresh.labels.is_empty(), "{:?}", fresh.labels);
    assert!(
        again
            .registry()
            .meta("local:lab-a")
            .is_none_or(|m| m.labels.is_empty())
    );
}

#[tokio::test]
async fn labels_are_checked_before_anything_is_created() {
    let rt = Arc::new(PortRuntime::default());
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = local_spaces(&rt, reg.path(), state.path());
    let refused = |c: SpaceCreate| {
        let spaces = spaces.clone();
        async move {
            let Err(e) = spaces.create(c).await else {
                panic!("the create must be refused");
            };
            assert_eq!(e.tag(), "invalid_argument", "{e}");
            e.to_string()
        }
    };
    assert!(
        refused(labelled("x", &[("", "v")]))
            .await
            .contains("not empty")
    );
    assert!(
        refused(labelled("x", &[("k", "")]))
            .await
            .contains("not empty")
    );
    assert!(
        refused(labelled("x", &[(&"k".repeat(64), "v")]))
            .await
            .contains("at most 63")
    );
    assert!(
        refused(labelled("x", &[("k", &"v".repeat(257))]))
            .await
            .contains("at most 256")
    );
    // 63 and 256 bytes pass the check (multi-byte counts bytes).
    cua_spaces::registry::validate_labels([(&"k".repeat(63), &"v".repeat(256))].into_iter())
        .unwrap();
    let e =
        cua_spaces::registry::validate_labels([(&"é".repeat(32), &"v".to_string())].into_iter())
            .unwrap_err();
    assert!(e.to_string().contains("64 bytes"), "{e}");
    let many: Vec<(String, String)> = (0..65).map(|i| (format!("k{i}"), "v".into())).collect();
    let mut c = labelled("x", &[]);
    c.labels = many.iter().cloned().collect();
    assert!(refused(c).await.contains("at most 64"));
    let mut c = labelled("x", &[("a", "b")]);
    c.reuse = true;
    assert!(refused(c).await.contains("reuse"));
    let mut c = labelled("x", &[("a", "b")]);
    c.durable = true;
    assert!(refused(c).await.contains("durable"));
    assert!(rt.started.lock().unwrap().is_empty(), "nothing was started");
    assert!(spaces.list().unwrap().is_empty());
    // The CLI/MCP word for a label.
    assert_eq!(
        cua_spaces::registry::parse_label("a=b=c").unwrap(),
        ("a".to_string(), "b=c".to_string())
    );
    assert!(cua_spaces::registry::parse_label("a").is_err());
    assert!(cua_spaces::registry::parse_label("a=").is_err());
    assert!(cua_spaces::registry::parse_label("=b").is_err());
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn creation_time_and_lifetime_are_listed() {
    let server = McpTestServer::start("").await.unwrap();
    let rt = Arc::new(PortRuntime::default());
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

    let before = now_unix();
    let info = spaces
        .create(labelled("clock", &[]))
        .await
        .unwrap()
        .ready()
        .unwrap();
    let after = now_unix();
    // Created now (the create finished between `before` and `after`), no
    // lifetime yet.
    assert!(
        (before..=after).contains(&info.created_at_unix),
        "{} not in {before}..={after}",
        info.created_at_unix
    );
    assert_eq!((info.expires_at_unix, info.on_expire.as_str()), (0, ""));
    let json = serde_json::to_value(&info).unwrap();
    assert!(json.get("expires_at_unix").is_none(), "zero is left out");
    assert_eq!(json["created_at_unix"], info.created_at_unix);

    // A lifetime is readable back, in the list and in a fresh handle.
    let at = spaces
        .set_lifetime(&info.id, Duration::from_secs(90), None)
        .unwrap();
    let at = at.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    // One store, one view: the sandbox reports the very same expiry (what
    // `Sandbox.expires_at_unix` reads), and no second copy is in the file.
    let sandbox_name = info.id.strip_prefix("local:").unwrap();
    let (sandbox_at, how) = Sandboxes::builder()
        .state_dir(state.path())
        .build()
        .local_expiry(sandbox_name)
        .expect("Spaces::set_lifetime is the sandbox's lifetime");
    assert_eq!(
        sandbox_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        at
    );
    assert_eq!(how, cua_sandbox_core::LocalExpiry::Delete);
    assert!(
        !std::fs::read_to_string(reg.path().join("space-lifetimes.json"))
            .unwrap_or_default()
            .contains(&info.id),
        "a local Space's lifetime is not kept twice"
    );
    // And the other way: a lifetime set on the sandbox shows in the Space.
    Sandboxes::builder()
        .state_dir(state.path())
        .build()
        .set_local_expiry(sandbox_name, Duration::from_secs(7200), None)
        .unwrap();
    let via_sandbox = spaces.lifetime(&info.id).unwrap();
    assert!(
        via_sandbox.expires_at_unix >= now_unix() + 7190,
        "{via_sandbox:?}"
    );
    let at = spaces
        .set_lifetime(&info.id, Duration::from_secs(90), None)
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let listed = spaces.list().unwrap();
    let row = listed.iter().find(|i| i.id == info.id).unwrap();
    assert_eq!(
        (
            row.expires_at_unix,
            row.on_expire.as_str(),
            row.created_at_unix
        ),
        (at, "delete", info.created_at_unix)
    );
    spaces
        .set_lifetime(
            &info.id,
            Duration::from_secs(600),
            Some(cua_sandbox_core::LocalExpiry::Suspend),
        )
        .unwrap();
    let again = local_spaces(&rt, reg.path(), state.path());
    let row = again
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == info.id)
        .unwrap();
    assert_eq!(row.on_expire, "suspend");
    assert!(row.expires_at_unix >= now_unix() + 590, "{row:?}");
    // Clearing it lists none.
    again.clear_lifetime(&info.id).unwrap();
    let row = again
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == info.id)
        .unwrap();
    assert_eq!((row.expires_at_unix, row.on_expire.as_str()), (0, ""));

    // A Space added by address was "created" when it was added.
    let added = again
        .add(&format!("{}/mcp", server.url), None, Some("adopted".into()))
        .await
        .unwrap();
    assert!(
        (before..=now_unix()).contains(&added.created_at_unix) && added.created_at_unix > 0,
        "{added:?}"
    );
    let row = again
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == added.id)
        .unwrap();
    assert_eq!(row.created_at_unix, added.created_at_unix);

    // The name made again gets a new creation time and no old lifetime.
    again
        .set_lifetime(&info.id, Duration::from_secs(600), None)
        .unwrap();
    again.delete(&info.id).await.unwrap();
    let fresh = again
        .create(labelled("clock", &[]))
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert!(fresh.created_at_unix >= info.created_at_unix);
}

#[tokio::test]
async fn egress_is_refused_before_anything_is_created_or_run() {
    let rt = Arc::new(PortRuntime::default());
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = local_spaces(&rt, reg.path(), state.path());
    let policy = |allow: &[&str], deny: &[&str]| cua_spaces::egress::EgressPolicy {
        allow: allow.iter().map(|s| s.to_string()).collect(),
        deny: deny.iter().map(|s| s.to_string()).collect(),
    };
    let create = |egress: cua_spaces::egress::EgressPolicy, f: &dyn Fn(&mut SpaceCreate)| {
        let mut c = labelled("eg", &[]);
        c.egress = egress;
        f(&mut c);
        c
    };
    let tag = |c: SpaceCreate| {
        let spaces = spaces.clone();
        async move {
            let Err(e) = spaces.create(c).await else {
                panic!("the create must be refused");
            };
            (e.tag(), e.to_string())
        }
    };
    // A hostname, a bad CIDR, durable, reuse: InvalidArgument.
    let (kind, msg) = tag(create(policy(&[], &["example.com"]), &|_| {})).await;
    assert_eq!(kind, "invalid_argument");
    assert!(msg.contains("by IP/CIDR"), "{msg}");
    assert_eq!(
        tag(create(policy(&[], &["10.0.0.0/40"]), &|_| {})).await.0,
        "invalid_argument"
    );
    let (kind, msg) = tag(create(policy(&[], &["all"]), &|c| c.reuse = true)).await;
    assert_eq!((kind, msg.contains("egress")), ("invalid_argument", true));
    assert_eq!(
        tag(create(policy(&[], &["all"]), &|c| c.durable = true))
            .await
            .0,
        "invalid_argument"
    );
    // On Cua Cloud: it has ended, so the create is refused before anything
    // is made (the egress policy never reaches a cloud).
    let (_, msg) = tag(create(policy(&[], &["all"]), &|c| {
        c.on = Some(cua_sandbox_core::placement::On::Cloud)
    }))
    .await;
    assert!(msg.contains(cua_sandbox_core::CLOUD_ENDED), "{msg}");
    // A create that returns before the Space is ready would hand it back
    // without its rules: refused wherever it runs.
    let (kind, msg) = tag(create(policy(&[], &["all"]), &|c| c.wait = Some(false))).await;
    assert_eq!(
        (kind, msg.contains("wait = false")),
        ("invalid_argument", true),
        "{msg}"
    );
    // No outbound network cannot be kept by a durable or reused Space.
    for f in [
        (&|c: &mut SpaceCreate| c.reuse = true) as &dyn Fn(&mut SpaceCreate),
        &|c: &mut SpaceCreate| c.durable = true,
    ] {
        let (kind, msg) = tag(create(policy(&[], &[]), &|c| {
            c.network = cua_sandbox_core::NetworkMode::None;
            f(c)
        }))
        .await;
        assert_eq!(
            (kind, msg.contains("network=none")),
            ("invalid_argument", true),
            "{msg}"
        );
    }
    assert!(rt.started.lock().unwrap().is_empty(), "nothing was started");
    // A local runtime that does not enforce it (this fake keeps none):
    // the sandbox it started is deleted, never handed out unfiltered.
    let (kind, msg) = tag(create(policy(&[], &["all"]), &|_| {})).await;
    assert_eq!(kind, "wrong_provider", "{msg}");
    assert!(msg.contains("without enforcing it"), "{msg}");
    assert_eq!(rt.started.lock().unwrap().len(), 1);
    assert!(
        !rt.running.lock().unwrap().contains_key("eg"),
        "deleted: {:?}",
        rt.running.lock().unwrap()
    );

    // A Space added by address is not run by cua: refused (a bad entry
    // never gets that far), nothing recorded.
    let server = McpTestServer::start("").await.unwrap();
    let added = spaces
        .add(&format!("{}/mcp", server.url), None, Some("plain".into()))
        .await
        .unwrap();
    let e = spaces
        .set_egress(&added.id, &policy(&[], &["example.com"]))
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument");
    let e = spaces
        .set_egress(&added.id, &policy(&[], &["10.0.0.0/8"]))
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "wrong_provider", "{e}");
    assert!(e.to_string().contains("not run by cua"), "{e}");
    assert!(spaces.egress(&added.id).is_empty(), "nothing was recorded");
    assert_eq!(spaces.egress_enforced(&added.id), None);
    assert_eq!(
        spaces
            .set_egress("local:nope", &policy(&[], &["10.0.0.0/8"]))
            .await
            .unwrap_err()
            .tag(),
        "not_found"
    );
}
