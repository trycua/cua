//! Daytona computer use at the wire over the mock desktop: each route's
//! request reaches the Space's computer service as the action it names, and
//! the answers have the shapes the SDK models read.

use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use serde_json::{Value, json};
use std::sync::Arc;

struct Gw {
    gw: Gateway,
    mock: Arc<MockBackend>,
    http: reqwest::Client,
    api: String,
    id: String,
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
    let http = reqwest::Client::new();
    let sb: Value = http
        .post(format!("{api}/sandbox"))
        .bearer_auth(&gw.api_key)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    Gw {
        id: sb["id"].as_str().unwrap().into(),
        gw,
        mock,
        http,
        api,
        _dir: dir,
    }
}

impl Gw {
    async fn cu(&self, m: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self
            .http
            .request(
                m,
                format!("{}/toolbox/{}/computeruse{path}", self.api, self.id),
            )
            .bearer_auth(&self.gw.api_key);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let s = resp.status().as_u16();
        let t = resp.text().await.unwrap();
        (s, serde_json::from_str(&t).unwrap_or(Value::String(t)))
    }
    async fn observed(&self) -> (Vec<String>, Vec<String>) {
        let name = self.mock.created.lock().await[0].name.clone();
        let st = self.mock.state(&name).await.unwrap();
        let p = st.observed.pointer.lock().unwrap().clone();
        let k = st.observed.keyboard.lock().unwrap().clone();
        (p, k)
    }
}

#[tokio::test]
async fn pointer_and_keyboard_requests_become_computer_service_actions() {
    let g = start().await;
    let (s, _) = g
        .cu(
            reqwest::Method::POST,
            "/mouse/click",
            Some(
                json!({"x": 10, "y": 20, "button": "right", "double": true, "modifiers": ["ctrl"]}),
            ),
        )
        .await;
    assert_eq!(s, 200);
    let (s, p) = g
        .cu(
            reqwest::Method::POST,
            "/mouse/move",
            Some(json!({"x": 5, "y": 6})),
        )
        .await;
    assert_eq!(
        (s, p["x"].as_i64(), p["y"].as_i64()),
        (200, Some(5), Some(6))
    );
    let (s, _) = g
        .cu(
            reqwest::Method::POST,
            "/mouse/drag",
            Some(json!({"startX": 1, "startY": 2, "endX": 30, "endY": 40})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, sc) = g
        .cu(
            reqwest::Method::POST,
            "/mouse/scroll",
            Some(json!({"x": 1, "y": 1, "direction": "up", "amount": 4})),
        )
        .await;
    assert_eq!((s, sc["success"].as_bool()), (200, Some(true)));
    g.cu(
        reqwest::Method::POST,
        "/keyboard/type",
        Some(json!({"text": "hi", "delay": 5})),
    )
    .await;
    g.cu(
        reqwest::Method::POST,
        "/keyboard/key",
        Some(json!({"key": "Return", "modifiers": ["shift"]})),
    )
    .await;
    g.cu(
        reqwest::Method::POST,
        "/keyboard/hotkey",
        Some(json!({"keys": "ctrl+c"})),
    )
    .await;
    let (ptr, kb) = g.observed().await;
    assert!(
        ptr[0].contains("Click") && ptr[0].contains("count: 2") && ptr[0].contains("button: Right"),
        "{ptr:?}"
    );
    assert!(
        ptr.iter().any(|a| a.contains("Drag")) && ptr.iter().any(|a| a.contains("delta_y: -4.0")),
        "{ptr:?}"
    );
    assert!(
        kb[0].contains("Type") && kb[1].contains("Press") && kb[2].contains("Hotkey"),
        "{kb:?}"
    );
    // bad input is the client's error, not the guest's
    assert_eq!(
        g.cu(
            reqwest::Method::POST,
            "/mouse/click",
            Some(json!({"x": 1, "y": 1, "button": "laser"}))
        )
        .await
        .0,
        400
    );
    assert_eq!(
        g.cu(
            reqwest::Method::POST,
            "/keyboard/key",
            Some(json!({"key": "no-such"}))
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn screenshots_ask_for_the_requested_region_format_and_scale() {
    let g = start().await;
    let (s, b) = g
        .cu(
            reqwest::Method::GET,
            "/screenshot/region?x=10&y=20&width=300&height=200&showCursor=true",
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert!(b["screenshot"].as_str().unwrap().len() > 10 && b["sizeBytes"].as_u64().unwrap() > 0);
    let (s, _) = g
        .cu(
            reqwest::Method::GET,
            "/screenshot/compressed?format=jpeg&quality=70&scale=0.5",
            None,
        )
        .await;
    assert_eq!(s, 200);
    let name = g.mock.created.lock().await[0].name.clone();
    let reqs = g
        .mock
        .state(&name)
        .await
        .unwrap()
        .observed
        .screenshots
        .lock()
        .unwrap()
        .clone();
    let region = reqs[0].region.as_ref().unwrap();
    assert_eq!((region.x, region.width), (10.0, 300.0));
    assert!(reqs[0].include_cursor);
    assert_eq!(reqs[1].format, 2, "jpeg");
    assert_eq!(reqs[1].quality, 70);
    assert_eq!(reqs[1].max_dimension, 640, "half of the 1280 px long edge");
    assert_eq!(
        g.cu(
            reqwest::Method::GET,
            "/screenshot/compressed?format=bmp",
            None
        )
        .await
        .0,
        400
    );
    assert_eq!(
        g.cu(reqwest::Method::GET, "/screenshot/compressed?scale=2", None)
            .await
            .0,
        400
    );
    assert_eq!(
        g.cu(reqwest::Method::GET, "/screenshot/region?x=1", None)
            .await
            .0,
        400
    );
}

#[tokio::test]
async fn displays_windows_and_the_desktop_processes() {
    let g = start().await;
    let (_, d) = g.cu(reqwest::Method::GET, "/display/info", None).await;
    assert_eq!(
        (
            d["displays"][0]["width"].as_i64(),
            d["displays"][0]["isActive"].as_bool()
        ),
        (Some(1280), Some(true))
    );
    let (s, w) = g.cu(reqwest::Method::GET, "/display/windows", None).await;
    assert_eq!(s, 200);
    assert!(w["windows"].is_array());
    let (_, st) = g.cu(reqwest::Method::GET, "/status", None).await;
    assert_eq!(st["status"], "active");
    let (_, p) = g.cu(reqwest::Method::POST, "/start", None).await;
    assert_eq!(p["status"]["xvfb"]["running"], true);
    let (s, e) = g
        .cu(reqwest::Method::GET, "/process/nope/status", None)
        .await;
    assert_eq!((s, e["code"].as_str()), (404, Some("PROCESS_NOT_FOUND")));
}

#[tokio::test]
async fn accessibility_nodes_are_addressed_by_snapshot_and_element() {
    let g = start().await;
    let (s, t) = g.cu(reqwest::Method::GET, "/a11y/tree", None).await;
    assert!(s == 200 || s == 503, "{s} {t}");
    let (s, e) = g
        .cu(
            reqwest::Method::POST,
            "/a11y/node/focus",
            Some(json!({"id": "garbage"})),
        )
        .await;
    assert_eq!(s, 404, "{e}");
}
