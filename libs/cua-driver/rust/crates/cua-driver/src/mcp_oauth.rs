//! Experimental OAuth 2.0 front door for the local MCP HTTP transport.
//!
//! This is intentionally opt-in and loopback-oriented: callers expose it through
//! their own HTTPS tunnel if they want ChatGPT-style OAuth/DCR discovery.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cua_driver_core::protocol::{Request, Response};
use cua_driver_core::server::handle_request;
use cua_driver_core::tool::ToolRegistry;
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

const DEFAULT_LISTEN: &str = "127.0.0.1:7676";
const DEFAULT_SCOPE: &str = "mcp:tools";

#[derive(Clone, Debug)]
pub struct Options {
    pub public_url: String,
    pub listen: String,
    pub storage_dir: Option<PathBuf>,
    pub token_ttl_seconds: u64,
    pub code_ttl_seconds: u64,
    pub require_user_consent: bool,
}

impl Options {
    pub fn new(
        public_url: String,
        listen: Option<String>,
        storage_dir: Option<String>,
        token_ttl_seconds: Option<u64>,
        code_ttl_seconds: Option<u64>,
        require_user_consent: bool,
    ) -> Self {
        Self {
            public_url: trim_trailing_slash(&public_url),
            listen: listen.unwrap_or_else(|| DEFAULT_LISTEN.to_owned()),
            storage_dir: storage_dir.map(PathBuf::from),
            token_ttl_seconds: token_ttl_seconds.unwrap_or(86_400),
            code_ttl_seconds: code_ttl_seconds.unwrap_or(300),
            require_user_consent,
        }
    }
}

pub fn run(options: Options, registry: Arc<ToolRegistry>) -> anyhow::Result<()> {
    if !options.public_url.starts_with("https://") {
        anyhow::bail!("mcp-oauth requires --public-url to start with https://");
    }
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
    rt.block_on(async move { serve(options, registry, addr).await })
}

async fn serve(
    options: Options,
    registry: Arc<ToolRegistry>,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    let store = Arc::new(JsonStore::new(
        options
            .storage_dir
            .clone()
            .unwrap_or_else(default_storage_dir),
        options.token_ttl_seconds,
        options.code_ttl_seconds,
    ));
    fs::create_dir_all(&store.dir)?;
    let listener = TcpListener::bind(addr).await?;
    eprintln!(
        "cua-driver mcp-oauth listening on http://{}; configure your HTTPS tunnel to target this address",
        addr
    );
    loop {
        let (stream, _) = listener.accept().await?;
        let ctx = HandlerContext {
            options: options.clone(),
            registry: registry.clone(),
            store: store.clone(),
        };
        tokio::spawn(async move {
            let _ = serve_conn(stream, ctx).await;
        });
    }
}

#[derive(Clone)]
struct HandlerContext {
    options: Options,
    registry: Arc<ToolRegistry>,
    store: Arc<JsonStore>,
}

async fn serve_conn(mut stream: TcpStream, ctx: HandlerContext) -> anyhow::Result<()> {
    loop {
        let Some(req) = read_http_request(&mut stream).await? else {
            return Ok(());
        };
        let keep_alive = req.keep_alive;
        let resp = handle_http(req, &ctx).await;
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
        ("GET", "/.well-known/oauth-authorization-server") => {
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
        (_, "/mcp") => handle_mcp(req, ctx).await,
        _ => json_response(404, json!({"error": "not_found"})),
    }
}

fn authorization_server_metadata(public_url: &str) -> Value {
    json!({
        "issuer": public_url,
        "authorization_endpoint": format!("{public_url}/authorize"),
        "token_endpoint": format!("{public_url}/token"),
        "registration_endpoint": format!("{public_url}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "scopes_supported": [DEFAULT_SCOPE],
    })
}

fn protected_resource_metadata(public_url: &str) -> Value {
    json!({
        "resource": format!("{public_url}/mcp"),
        "authorization_servers": [public_url],
        "bearer_methods_supported": ["header"],
        "scopes_supported": [DEFAULT_SCOPE],
    })
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
            let uris: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
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
    json_response(
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
    )
}

fn handle_authorize_get(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let query = parse_query(&req.path);
    match validate_authorize_query(&query, ctx) {
        Ok(pending) if !ctx.options.require_user_consent => issue_authorization_code(pending, ctx),
        Ok(pending) => html_response(200, authorize_page(&pending, &ctx.options.public_url)),
        Err(resp) => resp,
    }
}

fn handle_authorize_post(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let form = parse_form(&req.body);
    let decision = form.get("decision").map(String::as_str).unwrap_or("deny");
    let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
    let state = form.get("state").cloned().unwrap_or_default();
    if decision != "allow" {
        return redirect(&append_query(
            &redirect_uri,
            &[("error", "access_denied"), ("state", &state)],
        ));
    }
    match validate_authorize_query(&form, ctx) {
        Ok(pending) => issue_authorization_code(pending, ctx),
        Err(resp) => resp,
    }
}

fn validate_authorize_query(
    params: &HashMap<String, String>,
    ctx: &HandlerContext,
) -> Result<PendingAuthorization, HttpResponse> {
    let client_id = required(params, "client_id")?;
    let redirect_uri = required(params, "redirect_uri")?;
    let code_challenge = required(params, "code_challenge")?;
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
    let resource = params.get("resource").cloned();
    let expected_resource = format!("{}/mcp", ctx.options.public_url);
    if let Some(r) = &resource {
        if r != &expected_resource {
            return Err(oauth_error(
                400,
                "invalid_target",
                "resource does not match this MCP endpoint",
            ));
        }
    }
    Ok(PendingAuthorization {
        client_id,
        client_name: client.client_name.clone(),
        redirect_uri,
        scope: params
            .get("scope")
            .cloned()
            .unwrap_or_else(|| DEFAULT_SCOPE.to_owned()),
        state: params.get("state").cloned(),
        resource,
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
    let mut codes = match ctx.store.load_codes() {
        Ok(v) => v,
        Err(e) => return oauth_error(500, "server_error", &e.to_string()),
    };
    let Some(stored) = codes.get_mut(&code) else {
        return oauth_error(400, "invalid_grant", "unknown authorization code");
    };
    if stored.used {
        return oauth_error(
            400,
            "invalid_grant",
            "authorization code has already been used",
        );
    }
    if now_secs().saturating_sub(stored.created_at) > ctx.store.code_ttl_seconds {
        return oauth_error(400, "invalid_grant", "authorization code has expired");
    }
    if stored.client_id != client_id || stored.redirect_uri != redirect_uri {
        return oauth_error(400, "invalid_grant", "authorization code binding mismatch");
    }
    if !verify_pkce(
        &verifier,
        &stored.code_challenge,
        &stored.code_challenge_method,
    ) {
        return oauth_error(400, "invalid_grant", "PKCE verification failed");
    }
    let token_scope = stored.scope.clone();
    let token_resource = stored.resource.clone();
    stored.used = true;
    if let Err(e) = ctx.store.save_codes(&codes) {
        return oauth_error(500, "server_error", &e.to_string());
    }
    let access_token = random_id();
    let now = now_secs();
    let token = AccessToken {
        client_id,
        scope: token_scope,
        resource: token_resource,
        created_at: now,
        expires_at: now.saturating_add(ctx.store.token_ttl_seconds),
    };
    if let Err(e) = ctx.store.upsert_token(&access_token, token) {
        return oauth_error(500, "server_error", &e.to_string());
    }
    json_response(
        200,
        json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": ctx.store.token_ttl_seconds,
            "scope": DEFAULT_SCOPE,
        }),
    )
}

async fn handle_mcp(req: HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    if !req.method.eq_ignore_ascii_case("POST") {
        return json_response(
            405,
            json!({"error": "method_not_allowed", "error_description": "Use POST /mcp with a JSON-RPC body"}),
        );
    }
    match authorize_bearer(&req, ctx) {
        Ok(()) => {}
        Err(resp) => return resp,
    }
    match dispatch_mcp(&req.body, &ctx.registry).await {
        Some(body) => HttpResponse::new(200, "application/json", body.into_bytes()),
        None => HttpResponse::new(202, "application/json", Vec::new()),
    }
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
        .ok_or_else(|| bearer_error("Missing Bearer token"))?;
    let mut tokens = ctx
        .store
        .load_tokens()
        .map_err(|e| oauth_error(500, "server_error", &e.to_string()))?;
    ctx.store.prune_tokens(&mut tokens);
    let Some(stored) = tokens.get(token) else {
        return Err(bearer_error("Unknown token"));
    };
    if now_secs() > stored.expires_at {
        return Err(bearer_error("Expired token"));
    }
    if stored.scope.split_whitespace().all(|s| s != DEFAULT_SCOPE) {
        return Err(bearer_error("Token does not carry mcp:tools scope"));
    }
    Ok(())
}

async fn dispatch_mcp(body: &[u8], registry: &Arc<ToolRegistry>) -> Option<String> {
    let req: Request = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => return Some(serialize(&Response::parse_error())),
    };
    if req.id.is_none() {
        return None;
    }
    let id = req.id.clone().unwrap_or(Value::Null);
    Some(serialize(&handle_request(req, id, registry).await))
}

fn serialize(resp: &Response) -> String {
    serde_json::to_string(resp).unwrap_or_else(|e| {
        format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#)
    })
}

#[derive(Clone, Debug)]
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

fn authorize_page(pending: &PendingAuthorization, public_url: &str) -> String {
    let resource = pending
        .resource
        .clone()
        .unwrap_or_else(|| format!("{public_url}/mcp"));
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Authorize cua-driver</title><style>body{{font-family:system-ui,sans-serif;background:#111;color:#f5f5f5;display:grid;place-items:center;min-height:100vh;margin:0}}main{{max-width:520px;padding:32px}}button{{padding:10px 16px;margin-right:8px}}</style></head><body><main><h1>Authorize cua-driver MCP</h1><p><strong>{}</strong> is requesting access to <code>{}</code>.</p><p>Scope: <code>{}</code></p><form method="post" action="/authorize"><input type="hidden" name="client_id" value="{}"><input type="hidden" name="redirect_uri" value="{}"><input type="hidden" name="scope" value="{}"><input type="hidden" name="resource" value="{}"><input type="hidden" name="code_challenge" value="{}"><input type="hidden" name="code_challenge_method" value="{}"><input type="hidden" name="state" value="{}"><button name="decision" value="allow">Authorize</button><button name="decision" value="deny">Deny</button></form></main></body></html>"#,
        html_escape(&pending.client_name),
        html_escape(&resource),
        html_escape(&pending.scope),
        html_escape(&pending.client_id),
        html_escape(&pending.redirect_uri),
        html_escape(&pending.scope),
        html_escape(&resource),
        html_escape(&pending.code_challenge),
        html_escape(&pending.code_challenge_method),
        html_escape(pending.state.as_deref().unwrap_or("")),
    )
}

#[derive(Clone, Debug)]
struct JsonStore {
    dir: PathBuf,
    token_ttl_seconds: u64,
    code_ttl_seconds: u64,
}

impl JsonStore {
    fn new(dir: PathBuf, token_ttl_seconds: u64, code_ttl_seconds: u64) -> Self {
        Self {
            dir,
            token_ttl_seconds,
            code_ttl_seconds,
        }
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

    fn load_clients(&self) -> io::Result<HashMap<String, Client>> {
        read_json_map(&self.clients_path())
    }

    fn load_codes(&self) -> io::Result<HashMap<String, AuthCode>> {
        read_json_map(&self.codes_path())
    }

    fn load_tokens(&self) -> io::Result<HashMap<String, AccessToken>> {
        read_json_map(&self.tokens_path())
    }

    fn save_codes(&self, codes: &HashMap<String, AuthCode>) -> io::Result<()> {
        write_json_atomic(&self.codes_path(), codes)
    }

    fn upsert_client(&self, id: &str, client: Client) -> io::Result<()> {
        let mut clients = self.load_clients()?;
        clients.insert(id.to_owned(), client);
        write_json_atomic(&self.clients_path(), &clients)
    }

    fn upsert_code(&self, code: &str, value: AuthCode) -> io::Result<()> {
        let mut codes = self.load_codes()?;
        prune_codes(&mut codes, self.code_ttl_seconds);
        codes.insert(code.to_owned(), value);
        write_json_atomic(&self.codes_path(), &codes)
    }

    fn upsert_token(&self, token: &str, value: AccessToken) -> io::Result<()> {
        let mut tokens = self.load_tokens()?;
        self.prune_tokens(&mut tokens);
        tokens.insert(token.to_owned(), value);
        write_json_atomic(&self.tokens_path(), &tokens)
    }

    fn prune_tokens(&self, tokens: &mut HashMap<String, AccessToken>) {
        let now = now_secs();
        tokens.retain(|_, t| t.expires_at > now);
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

fn prune_codes(codes: &mut HashMap<String, AuthCode>, ttl: u64) {
    let now = now_secs();
    codes.retain(|_, c| !c.used && now.saturating_sub(c.created_at) <= ttl);
}

fn read_json_map<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<HashMap<String, T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e),
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

fn verify_pkce(verifier: &str, challenge: &str, method: &str) -> bool {
    if method != "S256" {
        return false;
    }
    let digest = digest::digest(&digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.as_ref()) == challenge
}

fn random_id() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn default_storage_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cua-driver")
        .join("oauth")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
}

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_owned()
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    keep_alive: bool,
}

struct HttpResponse {
    status: u16,
    reason: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            reason: reason(status),
            headers: vec![("Content-Type".into(), content_type.into())],
            body,
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            reason: reason(status),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

async fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<Option<HttpRequest>> {
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            return Ok(None);
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
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_owned();
            if key == "content-length" {
                content_length = val.parse().unwrap_or(0);
            } else if key == "connection" {
                if val.eq_ignore_ascii_case("close") {
                    keep_alive = false;
                } else if val.eq_ignore_ascii_case("keep-alive") {
                    keep_alive = true;
                }
            }
            headers.insert(key, val);
        }
    }
    if content_length > 16 * 1024 * 1024 {
        anyhow::bail!("HTTP body too large");
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).await?;
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
    mut resp: HttpResponse,
    keep_alive: bool,
) -> anyhow::Result<()> {
    add_cors(&mut resp.headers);
    let conn = if keep_alive { "keep-alive" } else { "close" };
    resp.headers.push(("Connection".into(), conn.into()));
    resp.headers
        .push(("Content-Length".into(), resp.body.len().to_string()));
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, resp.reason);
    for (k, v) in &resp.headers {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    if !resp.body.is_empty() {
        stream.write_all(&resp.body).await?;
    }
    stream.flush().await?;
    Ok(())
}

fn add_cors(headers: &mut Vec<(String, String)>) {
    headers.push(("Access-Control-Allow-Origin".into(), "*".into()));
    headers.push((
        "Access-Control-Allow-Methods".into(),
        "GET, POST, DELETE, OPTIONS".into(),
    ));
    headers.push((
        "Access-Control-Allow-Headers".into(),
        "Content-Type, Authorization, MCP-Session-Id, Last-Event-ID".into(),
    ));
    headers.push((
        "Access-Control-Expose-Headers".into(),
        "MCP-Session-Id".into(),
    ));
    headers.push(("Access-Control-Max-Age".into(), "86400".into()));
}

fn json_response(status: u16, value: Value) -> HttpResponse {
    HttpResponse::new(
        status,
        "application/json",
        serde_json::to_vec(&value).unwrap_or_default(),
    )
}

fn html_response(status: u16, body: String) -> HttpResponse {
    HttpResponse::new(status, "text/html; charset=utf-8", body.into_bytes())
}

fn oauth_error(status: u16, error: &str, description: &str) -> HttpResponse {
    json_response(
        status,
        json!({"error": error, "error_description": description}),
    )
}

fn bearer_error(description: &str) -> HttpResponse {
    let mut resp = oauth_error(401, "invalid_token", description);
    resp.headers.push((
        "WWW-Authenticate".into(),
        format!(r#"Bearer realm="mcp", error="invalid_token", error_description="{description}""#),
    ));
    resp
}

fn redirect(location: &str) -> HttpResponse {
    let mut resp = HttpResponse::empty(302);
    resp.headers.push(("Location".into(), location.to_owned()));
    resp
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
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn parse_query(path: &str) -> HashMap<String, String> {
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    parse_pairs(query.as_bytes())
}

fn parse_form(body: &[u8]) -> HashMap<String, String> {
    parse_pairs(body)
}

fn parse_pairs(input: &[u8]) -> HashMap<String, String> {
    let raw = String::from_utf8_lossy(input);
    raw.split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((percent_decode(k), percent_decode(v)))
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
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

    #[tokio::test]
    async fn oauth_flow_register_authorize_token_and_reject_replay() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = HandlerContext {
            options: Options {
                public_url: "https://example.test".to_owned(),
                listen: DEFAULT_LISTEN.to_owned(),
                storage_dir: Some(dir.path().to_path_buf()),
                token_ttl_seconds: 3600,
                code_ttl_seconds: 300,
                require_user_consent: false,
            },
            registry: Arc::new(ToolRegistry::new()),
            store: Arc::new(JsonStore::new(dir.path().to_path_buf(), 3600, 300)),
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
}
