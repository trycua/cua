//! Per-claim env tokens delivered by Fleet's claim Secrets.
//!
//! Fleet (trycua/cloud#7885) delivers a Secret into an already-running warm
//! sandbox when a claim binds: the SDK writes the Opaque Secret
//! `cua-claim-<claim>` (key `env-token`) and creates the claim with
//! `spec.secretRef` against a template with `vmTemplate.claimSecrets: true`;
//! the guest sees the token at `/run/cua/env-token`. cua-spacesd follows
//! that file in its await-token-file mode, so the token never crosses the
//! network to the guest and no first-caller bootstrap window exists.
//!
//! The `libs/fleet` mirror predates these fields, so the claim and its
//! Secret are written as JSON with the shapes the gateway admits.
//!
//! Delivery is asynchronous (kubelet sync on gVisor, about 40 s live; the
//! virtiofs share on KubeVirt). [`FleetClient::acquire`] therefore waits
//! after Bound until the driver answers an authenticated call with the
//! token, polling at most [`DEFAULT_WAIT`]; if the driver still says
//! `awaiting token` it releases the claim and returns
//! [`crate::Error::ClaimSecretsNotDelivered`]. It never hangs.

use crate::{BoundSandbox, Claim, Error, FleetClient, Pool, Result, RuntimeKind, SdkError};
use base64::Engine as _;
use cyclops_sdk::{HttpHeader, HttpRequest};
use cyclops_sdk_schema::ClaimSpec;
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// Claim secrets are always available: they are written as JSON.
pub const SUPPORTED: bool = true;

/// The Secret key (and in-guest file name) carrying the env token.
pub const ENV_TOKEN_KEY: &str = "env-token";

/// Name prefix of a claim's Secret (`cua-claim-<claim>`).
pub const CLAIM_SECRET_PREFIX: &str = "cua-claim-";

/// Label on a claim Secret naming its claim (as the Fleet SDK writes it).
pub const CLAIM_SECRET_CLAIM_LABEL: &str = "osgym.cua.ai/claim";

/// Longest wait after Bound for the token to reach the driver.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(90);

/// Pause between token probes.
pub const PROBE_EVERY: Duration = Duration::from_secs(3);

/// Shortest and longest token cua-spacesd accepts from its token file.
const MIN_LEN: usize = 16;
const MAX_LEN: usize = 4096;

/// A fresh env token: 64 hex characters (256 bits) from the OS RNG.
pub fn generate_claim_token() -> String {
    let a: u128 = rand::random();
    let b: u128 = rand::random();
    format!("{a:032x}{b:032x}")
}

/// Checks a claim token with cua-spacesd's token-file rules: 16 to 4096
/// bytes of the RFC 6750 `b64token` alphabet (`A-Z a-z 0-9 - . _ ~ + / =`).
/// A token the driver would refuse would leave the sandbox unreachable.
pub fn validate_claim_token(token: &str) -> crate::Result<()> {
    let bad = |why: &str| Err(crate::Error::InvalidArgument(format!("claim_token {why}")));
    if token.len() < MIN_LEN {
        return bad(&format!("must be at least {MIN_LEN} bytes"));
    }
    if token.len() > MAX_LEN {
        return bad(&format!("must be at most {MAX_LEN} bytes"));
    }
    if !token
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
    {
        return bad("may only contain [A-Za-z0-9-._~+/=]");
    }
    Ok(())
}

/// What one token probe saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenState {
    /// The driver accepted an authenticated call with the token.
    Delivered,
    /// The driver answered `awaiting token` (or refused the token: a
    /// previous claimant's file not replaced yet).
    Awaiting(String),
    /// Nothing that could tell (no env service, not a spacesd, not
    /// reachable yet).
    Unknown(String),
}

/// Asks a bound sandbox whether its driver has the claim's token.
#[async_trait::async_trait]
pub trait TokenProbe: Send + Sync {
    /// One bounded probe.
    async fn probe(&self, fleet: &FleetClient, sandbox: &BoundSandbox, token: &str) -> TokenState;
}

/// The real probe: connects to the `env` service through the gateway with
/// the token and calls `SystemService.Init` (a no-op for an initialized
/// driver; `FailedPrecondition: awaiting token` before delivery).
pub struct EnvTokenProbe;

#[async_trait::async_trait]
impl TokenProbe for EnvTokenProbe {
    async fn probe(&self, fleet: &FleetClient, sandbox: &BoundSandbox, token: &str) -> TokenState {
        if !sandbox.services.iter().any(|s| s == "env") {
            return TokenState::Unknown("the sandbox has no env service".into());
        }
        let opts = match fleet.env_connect_options(sandbox, "env", Some(token.to_string())) {
            Ok(o) => o.probe_timeout(Duration::from_secs(10)),
            Err(e) => return TokenState::Unknown(e.to_string()),
        };
        let client = match tokio::time::timeout(
            Duration::from_secs(20),
            cua_spacesd_client::SpacesdClient::connect(opts),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return classify(e),
            Err(_) => return TokenState::Unknown("connect timed out".into()),
        };
        match tokio::time::timeout(
            Duration::from_secs(15),
            client.init(cua_spacesd_client::pb::InitRequest::default()),
        )
        .await
        {
            Ok(Ok(_)) => TokenState::Delivered,
            Ok(Err(e)) => classify(e),
            Err(_) => TokenState::Unknown("Init timed out".into()),
        }
    }
}

fn classify(e: cua_spacesd_client::Error) -> TokenState {
    match e {
        e @ (cua_spacesd_client::Error::NotInitialized(_)
        | cua_spacesd_client::Error::Unauthenticated(_)) => TokenState::Awaiting(e.to_string()),
        other => TokenState::Unknown(other.to_string()),
    }
}

/// Probe settings of a [`FleetClient`] (tests shorten them).
#[derive(Clone)]
pub struct ClaimSecretsWait {
    /// Longest wait after Bound.
    pub budget: Duration,
    /// Pause between probes.
    pub every: Duration,
    /// The probe.
    pub probe: Arc<dyn TokenProbe>,
}

impl Default for ClaimSecretsWait {
    fn default() -> Self {
        Self {
            budget: DEFAULT_WAIT,
            every: PROBE_EVERY,
            probe: Arc::new(EnvTokenProbe),
        }
    }
}

impl std::fmt::Debug for ClaimSecretsWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimSecretsWait")
            .field("budget", &self.budget)
            .field("every", &self.every)
            .finish_non_exhaustive()
    }
}

/// The Opaque Secret the SDK writes for claim `claim` in `namespace`:
/// `cua-claim-<claim>`, labeled with its claim, holding the env token
/// (base64) under [`ENV_TOKEN_KEY`]. The claim names it in `spec.secretRef`.
pub fn claim_secret_body(namespace: &str, claim: &str, token: &str) -> Value {
    let b64 = base64::engine::general_purpose::STANDARD;
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {"name": format!("{CLAIM_SECRET_PREFIX}{claim}"), "namespace": namespace,
            "labels": {CLAIM_SECRET_CLAIM_LABEL: claim}},
        "type": "Opaque",
        "data": {ENV_TOKEN_KEY: b64.encode(token.as_bytes())},
    })
}

fn claim_name() -> String {
    format!("claim-{:012x}", rand::random::<u64>() & 0xffff_ffff_ffff)
}

impl FleetClient {
    /// Replaces how [`Self::acquire`] waits for claim secrets.
    pub fn with_claim_secrets_wait(mut self, wait: ClaimSecretsWait) -> Self {
        self.claim_wait = Arc::new(wait);
        self
    }

    fn secret_url(&self, ns: &str, name: Option<&str>) -> String {
        let base = self.config().base_url.trim_end_matches('/');
        match name {
            Some(n) => format!("{base}/api/k8s/api/v1/namespaces/{ns}/secrets/{n}"),
            None => format!("{base}/api/k8s/api/v1/namespaces/{ns}/secrets"),
        }
    }

    /// A request whose body may hold a credential: errors carry the status
    /// only, never a body.
    async fn quiet(&self, method: &str, url: String, body: Option<&Value>) -> Result<u16> {
        let request = HttpRequest {
            method: method.into(),
            url,
            headers: vec![
                HttpHeader {
                    name: "accept".into(),
                    value: "application/json".into(),
                },
                HttpHeader {
                    name: "content-type".into(),
                    value: "application/json".into(),
                },
            ],
            body: body.map(|b| b.to_string().into_bytes()),
            timeout_secs: Some(30),
            max_response_bytes: Some(1 << 20),
        };
        match self.sdk().execute_authenticated(request).await {
            Ok(r) => Ok(r.status),
            Err(SdkError::Status { status, .. }) => Ok(status),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn claim_secret_written(&self, ns: &str, claim: &str) -> bool {
        self.secret_claims
            .lock()
            .unwrap()
            .contains(&(ns.to_string(), claim.to_string()))
    }

    /// Deletes claim `claim`'s `cua-claim-<claim>` Secret (missing is fine).
    pub async fn delete_claim_secret(&self, ns: &str, claim: &str) -> Result<()> {
        let name = format!("{CLAIM_SECRET_PREFIX}{claim}");
        match self
            .quiet("DELETE", self.secret_url(ns, Some(&name)), None)
            .await?
        {
            200..=204 | 404 => {
                self.secret_claims
                    .lock()
                    .unwrap()
                    .remove(&(ns.to_string(), claim.to_string()));
                Ok(())
            }
            s => Err(Error::InvalidArgument(format!(
                "could not delete claim secret {name}: HTTP {s}"
            ))),
        }
    }

    /// Writes the claim's Secret, then the claim with `spec.secretRef`; the
    /// Secret is deleted again if the claim is refused.
    pub(crate) async fn create_claim_with_secret(
        &self,
        pool: &Pool,
        spec: Option<ClaimSpec>,
        name: Option<String>,
        labels: Option<HashMap<String, String>>,
        token: String,
    ) -> Result<Claim> {
        let ns = pool.metadata.namespace.clone();
        let name = name.unwrap_or_else(claim_name);
        cyclops_sdk::validate_dns_label(&name)?;
        let secret = format!("{CLAIM_SECRET_PREFIX}{name}");
        let body = claim_secret_body(&ns, &name, &token);
        // 409 is an error on purpose: an existing Secret of that name
        // belongs to another claim attempt and must not be overwritten.
        match self
            .quiet("POST", self.secret_url(&ns, None), Some(&body))
            .await?
        {
            200..=202 => {}
            s => {
                return Err(Error::InvalidArgument(format!(
                    "could not create claim secret {secret}: HTTP {s}"
                )));
            }
        }
        self.secret_claims
            .lock()
            .unwrap()
            .insert((ns.clone(), name.clone()));
        let mut spec = serde_json::to_value(spec.unwrap_or_else(|| ClaimSpec {
            sandbox_template_ref: pool.spec.sandbox_template_ref.clone(),
            warmpool: None,
            bind_deadline: None,
            lifecycle: None,
            ttl_seconds_after_created: None,
            secret_ref: None,
        }))
        .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        spec["secretRef"] = json!({"name": secret});
        if spec.get("bindDeadline").is_none_or(Value::is_null) {
            spec["bindDeadline"] = json!(900);
        }
        let mut metadata = json!({"namespace": ns, "name": name});
        if let Some(l) = labels.filter(|l| !l.is_empty()) {
            metadata["labels"] = json!(l);
        }
        let claim = json!({
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxClaim",
            "metadata": metadata,
            "spec": spec,
        });
        let url = self.k8s_url(&ns, "osgymsandboxclaims", None);
        match self.raw("POST", url, Some(claim)).await {
            Ok((200..=202, v)) => {
                serde_json::from_value(v).map_err(|e| Error::InvalidArgument(e.to_string()))
            }
            other => {
                let _ = self.delete_claim_secret(&ns, &name).await;
                match other {
                    Ok((s, v)) => {
                        Err(SdkError::status("create claim", s, v.to_string().as_bytes()).into())
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }

    /// Waits until the bound sandbox's spacesd has `token`, polling for
    /// at most the configured budget (default [`DEFAULT_WAIT`]). When the
    /// driver still says `awaiting token`:
    /// [`Error::ClaimSecretsNotDelivered`] (the caller releases the claim).
    /// A sandbox that cannot tell (no env service, not a spacesd) is
    /// accepted after the budget with a warning: delivery is Fleet's
    /// contract, and readiness of the image is `wait_for`'s.
    pub async fn await_claim_secrets(
        &self,
        sandbox: &BoundSandbox,
        token: &str,
        runtime: &RuntimeKind,
    ) -> Result<()> {
        let wait = Arc::clone(&self.claim_wait);
        let started = tokio::time::Instant::now();
        let deadline = started + wait.budget;
        // Bounded: at most budget / every + 1 probes.
        let max_probes = (wait.budget.as_millis() / wait.every.as_millis().max(1)) as u64 + 2;
        let mut last = TokenState::Unknown("not probed".into());
        for _ in 0..max_probes {
            last = wait.probe.probe(self, sandbox, token).await;
            match &last {
                TokenState::Delivered => {
                    tracing::debug!(claim = %sandbox.claim, waited = ?started.elapsed(),
                        "claim secrets delivered");
                    return Ok(());
                }
                TokenState::Unknown(why) if !sandbox.services.iter().any(|s| s == "env") => {
                    tracing::debug!(claim = %sandbox.claim, %why, "claim secrets not verifiable");
                    return Ok(());
                }
                _ => {}
            }
            if tokio::time::Instant::now() + wait.every > deadline {
                break;
            }
            tokio::time::sleep(wait.every).await;
        }
        match last {
            TokenState::Awaiting(detail) => Err(Error::ClaimSecretsNotDelivered {
                claim: sandbox.claim.clone(),
                runtime: crate::runtime_name(runtime).to_string(),
                waited: started.elapsed(),
                detail,
            }),
            TokenState::Unknown(why) => {
                tracing::warn!(claim = %sandbox.claim, %why,
                    "could not confirm the claim token reached the sandbox");
                Ok(())
            }
            TokenState::Delivered => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_valid_and_distinct() {
        let a = generate_claim_token();
        let b = generate_claim_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        validate_claim_token(&a).unwrap();
    }

    #[test]
    fn validation_matches_the_driver() {
        assert!(validate_claim_token("short").is_err());
        assert!(validate_claim_token(&"a".repeat(4097)).is_err());
        assert!(validate_claim_token("has space 0123456789").is_err());
        assert!(validate_claim_token("line\nbreak0123456789").is_err());
        validate_claim_token("aZ09-._~+/=aZ09-._~+/=").unwrap();
    }
}
