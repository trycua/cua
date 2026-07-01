//! Experimental OAuth 2.0 front door for the local MCP HTTP transport.
//!
//! This is intentionally opt-in and loopback-oriented: callers expose it
//! through their own HTTPS tunnel if they want OAuth/DCR discovery.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

const DEFAULT_LISTEN: &str = "127.0.0.1:7676";
const DEFAULT_MCP_UPSTREAM: &str = "http://127.0.0.1:7677/mcp";
const DEFAULT_SCOPE: &str = "mcp:tools";
const HTTP_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_BODY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Options {
    pub public_url: String,
    pub listen: String,
    pub mcp_upstream: String,
    pub storage_dir: Option<PathBuf>,
    pub token_ttl_seconds: u64,
    pub code_ttl_seconds: u64,
    pub require_user_consent: bool,
}

impl Options {
    pub fn new(
        public_url: String,
        listen: Option<String>,
        mcp_upstream: Option<String>,
        storage_dir: Option<String>,
        token_ttl_seconds: Option<u64>,
        code_ttl_seconds: Option<u64>,
        require_user_consent: bool,
    ) -> Self {
        Self {
            public_url: trim_trailing_slash(&public_url),
            listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_owned()),
            mcp_upstream: mcp_upstream.unwrap_or_else(|| DEFAULT_MCP_UPSTREAM.to_owned()),
            storage_dir: storage_dir.map(PathBuf::from),
            token_ttl_seconds: token_ttl_seconds.unwrap_or(86_400),
            code_ttl_seconds: code_ttl_seconds.unwrap_or(300),
            require_user_consent,
        }
    }
}

pub fn run(options: Options) -> anyhow::Result<()> {
    if !options.public_url.starts_with("https://") {
        anyhow::bail!("mcp-oauth requires --public-url to start with https://");
    }
    let upstream = parse_mcp_upstream(&options.mcp_upstream)
        .map_err(|e| anyhow::anyhow!("invalid --mcp-upstream: {e}"))?;
    let addr: SocketAddr = options.listen.parse()?;
    if !addr.ip().is_loopback() {
        eprintln!(
            "WARNING: mcp-oauth is listening on a non-loopback address. This exposes desktop automation; use a trusted HTTPS tunnel and stop it when idle."
        );
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move { serve(options, addr, upstream).await })
}

async fn serve(options: Options, addr: SocketAddr, upstream: UpstreamTarget) -> anyhow::Result<()> {
    let store = std::sync::Arc::new(JsonStore::new(
        options
            .storage_dir
            .clone()
            .unwrap_or_else(default_storage_dir),
        options.token_ttl_seconds,
        options.code_ttl_seconds,
    ));
    ensure_private_dir(&store.dir)?;
    let listener = TcpListener::bind(addr).await?;
    eprintln!(
        "cua-driver mcp-oauth listening on http://{}; configure your HTTPS tunnel to target this address",
        addr
    );
    let trace = trace_enabled();
    if trace {
        eprintln!("mcp-oauth trace enabled via CUA_DRIVER_RS_MCP_OAUTH_TRACE");
    }
    loop {
        let (stream, _) = listener.accept().await?;
        let ctx = HandlerContext {
            options: options.clone(),
            store: store.clone(),
            upstream: upstream.clone(),
            trace,
        };
        tokio::spawn(async move {
            let _ = serve_conn(stream, ctx).await;
        });
    }
}

#[derive(Clone)]
struct HandlerContext {
    options: Options,
    store: std::sync::Arc<JsonStore>,
    upstream: UpstreamTarget,
    trace: bool,
}

#[derive(Clone, Debug)]
struct UpstreamTarget {
    host: String,
    port: u16,
    path: String,
}

impl UpstreamTarget {
    fn host_header(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[derive(Clone, Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    keep_alive: bool,
}

#[derive(Clone, Debug)]
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_owned(), content_type.to_owned())],
            body,
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Client {
    client_secret: String,
    client_name: String,
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: String,
    created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AuthCode {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    scope: String,
    resource: Option<String>,
    created_at: u64,
    used: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccessToken {
    client_id: String,
    scope: String,
    resource: Option<String>,
    created_at: u64,
    expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingAuthorization {
    client_id: String,
    client_name: String,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
    resource: Option<String>,
    code_challenge: String,
    code_challenge_method: String,
}

impl PendingAuthorization {
    fn matches_form(&self, form: &HashMap<String, String>) -> bool {
        form.get("client_id") == Some(&self.client_id)
            && form.get("redirect_uri") == Some(&self.redirect_uri)
            && form.get("scope") == Some(&self.scope)
            && form.get("code_challenge") == Some(&self.code_challenge)
            && form.get("code_challenge_method") == Some(&self.code_challenge_method)
            && optional_form_value(form, "state") == self.state.as_deref()
            && optional_form_value(form, "resource") == self.resource.as_deref()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredConsent {
    pending: PendingAuthorization,
    created_at: u64,
}

#[derive(Debug)]
struct TokenIssue {
    status: u16,
    access_token: String,
    scope: String,
    resource: Option<String>,
}

enum TokenRedeem {
    Issued(TokenIssue),
    InvalidGrant(&'static str),
    InvalidScope,
}

struct JsonStore {
    dir: PathBuf,
    token_ttl_seconds: u64,
    code_ttl_seconds: u64,
    mutation_lock: Mutex<()>,
}

impl JsonStore {
    fn new(dir: PathBuf, token_ttl_seconds: u64, code_ttl_seconds: u64) -> Self {
        Self {
            dir,
            token_ttl_seconds,
            code_ttl_seconds,
            mutation_lock: Mutex::new(()),
        }
    }

    fn lock_mutation(&self) -> io::Result<std::sync::MutexGuard<'_, ()>> {
        self.mutation_lock
            .lock()
            .map_err(|_| io::Error::other("OAuth state lock poisoned"))
    }

    fn clients_path(&self) -> PathBuf {
        self.dir.join("clients.json")
    }

    fn codes_path(&self) -> PathBuf {
        self.dir.join("codes.json")
    }

    fn tokens_path(&self) -> PathBuf {
        self.dir.join("tokens.json")
    }

    fn consents_path(&self) -> PathBuf {
        self.dir.join("consents.json")
    }

    fn load_clients(&self) -> io::Result<HashMap<String, Client>> {
        read_json_map(&self.clients_path())
    }

    fn load_codes(&self) -> io::Result<HashMap<String, AuthCode>> {
        read_json_map(&self.codes_path())
    }

    fn load_tokens(&self) -> io::Result<HashMap<String, AccessToken>> {
        read_json_map(&self.tokens_path())
    }

    fn load_consents(&self) -> io::Result<HashMap<String, StoredConsent>> {
        read_json_map(&self.consents_path())
    }

    fn upsert_client(&self, key: &str, value: Client) -> io::Result<()> {
        let _guard = self.lock_mutation()?;
        let mut map = self.load_clients()?;
        map.insert(key.to_owned(), value);
        write_json_map(&self.clients_path(), &map)
    }

    fn upsert_code(&self, key: &str, value: AuthCode) -> io::Result<()> {
        let _guard = self.lock_mutation()?;
        let mut map = self.load_codes()?;
        map.insert(key.to_owned(), value);
        write_json_map(&self.codes_path(), &map)
    }

    #[cfg(test)]
    fn upsert_token(&self, key: &str, value: AccessToken) -> io::Result<()> {
        let _guard = self.lock_mutation()?;
        let mut map = self.load_tokens()?;
        self.prune_tokens(&mut map);
        map.insert(key.to_owned(), value);
        write_json_map(&self.tokens_path(), &map)
    }

    fn upsert_consent(&self, key: &str, value: &PendingAuthorization) -> io::Result<()> {
        let _guard = self.lock_mutation()?;
        let mut map = self.load_consents()?;
        map.insert(
            key.to_owned(),
            StoredConsent {
                pending: value.clone(),
                created_at: now_secs(),
            },
        );
        write_json_map(&self.consents_path(), &map)
    }

    #[cfg(test)]
    fn upsert_consent_with_created_at(
        &self,
        key: &str,
        value: &PendingAuthorization,
        created_at: u64,
    ) -> io::Result<()> {
        let _guard = self.lock_mutation()?;
        let mut map = self.load_consents()?;
        map.insert(
            key.to_owned(),
            StoredConsent {
                pending: value.clone(),
                created_at,
            },
        );
        write_json_map(&self.consents_path(), &map)
    }

    fn consume_consent(&self, key: &str) -> io::Result<Option<PendingAuthorization>> {
        let _guard = self.lock_mutation()?;
        let mut map = self.load_consents()?;
        self.prune_consents(&mut map);
        let out = map.remove(key).map(|stored| stored.pending);
        write_json_map(&self.consents_path(), &map)?;
        Ok(out)
    }

    fn prune_tokens(&self, tokens: &mut HashMap<String, AccessToken>) {
        let now = now_secs();
        tokens.retain(|_, token| token.expires_at > now);
    }

    fn prune_consents(&self, consents: &mut HashMap<String, StoredConsent>) {
        let now = now_secs();
        consents
            .retain(|_, consent| consent.created_at.saturating_add(self.code_ttl_seconds) > now);
    }

    fn redeem_code_for_token(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        verifier: &str,
        resource: Option<&str>,
    ) -> io::Result<TokenRedeem> {
        let _guard = self.lock_mutation()?;
        let mut codes = self.load_codes()?;
        let mut tokens = self.load_tokens()?;
        self.prune_tokens(&mut tokens);
        let now = now_secs();

        let Some(stored) = codes.get_mut(code) else {
            return Ok(TokenRedeem::InvalidGrant("unknown authorization code"));
        };
        if stored.used {
            return Ok(TokenRedeem::InvalidGrant("authorization code already used"));
        }
        if now > stored.created_at.saturating_add(self.code_ttl_seconds) {
            return Ok(TokenRedeem::InvalidGrant("authorization code expired"));
        }
        if stored.client_id != client_id {
            return Ok(TokenRedeem::InvalidGrant(
                "authorization code was not issued to this client",
            ));
        }
        if stored.redirect_uri != redirect_uri {
            return Ok(TokenRedeem::InvalidGrant("redirect_uri mismatch"));
        }
        if let Some(resource) = resource {
            if stored.resource.as_deref() != Some(resource) {
                return Ok(TokenRedeem::InvalidGrant("resource mismatch"));
            }
        }
        if !is_valid_scope(&stored.scope) {
            return Ok(TokenRedeem::InvalidScope);
        }
        if !verify_pkce(
            verifier,
            &stored.code_challenge,
            &stored.code_challenge_method,
        ) {
            return Ok(TokenRedeem::InvalidGrant("PKCE verification failed"));
        }

        stored.used = true;
        let scope = stored.scope.clone();
        let issued_resource = stored.resource.clone();
        write_json_map(&self.codes_path(), &codes)?;

        let access_token = random_id();
        tokens.insert(
            access_token.clone(),
            AccessToken {
                client_id: client_id.to_owned(),
                scope: scope.clone(),
                resource: issued_resource.clone(),
                created_at: now,
                expires_at: now.saturating_add(self.token_ttl_seconds),
            },
        );
        write_json_map(&self.tokens_path(), &tokens)?;

        Ok(TokenRedeem::Issued(TokenIssue {
            status: 200,
            access_token,
            scope,
            resource: issued_resource,
        }))
    }
}

async fn serve_conn(mut stream: TcpStream, ctx: HandlerContext) -> anyhow::Result<()> {
    loop {
        let Some(req) = read_http_request(&mut stream).await? else {
            return Ok(());
        };
        let keep_alive = req.keep_alive;
        let method = req.method.clone();
        let path = req.path.clone();
        trace_request(&ctx, &method, &path, req.body.len());
        if req.method.eq_ignore_ascii_case("OPTIONS") {
            let resp = handle_http(req, &ctx).await;
            trace_response(&ctx, &method, &path, resp.status);
            write_http(&mut stream, resp, keep_alive).await?;
            if !keep_alive {
                return Ok(());
            }
            continue;
        }
        if req.path.split('?').next().unwrap_or("/") == "/mcp" {
            forward_mcp_request(&mut stream, req, &ctx).await?;
            return Ok(());
        }
        let resp = handle_http(req, &ctx).await;
        trace_response(&ctx, &method, &path, resp.status);
        write_http(&mut stream, resp, keep_alive).await?;
        if !keep_alive {
            return Ok(());
        }
    }
}

async fn handle_http(req: HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    if req.method.eq_ignore_ascii_case("OPTIONS") {
        return HttpResponse::empty(204);
    }
    let path = req.path.split('?').next().unwrap_or("/");
    match (req.method.as_str(), path) {
        ("GET", "/health") => json_response(200, json!({"ok": true})),
        ("GET", "/.well-known/oauth-authorization-server")
        | ("GET", "/.well-known/openid-configuration") => {
            json_response(200, authorization_server_metadata(&ctx.options.public_url))
        }
        ("GET", "/.well-known/oauth-protected-resource")
        | ("GET", "/.well-known/oauth-protected-resource/mcp") => {
            json_response(200, protected_resource_metadata(&ctx.options.public_url))
        }
        ("POST", "/register") => handle_register(&req, ctx),
        ("GET", "/authorize") => handle_authorize_get(&req, ctx),
        ("POST", "/authorize") => handle_authorize_post(&req, ctx),
        ("POST", "/token") => handle_token(&req, ctx),
        _ => json_response(404, json!({"error": "not_found"})),
    }
}

fn authorization_server_metadata(public_url: &str) -> Value {
    json!({
        "issuer": public_url,
        "authorization_endpoint": format!("{public_url}/authorize"),
        "token_endpoint": format!("{public_url}/token"),
        "registration_endpoint": format!("{public_url}/register"),
        "resource": mcp_resource(public_url),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "scopes_supported": [DEFAULT_SCOPE],
    })
}

fn protected_resource_metadata(public_url: &str) -> Value {
    json!({
        "resource": mcp_resource(public_url),
        "authorization_servers": [public_url],
        "bearer_methods_supported": ["header"],
        "scopes_supported": [DEFAULT_SCOPE],
    })
}

fn mcp_resource(public_url: &str) -> String {
    format!("{}/mcp", trim_trailing_slash(public_url))
}

fn handle_register(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let input: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(_) => {
            return oauth_error(
                400,
                "invalid_client_metadata",
                "registration body must be JSON",
            )
        }
    };

    let redirect_uris = match input.get("redirect_uris").and_then(|v| v.as_array()) {
        Some(items) => {
            let mut uris = Vec::new();
            for item in items {
                let Some(uri) = item.as_str() else {
                    return oauth_error(
                        400,
                        "invalid_redirect_uri",
                        "redirect_uris must contain strings",
                    );
                };
                if !is_allowed_redirect_uri(uri) {
                    return oauth_error(
                        400,
                        "invalid_redirect_uri",
                        "redirect_uri must be https or loopback http and must not contain fragments or control characters",
                    );
                }
                uris.push(uri.to_owned());
            }
            if uris.is_empty() {
                return oauth_error(
                    400,
                    "invalid_redirect_uri",
                    "redirect_uris must contain at least one URI",
                );
            }
            uris
        }
        None => return oauth_error(400, "invalid_redirect_uri", "redirect_uris is required"),
    };

    let auth_method = input
        .get("token_endpoint_auth_method")
        .and_then(|v| v.as_str())
        .unwrap_or("client_secret_post");
    if auth_method != "client_secret_post" && auth_method != "none" {
        return oauth_error(
            400,
            "invalid_client_metadata",
            "unsupported token_endpoint_auth_method",
        );
    }

    let client_id = random_id();
    let client_secret = random_id();
    let client = Client {
        client_secret: client_secret.clone(),
        client_name: input
            .get("client_name")
            .and_then(|v| v.as_str())
            .unwrap_or("MCP client")
            .to_owned(),
        redirect_uris: redirect_uris.clone(),
        token_endpoint_auth_method: auth_method.to_owned(),
        created_at: now_secs(),
    };
    if let Err(e) = ctx.store.upsert_client(&client_id, client) {
        return oauth_error(
            500,
            "server_error",
            &format!("failed to persist client: {e}"),
        );
    }

    no_store_response(json_response(
        201,
        json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "client_id_issued_at": now_secs(),
            "client_secret_expires_at": 0,
            "redirect_uris": redirect_uris,
            "grant_types": ["authorization_code"],
            "token_endpoint_auth_method": auth_method,
            "response_types": ["code"],
            "scope": DEFAULT_SCOPE,
        }),
    ))
}

fn handle_authorize_get(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let query = parse_query(&req.path);
    match validate_authorize_query(&query, ctx) {
        Ok(pending) if !ctx.options.require_user_consent => issue_authorization_code(pending, ctx),
        Ok(pending) => {
            let nonce = random_id();
            if let Err(e) = ctx.store.upsert_consent(&nonce, &pending) {
                return oauth_error(
                    500,
                    "server_error",
                    &format!("failed to persist consent transaction: {e}"),
                );
            }
            html_response(
                200,
                authorize_page(&pending, &ctx.options.public_url, &nonce),
            )
        }
        Err(resp) => resp,
    }
}

fn handle_authorize_post(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let form = parse_form(&req.body);
    let decision = form.get("decision").map(String::as_str).unwrap_or("deny");
    let nonce = match form.get("consent_nonce").filter(|v| !v.is_empty()) {
        Some(v) => v.to_owned(),
        None => return oauth_error(400, "invalid_request", "missing consent_nonce"),
    };
    match ctx.store.consume_consent(&nonce) {
        Ok(Some(pending)) if pending.matches_form(&form) && decision == "allow" => {
            issue_authorization_code(pending, ctx)
        }
        Ok(Some(pending)) if pending.matches_form(&form) => {
            let mut pairs = vec![("error", "access_denied")];
            if let Some(state) = pending.state.as_deref() {
                pairs.push(("state", state));
            }
            redirect(&append_query(&pending.redirect_uri, &pairs))
        }
        Ok(Some(_)) => oauth_error(400, "invalid_request", "consent transaction mismatch"),
        Ok(None) => oauth_error(
            400,
            "invalid_request",
            "unknown or expired consent transaction",
        ),
        Err(e) => oauth_error(500, "server_error", &e.to_string()),
    }
}

fn validate_authorize_query(
    params: &HashMap<String, String>,
    ctx: &HandlerContext,
) -> Result<PendingAuthorization, HttpResponse> {
    let client_id = required(params, "client_id")?;
    let redirect_uri = required(params, "redirect_uri")?;
    let code_challenge = required(params, "code_challenge")?;
    if params.get("response_type").map(String::as_str) != Some("code") {
        return Err(oauth_error(
            400,
            "unsupported_response_type",
            "response_type must be code",
        ));
    }

    let scope = params
        .get("scope")
        .cloned()
        .unwrap_or_else(|| DEFAULT_SCOPE.to_owned());
    if !is_valid_scope(&scope) {
        return Err(oauth_error(
            400,
            "invalid_scope",
            "only mcp:tools is supported",
        ));
    }

    let method = params
        .get("code_challenge_method")
        .map(String::as_str)
        .unwrap_or("S256");
    if method != "S256" {
        return Err(oauth_error(
            400,
            "invalid_request",
            "code_challenge_method must be S256",
        ));
    }

    let clients = ctx
        .store
        .load_clients()
        .map_err(|e| oauth_error(500, "server_error", &e.to_string()))?;
    let Some(client) = clients.get(&client_id) else {
        return Err(oauth_error(400, "invalid_client", "unknown client_id"));
    };
    if !client.redirect_uris.iter().any(|uri| uri == &redirect_uri) {
        return Err(oauth_error(
            400,
            "invalid_request",
            "redirect_uri was not registered for this client",
        ));
    }

    let expected_resource = mcp_resource(&ctx.options.public_url);
    let resource = params
        .get("resource")
        .cloned()
        .unwrap_or_else(|| expected_resource.clone());
    if resource != expected_resource {
        return Err(oauth_error(
            400,
            "invalid_target",
            "resource does not match this MCP endpoint",
        ));
    }

    Ok(PendingAuthorization {
        client_id,
        client_name: client.client_name.clone(),
        redirect_uri,
        scope,
        state: params.get("state").cloned(),
        resource: Some(resource),
        code_challenge,
        code_challenge_method: method.to_owned(),
    })
}

fn required(params: &HashMap<String, String>, key: &str) -> Result<String, HttpResponse> {
    params
        .get(key)
        .filter(|v| !v.is_empty())
        .cloned()
        .ok_or_else(|| oauth_error(400, "invalid_request", &format!("missing {key}")))
}

fn issue_authorization_code(pending: PendingAuthorization, ctx: &HandlerContext) -> HttpResponse {
    let code = random_id();
    let stored = AuthCode {
        client_id: pending.client_id,
        redirect_uri: pending.redirect_uri.clone(),
        code_challenge: pending.code_challenge,
        code_challenge_method: pending.code_challenge_method,
        scope: pending.scope,
        resource: pending.resource,
        created_at: now_secs(),
        used: false,
    };
    if let Err(e) = ctx.store.upsert_code(&code, stored) {
        return oauth_error(
            500,
            "server_error",
            &format!("failed to persist authorization code: {e}"),
        );
    }
    let mut pairs = vec![("code", code.as_str())];
    if let Some(state) = pending.state.as_deref() {
        pairs.push(("state", state));
    }
    redirect(&append_query(&pending.redirect_uri, &pairs))
}

fn handle_token(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let form = parse_form(&req.body);
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return oauth_error(
            400,
            "unsupported_grant_type",
            "only authorization_code is supported",
        );
    }

    let code = match form.get("code") {
        Some(v) => v.to_owned(),
        None => return oauth_error(400, "invalid_request", "missing code"),
    };
    let client_id = match form.get("client_id") {
        Some(v) => v.to_owned(),
        None => return oauth_error(400, "invalid_request", "missing client_id"),
    };
    let redirect_uri = match form.get("redirect_uri") {
        Some(v) => v.to_owned(),
        None => return oauth_error(400, "invalid_request", "missing redirect_uri"),
    };
    let verifier = match form.get("code_verifier") {
        Some(v) => v.to_owned(),
        None => return oauth_error(400, "invalid_request", "missing code_verifier"),
    };

    let expected_resource = mcp_resource(&ctx.options.public_url);
    let resource = form.get("resource").cloned();
    if let Some(resource) = &resource {
        if resource != &expected_resource {
            return oauth_error(
                400,
                "invalid_target",
                "resource does not match this MCP endpoint",
            );
        }
    }

    let clients = match ctx.store.load_clients() {
        Ok(v) => v,
        Err(e) => return oauth_error(500, "server_error", &e.to_string()),
    };
    let Some(client) = clients.get(&client_id) else {
        return oauth_error(401, "invalid_client", "unknown client_id");
    };
    if client.token_endpoint_auth_method == "client_secret_post" {
        if form.get("client_secret") != Some(&client.client_secret) {
            return oauth_error(401, "invalid_client", "invalid client_secret");
        }
    }

    let token = match ctx.store.redeem_code_for_token(
        &code,
        &client_id,
        &redirect_uri,
        &verifier,
        resource.as_deref(),
    ) {
        Ok(TokenRedeem::Issued(v)) => v,
        Ok(TokenRedeem::InvalidGrant(description)) => {
            return oauth_error(400, "invalid_grant", description)
        }
        Ok(TokenRedeem::InvalidScope) => {
            return oauth_error(400, "invalid_scope", "only mcp:tools is supported")
        }
        Err(e) => return oauth_error(500, "server_error", &e.to_string()),
    };

    let mut body = json!({
        "access_token": token.access_token,
        "token_type": "Bearer",
        "expires_in": ctx.store.token_ttl_seconds,
        "scope": token.scope,
    });
    if let Some(resource) = token.resource {
        body["resource"] = Value::String(resource);
    }
    no_store_response(json_response(token.status, body))
}

fn authorize_bearer(req: &HttpRequest, ctx: &HandlerContext) -> Result<(), HttpResponse> {
    let auth = req
        .headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or("");
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .ok_or_else(|| bearer_error(&ctx.options.public_url, "Missing Bearer token"))?;

    let mut tokens = ctx
        .store
        .load_tokens()
        .map_err(|e| oauth_error(500, "server_error", &e.to_string()))?;
    ctx.store.prune_tokens(&mut tokens);
    let Some(stored) = tokens.get(token) else {
        return Err(bearer_error(&ctx.options.public_url, "Unknown token"));
    };
    if now_secs() > stored.expires_at {
        return Err(bearer_error(&ctx.options.public_url, "Expired token"));
    }
    if !is_valid_scope(&stored.scope) {
        return Err(bearer_error(
            &ctx.options.public_url,
            "Token does not carry mcp:tools scope",
        ));
    }
    let expected_resource = mcp_resource(&ctx.options.public_url);
    if stored.resource.as_deref() != Some(expected_resource.as_str()) {
        return Err(bearer_error(
            &ctx.options.public_url,
            "Token audience does not match this MCP resource",
        ));
    }
    Ok(())
}

async fn forward_mcp_request(
    client: &mut TcpStream,
    req: HttpRequest,
    ctx: &HandlerContext,
) -> anyhow::Result<()> {
    let method = req.method.clone();
    let path = req.path.clone();
    let json_rpc_method = json_rpc_method_name(&req);
    if let Err(resp) = authorize_bearer(&req, ctx) {
        trace_mcp_done(ctx, &method, &path, json_rpc_method, None, resp.status);
        write_http(client, resp, false).await?;
        return Ok(());
    }

    let mut upstream = match tokio::time::timeout(
        HTTP_HEADER_TIMEOUT,
        TcpStream::connect((ctx.upstream.host.as_str(), ctx.upstream.port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            trace_mcp_error(ctx, &method, &path, json_rpc_method, "connect_failed");
            write_http(
                client,
                proxy_error_response(&format!("failed to connect to MCP upstream: {e}")),
                false,
            )
            .await?;
            return Ok(());
        }
        Err(_) => {
            trace_mcp_error(ctx, &method, &path, json_rpc_method, "connect_timeout");
            write_http(
                client,
                proxy_error_response("timed out connecting to MCP upstream"),
                false,
            )
            .await?;
            return Ok(());
        }
    };

    if let Err(e) = write_upstream_request(&mut upstream, &req, &ctx.upstream).await {
        trace_mcp_error(ctx, &method, &path, json_rpc_method, "forward_failed");
        write_http(
            client,
            proxy_error_response(&format!("failed to forward MCP request: {e}")),
            false,
        )
        .await?;
        return Ok(());
    }

    let head = match read_upstream_response_head(&mut upstream).await {
        Ok(head) => head,
        Err(e) => {
            trace_mcp_error(ctx, &method, &path, json_rpc_method, "upstream_head_failed");
            write_http(
                client,
                proxy_error_response(&format!("failed to read MCP upstream response: {e}")),
                false,
            )
            .await?;
            return Ok(());
        }
    };

    if is_tools_list_request(&req) {
        let upstream_body = match read_upstream_body(&mut upstream).await {
            Ok(body) => body,
            Err(e) => {
                trace_mcp_error(ctx, &method, &path, json_rpc_method, "upstream_body_failed");
                write_http(
                    client,
                    proxy_error_response(&format!("failed to read MCP upstream body: {e}")),
                    false,
                )
                .await?;
                return Ok(());
            }
        };
        let upstream_status = head.status;
        let Some(rewrite_body) = tools_list_body_for_rewrite(&head, upstream_body) else {
            trace_mcp_error(
                ctx,
                &method,
                &path,
                json_rpc_method,
                "upstream_chunked_body_failed",
            );
            write_http(
                client,
                proxy_error_response("failed to decode chunked MCP upstream response"),
                false,
            )
            .await?;
            return Ok(());
        };
        let (body, rewrite) = match rewrite_tools_list_response(&rewrite_body) {
            Some(body) => (body, true),
            None if is_chunked_response(&head) => {
                trace_mcp_error(
                    ctx,
                    &method,
                    &path,
                    json_rpc_method,
                    "chunked_tools_list_rewrite_failed",
                );
                write_http(
                    client,
                    proxy_error_response("failed to rewrite chunked MCP tools/list response"),
                    false,
                )
                .await?;
                return Ok(());
            }
            None => (rewrite_body, false),
        };
        trace_tools_list(ctx, upstream_status, rewrite, body.len());
        write_http(client, proxy_http_response(head, body), false).await?;
        trace_mcp_done(
            ctx,
            &method,
            &path,
            json_rpc_method,
            Some(upstream_status),
            upstream_status,
        );
        return Ok(());
    }

    trace_mcp_done(
        ctx,
        &method,
        &path,
        json_rpc_method,
        Some(head.status),
        head.status,
    );
    write_proxy_response_head(client, &head).await?;
    let _ = tokio::io::copy(&mut upstream, client).await;
    let _ = client.flush().await;
    Ok(())
}

fn trace_enabled() -> bool {
    std::env::var("CUA_DRIVER_RS_MCP_OAUTH_TRACE")
        .ok()
        .is_some_and(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !normalized.is_empty() && normalized != "0" && normalized != "false"
        })
}

fn trace_request(ctx: &HandlerContext, method: &str, path: &str, body_len: usize) {
    if ctx.trace {
        eprintln!("[mcp-oauth] <- {method} {path} body={body_len}");
    }
}

fn trace_response(ctx: &HandlerContext, method: &str, path: &str, status: u16) {
    if ctx.trace {
        eprintln!("[mcp-oauth] -> {status} {method} {path}");
    }
}

fn trace_mcp_done(
    ctx: &HandlerContext,
    method: &str,
    path: &str,
    json_rpc_method: Option<String>,
    upstream_status: Option<u16>,
    status: u16,
) {
    if !ctx.trace {
        return;
    }
    let rpc = json_rpc_method.as_deref().unwrap_or("-");
    match upstream_status {
        Some(upstream) => {
            eprintln!("[mcp-oauth] -> {status} {method} {path} rpc={rpc} upstream={upstream}");
        }
        None => {
            eprintln!("[mcp-oauth] -> {status} {method} {path} rpc={rpc}");
        }
    }
}

fn trace_mcp_error(
    ctx: &HandlerContext,
    method: &str,
    path: &str,
    json_rpc_method: Option<String>,
    error: &str,
) {
    if ctx.trace {
        let rpc = json_rpc_method.as_deref().unwrap_or("-");
        eprintln!("[mcp-oauth] !! {method} {path} rpc={rpc} {error}");
    }
}

fn trace_tools_list(ctx: &HandlerContext, upstream_status: u16, rewritten: bool, body_len: usize) {
    if ctx.trace {
        eprintln!(
            "[mcp-oauth] tools/list upstream={upstream_status} rewritten={rewritten} body={body_len}"
        );
    }
}

fn json_rpc_method_name(req: &HttpRequest) -> Option<String> {
    if !req.method.eq_ignore_ascii_case("POST") {
        return None;
    }
    serde_json::from_slice::<Value>(&req.body)
        .ok()
        .and_then(|value| {
            value
                .get("method")
                .and_then(|method| method.as_str())
                .map(str::to_owned)
        })
}

fn is_tools_list_request(req: &HttpRequest) -> bool {
    if !req.method.eq_ignore_ascii_case("POST") {
        return false;
    }
    match serde_json::from_slice::<Value>(&req.body) {
        Ok(value) => value.get("method").and_then(|v| v.as_str()) == Some("tools/list"),
        Err(_) => false,
    }
}

async fn read_upstream_body(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut body = Vec::new();
    let limit = MAX_HTTP_BODY_BYTES + 1;
    tokio::time::timeout(
        HTTP_BODY_TIMEOUT,
        stream.take(limit as u64).read_to_end(&mut body),
    )
    .await??;
    ensure_body_within_limit(&body)?;
    Ok(body)
}

fn ensure_body_within_limit(body: &[u8]) -> anyhow::Result<()> {
    if body.len() > MAX_HTTP_BODY_BYTES {
        anyhow::bail!("upstream response body too large");
    }
    Ok(())
}

fn proxy_http_response(head: UpstreamResponseHead, body: Vec<u8>) -> HttpResponse {
    let mut headers = Vec::new();
    for (k, v) in head.headers {
        if should_skip_proxy_response_header(&k)
            || k.eq_ignore_ascii_case("content-length")
            || k.eq_ignore_ascii_case("transfer-encoding")
        {
            continue;
        }
        headers.push((k, v));
    }
    HttpResponse {
        status: head.status,
        headers,
        body,
    }
}

fn rewrite_tools_list_response(body: &[u8]) -> Option<Vec<u8>> {
    let mut json: Value = serde_json::from_slice(body).ok()?;
    let result = json.get_mut("result")?.as_object_mut()?;
    let tools = result.get("tools")?.as_array()?;
    let rewritten: Vec<Value> = tools.iter().map(chatgpt_compat_tool_entry).collect();
    result.insert("tools".to_owned(), Value::Array(rewritten));
    serde_json::to_vec(&json).ok()
}

fn tools_list_body_for_rewrite(head: &UpstreamResponseHead, body: Vec<u8>) -> Option<Vec<u8>> {
    if !is_chunked_response(head) {
        return Some(body);
    }
    decode_chunked_body(&body).ok()
}

fn is_chunked_response(head: &UpstreamResponseHead) -> bool {
    head.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding")
            && value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn decode_chunked_body(body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut cursor = 0usize;
    loop {
        let size_line_end = find_crlf(body, cursor)
            .ok_or_else(|| anyhow::anyhow!("chunked upstream body missing size terminator"))?;
        let size_line = std::str::from_utf8(&body[cursor..size_line_end])
            .map_err(|_| anyhow::anyhow!("chunk size line is not UTF-8"))?;
        let size_token = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|_| anyhow::anyhow!("invalid chunk size"))?;
        cursor = size_line_end + 2;
        if size == 0 {
            if cursor + 2 <= body.len() && &body[cursor..cursor + 2] == b"\r\n" {
                return Ok(decoded);
            }
            let trailer_end = find_double_crlf(body, cursor)
                .ok_or_else(|| anyhow::anyhow!("chunked upstream body missing final CRLF"))?;
            if trailer_end > MAX_HTTP_BODY_BYTES {
                anyhow::bail!("upstream response body too large");
            }
            return Ok(decoded);
        }
        if cursor.checked_add(size + 2).is_none() || cursor + size + 2 > body.len() {
            anyhow::bail!("chunked upstream body ended mid-chunk");
        }
        decoded.extend_from_slice(&body[cursor..cursor + size]);
        ensure_body_within_limit(&decoded)?;
        cursor += size;
        if &body[cursor..cursor + 2] != b"\r\n" {
            anyhow::bail!("chunked upstream body missing chunk terminator");
        }
        cursor += 2;
    }
}

fn find_crlf(body: &[u8], start: usize) -> Option<usize> {
    body.get(start..)?
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|pos| start + pos)
}

fn find_double_crlf(body: &[u8], start: usize) -> Option<usize> {
    body.get(start..)?
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|pos| start + pos + 4)
}

fn chatgpt_compat_tool_entry(tool: &Value) -> Value {
    let name = tool
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("tool")
        .to_owned();
    let description = tool
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let input_schema = tool
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "additionalProperties": true}));
    let annotations = tool
        .get("annotations")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let security_schemes = json!([
        {
            "type": "oauth2",
            "scopes": [DEFAULT_SCOPE]
        }
    ]);
    let mut meta = tool
        .get("_meta")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    meta.insert("securitySchemes".to_owned(), security_schemes.clone());

    json!({
        "name": name,
        "title": humanize_tool_name(&name),
        "description": description,
        "inputSchema": input_schema,
        "annotations": annotations,
        "outputSchema": {
            "type": "object",
            "additionalProperties": true
        },
        "securitySchemes": security_schemes,
        "_meta": Value::Object(meta),
    })
}

fn humanize_tool_name(name: &str) -> String {
    let words: Vec<String> = name
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = first.to_ascii_uppercase().to_string();
                    word.push_str(&chars.as_str().to_ascii_lowercase());
                    word
                }
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        "Tool".to_owned()
    } else {
        words.join(" ")
    }
}

async fn write_upstream_request(
    upstream: &mut TcpStream,
    req: &HttpRequest,
    target: &UpstreamTarget,
) -> anyhow::Result<()> {
    let path = upstream_request_path(target, &req.path);
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
        req.method,
        path,
        target.host_header(),
        req.body.len()
    );
    for (k, v) in &req.headers {
        if should_forward_upstream_request_header(k) {
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
    }
    head.push_str("\r\n");
    upstream.write_all(head.as_bytes()).await?;
    if !req.body.is_empty() {
        upstream.write_all(&req.body).await?;
    }
    upstream.flush().await?;
    Ok(())
}

fn upstream_request_path(target: &UpstreamTarget, incoming_path: &str) -> String {
    match incoming_path.split_once('?') {
        Some((_, query)) if !query.is_empty() => format!("{}?{}", target.path, query),
        _ => target.path.clone(),
    }
}

fn should_forward_upstream_request_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "accept" | "content-type" | "mcp-session-id" | "mcp-protocol-version" | "last-event-id"
    )
}

struct UpstreamResponseHead {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
}

async fn read_upstream_response_head(
    stream: &mut TcpStream,
) -> anyhow::Result<UpstreamResponseHead> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = tokio::time::timeout(HTTP_HEADER_TIMEOUT, stream.read(&mut byte)).await??;
        if n == 0 {
            anyhow::bail!("upstream closed before response headers");
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 64 * 1024 {
            anyhow::bail!("upstream response headers too large");
        }
    }

    let head_str = String::from_utf8_lossy(&head);
    let mut lines = head_str.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        anyhow::bail!("upstream did not return an HTTP response");
    }
    let status = parts
        .next()
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or_else(|| anyhow::anyhow!("upstream response missing status code"))?;
    let reason = parts.next().unwrap_or(reason(status)).to_owned();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }

    Ok(UpstreamResponseHead {
        status,
        reason,
        headers,
    })
}

async fn write_proxy_response_head(
    client: &mut TcpStream,
    head: &UpstreamResponseHead,
) -> anyhow::Result<()> {
    let mut headers = Vec::new();
    for (k, v) in &head.headers {
        if should_skip_proxy_response_header(k) {
            continue;
        }
        headers.push((k.clone(), v.clone()));
    }
    add_cors(&mut headers);
    headers.push(("Connection".into(), "close".into()));

    let mut out = format!("HTTP/1.1 {} {}\r\n", head.status, head.reason);
    for (k, v) in &headers {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    client.write_all(out.as_bytes()).await?;
    client.flush().await?;
    Ok(())
}

fn should_skip_proxy_response_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "connection" || name.starts_with("access-control-")
}

fn proxy_error_response(description: &str) -> HttpResponse {
    json_response(
        502,
        json!({"error": "mcp_upstream_error", "error_description": description}),
    )
}

fn parse_mcp_upstream(input: &str) -> Result<UpstreamTarget, String> {
    let rest = input
        .strip_prefix("http://")
        .ok_or_else(|| "only http:// loopback upstream URLs are supported".to_owned())?;
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| "upstream URL must include a path such as /mcp".to_owned())?;
    if authority.is_empty() {
        return Err("upstream URL must include a host".to_owned());
    }
    let path = format!("/{}", path);
    if path.contains('?') || path.contains('#') {
        return Err("upstream URL must not include query strings or fragments".to_owned());
    }

    let (host, port) = parse_authority(authority)?;
    if !is_loopback_host(&host) {
        return Err("upstream host must be loopback (127.0.0.1, ::1, or localhost)".to_owned());
    }
    Ok(UpstreamTarget { host, port, path })
}

fn parse_authority(authority: &str) -> Result<(String, u16), String> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| "invalid bracketed IPv6 host".to_owned())?;
        let port = match suffix.strip_prefix(':') {
            Some(port) if !port.is_empty() => parse_port(port)?,
            Some(_) => return Err("port must not be empty".to_owned()),
            None if suffix.is_empty() => 80,
            None => return Err("invalid authority".to_owned()),
        };
        return Ok((host.to_owned(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            if host.is_empty() {
                return Err("host must not be empty".to_owned());
            }
            (host.to_owned(), parse_port(port)?)
        }
        Some(_) => return Err("IPv6 hosts must be bracketed".to_owned()),
        None => (authority.to_owned(), 80),
    };
    Ok((host, port))
}

fn parse_port(port: &str) -> Result<u16, String> {
    port.parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| "port must be a positive integer".to_owned())
}

fn oauth_error(status: u16, error: &str, description: &str) -> HttpResponse {
    no_store_response(json_response(
        status,
        json!({"error": error, "error_description": description}),
    ))
}

fn bearer_error(public_url: &str, description: &str) -> HttpResponse {
    let mut resp = json_response(
        401,
        json!({"error": "invalid_token", "error_description": description}),
    );
    resp.headers.push((
        "WWW-Authenticate".to_owned(),
        format!(
            "Bearer realm=\"mcp\", error=\"invalid_token\", error_description=\"{}\", resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
            www_auth_escape(description),
            trim_trailing_slash(public_url),
        ),
    ));
    resp
}

fn json_response(status: u16, body: Value) -> HttpResponse {
    HttpResponse::new(
        status,
        "application/json",
        serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec()),
    )
}

fn no_store_response(mut response: HttpResponse) -> HttpResponse {
    response
        .headers
        .push(("Cache-Control".to_owned(), "no-store".to_owned()));
    response
        .headers
        .push(("Pragma".to_owned(), "no-cache".to_owned()));
    response
}

fn html_response(status: u16, body: String) -> HttpResponse {
    HttpResponse::new(status, "text/html; charset=utf-8", body.into_bytes())
}

fn redirect(location: &str) -> HttpResponse {
    HttpResponse {
        status: 302,
        headers: vec![("Location".to_owned(), location.to_owned())],
        body: Vec::new(),
    }
}

async fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<Option<HttpRequest>> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = tokio::time::timeout(HTTP_HEADER_TIMEOUT, stream.read(&mut byte)).await??;
        if n == 0 {
            if head.is_empty() {
                return Ok(None);
            }
            anyhow::bail!("peer closed mid-header");
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 64 * 1024 {
            anyhow::bail!("HTTP headers too large");
        }
    }

    let head_str = String::from_utf8_lossy(&head);
    let mut lines = head_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("/").to_owned();
    let version = parts.next().unwrap_or("HTTP/1.1");

    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    let mut keep_alive = version.eq_ignore_ascii_case("HTTP/1.1");
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let name = k.trim().to_ascii_lowercase();
            let value = v.trim().to_owned();
            if name == "content-length" {
                content_length = value.parse::<usize>().unwrap_or(0);
            } else if name == "connection" {
                if value.eq_ignore_ascii_case("close") {
                    keep_alive = false;
                } else if value.eq_ignore_ascii_case("keep-alive") {
                    keep_alive = true;
                }
            }
            headers.insert(name, value);
        }
    }

    if content_length > MAX_HTTP_BODY_BYTES {
        anyhow::bail!("HTTP body too large");
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        tokio::time::timeout(HTTP_BODY_TIMEOUT, stream.read_exact(&mut body)).await??;
    }

    Ok(Some(HttpRequest {
        method,
        path,
        headers,
        body,
        keep_alive,
    }))
}

async fn write_http(
    stream: &mut TcpStream,
    response: HttpResponse,
    keep_alive: bool,
) -> anyhow::Result<()> {
    let mut headers = response.headers;
    add_cors(&mut headers);
    headers.push(("Content-Length".to_owned(), response.body.len().to_string()));
    headers.push((
        "Connection".to_owned(),
        if keep_alive { "keep-alive" } else { "close" }.to_owned(),
    ));

    let mut out = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (k, v) in &headers {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes()).await?;
    if !response.body.is_empty() {
        stream.write_all(&response.body).await?;
    }
    stream.flush().await?;
    Ok(())
}

fn add_cors(headers: &mut Vec<(String, String)>) {
    headers.push(("Access-Control-Allow-Origin".to_owned(), "*".to_owned()));
    headers.push((
        "Access-Control-Allow-Methods".to_owned(),
        "GET, POST, DELETE, OPTIONS".to_owned(),
    ));
    headers.push((
        "Access-Control-Allow-Headers".to_owned(),
        "Content-Type, Authorization, MCP-Session-Id, Mcp-Session-Id, MCP-Protocol-Version, Last-Event-ID".to_owned(),
    ));
    headers.push((
        "Access-Control-Expose-Headers".to_owned(),
        "MCP-Session-Id, Mcp-Session-Id".to_owned(),
    ));
    headers.push(("Access-Control-Max-Age".to_owned(), "86400".to_owned()));
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "OK",
    }
}

fn default_storage_dir() -> PathBuf {
    home_dir().join(".cua-driver").join("oauth")
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    set_private_dir_permissions(dir)
}

fn read_json_map<T>(path: &Path) -> io::Result<HashMap<String, T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Ok(HashMap::new());
    }
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn write_json_map<T>(path: &Path, value: &HashMap<String, T>) -> io::Result<()>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(path, &bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let tmp_name = format!(
        "{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("oauth-state"),
        Uuid::new_v4()
    );
    let tmp_path = path.with_file_name(tmp_name);
    write_private_file(&tmp_path, bytes)?;
    fs::rename(&tmp_path, path)?;
    set_private_file_permissions(path)?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    set_private_file_permissions(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
fn set_private_dir_permissions(path: &Path) -> io::Result<()> {
    tighten_windows_acl(path, true)
}

#[cfg(windows)]
fn set_private_file_permissions(path: &Path) -> io::Result<()> {
    tighten_windows_acl(path, false)
}

#[cfg(windows)]
fn tighten_windows_acl(path: &Path, inherit_children: bool) -> io::Result<()> {
    let current_user = windows_current_user()?;
    let rights = if inherit_children { "(OI)(CI)F" } else { "F" };
    let current_user_grant = format!("{current_user}:{rights}");
    let admins_grant = format!("*S-1-5-32-544:{rights}");
    let system_grant = format!("*S-1-5-18:{rights}");

    let status = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(current_user_grant)
        .arg(admins_grant)
        .arg(system_grant)
        .arg("/C")
        .arg("/Q")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("failed to tighten ACLs for {}", path.display()),
        ))
    }
}

#[cfg(windows)]
fn windows_current_user() -> io::Result<String> {
    let user = std::env::var("USERNAME").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "USERNAME is required to secure OAuth state files",
        )
    })?;
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => Ok(format!("{domain}\\{user}")),
        _ => Ok(user),
    }
}

#[cfg(not(any(unix, windows)))]
fn set_private_dir_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn set_private_file_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs()
}

fn random_id() -> String {
    let mut bytes = [0u8; 32];
    if SystemRandom::new().fill(&mut bytes).is_ok() {
        return URL_SAFE_NO_PAD.encode(bytes);
    }
    URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes())
}

fn trim_trailing_slash(input: &str) -> String {
    input.trim_end_matches('/').to_owned()
}

fn verify_pkce(verifier: &str, challenge: &str, method: &str) -> bool {
    if method != "S256" {
        return false;
    }
    let digest = digest::digest(&digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.as_ref()) == challenge
}

fn is_valid_scope(scope: &str) -> bool {
    let mut parts = scope.split_whitespace();
    matches!((parts.next(), parts.next()), (Some(DEFAULT_SCOPE), None))
}

fn is_allowed_redirect_uri(uri: &str) -> bool {
    if uri.chars().any(|c| c.is_control()) || uri.contains('#') {
        return false;
    }
    let Some((scheme, rest)) = uri.split_once("://") else {
        return false;
    };
    let authority_and_path = rest.split_once('/').map(|(auth, _)| auth).unwrap_or(rest);
    let authority = authority_and_path
        .split_once('?')
        .map(|(auth, _)| auth)
        .unwrap_or(authority_and_path);
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let Ok((host, _)) = parse_authority(authority) else {
        return false;
    };
    match scheme {
        "https" => true,
        "http" => is_loopback_host(&host),
        _ => false,
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn authorize_page(pending: &PendingAuthorization, public_url: &str, nonce: &str) -> String {
    let display_resource = pending
        .resource
        .clone()
        .unwrap_or_else(|| mcp_resource(public_url));
    let form_resource = pending.resource.as_deref().unwrap_or("");
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Authorize cua-driver</title><style>body{{font-family:system-ui,sans-serif;background:#111;color:#f5f5f5;display:grid;place-items:center;min-height:100vh;margin:0}}main{{max-width:520px;padding:32px}}button{{padding:10px 16px;margin-right:8px}}</style></head><body><main><h1>Authorize cua-driver MCP</h1><p><strong>{}</strong> is requesting access to <code>{}</code>.</p><p>Scope: <code>{}</code></p><form method="post" action="/authorize"><input type="hidden" name="client_id" value="{}"><input type="hidden" name="redirect_uri" value="{}"><input type="hidden" name="scope" value="{}"><input type="hidden" name="resource" value="{}"><input type="hidden" name="code_challenge" value="{}"><input type="hidden" name="code_challenge_method" value="{}"><input type="hidden" name="state" value="{}"><input type="hidden" name="consent_nonce" value="{}"><button name="decision" value="allow">Authorize</button><button name="decision" value="deny">Deny</button></form></main></body></html>"#,
        html_escape(&pending.client_name),
        html_escape(&display_resource),
        html_escape(&pending.scope),
        html_escape(&pending.client_id),
        html_escape(&pending.redirect_uri),
        html_escape(&pending.scope),
        html_escape(form_resource),
        html_escape(&pending.code_challenge),
        html_escape(&pending.code_challenge_method),
        html_escape(pending.state.as_deref().unwrap_or("")),
        html_escape(nonce),
    )
}

fn www_auth_escape(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_query(path: &str) -> HashMap<String, String> {
    match path.split_once('?') {
        Some((_, query)) => parse_key_value_pairs(query),
        None => HashMap::new(),
    }
}

fn parse_form(body: &[u8]) -> HashMap<String, String> {
    parse_key_value_pairs(&String::from_utf8_lossy(body))
}

fn parse_key_value_pairs(input: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in input.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(url_decode(raw_key), url_decode(raw_value));
    }
    out
}

fn optional_form_value<'a>(form: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    form.get(key).map(String::as_str).filter(|v| !v.is_empty())
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(h * 16 + l);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn append_query(base: &str, pairs: &[(&str, &str)]) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    let query = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}{sep}{query}")
}

fn url_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn authorization_metadata_discovery_paths_return_authorization_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);

        for path in [
            "/.well-known/oauth-authorization-server",
            "/.well-known/openid-configuration",
        ] {
            let response = handle_http(
                HttpRequest {
                    method: "GET".to_owned(),
                    path: path.to_owned(),
                    headers: HashMap::new(),
                    body: Vec::new(),
                    keep_alive: false,
                },
                &ctx,
            )
            .await;

            assert_eq!(response.status, 200, "{path}");
            let body: Value = serde_json::from_slice(&response.body).unwrap();
            assert_eq!(body["issuer"], "https://example.test");
            assert_eq!(body["resource"], "https://example.test/mcp");
        }

        for path in [
            "/.well-known/oauth-authorization-server/mcp",
            "/mcp/.well-known/oauth-authorization-server",
            "/.well-known/openid-configuration/mcp",
            "/mcp/.well-known/openid-configuration",
        ] {
            let response = handle_http(
                HttpRequest {
                    method: "GET".to_owned(),
                    path: path.to_owned(),
                    headers: HashMap::new(),
                    body: Vec::new(),
                    keep_alive: false,
                },
                &ctx,
            )
            .await;
            assert_eq!(response.status, 404, "{path}");
        }
    }

    #[test]
    fn metadata_uses_https_public_url() {
        let meta = authorization_server_metadata("https://example.test");
        assert_eq!(meta["issuer"], "https://example.test");
        assert_eq!(
            meta["registration_endpoint"],
            "https://example.test/register"
        );
        assert_eq!(meta["code_challenge_methods_supported"], json!(["S256"]));
    }

    #[test]
    fn protected_resource_points_at_mcp() {
        let meta = protected_resource_metadata("https://example.test");
        assert_eq!(meta["resource"], "https://example.test/mcp");
        assert_eq!(
            meta["authorization_servers"],
            json!(["https://example.test"])
        );
    }

    #[test]
    fn pkce_s256_verifies_rfc_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verify_pkce(verifier, challenge, "S256"));
        assert!(!verify_pkce("wrong", challenge, "S256"));
        assert!(!verify_pkce(verifier, verifier, "plain"));
    }

    #[test]
    fn form_parser_decodes_plus_and_percent() {
        let parsed = parse_form(
            b"client_name=ChatGPT+Connector&redirect_uri=https%3A%2F%2Fexample.test%2Fcb",
        );
        assert_eq!(parsed["client_name"], "ChatGPT Connector");
        assert_eq!(parsed["redirect_uri"], "https://example.test/cb");
    }

    #[test]
    fn append_query_preserves_existing_query() {
        let url = append_query("https://example.test/cb?x=1", &[("code", "a b")]);
        assert_eq!(url, "https://example.test/cb?x=1&code=a%20b");
    }

    #[test]
    fn redirect_uri_validation_allows_https_and_loopback_http_only() {
        assert!(is_allowed_redirect_uri("https://client.test/callback"));
        assert!(is_allowed_redirect_uri("http://localhost:3000/callback"));
        assert!(is_allowed_redirect_uri("http://127.0.0.1/callback"));
        assert!(is_allowed_redirect_uri("http://[::1]:3000/callback"));

        assert!(!is_allowed_redirect_uri("http://evil.test/callback"));
        assert!(!is_allowed_redirect_uri(
            "http://127.0.0.1.evil.test/callback"
        ));
        assert!(!is_allowed_redirect_uri("http://127.evil.test/callback"));
        assert!(!is_allowed_redirect_uri(
            "https://client.test/callback#frag"
        ));
        assert!(!is_allowed_redirect_uri(
            "https://client.test/\r\nX-Bad: yes"
        ));
        assert!(!is_allowed_redirect_uri("javascript:alert(1)"));
        assert!(!is_allowed_redirect_uri(
            "https://user@client.test/callback"
        ));
    }

    #[test]
    fn read_json_map_reports_malformed_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clients.json");
        fs::write(&path, b"{not-json").unwrap();

        let err = read_json_map::<Client>(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn oversized_upstream_body_is_rejected_before_rewrite() {
        let body = vec![0u8; MAX_HTTP_BODY_BYTES + 1];
        assert!(ensure_body_within_limit(&body).is_err());
    }

    #[test]
    fn decode_chunked_body_handles_extensions_and_trailers() {
        let decoded =
            decode_chunked_body(b"8;ext=value\r\n{\"ok\":1}\r\n0\r\nX-Trailer: ignored\r\n\r\n")
                .unwrap();
        assert_eq!(decoded, br#"{"ok":1}"#);
    }

    #[test]
    fn authorize_requires_code_response_type_and_supported_scope() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);
        ctx.store
            .upsert_client(
                "client",
                Client {
                    client_secret: "secret".to_owned(),
                    client_name: "test".to_owned(),
                    redirect_uris: vec!["https://client.test/callback".to_owned()],
                    token_endpoint_auth_method: "client_secret_post".to_owned(),
                    created_at: now_secs(),
                },
            )
            .unwrap();

        let mut params = HashMap::from([
            ("client_id".to_owned(), "client".to_owned()),
            (
                "redirect_uri".to_owned(),
                "https://client.test/callback".to_owned(),
            ),
            ("response_type".to_owned(), "token".to_owned()),
            ("scope".to_owned(), DEFAULT_SCOPE.to_owned()),
            ("code_challenge".to_owned(), "challenge".to_owned()),
            ("code_challenge_method".to_owned(), "S256".to_owned()),
        ]);
        let err = validate_authorize_query(&params, &ctx).unwrap_err();
        assert_eq!(err.status, 400);
        let body: Value = serde_json::from_slice(&err.body).unwrap();
        assert_eq!(body["error"], "unsupported_response_type");

        params.insert("response_type".to_owned(), "code".to_owned());
        params.insert("scope".to_owned(), "mcp:tools admin".to_owned());
        let err = validate_authorize_query(&params, &ctx).unwrap_err();
        assert_eq!(err.status, 400);
        let body: Value = serde_json::from_slice(&err.body).unwrap();
        assert_eq!(body["error"], "invalid_scope");
    }

    #[tokio::test]
    async fn register_rejects_unsafe_redirect_uris() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);

        for uri in [
            "http://client.test/callback",
            "https://client.test/callback#frag",
            "https://client.test/\r\nLocation: https://evil.test/",
        ] {
            let register = handle_http(
                HttpRequest {
                    method: "POST".to_owned(),
                    path: "/register".to_owned(),
                    headers: HashMap::new(),
                    body: serde_json::to_vec(&json!({
                        "client_name": "bad",
                        "redirect_uris": [uri],
                    }))
                    .unwrap(),
                    keep_alive: false,
                },
                &ctx,
            )
            .await;
            assert_eq!(register.status, 400);
            let body: Value = serde_json::from_slice(&register.body).unwrap();
            assert_eq!(body["error"], "invalid_redirect_uri");
        }
    }

    #[tokio::test]
    async fn register_and_token_responses_are_not_cacheable() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), false);

        let register = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/register".to_owned(),
                headers: HashMap::new(),
                body: br#"{"client_name":"ChatGPT","redirect_uris":["https://client.test/callback"],"token_endpoint_auth_method":"client_secret_post"}"#.to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(register.status, 201);
        assert_eq!(header_value(&register, "Cache-Control"), Some("no-store"));
        assert_eq!(header_value(&register, "Pragma"), Some("no-cache"));
        let reg_body: Value = serde_json::from_slice(&register.body).unwrap();

        let client_id = reg_body["client_id"].as_str().unwrap();
        let client_secret = reg_body["client_secret"].as_str().unwrap();
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let authorize_path = format!(
            "/authorize?response_type=code&client_id={client_id}&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge={challenge}&code_challenge_method=S256"
        );
        let authorize = handle_http(
            HttpRequest {
                method: "GET".to_owned(),
                path: authorize_path,
                headers: HashMap::new(),
                body: Vec::new(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        let location = header_value(&authorize, "Location").unwrap();
        let code = parse_query(location).remove("code").unwrap();

        let token_body = format!(
            "grant_type=authorization_code&code={code}&client_id={client_id}&client_secret={client_secret}&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&code_verifier={verifier}"
        );
        let token = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/token".to_owned(),
                headers: HashMap::new(),
                body: token_body.into_bytes(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(token.status, 200);
        assert_eq!(header_value(&token, "Cache-Control"), Some("no-store"));
        assert_eq!(header_value(&token, "Pragma"), Some("no-cache"));
    }

    #[tokio::test]
    async fn consent_post_requires_nonce_and_rejects_replay() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);
        ctx.store
            .upsert_client(
                "client",
                Client {
                    client_secret: "secret".to_owned(),
                    client_name: "test".to_owned(),
                    redirect_uris: vec!["https://client.test/callback".to_owned()],
                    token_endpoint_auth_method: "client_secret_post".to_owned(),
                    created_at: now_secs(),
                },
            )
            .unwrap();

        let authorize_path = "/authorize?response_type=code&client_id=client&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&state=abc&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge=challenge&code_challenge_method=S256";
        let authorize = handle_http(
            HttpRequest {
                method: "GET".to_owned(),
                path: authorize_path.to_owned(),
                headers: HashMap::new(),
                body: Vec::new(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(authorize.status, 200);
        let page = String::from_utf8(authorize.body).unwrap();
        let nonce = extract_hidden_value(&page, "consent_nonce").unwrap();

        let without_nonce = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/authorize".to_owned(),
                headers: HashMap::new(),
                body: b"decision=allow&client_id=client&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&state=abc&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge=challenge&code_challenge_method=S256".to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(without_nonce.status, 400);

        let form = format!(
            "decision=allow&client_id=client&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&state=abc&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge=challenge&code_challenge_method=S256&consent_nonce={nonce}"
        );
        let approved = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/authorize".to_owned(),
                headers: HashMap::new(),
                body: form.clone().into_bytes(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(approved.status, 302);

        let replay = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/authorize".to_owned(),
                headers: HashMap::new(),
                body: form.into_bytes(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(replay.status, 400);
    }

    #[tokio::test]
    async fn consent_deny_validates_nonce_before_redirecting() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);
        let pending = PendingAuthorization {
            client_id: "client".to_owned(),
            client_name: "test".to_owned(),
            redirect_uri: "https://client.test/callback".to_owned(),
            scope: DEFAULT_SCOPE.to_owned(),
            state: Some("safe-state".to_owned()),
            resource: Some("https://example.test/mcp".to_owned()),
            code_challenge: "challenge".to_owned(),
            code_challenge_method: "S256".to_owned(),
        };
        ctx.store.upsert_consent("deny-nonce", &pending).unwrap();

        let denied = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/authorize".to_owned(),
                headers: HashMap::new(),
                body: b"decision=deny&client_id=client&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&state=safe-state&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge=challenge&code_challenge_method=S256&consent_nonce=deny-nonce".to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(denied.status, 302);
        let location = header_value(&denied, "Location").unwrap();
        assert!(location.starts_with("https://client.test/callback?"));
        assert!(location.contains("error=access_denied"));
        assert!(location.contains("state=safe-state"));

        ctx.store.upsert_consent("evil-nonce", &pending).unwrap();
        let tampered = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/authorize".to_owned(),
                headers: HashMap::new(),
                body: b"decision=deny&client_id=client&redirect_uri=https%3A%2F%2Fevil.test%2Fcallback&scope=mcp%3Atools&state=safe-state&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge=challenge&code_challenge_method=S256&consent_nonce=evil-nonce".to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(tampered.status, 400);
        assert!(header_value(&tampered, "Location").is_none());
        assert!(ctx.store.consume_consent("evil-nonce").unwrap().is_none());
    }

    #[tokio::test]
    async fn consent_post_rejects_expired_nonce_and_prunes_it() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);
        let pending = PendingAuthorization {
            client_id: "client".to_owned(),
            client_name: "test".to_owned(),
            redirect_uri: "https://client.test/callback".to_owned(),
            scope: DEFAULT_SCOPE.to_owned(),
            state: Some("abc".to_owned()),
            resource: Some("https://example.test/mcp".to_owned()),
            code_challenge: "challenge".to_owned(),
            code_challenge_method: "S256".to_owned(),
        };
        let nonce = "expired-consent";
        ctx.store
            .upsert_consent_with_created_at(
                nonce,
                &pending,
                now_secs() - ctx.store.code_ttl_seconds - 1,
            )
            .unwrap();

        let form = format!(
            "decision=allow&client_id=client&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&state=abc&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge=challenge&code_challenge_method=S256&consent_nonce={nonce}"
        );
        let expired = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/authorize".to_owned(),
                headers: HashMap::new(),
                body: form.into_bytes(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(expired.status, 400);
        let body: Value = serde_json::from_slice(&expired.body).unwrap();
        assert_eq!(
            body["error_description"],
            "unknown or expired consent transaction"
        );
        assert!(ctx.store.consume_consent(nonce).unwrap().is_none());
    }

    #[tokio::test]
    async fn oauth_flow_register_authorize_token_and_reject_replay() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = HandlerContext {
            options: Options {
                public_url: "https://example.test".to_owned(),
                listen: DEFAULT_LISTEN.to_owned(),
                mcp_upstream: DEFAULT_MCP_UPSTREAM.to_owned(),
                storage_dir: Some(dir.path().to_path_buf()),
                token_ttl_seconds: 3600,
                code_ttl_seconds: 300,
                require_user_consent: false,
            },
            store: std::sync::Arc::new(JsonStore::new(dir.path().to_path_buf(), 3600, 300)),
            upstream: parse_mcp_upstream(DEFAULT_MCP_UPSTREAM).unwrap(),
            trace: false,
        };

        let register = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/register".to_owned(),
                headers: HashMap::new(),
                body: br#"{"client_name":"ChatGPT","redirect_uris":["https://client.test/callback"],"token_endpoint_auth_method":"client_secret_post"}"#.to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(register.status, 201);
        let reg_body: Value = serde_json::from_slice(&register.body).unwrap();
        let client_id = reg_body["client_id"].as_str().unwrap();
        let client_secret = reg_body["client_secret"].as_str().unwrap();

        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        let authorize_path = format!(
            "/authorize?response_type=code&client_id={client_id}&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&scope=mcp%3Atools&state=abc&resource=https%3A%2F%2Fexample.test%2Fmcp&code_challenge={challenge}&code_challenge_method=S256"
        );
        let authorize = handle_http(
            HttpRequest {
                method: "GET".to_owned(),
                path: authorize_path,
                headers: HashMap::new(),
                body: Vec::new(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(authorize.status, 302);
        let location = authorize
            .headers
            .iter()
            .find(|(k, _)| k == "Location")
            .map(|(_, v)| v.clone())
            .unwrap();
        let query = parse_query(&location);
        let code = query.get("code").unwrap();
        assert_eq!(query.get("state").map(String::as_str), Some("abc"));

        let token_body = format!(
            "grant_type=authorization_code&code={code}&client_id={client_id}&client_secret={client_secret}&redirect_uri=https%3A%2F%2Fclient.test%2Fcallback&code_verifier={verifier}"
        );
        let token = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/token".to_owned(),
                headers: HashMap::new(),
                body: token_body.clone().into_bytes(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(token.status, 200);
        let token_body_json: Value = serde_json::from_slice(&token.body).unwrap();
        assert_eq!(token_body_json["token_type"], "Bearer");
        assert!(token_body_json["access_token"].as_str().is_some());
        assert_eq!(token_body_json["resource"], "https://example.test/mcp");

        let replay = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/token".to_owned(),
                headers: HashMap::new(),
                body: token_body.into_bytes(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(replay.status, 400);
        let replay_body: Value = serde_json::from_slice(&replay.body).unwrap();
        assert_eq!(replay_body["error"], "invalid_grant");
    }

    #[test]
    fn authorization_code_redeem_is_single_use_under_concurrency() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(JsonStore::new(dir.path().to_path_buf(), 3600, 300));
        store
            .upsert_code(
                "code",
                AuthCode {
                    client_id: "client".to_owned(),
                    redirect_uri: "https://client.test/callback".to_owned(),
                    code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned(),
                    code_challenge_method: "S256".to_owned(),
                    scope: DEFAULT_SCOPE.to_owned(),
                    resource: Some("https://example.test/mcp".to_owned()),
                    created_at: now_secs(),
                    used: false,
                },
            )
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    matches!(
                        store
                            .redeem_code_for_token(
                                "code",
                                "client",
                                "https://client.test/callback",
                                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
                                Some("https://example.test/mcp"),
                            )
                            .unwrap(),
                        TokenRedeem::Issued(_)
                    )
                })
            })
            .collect();

        let issued = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|issued| *issued)
            .count();
        assert_eq!(issued, 1);
    }

    #[tokio::test]
    async fn mcp_proxy_forwards_post_request_without_rewriting_body_or_mcp_headers() {
        let (upstream_url, captured) =
            spawn_json_upstream(br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#.to_vec()).await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_upstream(dir.path(), upstream_url);
        upsert_valid_token(&ctx, "token", Some("https://example.test/mcp"));

        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let request = format!(
            "POST /mcp?trace=1 HTTP/1.1\r\nHost: example.test\r\nAuthorization: Bearer token\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nMCP-Session-Id: session-1\r\nMCP-Protocol-Version: 2025-06-18\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        let response = send_raw_to_oauth(ctx, request.into_bytes()).await;
        let response_text = String::from_utf8_lossy(&response);

        assert!(
            response_text.starts_with("HTTP/1.1 200 OK"),
            "{response_text}"
        );
        assert!(response_text.contains("MCP-Session-Id: upstream-session"));
        assert!(
            response_text.contains("Access-Control-Expose-Headers: MCP-Session-Id, Mcp-Session-Id")
        );
        assert!(response_text.contains(r#""ok":true"#));

        let forwarded = captured.await.unwrap();
        assert_eq!(forwarded.method, "POST");
        assert_eq!(forwarded.path, "/mcp?trace=1");
        assert_eq!(forwarded.body, body);
        assert!(forwarded.headers.get("authorization").is_none());
        assert_ne!(
            forwarded.headers.get("host").map(String::as_str),
            Some("example.test")
        );
        assert_eq!(
            forwarded.headers.get("mcp-session-id").map(String::as_str),
            Some("session-1")
        );
        assert_eq!(
            forwarded
                .headers
                .get("mcp-protocol-version")
                .map(String::as_str),
            Some("2025-06-18")
        );
    }

    #[tokio::test]
    async fn mcp_proxy_forwards_delete_request_without_rewriting_path() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_http_request(&mut stream).await.unwrap().unwrap();
            let _ = tx.send(req);
            stream
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
                )
                .await
                .unwrap();
            stream.flush().await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_upstream(dir.path(), upstream_url);
        upsert_valid_token(&ctx, "token", Some("https://example.test/mcp"));
        let request = b"DELETE /mcp?trace=delete HTTP/1.1\r\nHost: example.test\r\nAuthorization: Bearer token\r\nMCP-Session-Id: session-9\r\n\r\n".to_vec();
        let response = send_raw_to_oauth(ctx, request).await;
        let response_text = String::from_utf8_lossy(&response);
        assert!(response_text.starts_with("HTTP/1.1 202 Accepted"));

        let forwarded = rx.await.unwrap();
        assert_eq!(forwarded.method, "DELETE");
        assert_eq!(forwarded.path, "/mcp?trace=delete");
        assert_eq!(
            forwarded.headers.get("mcp-session-id").map(String::as_str),
            Some("session-9")
        );
    }

    #[tokio::test]
    async fn options_mcp_returns_cors_without_bearer_token() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);
        let response = send_raw_to_oauth(
            ctx,
            b"OPTIONS /mcp HTTP/1.1\r\nHost: example.test\r\n\r\n".to_vec(),
        )
        .await;
        let response_text = String::from_utf8_lossy(&response);

        assert!(
            response_text.starts_with("HTTP/1.1 204 No Content"),
            "{response_text}"
        );
        assert!(response_text.contains("Access-Control-Allow-Origin: *"));
        assert!(response_text.contains("Access-Control-Allow-Headers:"));
    }

    #[tokio::test]
    async fn mcp_proxy_streams_sse_without_buffering_complete_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut stream).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nMCP-Session-Id: stream-session\r\nCache-Control: no-cache\r\n\r\n: first\n\n",
                )
                .await
                .unwrap();
            stream.flush().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_upstream(dir.path(), upstream_url);
        upsert_valid_token(&ctx, "token", Some("https://example.test/mcp"));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_ctx = ctx.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_conn(stream, server_ctx).await.unwrap();
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(
                b"GET /mcp HTTP/1.1\r\nHost: example.test\r\nAuthorization: Bearer token\r\nAccept: text/event-stream\r\n\r\n",
            )
            .await
            .unwrap();

        let bytes = read_until_contains(&mut client, b": first\n\n", Duration::from_secs(1)).await;
        let response_text = String::from_utf8_lossy(&bytes);
        assert!(
            response_text.starts_with("HTTP/1.1 200 OK"),
            "{response_text}"
        );
        assert!(response_text.contains("Content-Type: text/event-stream"));
        assert!(response_text.contains("MCP-Session-Id: stream-session"));
        assert!(response_text.contains(": first\n\n"));
    }

    #[tokio::test]
    async fn tools_list_chunked_response_is_decoded_before_rewrite() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"list_windows","description":"sample","inputSchema":{"type":"object"},"capabilities":["window.list"]}],"capability_version":"1","schema_version":"1"}}"#;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut stream).await.unwrap();
            let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
            stream.write_all(head).await.unwrap();
            let split = body.len() / 2;
            for chunk in [&body[..split], &body[split..]] {
                stream
                    .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
                    .await
                    .unwrap();
                stream.write_all(chunk).await.unwrap();
                stream.write_all(b"\r\n").await.unwrap();
            }
            stream.write_all(b"0\r\n\r\n").await.unwrap();
            stream.flush().await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_upstream(dir.path(), upstream_url);
        upsert_valid_token(&ctx, "token", Some("https://example.test/mcp"));
        let request_body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: example.test\r\nAuthorization: Bearer token\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            request_body.len(),
            String::from_utf8_lossy(request_body)
        );
        let response = send_raw_to_oauth(ctx, request.into_bytes()).await;
        let response_text = String::from_utf8_lossy(&response);
        assert!(response_text.starts_with("HTTP/1.1 200 OK"));
        assert!(!response_text.contains("Transfer-Encoding: chunked"));
        let response_json: Value = serde_json::from_slice(http_body(&response)).unwrap();
        assert_eq!(response_json["result"]["tools"][0]["title"], "List Windows");
        assert!(response_json["result"]["tools"][0]
            .get("capabilities")
            .is_none());
    }

    #[test]
    fn tools_list_chatgpt_wrapper_adds_required_fields() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"list_windows","description":"sample","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true},"capabilities":["window.list"]}],"capability_version":"1","schema_version":"1"}}"#;
        let rewritten = rewrite_tools_list_response(body).expect("tools/list should rewrite");
        let json: Value = serde_json::from_slice(&rewritten).unwrap();
        let tool = &json["result"]["tools"][0];

        assert_eq!(tool["name"], "list_windows");
        assert_eq!(tool["title"], "List Windows");
        assert_eq!(tool["outputSchema"]["type"], "object");
        assert_eq!(
            tool["outputSchema"]["additionalProperties"],
            Value::Bool(true)
        );
        assert_eq!(
            tool["securitySchemes"],
            json!([{ "type": "oauth2", "scopes": [DEFAULT_SCOPE] }])
        );
        assert_eq!(
            tool["_meta"]["securitySchemes"],
            json!([{ "type": "oauth2", "scopes": [DEFAULT_SCOPE] }])
        );
        assert!(tool.get("capabilities").is_none());
    }

    #[test]
    fn tools_list_chatgpt_wrapper_preserves_top_level_versions() {
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"sample_tool","description":"sample","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":false},"capabilities":["input.pointer.click"]}],"capability_version":"1","schema_version":"1"}}"#;
        let rewritten = rewrite_tools_list_response(body).expect("tools/list should rewrite");
        let json: Value = serde_json::from_slice(&rewritten).unwrap();

        assert_eq!(json["result"]["capability_version"], "1");
        assert_eq!(json["result"]["schema_version"], "1");
    }

    #[tokio::test]
    async fn non_tools_list_proxy_path_remains_streaming_passthrough() {
        let (upstream_url, _captured) = spawn_json_upstream(
            br#"{"jsonrpc":"2.0","id":7,"result":{"capabilities":["keep-me"],"ok":true}}"#.to_vec(),
        )
        .await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx_with_upstream(dir.path(), upstream_url);
        upsert_valid_token(&ctx, "token", Some("https://example.test/mcp"));

        let body = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_windows","arguments":{}}}"#;
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: example.test\r\nAuthorization: Bearer token\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        let response = send_raw_to_oauth(ctx, request.into_bytes()).await;
        let response_json: Value = serde_json::from_slice(http_body(&response)).unwrap();

        assert_eq!(response_json["id"], 7);
        assert_eq!(response_json["result"]["ok"], Value::Bool(true));
        assert_eq!(response_json["result"]["capabilities"], json!(["keep-me"]));
    }

    #[test]
    fn bearer_token_must_match_mcp_resource_audience() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(dir.path(), true);
        upsert_valid_token(&ctx, "wrong", Some("https://other.test/mcp"));
        upsert_valid_token(&ctx, "missing", None);

        for token in ["wrong", "missing"] {
            let req = HttpRequest {
                method: "POST".to_owned(),
                path: "/mcp".to_owned(),
                headers: HashMap::from([("authorization".to_owned(), format!("Bearer {token}"))]),
                body: Vec::new(),
                keep_alive: false,
            };
            let err = authorize_bearer(&req, &ctx).unwrap_err();
            assert_eq!(err.status, 401);
            let body: Value = serde_json::from_slice(&err.body).unwrap();
            assert_eq!(body["error"], "invalid_token");
        }
    }

    fn test_ctx(dir: &Path, require_user_consent: bool) -> HandlerContext {
        test_ctx_with_upstream(dir, DEFAULT_MCP_UPSTREAM.to_owned())
            .with_consent(require_user_consent)
    }

    fn test_ctx_with_upstream(dir: &Path, upstream_url: String) -> HandlerContext {
        let upstream = parse_mcp_upstream(&upstream_url).unwrap();
        HandlerContext {
            options: Options {
                public_url: "https://example.test".to_owned(),
                listen: DEFAULT_LISTEN.to_owned(),
                mcp_upstream: upstream_url,
                storage_dir: Some(dir.to_path_buf()),
                token_ttl_seconds: 3600,
                code_ttl_seconds: 300,
                require_user_consent: true,
            },
            store: std::sync::Arc::new(JsonStore::new(dir.to_path_buf(), 3600, 300)),
            upstream,
            trace: false,
        }
    }

    trait TestContextExt {
        fn with_consent(self, require_user_consent: bool) -> Self;
    }

    impl TestContextExt for HandlerContext {
        fn with_consent(mut self, require_user_consent: bool) -> Self {
            self.options.require_user_consent = require_user_consent;
            self
        }
    }

    fn extract_hidden_value(page: &str, name: &str) -> Option<String> {
        let needle = format!(r#"name="{name}" value=""#);
        let start = page.find(&needle)? + needle.len();
        let end = page[start..].find('"')?;
        Some(page[start..start + end].to_owned())
    }

    fn upsert_valid_token(ctx: &HandlerContext, token: &str, resource: Option<&str>) {
        ctx.store
            .upsert_token(
                token,
                AccessToken {
                    client_id: "client".to_owned(),
                    scope: DEFAULT_SCOPE.to_owned(),
                    resource: resource.map(str::to_owned),
                    created_at: now_secs(),
                    expires_at: now_secs().saturating_add(3600),
                },
            )
            .unwrap();
    }

    fn header_value<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    async fn spawn_json_upstream(
        body: Vec<u8>,
    ) -> (String, tokio::sync::oneshot::Receiver<HttpRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let req = read_http_request(&mut stream).await.unwrap().unwrap();
            let _ = tx.send(req);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMCP-Session-Id: upstream-session\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
            stream.flush().await.unwrap();
        });
        (url, rx)
    }

    async fn send_raw_to_oauth(ctx: HandlerContext, request: Vec<u8>) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_conn(stream, ctx).await.unwrap();
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(&request).await.unwrap();
        let _ = client.shutdown().await;
        let mut response = Vec::new();
        match client.read_to_end(&mut response).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(e) => panic!("failed to read OAuth response: {e}"),
        }
        response
    }

    async fn read_until_contains(
        stream: &mut TcpStream,
        needle: &[u8],
        timeout: Duration,
    ) -> Vec<u8> {
        tokio::time::timeout(timeout, async {
            let mut out = Vec::new();
            let mut buf = [0u8; 256];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
                if out.windows(needle.len()).any(|window| window == needle) {
                    break;
                }
            }
            out
        })
        .await
        .expect("timed out waiting for streamed response")
    }

    fn http_body(response: &[u8]) -> &[u8] {
        let marker = b"\r\n\r\n";
        let start = response
            .windows(marker.len())
            .position(|window| window == marker)
            .map(|pos| pos + marker.len())
            .expect("HTTP response has a header terminator");
        &response[start..]
    }
}
