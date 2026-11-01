//! The Daytona gateway's control plane over mock Spaces, at the wire: the
//! error envelope, create validation and parameters, list filters and
//! cursors, the lifecycle rules, snapshots, volumes, secrets, warm pools,
//! network settings, the build-context object store and Dockerfile builds.
//! The unmodified Python and TypeScript SDKs are driven against the same
//! gateway by tests/compat/daytona.

use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use serde_json::{Value, json};
use std::sync::Arc;

struct Gw {
    gw: Gateway,
    mock: Arc<MockBackend>,
    http: reqwest::Client,
    api: String,
    _dir: tempfile::TempDir,
}

async fn start() -> Gw {
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockBackend::new());
    let mut o = GatewayOptions::new(Provider::Daytona);
    o.listen = "127.0.0.1:0".parse().unwrap();
    o.host_port = 0;
    o.dir = Some(dir.path().to_path_buf());
    o.create_default_user = false;
    let gw = Gateway::start(o, Some(mock.clone())).await.unwrap();
    let api = format!("http://{}/api", gw.api_addr);
    Gw {
        gw,
        mock,
        http: reqwest::Client::new(),
        api,
        _dir: dir,
    }
}

impl Gw {
    fn req(&self, m: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(m, format!("{}{path}", self.api))
            .bearer_auth(&self.gw.api_key)
    }
    async fn call(&self, m: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.req(m, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
    async fn get(&self, path: &str) -> (u16, Value) {
        self.call(reqwest::Method::GET, path, None).await
    }
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }
    async fn create(&self, body: Value) -> Value {
        let (s, v) = self.post("/sandbox", body).await;
        assert_eq!(s, 200, "{v}");
        v
    }
}

#[tokio::test]
async fn errors_carry_the_envelope_the_sdks_translate() {
    let g = start().await;
    let r = g
        .http
        .get(format!("{}/sandbox", g.api))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let b: Value = r.json().await.unwrap();
    assert_eq!(b["statusCode"], 401);
    assert_eq!(b["source"], "DAYTONA_API");
    assert!(b["message"].is_string() && b["error"].is_string());
    let (s, b) = g.get("/sandbox/nope").await;
    assert_eq!(s, 404);
    assert_eq!(b["source"], "DAYTONA_API");
    assert!(b["message"].as_str().unwrap().contains("nope"));
}

#[tokio::test]
async fn the_sandbox_dto_has_every_required_field() {
    let g = start().await;
    let sb = g.create(json!({"name": "dto"})).await;
    // daytona_api_client 0.225.0 models/sandbox.py: required fields.
    for k in [
        "id",
        "organizationId",
        "name",
        "user",
        "env",
        "labels",
        "public",
        "networkBlockAll",
        "kvm",
        "target",
        "cpu",
        "gpu",
        "memory",
        "disk",
        "toolboxProxyUrl",
    ] {
        assert!(!sb[k].is_null(), "{k} is missing from {sb}");
    }
    assert_eq!(sb["state"], "started");
    assert_eq!(sb["desiredState"], "started");
    assert_eq!(sb["autoStopInterval"], 15);
    assert_eq!(sb["autoArchiveInterval"], 10080);
    assert_eq!(sb["autoDeleteInterval"], -1);
}

#[tokio::test]
async fn create_validation_is_enforced_server_side() {
    let g = start().await;
    for (why, body) in [
        ("negative stop", json!({"autoStopInterval": -1})),
        (
            "stop and pause",
            json!({"autoStopInterval": 5, "autoPauseInterval": 5}),
        ),
        (
            "pause with ephemeral",
            json!({"autoPauseInterval": 5, "autoDeleteInterval": 0}),
        ),
        ("queue timeout", json!({"queueTimeout": 0})),
        ("zero cpu", json!({"cpu": 0})),
        ("shell in user", json!({"user": "bob; id"})),
        ("bad cidr", json!({"networkAllowList": "10.0.0.0/33"})),
        ("bad proxy", json!({"outboundProxyUrl": "ftp://x"})),
        (
            "snapshot and build",
            json!({"snapshot": "a", "buildInfo": {"dockerfileContent": "FROM x"}}),
        ),
        (
            "relative mount",
            json!({"volumes": [{"volumeId": "v", "mountPath": "x"}]}),
        ),
        ("not an object", json!([1])),
    ] {
        let (s, b) = g.post("/sandbox", body).await;
        assert_eq!(s, 400, "{why}: {b}");
        assert_eq!(b["source"], "DAYTONA_API");
    }
    let (s, _) = g.post("/sandbox", json!({"name": "dup"})).await;
    assert_eq!(s, 200);
    let (s, b) = g.post("/sandbox", json!({"name": "dup"})).await;
    assert_eq!(s, 409, "{b}");
    assert!(b["message"].as_str().unwrap().contains("dup"));
}

#[tokio::test]
async fn create_parameters_reach_the_record_and_the_backend() {
    let g = start().await;
    let sb = g
        .create(json!({
            "name": "p", "user": "alice", "env": {"A": "1"}, "labels": {"k": "v", "cua.kind": "container"},
            "public": true, "cpu": 3, "memory": 6, "disk": 12,
            "outboundProxyUrl": "http://proxy:3128", "otelEndpointOverride": "http://otel:4318",
            "autoStopInterval": 0, "autoDeleteInterval": 30, "ttlMinutes": 90, "queueTimeout": 5,
        }))
        .await;
    assert_eq!(sb["user"], "alice");
    assert_eq!(sb["public"], true);
    assert_eq!(
        (sb["cpu"].as_u64(), sb["memory"].as_u64()),
        (Some(3), Some(6))
    );
    assert_eq!(sb["env"]["A"], "1");
    assert_eq!(sb["env"]["HTTPS_PROXY"], "http://proxy:3128");
    assert_eq!(sb["env"]["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://otel:4318");
    assert_eq!(sb["outboundProxyUrl"], "http://proxy:3128");
    assert_eq!(sb["autoStopInterval"], 0);
    assert_eq!(sb["autoDeleteInterval"], 30);
    assert_eq!(sb["queueTimeout"], 5);
    assert!(sb["autoDestroyAt"].is_string(), "ttl gives an end: {sb}");
    let spec = g.mock.created.lock().await.last().cloned().unwrap();
    assert_eq!(spec.place.kind.as_deref(), Some("container"));
    assert_eq!(spec.place.cpus, Some(3));
    assert_eq!(spec.place.memory_mb, Some(6144));
    let extras = g.mock.extras.lock().await.last().cloned().unwrap();
    assert_eq!(extras.disk_gb, Some(12));
}

#[tokio::test]
async fn list_filters_sort_and_cursor() {
    let g = start().await;
    let mut ids = Vec::new();
    for i in 0..5 {
        let sb = g
            .create(json!({"name": format!("n{i}"), "labels": {"grp": "x", "i": i.to_string()}, "public": i % 2 == 0, "cpu": i + 1}))
            .await;
        ids.push(sb["id"].as_str().unwrap().to_string());
    }
    let labels = "labels=%7B%22grp%22%3A%22x%22%7D";
    let (s, b) = g
        .get(&format!("/sandbox?{labels}&limit=2&sort=name&order=asc"))
        .await;
    assert_eq!(s, 200);
    let names: Vec<_> = b["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["n0", "n1"]);
    let mut cursor = b["nextCursor"].as_str().map(str::to_string);
    let mut all = names;
    while let Some(c) = cursor {
        let (_, b) = g
            .get(&format!(
                "/sandbox?{labels}&limit=2&sort=name&order=asc&cursor={c}"
            ))
            .await;
        all.extend(
            b["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["name"].as_str().unwrap().to_string()),
        );
        cursor = b["nextCursor"].as_str().map(str::to_string);
    }
    assert_eq!(all, ["n0", "n1", "n2", "n3", "n4"]);
    let (_, b) = g
        .get(&format!("/sandbox?{labels}&sort=cpu&order=desc"))
        .await;
    assert_eq!(b["items"][0]["name"], "n4");
    let (_, b) = g.get(&format!("/sandbox?{labels}&isPublic=true")).await;
    assert_eq!(b["items"].as_array().unwrap().len(), 3);
    let (_, b) = g.get(&format!("/sandbox?{labels}&minCpu=2&maxCpu=3")).await;
    assert_eq!(b["items"].as_array().unwrap().len(), 2);
    let (_, b) = g.get(&format!("/sandbox?{labels}&states=stopped")).await;
    assert_eq!(b["items"].as_array().unwrap().len(), 0);
    let (_, b) = g.get("/sandbox?name=N3").await;
    assert_eq!(b["items"].as_array().unwrap().len(), 1);
    for bad in [
        "limit=0",
        "limit=500",
        "states=bogus",
        "sort=x",
        "cursor=!!",
        "minCpu=a",
        "labels=%5B%5D",
    ] {
        let (s, _) = g.get(&format!("/sandbox?{bad}")).await;
        assert_eq!(s, 400, "{bad}");
    }
}

#[tokio::test]
async fn lifecycle_follows_the_documented_rules() {
    let g = start().await;
    let id = g.create(json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let call = |op: &'static str| {
        let g = &g;
        let id = id.clone();
        async move { g.post(&format!("/sandbox/{id}/{op}"), json!({})).await }
    };
    assert_eq!(
        call("archive").await.0,
        400,
        "archive needs a stopped sandbox"
    );
    assert_eq!(call("recover").await.0, 400, "recover needs an error state");
    assert_eq!(call("start").await.1["state"], "started");
    let (s, b) = call("stop").await;
    assert_eq!((s, b["state"].as_str()), (200, Some("stopped")));
    assert_eq!(call("stop").await.0, 200);
    assert_eq!(call("pause").await.0, 400);
    let (s, b) = call("archive").await;
    assert_eq!((s, b["state"].as_str()), (200, Some("archived")));
    let (s, b) = call("start").await;
    assert_eq!((s, b["state"].as_str()), (200, Some("started")));
    let (s, b) = call("pause").await;
    assert_eq!((s, b["state"].as_str()), (200, Some("paused")));
    assert_eq!(call("start").await.1["state"], "started");
    let (s, _) = g
        .call(reqwest::Method::DELETE, &format!("/sandbox/{id}"), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(g.get(&format!("/sandbox/{id}")).await.0, 404);
}

#[tokio::test]
async fn build_info_becomes_an_image_or_layers() {
    let g = start().await;
    // A bare FROM is just an image.
    let sb = g
        .create(json!({"buildInfo": {"dockerfileContent": "FROM python:3.12\n"}}))
        .await;
    assert_eq!(sb["state"], "started");
    let spec = g.mock.created.lock().await.last().cloned().unwrap();
    assert!(
        matches!(spec.source, cua_compat::backend::Source::Image(Some(ref i)) if i == "python:3.12")
    );
    assert!(g.mock.extras.lock().await.last().unwrap().build.is_none());
    // Layers build, in the background: pending_build, then started.
    let sb = g
        .create(json!({"buildInfo": {"dockerfileContent": "FROM python:3.12\nRUN pip install x\nENV A=b\nWORKDIR /w\n"}}))
        .await;
    assert_eq!(sb["state"], "pending_build");
    let id = sb["id"].as_str().unwrap().to_string();
    let mut state = String::new();
    for _ in 0..100 {
        state = g.get(&format!("/sandbox/{id}")).await.1["state"]
            .as_str()
            .unwrap()
            .to_string();
        if state != "pending_build" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(state, "started");
    let b = g
        .mock
        .extras
        .lock()
        .await
        .last()
        .cloned()
        .unwrap()
        .build
        .unwrap();
    assert_eq!(b.env["A"], "b");
    assert!(b.layers.len() >= 2);
    let (s, u) = g.get(&format!("/sandbox/{id}/build-logs-url")).await;
    assert_eq!(s, 200);
    let logs = g
        .http
        .get(u["url"].as_str().unwrap())
        .bearer_auth(&g.gw.api_key)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(logs.contains("Sandbox started"), "{logs}");
    // What cannot be built is refused with the instruction.
    let (s, b) = g
        .post(
            "/sandbox",
            json!({"buildInfo": {"dockerfileContent": "FROM a\nONBUILD RUN x\n"}}),
        )
        .await;
    assert_eq!(s, 400);
    assert!(b["message"].as_str().unwrap().contains("ONBUILD"));
}

#[tokio::test]
async fn snapshots_pull_build_and_report_failures() {
    let g = start().await;
    let (s, b) = g
        .post(
            "/snapshots",
            json!({"name": "s1", "imageName": "python:3.12", "cpu": 2, "memory": 4, "disk": 9}),
        )
        .await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(b["state"], "pending");
    let id = b["id"].as_str().unwrap().to_string();
    let mut st = String::new();
    for _ in 0..100 {
        st = g.get(&format!("/snapshots/{id}")).await.1["state"]
            .as_str()
            .unwrap()
            .to_string();
        if st == "active" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(st, "active");
    assert_eq!(g.get("/snapshots/s1").await.1["imageName"], "python:3.12");
    assert_eq!(
        g.post("/snapshots", json!({"name": "s1", "imageName": "x"}))
            .await
            .0,
        409
    );
    assert_eq!(
        g.post("/snapshots", json!({"name": "bad name", "imageName": "x"}))
            .await
            .0,
        400
    );
    assert_eq!(g.post("/snapshots", json!({"name": "n"})).await.0, 400);
    assert_eq!(
        g.post(
            "/snapshots",
            json!({"name": "n", "imageName": "x", "buildInfo": {"dockerfileContent": "FROM x"}})
        )
        .await
        .0,
        400
    );
    // Resources of the snapshot are the defaults of its sandboxes.
    let sb = g.create(json!({"snapshot": "s1"})).await;
    assert_eq!(
        (
            sb["cpu"].as_u64(),
            sb["memory"].as_u64(),
            sb["disk"].as_u64()
        ),
        (Some(2), Some(4), Some(9))
    );
    // A Dockerfile that cannot be built ends build_failed with the reason.
    let (_, b) = g
        .post(
            "/snapshots",
            json!({"name": "bad", "buildInfo": {"dockerfileContent": "FROM a\nONBUILD RUN x\n"}}),
        )
        .await;
    let bid = b["id"].as_str().unwrap().to_string();
    let mut bad = Value::Null;
    for _ in 0..100 {
        bad = g.get(&format!("/snapshots/{bid}")).await.1;
        if bad["state"] != "pending" && bad["state"] != "building" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(bad["state"], "build_failed");
    assert!(
        bad["errorReason"].as_str().unwrap().contains("ONBUILD"),
        "{bad}"
    );
    // activate / deactivate
    assert_eq!(
        g.post("/snapshots/s1/activate", json!({})).await.0,
        400,
        "already active"
    );
    assert_eq!(g.post("/snapshots/s1/deactivate", json!({})).await.0, 204);
    assert_eq!(g.post("/snapshots/s1/deactivate", json!({})).await.0, 400);
    assert_eq!(
        g.post("/snapshots", json!({"name": "x", "imageName": "i"}))
            .await
            .0,
        200
    );
    let (s, _) = g.post("/sandbox", json!({"snapshot": "s1"})).await;
    assert_eq!(s, 400, "an inactive snapshot cannot start sandboxes");
    assert_eq!(g.post("/snapshots/s1/activate", json!({})).await.0, 200);
    // pages
    let (_, p) = g.get("/snapshots?page=1&limit=1").await;
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    assert!(p["total"].as_u64().unwrap() >= 3 && p["totalPages"].as_u64().unwrap() >= 3);
    assert_eq!(g.get("/snapshots?page=0").await.0, 400);
    assert_eq!(
        g.call(reqwest::Method::DELETE, "/snapshots/s1", None)
            .await
            .0,
        200
    );
    assert_eq!(g.get("/snapshots/s1").await.0, 404);
}

#[tokio::test]
async fn volumes_and_secrets_manage_their_objects() {
    let g = start().await;
    let (s, v) = g.post("/volumes", json!({"name": "data"})).await;
    assert_eq!((s, v["state"].as_str()), (200, Some("ready")));
    assert_eq!(g.post("/volumes", json!({"name": "data"})).await.0, 409);
    assert_eq!(
        g.post("/volumes", json!({"name": "bad name!"})).await.0,
        400
    );
    assert_eq!(g.get("/volumes/by-name/data").await.1["id"], v["id"]);
    assert_eq!(g.get("/volumes/by-name/zzz").await.0, 404);
    assert_eq!(g.get("/volumes").await.1.as_array().unwrap().len(), 1);
    // The mock Space has no Cua Volume: a create that mounts one says so and leaves nothing behind.
    let (s, b) = g
        .post(
            "/sandbox",
            json!({"labels": {"v": "1"}, "volumes": [{"volumeId": v["id"], "mountPath": "/data"}]}),
        )
        .await;
    assert_eq!(s, 400);
    assert!(b["message"].as_str().unwrap().contains("Cua Volume"), "{b}");
    assert_eq!(
        g.get("/sandbox?labels=%7B%22v%22%3A%221%22%7D").await.1["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        g.post(
            "/sandbox",
            json!({"volumes": [{"volumeId": "nope", "mountPath": "/d"}]})
        )
        .await
        .0,
        404
    );
    assert_eq!(
        g.call(
            reqwest::Method::DELETE,
            &format!("/volumes/{}", v["id"].as_str().unwrap()),
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(g.get("/volumes").await.1.as_array().unwrap().len(), 0);

    let (s, sec) = g
        .post(
            "/secret",
            json!({"name": "api_key", "value": "hunter2", "hosts": ["api.example.com"]}),
        )
        .await;
    assert_eq!(s, 201, "{sec}");
    assert!(sec.get("value").is_none() && !sec.to_string().contains("hunter2"));
    assert_eq!(
        g.post("/secret", json!({"name": "api_key", "value": "x"}))
            .await
            .0,
        409
    );
    assert_eq!(
        g.post("/secret", json!({"name": "9", "value": "x"}))
            .await
            .0,
        400
    );
    assert_eq!(
        g.post(
            "/secret",
            json!({"name": "h", "value": "x", "hosts": ["a:1"]})
        )
        .await
        .0,
        400
    );
    let (_, page) = g.get("/secret/paginated?limit=10").await;
    assert_eq!(page["total"], 1);
    let (s, _) = g
        .post("/sandbox", json!({"secrets": [{"KEY": "api_key"}]}))
        .await;
    assert_eq!(s, 400, "no plaintext label, no mount");
    let sb = g
        .create(json!({"secrets": [{"KEY": "api_key"}], "labels": {"cua.secrets": "plaintext"}}))
        .await;
    assert_eq!(sb["env"]["KEY"], "hunter2");
    let (s, up) = g
        .call(
            reqwest::Method::PUT,
            &format!("/sandbox/{}/secrets", sb["id"].as_str().unwrap()),
            Some(json!({"secrets": []})),
        )
        .await;
    assert_eq!(s, 200);
    assert!(up["env"].get("KEY").is_none(), "detached: {up}");
}

#[tokio::test]
async fn warm_pools_map_to_cua_pools() {
    let g = start().await;
    let (_, s) = g
        .post(
            "/snapshots",
            json!({"name": "wp", "imageName": "python:3.12"}),
        )
        .await;
    for _ in 0..100 {
        if g.get("/snapshots/wp").await.1["state"] == "active" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let _ = s;
    let (st, p) = g
        .post("/warm-pools", json!({"snapshot": "wp", "pool": 2}))
        .await;
    assert_eq!(st, 201, "{p}");
    assert_eq!(p["pool"], 2);
    assert_eq!(p["currentSize"], 2);
    assert_eq!(g.mock.pools.lock().await.len(), 1);
    assert_eq!(
        g.post("/warm-pools", json!({"snapshot": "wp", "pool": 1}))
            .await
            .0,
        409
    );
    assert_eq!(
        g.post("/warm-pools", json!({"snapshot": "nope", "pool": 1}))
            .await
            .0,
        404
    );
    assert_eq!(
        g.post("/warm-pools", json!({"snapshot": "wp", "pool": 0}))
            .await
            .0,
        400
    );
    let id = p["id"].as_str().unwrap();
    let (st, up) = g
        .call(
            reqwest::Method::PATCH,
            &format!("/warm-pools/{id}"),
            Some(json!({"pool": 4})),
        )
        .await;
    assert_eq!((st, up["pool"].as_u64()), (200, Some(4)));
    // a create from the snapshot draws on the pool
    g.create(json!({"snapshot": "wp"})).await;
    assert!(g.mock.extras.lock().await.last().unwrap().pool.is_some());
    assert_eq!(
        g.call(reqwest::Method::DELETE, &format!("/warm-pools/{id}"), None)
            .await
            .0,
        204
    );
    assert!(g.mock.pools.lock().await.is_empty());
}

#[tokio::test]
async fn network_settings_validate_before_touching_the_guest() {
    let g = start().await;
    let id = g.create(json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/sandbox/{id}/network-settings");
    assert_eq!(
        g.post(&path, json!({})).await.0,
        400,
        "something must be set"
    );
    assert_eq!(
        g.post(&path, json!({"networkAllowList": "999.1.1.1/8"}))
            .await
            .0,
        400
    );
    assert_eq!(
        g.post(&path, json!({"domainAllowList": "a b"})).await.0,
        400
    );
}

#[tokio::test]
async fn the_object_store_takes_the_uploads_both_sdks_make() {
    let mut g = start().await;
    // The storage endpoint is HTTPS under the gateway's CA.
    let ca = reqwest::Certificate::from_pem(
        &std::fs::read(g.gw.dir.join("tls").join("ca.pem")).unwrap(),
    )
    .unwrap();
    g.http = reqwest::Client::builder()
        .add_root_certificate(ca)
        .build()
        .unwrap();
    let (s, acc) = g.get("/object-storage/push-access").await;
    assert_eq!(s, 200);
    let base = acc["storageUrl"].as_str().unwrap().to_string();
    let bucket = acc["bucket"].as_str().unwrap().to_string();
    let org = acc["organizationId"].as_str().unwrap().to_string();
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/20260101/us-east-1/s3/aws4_request, SignedHeaders=host, Signature=00",
        acc["accessKey"].as_str().unwrap()
    );
    let key = format!("{org}/abc123/context.tar");
    let url = format!("{base}/{bucket}/{key}");
    let head = g
        .http
        .head(&url)
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 404);
    let denied = g.http.head(&url).send().await.unwrap();
    assert_eq!(denied.status(), 403);
    // a tar with one file
    let mut tarball = Vec::new();
    {
        let mut b = tar::Builder::new(&mut tarball);
        let data = b"hello";
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "home/u/f.txt", &data[..]).unwrap();
        b.finish().unwrap();
    }
    // multipart (what obstore and lib-storage send), the last part aws-chunked
    let init = g
        .http
        .post(format!("{url}?uploads"))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let upload_id = init
        .split("<UploadId>")
        .nth(1)
        .unwrap()
        .split("</UploadId>")
        .next()
        .unwrap()
        .to_string();
    let (a, b) = tarball.split_at(100);
    let r1 = g
        .http
        .put(format!("{url}?partNumber=1&uploadId={upload_id}"))
        .header("authorization", &auth)
        .body(a.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), 200);
    let mut chunked = format!("{:x}\r\n", b.len()).into_bytes();
    chunked.extend_from_slice(b);
    chunked.extend_from_slice(b"\r\n0\r\nx-amz-checksum-crc32:AAAA\r\n\r\n");
    let r2 = g
        .http
        .put(format!("{url}?partNumber=2&uploadId={upload_id}"))
        .header("authorization", &auth)
        .header("content-encoding", "aws-chunked")
        .body(chunked)
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), 200);
    let done = g
        .http
        .post(format!("{url}?uploadId={upload_id}"))
        .header("authorization", &auth)
        .body("<CompleteMultipartUpload/>")
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), 200);
    assert_eq!(
        g.http
            .head(&url)
            .header("authorization", &auth)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let list = g
        .http
        .get(format!(
            "{base}/{bucket}?list-type=2&prefix={org}/abc123/&max-keys=1"
        ))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        list.contains("<Key>") && list.contains("context.tar"),
        "{list}"
    );
    // a Dockerfile COPYing from it
    let (s, sb) = g
        .post(
            "/sandbox",
            json!({"buildInfo": {"dockerfileContent": "FROM x\nCOPY home/u/f.txt /w/f.txt\n", "contextHashes": ["abc123"]}}),
        )
        .await;
    assert_eq!(s, 200, "{sb}");
    for _ in 0..100 {
        if g.get(&format!("/sandbox/{}", sb["id"].as_str().unwrap()))
            .await
            .1["state"]
            != "pending_build"
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let b = g
        .mock
        .extras
        .lock()
        .await
        .last()
        .cloned()
        .unwrap()
        .build
        .unwrap();
    assert_eq!(b.files.len(), 1);
    assert!(b.files[0].destination.starts_with("/tmp/.cua-ctx-"));
    assert!(std::path::Path::new(&b.files[0].source).is_file());
    // an unknown context hash is a clear error
    let (s, e) = g
        .post("/sandbox", json!({"buildInfo": {"dockerfileContent": "FROM x\nCOPY a /b\n", "contextHashes": ["missing"]}}))
        .await;
    assert_eq!(s, 400);
    assert!(
        e["message"].as_str().unwrap().contains("was not uploaded"),
        "{e}"
    );
}

#[tokio::test]
async fn unbuildable_requests_are_refused_with_their_reason_not_accepted() {
    let g = start().await;
    let (s, b) = g.post("/sandbox", json!({"linkedSandbox": "x"})).await;
    assert_eq!(s, 400);
    assert!(
        b["message"]
            .as_str()
            .unwrap()
            .contains("separate guest networks"),
        "{b}"
    );
    let id = g.create(json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // resize goes to the core: the mock accepts it and the DTO shows the new shape
    let path = format!("/sandbox/{id}/resize");
    let (s, b) = g.post(&path, json!({"cpu": 4, "memory": 8})).await;
    assert_eq!(s, 200, "{b}");
    assert_eq!(
        (b["cpu"].as_u64(), b["memory"].as_u64(), b["state"].as_str()),
        (Some(4), Some(8), Some("started"))
    );
    assert_eq!(
        g.mock.resized.lock().await.last().unwrap().1.memory_mb,
        Some(8192)
    );
    assert_eq!(
        g.post(&path, json!({"cpu": 1})).await.0,
        400,
        "no hot shrink"
    );
    assert_eq!(
        g.post(&path, json!({"disk": 50})).await.0,
        400,
        "disk only when stopped"
    );
    assert_eq!(g.post(&path, json!({})).await.0, 400);
    g.post(&format!("/sandbox/{id}/stop"), json!({})).await;
    assert_eq!(g.post(&path, json!({"disk": 50})).await.0, 200);
    g.post(&format!("/sandbox/{id}/start"), json!({})).await;
    // SSH needs a running sshd; the mock guest has none.
    let (s, b) = g
        .post(
            &format!("/sandbox/{id}/ssh-access?expiresInMinutes=5"),
            json!({}),
        )
        .await;
    assert_eq!(s, 400, "{b}");
    assert!(b["message"].as_str().unwrap().contains("sshd"), "{b}");
    assert_eq!(
        g.get("/sandbox/ssh-access/validate?token=nope").await.1["valid"],
        false
    );
    assert_eq!(
        g.post(
            &format!("/sandbox/{id}/ssh-access?expiresInMinutes=0"),
            json!({})
        )
        .await
        .0,
        400
    );
    // a registry image string is a bare FROM, not a Dockerfile build
    let sb = g.create(json!({"buildInfo": {"dockerfileContent": "FROM python:3.9.23-slim\n"}, "cpu": 1, "memory": 1, "disk": 3})).await;
    assert_eq!(sb["state"], "started");
    assert_eq!(sb["disk"], 3);
}

/// An account at its live-session limit gets Daytona's rate-limit error
/// (HTTP 429), with the plan-limit sentence and its upgrade path verbatim.
#[tokio::test]
async fn the_live_session_limit_is_a_429_with_its_sentence() {
    let g = start().await;
    *g.mock.create_fails.lock().await = Some(cua_compat::backend::BackendError::RateLimited(
        "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro.".into(),
    ));
    let (s, b) = g.post("/sandbox", json!({"name": "over"})).await;
    assert_eq!(s, 429, "{b}");
    assert!(
        b.to_string().contains(
            "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro."
        ),
        "{b}"
    );
}
