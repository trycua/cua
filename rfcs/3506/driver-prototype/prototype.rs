//! RFC-only focus-bound experiment. Not a target-bound input capability.
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub owner: String,
    pub pid: u32,
    pub token: u64,
    pub generation: String,
    pub internal_id: String,
    pub deadline_ns: u64,
    pub op_seq: u64,
    pub key: String,
    #[serde(default)] pub precheck_gate: Option<String>,
    #[serde(default)] pub postcheck_gate: Option<String>,
    #[serde(default)] pub drop_ack: bool,
}

pub struct Operation {
    pub expected: Expected,
    closed: AtomicBool,
    pub emission_lock: Mutex<()>,
    consumed: AtomicBool,
    pub may_start: AtomicBool,
    pub submission_ns: AtomicU64,
    pub confirmation_ns: AtomicU64,
    pub emission_ns: AtomicU64,
    pub worker_ack_ns: AtomicU64,
}
// ponytail: one operation per process, no reconnect/rearm; separate process per experiment.
static ARMED: Mutex<Option<Arc<Operation>>> = Mutex::new(None);
pub fn mono_ns() -> u64 {
    let mut t: libc::timespec = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) }, 0);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}
fn uuid(s: &str) -> bool {
    // QString QUuid rendering uses braces; compare original strings verbatim.
    let s = if s.starts_with('{') && s.ends_with('}') { &s[1..s.len()-1] } else { s };
    s.len() == 36 && s.bytes().enumerate().all(|(i,b)|
        if [8,13,18,23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() })
}
pub fn arm(expected: Expected) -> anyhow::Result<Arc<Operation>> {
    anyhow::ensure!(expected.owner.starts_with(':') && expected.pid > 0 && expected.token > 0
        && uuid(&expected.generation) && uuid(&expected.internal_id) && expected.op_seq > 0,
        "invalid expected identity");
    // Only a single unmodified evdev key, no chord or alternate input adapter.
    anyhow::ensure!(super::key_to_evdev(&expected.key).is_some(), "unsupported single key");
    let mut armed = ARMED.lock().unwrap();
    anyhow::ensure!(armed.is_none(), "operation already armed; never reconnect/resend");
    let op = Arc::new(Operation { expected, closed: AtomicBool::new(false),
        emission_lock: Mutex::new(()), consumed: AtomicBool::new(false), may_start: AtomicBool::new(false),
        submission_ns: AtomicU64::new(0), confirmation_ns: AtomicU64::new(0), emission_ns: AtomicU64::new(0), worker_ack_ns: AtomicU64::new(0) });
    *armed = Some(op.clone());
    Ok(op)
}
impl Operation {
    pub fn close(&self) {
        let _guard = self.emission_lock.lock().unwrap();
        self.closed.store(true, Ordering::SeqCst);
    }
    pub fn live(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.closed.load(Ordering::SeqCst), "closed");
        anyhow::ensure!(mono_ns() < self.expected.deadline_ns, "expired");
        Ok(())
    }
    pub fn check(&self) -> anyhow::Result<()> {
        self.live()?;
        let (owner, snapshot) = identity_snapshot().ok_or_else(|| anyhow::anyhow!("untrusted helper or stale snapshot"))?;
        validate(&self.expected, &owner, &snapshot)?;
        self.confirmation_ns.store(mono_ns(), Ordering::SeqCst);
        self.live()
    }
    pub fn gate(&self, path: &Option<String>) -> anyhow::Result<()> {
        if let Some(path) = path {
            std::fs::write(format!("{path}.reached"), mono_ns().to_string())?;
            while !std::path::Path::new(&format!("{path}.release")).exists() {
                self.live()?;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        Ok(())
    }
    pub fn report(&self) -> Value {
        json!({"op_seq":self.expected.op_seq,"may_start":self.may_start.load(Ordering::SeqCst),
            "submission_ns":self.submission_ns.load(Ordering::SeqCst),
            "confirmation_ns":self.confirmation_ns.load(Ordering::SeqCst),
            "emission_ns":self.emission_ns.load(Ordering::SeqCst),
            "worker_ack_ns":self.worker_ack_ns.load(Ordering::SeqCst)})
    }
}
fn validate(e: &Expected, owner: &str, s: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(owner == e.owner && s["generation"].as_str() == Some(&e.generation), "owner/generation changed");
    let ws = s["windows"].as_array().ok_or_else(|| anyhow::anyhow!("invalid windows"))?;
    // Reuse v1 parser's duplicate-token, geometry and single-active checks.
    anyhow::ensure!(super::kwin_helper::parse_snapshot(&serde_json::to_string(ws)?).is_some(), "invalid v1 snapshot");
    let mut ids = std::collections::HashSet::new();
    anyhow::ensure!(ws.iter().all(|w| w["internal_id"].as_str().is_some_and(|id| uuid(id) && ids.insert(id))), "invalid UUIDs");
    anyhow::ensure!(ws.iter().any(|w| w["pid"].as_u64() == Some(e.pid as u64)
        && w["token"].as_u64() == Some(e.token) && w["internal_id"].as_str() == Some(&e.internal_id)
        && w["active"] == true && w["minimized"] == false), "closed/stale/inactive target");
    Ok(())
}
/// Fresh call to a verified unique KWin owner; no snapshot cache or retry.
pub fn identity_snapshot() -> Option<(String, Value)> {
    let owner = super::kwin_helper::helper_owner()?;
    let destination = owner.clone();
    let s = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                let c = zbus::Connection::session().await.ok()?;
                let p = zbus::Proxy::new(&c, destination.as_str(), "/org/cua/KWinTarget", "org.cua.KWinTarget").await.ok()?;
                let raw: String = p.call("GetIdentitySnapshot", &()).await.ok()?;
                serde_json::from_str::<Value>(&raw).ok()
            }).await.ok().flatten()
        })
    }).join().ok().flatten()?;
    (super::kwin_helper::helper_owner()? == owner).then_some((owner, s))
}
pub async fn admitted(args: &Value) -> Option<cua_driver_core::protocol::ToolResult> {
    use cua_driver_core::protocol::ToolResult;
    let op = ARMED.lock().unwrap().clone()?;
    // Explicit foreground only; exact/background and every other tool stay unchanged.
    if args["delivery_mode"] != "foreground" { return None; }
    let e = &op.expected;
    let shape = args["pid"].as_u64() == Some(e.pid as u64) && args["window_id"].as_u64() == Some(e.token)
        && args["key"].as_str() == Some(e.key.as_str())
        && args.get("modifiers").is_none_or(|v| v.as_array().is_some_and(|v| v.is_empty()))
        && ["x","y","element_index","element_token","snapshot_id"].iter().all(|k| args.get(*k).is_none());
    if !shape { return Some(ToolResult::error("prototype accepts only armed single-key foreground target")); }
    if op.consumed.swap(true, Ordering::SeqCst) {
        return Some(ToolResult::error("operation consumed; never retry"));
    }
    op.submission_ns.store(mono_ns(), Ordering::SeqCst);
    let task_op = op.clone();
    let result = tokio::task::spawn_blocking(move || {
        task_op.live()?;
        super::libei::guarded_press_key(task_op)
    }).await;
    let result = match result { Ok(Ok(())) => ToolResult::text("prototype emission flushed; application effect unverifiable"),
        other => ToolResult::error(format!("prototype {}: {other:?}", if op.may_start.load(Ordering::SeqCst) {"unknown; never retry"} else {"rejected before emission"})) };
    let mut report = op.report();
    report["execution_state"] = json!(if op.may_start.load(Ordering::SeqCst) { "unknown" } else { "not_started" });
    if result.is_error != Some(true) {
        report["effect"] = json!("unverifiable");
        report["path"] = json!("wayland_focused");
    }
    Some(result.with_structured(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn expired_admitted_operation_is_consumed_without_starting_input() {
        let e = Expected { owner: ":1.8".into(), pid:123, token:7,
            generation:"{00000000-0000-0000-0000-000000000001}".into(),
            internal_id:"{00000000-0000-0000-0000-000000000002}".into(),
            deadline_ns:1, op_seq:1, key:"a".into(), precheck_gate:None, postcheck_gate:None, drop_ack:false };
        let args=json!({"pid":123,"window_id":7,"key":"a","delivery_mode":"foreground"});
        let op=arm(e).unwrap();
        assert!(admitted(&json!({"delivery_mode":"background"})).await.is_none());
        let first=admitted(&args).await.unwrap();
        assert_eq!(first.is_error,Some(true));
        assert_eq!(first.structured_content.unwrap()["execution_state"],"not_started");
        assert!(!op.may_start.load(Ordering::SeqCst));
        assert_eq!(op.emission_ns.load(Ordering::SeqCst),0);
        let second=admitted(&args).await.unwrap();
        assert_eq!(second.is_error,Some(true));
        op.close();
        assert!(op.live().unwrap_err().to_string().contains("closed"));
    }
    #[test]
    fn guard_rejects_stale_closed_inactive_and_uuid_reuse() {
        let e = Expected { owner: ":1.2".into(), pid: 123, token: 7,
            generation: "00000000-0000-0000-0000-000000000001".into(),
            internal_id: "00000000-0000-0000-0000-000000000002".into(), deadline_ns: 1, op_seq: 1,
            key: "a".into(), precheck_gate: None, postcheck_gate: None, drop_ack: false };
        let s = json!({"generation":e.generation,"windows":[{"pid":123,"token":7,"internal_id":e.internal_id,
            "active":true,"minimized":false,"x":0,"y":0,"w":1,"h":1,"stacking":0}]});
        assert!(validate(&e, ":1.2", &s).is_ok());
        assert!(validate(&e, ":1.3", &s).is_err());
        for (key, value) in [("active",json!(false)),("internal_id",json!("00000000-0000-0000-0000-000000000003")),("pid",json!(124)),("token",json!(8))] {
            let mut bad = s.clone(); bad["windows"][0][key] = value; assert!(validate(&e, ":1.2", &bad).is_err());
        }
        let mut bad = s.clone(); bad["generation"] = json!("00000000-0000-0000-0000-000000000003");
        assert!(validate(&e, ":1.2", &bad).is_err());
        bad["windows"] = json!([]); assert!(validate(&e, ":1.2", &bad).is_err());
        let op = Operation { expected:e, closed:AtomicBool::new(false), emission_lock:Mutex::new(()), consumed:AtomicBool::new(false),
            may_start:AtomicBool::new(false), submission_ns:AtomicU64::new(0), confirmation_ns:AtomicU64::new(0), emission_ns:AtomicU64::new(0), worker_ack_ns:AtomicU64::new(0) };
        assert!(op.live().is_err());
        op.closed.store(true, Ordering::SeqCst); assert!(op.live().unwrap_err().to_string().contains("closed"));
    }
}
