//! An in-memory fake of the Fleet control plane, plugged in through
//! `cyclops-sdk`'s [`HttpClient`] trait (feature `testing`).
//!
//! It serves namespaces, warm pools, templates and claims with Kubernetes
//! CRUD semantics (POST/GET/PATCH merge/DELETE and list), binds every claim
//! on first read, answers the `/api/svc` proxy with a canned response, serves
//! namespace listing, image resources, user API keys and write-only
//! `cua-claim-*` Secrets (as the gateway admits them), and records every
//! request.
//!
//! It also models what the auto pool manager depends on:
//!
//! - **tenants**: the caller's tenant is the `sub` of its bearer JWT
//!   (`"default"` for opaque tokens). Namespaces belong to the tenant that
//!   created them; `GET /api/namespaces` is tenant-scoped, a second
//!   `POST /api/namespaces` answers 409 whoever owns the name, and any
//!   access to another tenant's namespace answers 403 (Capsule);
//! - a **clock** ([`FakeFleet::advance`]) used for creation timestamps;
//! - the **operator**: a claim TTL sets `spec.lifecycle.shutdownTime`, and
//!   [`FakeFleet::reap`] deletes Bound claims past it and pools past their
//!   `ttlSecondsAfterCreated` (Pending/Failed claims are never reaped);
//! - **KEDA** ([`Faults::keda`]): claims bind only while the pool has
//!   replicas, and a Pending claim scales `spec.replicas` up to the claim
//!   count (as KEDA writes `/scale`).
//!
//! Creating a fake also installs an offline [`crate::ImageInspector`]: the
//! runtime rule sees only images registered with [`set_image_variant`], and
//! every other image as unreadable (explicit runtimes only), so tests never
//! reach a real registry.

use crate::runtime::{ImageEvidence, ImageInspector, ImageVariant};
use crate::{FleetClient, FleetConfig};
use base64::Engine as _;
use cyclops_sdk::{HttpClient, HttpError, HttpHeader, HttpRequest, HttpResponse};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Recorded request.
#[derive(Clone, Debug)]
pub struct Recorded {
    /// Method.
    pub method: String,
    /// Path (no origin).
    pub path: String,
    /// Headers.
    pub headers: Vec<HttpHeader>,
    /// JSON body, when any.
    pub body: Option<Value>,
}

impl Recorded {
    /// Header value (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
}

/// Fault injection and canned behaviour.
#[derive(Clone, Debug, Default)]
pub struct Faults {
    /// Template creation fails with this HTTP status.
    pub template_create_status: Option<u16>,
    /// Claims stay Pending for this many reads before binding.
    pub pending_reads: u32,
    /// Service proxy status (default 200).
    pub service_status: Option<u16>,
    /// Pool creation in a namespace created within this many pool POSTs
    /// answers 403 (Capsule adoption lag).
    pub adoption_denials: u32,
    /// KEDA mode: claims bind only when the pool has replicas; reading a
    /// Pending claim scales `spec.replicas` to the live claim count.
    pub keda: bool,
    /// Claims never bind (stay Pending).
    pub never_bind: bool,
    /// Claims fail (`Failed`, `BindDeadlineExceeded`) instead of binding.
    pub fail_bind: bool,
    /// Reads of resources in this namespace answer 403 (a namespace being
    /// deleted still lists but is no longer readable).
    pub forbid_reads_in: Option<String>,
    /// Pool POSTs answer 409 this many times even when absent (a racing
    /// creator), after inserting the pool on behalf of `race_tenant`.
    pub race_pool_create: u32,
    /// Image builds (`kind: container`) stay `Building` for this many reads
    /// before they are `Ready`.
    pub build_reads: u32,
    /// Image builds fail instead of finishing.
    pub fail_build: bool,
    /// The images API is absent (every image route answers 404), as on a
    /// Fleet without it.
    pub no_images_api: bool,
    /// `GET /api/config`'s `usage_pricing` (`vcpu_hour_usd`,
    /// `memory_gib_hour_usd`); `None` answers without rates.
    pub usage_pricing: Option<(f64, f64)>,
    /// Fleet's per-account size admission (trycua/cloud#7928): template
    /// writes asking for more than (vCPUs, MiB) answer 403 with
    /// [`SIZE_LIMIT_MESSAGE`]. `None`: the account is exempt (or the flag
    /// is off).
    pub size_cap: Option<(u32, u32)>,
    /// `GET /api/billing/status`'s body; `None` answers 404 (a Fleet
    /// without the route). [`FakeFleet::complete_checkout`] saves a card.
    pub billing: Option<Value>,
    /// The account is out of credit: new claims answer HTTP 402
    /// `cloud_credit_exhausted` with this billing URL.
    pub credit_exhausted: Option<String>,
}

/// The 403 message of Fleet's size admission (`PoolSizeLimitMessage`,
/// trycua/cloud#7928).
pub const SIZE_LIMIT_MESSAGE: &str = "sandbox size is over the Fleet limits: \
     vmTemplate.cpuCores must be a whole number from 1 to 8, vmTemplate.memory a \
     Kubernetes quantity from 512Mi to 32Gi (e.g. 8Gi), and vmTemplate.sidecars may \
     request at most 2 vCPU and 4Gi of memory in total. Contact support@trycua.com to \
     raise the limits for your account.";

/// Fleet's size admission over a template's `vmTemplate` (fields that are
/// absent are not checked, as on Fleet).
fn over_size_cap(vm: &Value, cap: Option<(u32, u32)>) -> bool {
    let Some((cpu, mem)) = cap else {
        return false;
    };
    let cores = vm["cpuCores"].as_u64();
    let memory = vm["memory"]
        .as_str()
        .and_then(crate::spec::parse_memory_mib);
    cores.is_some_and(|c| c > u64::from(cpu)) || memory.is_some_and(|m| m > mem)
}

#[derive(Clone, Debug)]
struct Ns {
    owner: String,
    created: i64,
    adoption_denials: u32,
}

impl Ns {
    /// Namespaces seeded without a tenant ([`DEFAULT_TENANT`]) are shared,
    /// so single-tenant tests work with any bearer.
    fn visible_to(&self, tenant: &str) -> bool {
        self.owner == tenant || self.owner == DEFAULT_TENANT
    }
}

#[derive(Default)]
struct Store {
    namespaces: BTreeMap<String, Ns>,
    now: i64,
    /// (kind, namespace, name) → resource
    objects: BTreeMap<(String, String, String), Value>,
    claim_reads: BTreeMap<String, u32>,
    /// id → user API key.
    user_keys: BTreeMap<String, Value>,
    /// Signed service URLs, in creation order.
    signed_urls: Vec<Value>,
    requests: Vec<Recorded>,
}

/// The fake Fleet API.
#[derive(Clone)]
pub struct FakeFleet {
    store: Arc<Mutex<Store>>,
    /// Faults.
    pub faults: Arc<Mutex<Faults>>,
}

const BASE: &str = "https://fleet.test";
const K8S: &str = "/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/";
const IMAGES: &str = "/api/k8s/apis/images.cua.ai/v1alpha1/namespaces/";
/// Core Secrets (claim secrets, trycua/cloud#7885). Stored as kind "secret".
const SECRETS: &str = "/api/k8s/api/v1/namespaces/";

/// Tenant of a caller that sends an opaque (non-JWT) token.
pub const DEFAULT_TENANT: &str = "default";

fn rfc3339(t: i64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(t.max(0) as u64)).to_string()
}

fn parse_time(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    humantime::parse_rfc3339_weak(s)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

fn namespace_json(name: &str, ns: &Ns) -> Value {
    json!({"name": name, "status": "Active", "createdAt": rfc3339(ns.created),
        "labels": {"capsule.clastix.io/tenant": ns.owner}})
}

/// A fake JWT (unsigned) whose `sub` is `tenant`.
pub fn fake_jwt(tenant: &str) -> String {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.sig",
        b64.encode(br#"{"alg":"none"}"#),
        b64.encode(json!({"sub": tenant}).to_string())
    )
}

fn tenant_of(headers: &[HttpHeader]) -> String {
    let token = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("authorization"))
        .and_then(|h| h.value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let mut parts = token.split('.');
    let payload = match (parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(p), Some(_)) => p,
        _ => return DEFAULT_TENANT.into(),
    };
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["sub"].as_str().map(str::to_string))
        .unwrap_or_else(|| DEFAULT_TENANT.into())
}

fn live_claims(s: &Store, ns: &str) -> u32 {
    s.objects
        .keys()
        .filter(|(k, n, _)| k == "claim" && n == ns)
        .count() as u32
}

/// The sidecar rules of Fleet's template admission (trycua/cloud#7893):
/// at most 8, unique DNS-label names other than `main`, an image each, and
/// with sidecars the service names `main`, `sidecars` and `sc` are
/// reserved. `None` when admitted.
fn template_admission(vm: &Value) -> Option<&'static str> {
    let sidecars = vm["sidecars"].as_array().filter(|a| !a.is_empty())?;
    let names: Vec<&str> = sidecars.iter().filter_map(|s| s["name"].as_str()).collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    if sidecars.len() > crate::MAX_SIDECARS
        || names.len() != sidecars.len()
        || unique.len() != names.len()
        || names
            .iter()
            .any(|n| *n == "main" || cyclops_sdk::validate_dns_label(n).is_err())
        || sidecars
            .iter()
            .any(|s| s["image"].as_str().is_none_or(str::is_empty))
    {
        return Some("vmTemplate.sidecars[].name must be a unique DNS label other than main");
    }
    let reserved = vm["services"].as_array().is_some_and(|svcs| {
        svcs.iter().any(|s| {
            s["name"]
                .as_str()
                .is_some_and(|n| crate::RESERVED_SERVICE_NAMES.contains(&n))
        })
    });
    reserved.then_some(
        "vmTemplate.services names main, sidecars and sc are reserved when vmTemplate.sidecars is set",
    )
}

fn resp(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body: serde_json::to_vec(&body).unwrap(),
    }
}

fn merge(target: &mut Value, patch: &Value) {
    if let Value::Object(p) = patch {
        if !target.is_object() {
            *target = json!({});
        }
        let t = target.as_object_mut().unwrap();
        for (k, v) in p {
            if v.is_null() {
                t.remove(k);
            } else {
                merge(t.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
    } else {
        *target = patch.clone();
    }
}

/// The offline image fixtures [`FakeFleet`] installs.
#[derive(Default)]
struct FakeImages(Mutex<BTreeMap<String, ImageVariant>>);

fn fake_images() -> Arc<FakeImages> {
    static IMAGES: std::sync::OnceLock<Arc<FakeImages>> = std::sync::OnceLock::new();
    IMAGES.get_or_init(Arc::default).clone()
}

#[async_trait::async_trait]
impl ImageInspector for FakeImages {
    async fn inspect(&self, image: &str) -> ImageEvidence {
        match self.0.lock().unwrap().get(image) {
            Some(v) => ImageEvidence::Known(*v),
            None => ImageEvidence::Unavailable(format!(
                "no image fixture for {image:?} (cua_fleet::testing::set_image_variant)"
            )),
        }
    }
}

/// Installs the offline image fixtures as the process-wide inspector.
pub fn install_image_fixtures() {
    crate::runtime::set_image_inspector(Some(fake_images()));
}

/// Registers what `image` is for the runtime rule (process-wide) and installs
/// the offline fixtures.
pub fn set_image_variant(image: &str, variant: ImageVariant) {
    fake_images()
        .0
        .lock()
        .unwrap()
        .insert(image.to_string(), variant);
    install_image_fixtures();
}

impl Default for FakeFleet {
    fn default() -> Self {
        install_image_fixtures();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self {
            store: Arc::new(Mutex::new(Store {
                now,
                ..Store::default()
            })),
            faults: Arc::default(),
        }
    }
}

impl FakeFleet {
    /// A card saved on the website billing page (Stripe's
    /// `setup_intent.succeeded` webhook, as Fleet applies it): the billing
    /// status now has a default card (pay as you go).
    pub fn complete_checkout(&self, brand: &str, last4: &str) {
        let mut f = self.faults.lock().unwrap();
        if let Some(b) = f.billing.as_mut() {
            b["payment_method_present"] = json!(true);
            b["card"] = json!({"brand": brand, "last4": last4, "exp_month": 12, "exp_year": 2030});
            b["plan"] = json!("payg");
        }
    }

    /// A new, empty fake. Its clock starts at the real time.
    pub fn new() -> Self {
        Self::default()
    }

    /// A [`FleetClient`] wired to this fake with a static token (tenant
    /// [`DEFAULT_TENANT`]).
    pub fn client(&self) -> FleetClient {
        self.client_with_base(BASE)
    }

    /// A client whose bearer is a JWT with `sub = tenant`.
    pub fn client_for_tenant(&self, tenant: &str) -> FleetClient {
        self.client_with(BASE, fake_jwt(tenant))
    }

    /// Same, with a different base URL (for example a local mock env
    /// server that emulates the gateway's `/api/svc`).
    pub fn client_with_base(&self, base: &str) -> FleetClient {
        self.client_with(base, "fake-fleet-token".into())
    }

    fn client_with(&self, base: &str, token: String) -> FleetClient {
        let config = FleetConfig {
            base_url: base.into(),
            fleet_token: Some(token),
            pool_poll_interval_ms: 1,
            claim_poll_interval_ms: 1,
            claim_poll_limit: 50,
            ..FleetConfig::default()
        };
        FleetClient::connect_with_http_client(config, Arc::new(self.clone())).unwrap()
    }

    /// All recorded requests.
    pub fn requests(&self) -> Vec<Recorded> {
        self.store.lock().unwrap().requests.clone()
    }

    /// Whether an object exists.
    pub fn exists(&self, kind: &str, namespace: &str, name: &str) -> bool {
        self.store.lock().unwrap().objects.contains_key(&(
            kind.into(),
            namespace.into(),
            name.into(),
        ))
    }

    /// An object's JSON.
    pub fn object(&self, kind: &str, namespace: &str, name: &str) -> Option<Value> {
        self.store
            .lock()
            .unwrap()
            .objects
            .get(&(kind.into(), namespace.into(), name.into()))
            .cloned()
    }

    /// Whether the namespace exists.
    pub fn namespace_exists(&self, ns: &str) -> bool {
        self.store.lock().unwrap().namespaces.contains_key(ns)
    }

    /// The tenant owning a namespace.
    pub fn namespace_owner(&self, ns: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .namespaces
            .get(ns)
            .map(|n| n.owner.clone())
    }

    /// Seeds a namespace owned by [`DEFAULT_TENANT`] (visible to every
    /// tenant).
    pub fn add_namespace(&self, ns: &str) {
        self.add_namespace_for(ns, DEFAULT_TENANT);
    }

    /// Seeds a namespace owned by `tenant` (another account squatting a
    /// name, for example).
    pub fn add_namespace_for(&self, ns: &str, tenant: &str) {
        let mut s = self.store.lock().unwrap();
        let created = s.now;
        s.namespaces.entry(ns.into()).or_insert(Ns {
            owner: tenant.into(),
            created,
            adoption_denials: 0,
        });
    }

    /// Seeds an object (`kind`: `pool`, `template`, `claim` or `image`).
    /// A missing namespace is created for [`DEFAULT_TENANT`].
    pub fn put_object(&self, kind: &str, namespace: &str, name: &str, value: Value) {
        self.add_namespace(namespace);
        let mut s = self.store.lock().unwrap();
        s.objects
            .insert((kind.into(), namespace.into(), name.into()), value);
    }

    /// Mutates an object in place.
    pub fn update_object(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
        f: impl FnOnce(&mut Value),
    ) {
        let mut s = self.store.lock().unwrap();
        if let Some(o) = s
            .objects
            .get_mut(&(kind.into(), namespace.into(), name.into()))
        {
            f(o);
        }
    }

    /// Names of objects of `kind` in `namespace`.
    pub fn names(&self, kind: &str, namespace: &str) -> Vec<String> {
        self.store
            .lock()
            .unwrap()
            .objects
            .keys()
            .filter(|(k, n, _)| k == kind && n == namespace)
            .map(|(_, _, name)| name.clone())
            .collect()
    }

    /// Every namespace name (all tenants).
    pub fn all_namespaces(&self) -> Vec<String> {
        self.store
            .lock()
            .unwrap()
            .namespaces
            .keys()
            .cloned()
            .collect()
    }

    /// The fake clock (unix seconds).
    pub fn now(&self) -> i64 {
        self.store.lock().unwrap().now
    }

    /// Advances the fake clock.
    pub fn advance(&self, by: Duration) {
        self.store.lock().unwrap().now += by.as_secs() as i64;
    }

    /// A clock closure for code under test that must share this fake's time.
    pub fn clock(&self) -> Arc<dyn Fn() -> i64 + Send + Sync> {
        let store = Arc::clone(&self.store);
        Arc::new(move || store.lock().unwrap().now)
    }

    /// Simulates KEDA writing `/scale`: sets a pool's `spec.replicas` and
    /// status.
    pub fn scale_pool(&self, ns: &str, replicas: u32) {
        self.update_object("pool", ns, ns, |p| {
            p["spec"]["replicas"] = json!(replicas);
            p["status"] = json!({"replicas": replicas, "readyReplicas": replicas});
        });
    }

    /// One pool-operator reaper pass at the fake clock's time: deletes Bound
    /// claims whose `spec.lifecycle.shutdownTime` passed and pools whose
    /// `ttlSecondsAfterCreated` expired (namespace and template stay, as on
    /// Fleet). Returns the deleted `(kind, namespace, name)` keys.
    pub fn reap(&self) -> Vec<(String, String, String)> {
        let mut s = self.store.lock().unwrap();
        let now = s.now;
        let mut dead = vec![];
        for ((kind, ns, name), o) in &s.objects {
            let expired = match kind.as_str() {
                "claim" => {
                    o["status"]["phase"] == "Bound"
                        && parse_time(&o["spec"]["lifecycle"]["shutdownTime"])
                            .is_some_and(|t| t < now)
                }
                "pool" => match (
                    parse_time(&o["metadata"]["creationTimestamp"]),
                    o["spec"]["ttlSecondsAfterCreated"].as_i64(),
                ) {
                    (Some(c), Some(ttl)) => c + ttl < now,
                    _ => false,
                },
                _ => false,
            };
            if expired {
                dead.push((kind.clone(), ns.clone(), name.clone()));
            }
        }
        for k in &dead {
            s.objects.remove(k);
        }
        dead
    }

    /// Current user API keys.
    pub fn user_keys(&self) -> Vec<Value> {
        self.store
            .lock()
            .unwrap()
            .user_keys
            .values()
            .cloned()
            .collect()
    }

    fn handle(&self, method: &str, path: &str, body: Option<Value>, tenant: &str) -> HttpResponse {
        let faults = self.faults.lock().unwrap().clone();
        let mut s = self.store.lock().unwrap();
        if path == "/api/namespaces" && method == "POST" {
            let name = body
                .as_ref()
                .and_then(|b| b["name"].as_str())
                .unwrap_or_default()
                .to_string();
            if s.namespaces.contains_key(&name) {
                return resp(409, json!({"error": "exists"}));
            }
            let ns = Ns {
                owner: tenant.into(),
                created: s.now,
                adoption_denials: faults.adoption_denials,
            };
            let body = namespace_json(&name, &ns);
            s.namespaces.insert(name, ns);
            return resp(201, body);
        }
        if let Some(route) = path.strip_prefix("/api/billing/") {
            let Some(status) = faults.billing.clone() else {
                return resp(404, json!({"error": "not found"}));
            };
            return match (method, route) {
                ("GET", "status") => resp(200, status),
                _ => resp(404, json!({"error": "not found"})),
            };
        }
        if path == "/api/config" && method == "GET" {
            let mut body = json!({"admin": false, "billing": false, "chat": false, "usage": false});
            if let Some((vcpu, gib)) = faults.usage_pricing {
                body["usage_pricing"] = json!({"vcpu_hour_usd": vcpu, "memory_gib_hour_usd": gib});
            }
            return resp(200, body);
        }
        if path == "/api/namespaces" && method == "GET" {
            let items: Vec<Value> = s
                .namespaces
                .iter()
                .filter(|(_, n)| n.visible_to(tenant))
                .map(|(name, n)| namespace_json(name, n))
                .collect();
            return resp(200, Value::Array(items));
        }
        if path == "/api/user-keys" {
            return match method {
                "GET" => resp(
                    200,
                    json!({"keys": s.user_keys.values().cloned().collect::<Vec<_>>()}),
                ),
                "POST" => {
                    let body = body.unwrap_or(Value::Null);
                    let n = s.user_keys.len() + 1;
                    let id = format!("key-{n}");
                    let client_id = format!("ukey-fake{n}");
                    let key = json!({"id": id, "client_id": client_id,
                        "name": body["name"], "scope": body["scope"]});
                    s.user_keys.insert(id, key);
                    resp(
                        201,
                        json!({"client_id": client_id, "client_secret": format!("secret-{n}"),
                            "token_url": "https://auth.test/token", "name": body["name"],
                            "scope": body["scope"]}),
                    )
                }
                _ => resp(405, json!({})),
            };
        }
        if let Some(id) = path.strip_prefix("/api/user-keys/") {
            return match (method, s.user_keys.remove(id)) {
                ("DELETE", Some(_)) => HttpResponse {
                    status: 204,
                    headers: vec![],
                    body: vec![],
                },
                _ => resp(404, json!({"error": "not found"})),
            };
        }
        if path == "/api/image-uploads/presign" && method == "POST" {
            // Every object "already exists": no presigned PUT to follow.
            let b = body.unwrap_or(Value::Null);
            let ns = b["namespace"].as_str().unwrap_or_default();
            let tenant_hex = {
                use sha2::Digest as _;
                hex::encode(sha2::Sha256::digest(ns.as_bytes()))
            };
            let files: Vec<Value> = b["files"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|f| {
                    use sha2::Digest as _;
                    let digest = f["digest"].as_str().unwrap_or_default();
                    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .encode(sha2::Sha256::digest(digest.as_bytes()));
                    json!({"digest": digest, "sizeBytes": f["sizeBytes"],
                        "reference": format!("uploads/tenant-{}/{id}", &tenant_hex[..32]),
                        "upload": null})
                })
                .collect();
            return resp(200, json!({"files": files}));
        }
        if let Some(rest) = path.strip_prefix(IMAGES) {
            if faults.no_images_api {
                return resp(404, json!({"error": "no route"}));
            }
            let parts: Vec<&str> = rest.split('/').collect();
            if let ("GET", [ns, "images", name]) = (method, parts.as_slice()) {
                // The builder: a container recipe progresses on reads.
                let key = ("image".to_string(), ns.to_string(), name.to_string());
                let reads = s
                    .claim_reads
                    .entry(format!("image/{ns}/{name}"))
                    .or_insert(0);
                *reads += 1;
                let reads = *reads;
                if let Some(o) = s.objects.get_mut(&key)
                    && o["spec"]["recipe"]["kind"] == json!("container")
                    && o["status"]["phase"] != json!("Ready")
                    && o["status"]["phase"] != json!("Failed")
                {
                    if faults.fail_build {
                        o["status"] = json!({"phase": "Failed", "conditions": [{
                            "type": "Ready", "status": "False", "reason": "BuildFailed",
                            "message": "RUN exited 1", "lastTransitionTime": rfc3339(0)}]});
                    } else if reads > faults.build_reads {
                        use sha2::Digest as _;
                        let digest = format!(
                            "sha256:{}",
                            hex::encode(sha2::Sha256::digest(name.as_bytes()))
                        );
                        o["status"] = json!({"phase": "Ready", "artifacts": {"oci": {
                            "reference": format!("registry.fleet.test/builds/{ns}:rootfs-{name}"),
                            "digest": digest}}});
                    } else {
                        o["status"] = json!({"phase": "Building"});
                    }
                }
            }
            return match (method, parts.as_slice()) {
                ("GET", [ns, "images"]) => {
                    let items: Vec<Value> = s
                        .objects
                        .iter()
                        .filter(|((k, n, _), _)| k == "image" && n == ns)
                        .map(|(_, v)| v.clone())
                        .collect();
                    resp(200, json!({"items": items}))
                }
                ("POST", [ns, "images"]) => {
                    let mut obj = body.unwrap_or(Value::Null);
                    let name = obj["metadata"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let key = ("image".to_string(), ns.to_string(), name);
                    if s.objects.contains_key(&key) {
                        return resp(409, json!({"reason": "AlreadyExists"}));
                    }
                    if !s.namespaces.contains_key(*ns) {
                        return resp(404, json!({"error": "namespace missing"}));
                    }
                    if obj["spec"]["recipe"]["kind"] == json!("container") {
                        obj["status"] = json!({"phase": "Pending"});
                    }
                    s.objects.insert(key, obj.clone());
                    resp(201, obj)
                }
                ("GET", [ns, "images", name]) => {
                    match s
                        .objects
                        .get(&("image".into(), ns.to_string(), name.to_string()))
                    {
                        Some(o) => resp(200, o.clone()),
                        None => resp(404, json!({"error": "not found"})),
                    }
                }
                ("DELETE", [ns, "images", name]) => {
                    match s
                        .objects
                        .remove(&("image".into(), ns.to_string(), name.to_string()))
                    {
                        Some(_) => resp(200, json!({})),
                        None => resp(404, json!({"error": "not found"})),
                    }
                }
                _ => resp(404, json!({})),
            };
        }
        if let Some(rest) = path.strip_prefix(SECRETS) {
            // Write-only, `cua-claim-*` only, like the /api/k8s gateway.
            let parts: Vec<&str> = rest.split('/').collect();
            return match (method, parts.as_slice()) {
                ("POST", [ns, "secrets"]) => {
                    let obj = body.unwrap_or(Value::Null);
                    let name = obj["metadata"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let claim = name.starts_with("cua-claim-") && obj["type"] == json!("Opaque");
                    // #7887's tenant_secret_admission: exactly a
                    // dockerconfigjson holding only `.dockerconfigjson`,
                    // labeled `cua.ai/registry-secret: "true"`.
                    let registry = name.starts_with("cua-registry-")
                        && obj["type"] == json!("kubernetes.io/dockerconfigjson")
                        && obj["metadata"]["labels"]["cua.ai/registry-secret"] == json!("true")
                        && obj["data"]
                            .as_object()
                            .is_some_and(|d| d.len() == 1 && d.contains_key(".dockerconfigjson"))
                        && obj["metadata"].get("annotations").is_none()
                        && obj["metadata"].get("generateName").is_none()
                        && obj["metadata"]["namespace"]
                            .as_str()
                            .is_none_or(|n| n == *ns);
                    if !claim && !registry {
                        return resp(403, json!({"error": "secret admission"}));
                    }
                    let key = ("secret".to_string(), ns.to_string(), name);
                    if s.objects.contains_key(&key) {
                        return resp(409, json!({"error": "exists"}));
                    }
                    s.objects.insert(key, obj.clone());
                    resp(201, obj)
                }
                ("DELETE", [ns, "secrets", name])
                    if name.starts_with("cua-claim-") || name.starts_with("cua-registry-") =>
                {
                    match s
                        .objects
                        .remove(&("secret".into(), ns.to_string(), name.to_string()))
                    {
                        Some(_) => resp(200, json!({})),
                        None => resp(404, json!({"error": "not found"})),
                    }
                }
                _ => resp(403, json!({"error": "secrets are write-only"})),
            };
        }
        if let Some(ns) = path.strip_prefix("/api/namespaces/") {
            let owner = s.namespaces.get(ns).map(|n| n.owner.clone());
            return match (method, owner) {
                (_, Some(o)) if o != tenant && o != DEFAULT_TENANT => {
                    resp(403, json!({"error": "forbidden"}))
                }
                ("DELETE", Some(_)) => {
                    s.namespaces.remove(ns);
                    s.objects.retain(|(_, n, _), _| n != ns);
                    resp(200, json!({}))
                }
                ("DELETE", None) => resp(404, json!({})),
                (_, Some(_)) => {
                    let n = s.namespaces[ns].clone();
                    resp(200, namespace_json(ns, &n))
                }
                _ => resp(404, json!({})),
            };
        }
        if let Some(ns) = path.strip_prefix("/api/signed-service-urls/")
            && method == "POST"
        {
            let b = body.unwrap_or(Value::Null);
            let id = format!("ssu-{}", s.requests.len());
            let created = json!({"id": id, "namespace": ns, "claim": b["claim"], "sandbox": b["sandbox"],
                "logicalService": b["logicalService"], "label": b["label"],
                "url": format!("https://signed.fleet.test/{ns}/{}/{id}",
                    b["service"].as_str().unwrap_or_default()),
                "createdAt": rfc3339(s.now), "expiresAt": rfc3339(s.now + 3600),
                "revokedAt": null});
            s.signed_urls.push(created.clone());
            return resp(201, created);
        }
        if let Some(rest) = path.strip_prefix("/api/signed-service-urls/")
            && method == "DELETE"
            && let Some((ns, id)) = rest.split_once('/')
        {
            let now = rfc3339(s.now);
            return match s
                .signed_urls
                .iter_mut()
                .find(|u| u["namespace"] == ns && u["id"] == id)
            {
                Some(u) => {
                    u["revokedAt"] = json!(now);
                    resp(204, Value::Null)
                }
                None => resp(404, json!({"error": "signed service URL not found"})),
            };
        }
        if let Some(rest) = path.strip_prefix("/api/svc/") {
            let status = faults.service_status.unwrap_or(200);
            return HttpResponse {
                status,
                headers: vec![],
                body: format!("svc:{rest}").into_bytes(),
            };
        }
        let Some(rest) = path.strip_prefix(K8S) else {
            return resp(404, json!({"error": "no route"}));
        };
        let parts: Vec<&str> = rest.split('/').collect();
        let (ns, plural, name) = match parts.as_slice() {
            [ns, plural] => (*ns, *plural, None),
            [ns, plural, name] => (*ns, *plural, Some(*name)),
            _ => return resp(404, json!({})),
        };
        let kind = match plural {
            "osgymsandboxwarmpools" => "pool",
            "osgymsandboxtemplates" => "template",
            "osgymsandboxclaims" => "claim",
            _ => return resp(404, json!({})),
        };
        if method == "GET" && faults.forbid_reads_in.as_deref() == Some(ns) {
            return resp(403, json!({"reason": "Forbidden"}));
        }
        // Capsule: another tenant's namespace is forbidden.
        if let Some(n) = s.namespaces.get(ns)
            && !n.visible_to(tenant)
        {
            return resp(403, json!({"error": "forbidden by capsule"}));
        }
        let now = s.now;
        match (method, name) {
            ("GET", None) => {
                let items: Vec<Value> = s
                    .objects
                    .iter()
                    .filter(|((k, n, _), _)| k == kind && n == ns)
                    .map(|(_, v)| v.clone())
                    .collect();
                resp(200, json!({"items": items}))
            }
            ("POST", None) => {
                let mut obj = body.unwrap_or(Value::Null);
                let name = obj["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if kind == "template"
                    && let Some(code) = faults.template_create_status
                {
                    return resp(code, json!({"error": "template refused"}));
                }
                if kind == "template"
                    && let Some(reason) = template_admission(&obj["spec"]["vmTemplate"])
                {
                    return resp(403, json!({"error": reason}));
                }
                if kind == "template" && over_size_cap(&obj["spec"]["vmTemplate"], faults.size_cap)
                {
                    return resp(403, json!({"error": SIZE_LIMIT_MESSAGE}));
                }
                if !s.namespaces.contains_key(ns) {
                    return resp(404, json!({"error": "namespace missing"}));
                }
                if kind == "pool" {
                    let n = s.namespaces.get_mut(ns).unwrap();
                    if n.adoption_denials > 0 {
                        n.adoption_denials -= 1;
                        return resp(403, json!({"error": "namespace not adopted yet"}));
                    }
                }
                let key = (kind.to_string(), ns.to_string(), name);
                if kind == "pool" && faults.race_pool_create > 0 {
                    // Another process of the same tenant won the create.
                    let mut f = self.faults.lock().unwrap();
                    f.race_pool_create -= 1;
                    let mut other = obj.clone();
                    other["metadata"]["creationTimestamp"] = json!(rfc3339(now));
                    other["status"] = json!({"replicas": obj["spec"]["replicas"], "readyReplicas": obj["spec"]["replicas"]});
                    s.objects.entry(key).or_insert(other);
                    return resp(409, json!({"reason": "AlreadyExists"}));
                }
                if s.objects.contains_key(&key) {
                    return resp(409, json!({"reason": "AlreadyExists"}));
                }
                // Out of credit: Fleet refuses new claims (HTTP 402).
                if kind == "claim"
                    && let Some(url) = &faults.credit_exhausted
                {
                    return resp(
                        402,
                        json!({"code": "cloud_credit_exhausted", "error": "You're out of Cua Cloud credit.", "billing_url": url}),
                    );
                }
                obj["metadata"]["creationTimestamp"] = json!(rfc3339(now));
                if kind == "pool" {
                    obj["status"] = json!({"replicas": obj["spec"]["replicas"], "readyReplicas": obj["spec"]["replicas"]});
                }
                if kind == "claim"
                    && let Some(ttl) = obj["spec"]["ttlSecondsAfterCreated"].as_i64()
                {
                    // claim_handlers: TTL => shutdownTime + shutdownPolicy Delete.
                    obj["spec"]["lifecycle"]["shutdownTime"] = json!(rfc3339(now + ttl));
                    obj["spec"]["lifecycle"]["shutdownPolicy"] = json!("Delete");
                }
                s.objects.insert(key, obj.clone());
                resp(201, obj)
            }
            ("GET", Some(name)) => {
                let key = (kind.to_string(), ns.to_string(), name.to_string());
                if kind == "claim" && s.objects.contains_key(&key) {
                    let reads = s.claim_reads.entry(name.to_string()).or_insert(0);
                    *reads += 1;
                    let mut bound = *reads > faults.pending_reads && !faults.never_bind;
                    if faults.keda && bound {
                        let pool_key = ("pool".to_string(), ns.to_string(), ns.to_string());
                        let demand = live_claims(&s, ns);
                        match s.objects.get_mut(&pool_key) {
                            Some(p) => {
                                let replicas = p["spec"]["replicas"].as_u64().unwrap_or(0) as u32;
                                if replicas < demand {
                                    // KEDA scales up on demand; this read stays Pending.
                                    p["spec"]["replicas"] = json!(demand);
                                    p["status"] =
                                        json!({"replicas": demand, "readyReplicas": demand});
                                    bound = false;
                                }
                            }
                            None => bound = false,
                        }
                    }
                    let obj = s.objects.get_mut(&key).unwrap();
                    if faults.fail_bind {
                        obj["status"] = json!({"phase": "Failed", "conditions": [
                            {"type": "Bound", "status": "False", "reason": "BindDeadlineExceeded"}]});
                    }
                    if obj["status"]["phase"] == "Failed" {
                        return resp(200, obj.clone());
                    }
                    obj["status"] = if bound {
                        json!({"phase": "Bound", "sandbox": {"name": format!("sbx-{name}")}})
                    } else {
                        json!({"phase": "Pending"})
                    };
                }
                match s.objects.get(&key) {
                    Some(o) => resp(200, o.clone()),
                    None => resp(404, json!({"error": "not found"})),
                }
            }
            ("PATCH", Some(name)) => {
                let key = (kind.to_string(), ns.to_string(), name.to_string());
                match s.objects.get_mut(&key) {
                    Some(o) => {
                        let mut merged = o.clone();
                        merge(&mut merged, &body.unwrap_or(Value::Null));
                        if kind == "template"
                            && let Some(reason) = template_admission(&merged["spec"]["vmTemplate"])
                        {
                            return resp(403, json!({"error": reason}));
                        }
                        if kind == "template"
                            && over_size_cap(&merged["spec"]["vmTemplate"], faults.size_cap)
                        {
                            return resp(403, json!({"error": SIZE_LIMIT_MESSAGE}));
                        }
                        *o = merged;
                        resp(200, o.clone())
                    }
                    None => resp(404, json!({})),
                }
            }
            ("DELETE", Some(name)) => {
                let key = (kind.to_string(), ns.to_string(), name.to_string());
                match s.objects.remove(&key) {
                    Some(_) => resp(200, json!({})),
                    None => resp(404, json!({})),
                }
            }
            _ => resp(405, json!({})),
        }
    }
}

#[async_trait::async_trait]
impl HttpClient for FakeFleet {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let url = url_path(&request.url);
        let body = request
            .body
            .as_ref()
            .and_then(|b| serde_json::from_slice::<Value>(b).ok());
        self.store.lock().unwrap().requests.push(Recorded {
            method: request.method.clone(),
            path: url.clone(),
            headers: request.headers.clone(),
            body: body.clone(),
        });
        let path = url.split('?').next().unwrap_or_default().to_string();
        let tenant = tenant_of(&request.headers);
        // `GET /api/signed-service-urls/{ns}?claim=<claim>` lists one claim's
        // signed URLs (the one route that reads the query string).
        if request.method == "GET"
            && let Some(ns) = path.strip_prefix("/api/signed-service-urls/")
        {
            let claim = url
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("claim=")))
                .map(str::to_string);
            let Some(claim) = claim else {
                return Ok(HttpResponse {
                    status: 400,
                    headers: vec![],
                    body: br#"{"error":"invalid claim"}"#.to_vec(),
                });
            };
            let listed: Vec<Value> = self
                .store
                .lock()
                .unwrap()
                .signed_urls
                .iter()
                .filter(|u| u["namespace"] == ns && u["claim"] == claim.as_str())
                .cloned()
                .collect();
            return Ok(HttpResponse {
                status: 200,
                headers: vec![HttpHeader {
                    name: "content-type".into(),
                    value: "application/json".into(),
                }],
                body: serde_json::to_vec(&listed).unwrap_or_default(),
            });
        }
        // Yield so concurrent callers interleave between requests, as they
        // would against the real API.
        tokio::task::yield_now().await;
        Ok(self.handle(&request.method, &path, body, &tenant))
    }
}

fn url_path(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match after_scheme.find('/') {
        Some(i) => after_scheme[i..].to_string(),
        None => "/".into(),
    }
}
