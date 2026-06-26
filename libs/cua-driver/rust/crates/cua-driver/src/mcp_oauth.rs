//! Experimental OAuth 2.0 front door for the local MCP HTTP transport.
//!
//! This is intentionally opt-in and loopback-oriented: callers expose it through
//! their own HTTPS tunnel if they want ChatGPT-style OAuth/DCR discovery.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use cua_driver_core::protocol::{Request, Response};
use cua_driver_core::server::handle_request;
use cua_driver_core::tool::ToolRegistry;
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time;
use uuid::Uuid;

const DEFAULT_LISTEN: &str = "127.0.0.1:7676";
const DEFAULT_SCOPE: &str = "mcp:tools";
const HTTP_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_BODY_TIMEOUT: Duration = Duration::from_secs(30);

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
    ensure_private_dir(&store.dir)?;
    let sessions = Arc::new(SessionStore::default());
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
            sessions: sessions.clone(),
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
    sessions: Arc<SessionStore>,
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
    let redirect_uri = form.get("redirect_uri").cloned().unwrap_or_default();
    let state = form.get("state").cloned().unwrap_or_default();
    if decision != "allow" {
        return redirect(&append_query(
            &redirect_uri,
            &[("error", "access_denied"), ("state", &state)],
        ));
    }
    let nonce = match form.get("consent_nonce").filter(|v| !v.is_empty()) {
        Some(v) => v.to_owned(),
        None => return oauth_error(400, "invalid_request", "missing consent_nonce"),
    };
    match ctx.store.consume_consent(&nonce) {
        Ok(Some(pending)) if pending.matches_form(&form) => issue_authorization_code(pending, ctx),
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
        scope,
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
    let token = match ctx
        .store
        .redeem_code_for_token(&code, &client_id, &redirect_uri, &verifier)
    {
        Ok(TokenRedeem::Issued(v)) => v,
        Ok(TokenRedeem::InvalidGrant(description)) => {
            return oauth_error(400, "invalid_grant", description)
        }
        Ok(TokenRedeem::InvalidScope) => {
            return oauth_error(400, "invalid_scope", "only mcp:tools is supported")
        }
        Err(e) => return oauth_error(500, "server_error", &e.to_string()),
    };
    json_response(
        token.status,
        json!({
            "access_token": token.access_token,
            "token_type": "Bearer",
            "expires_in": ctx.store.token_ttl_seconds,
            "scope": token.scope,
        }),
    )
}

async fn handle_mcp(req: HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    match authorize_bearer(&req, ctx) {
        Ok(()) => {}
        Err(resp) => return resp,
    }
    if let Err(resp) = validate_protocol_version(&req) {
        return resp;
    }
    match req.method.as_str() {
        "POST" => handle_mcp_post(req, ctx).await,
        "GET" => handle_mcp_get(&req, ctx),
        "DELETE" => handle_mcp_delete(&req, ctx),
        _ => json_response(
            405,
            json!({"error": "method_not_allowed", "error_description": "Use POST, GET, or DELETE /mcp"}),
        ),
    }
}

async fn handle_mcp_post(req: HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    if !accepts_post_response(&req) {
        return json_response(
            406,
            json!({"error": "not_acceptable", "error_description": "POST /mcp must accept application/json and text/event-stream"}),
        );
    }
    let mut rpc_req: Request = match serde_json::from_slice(&req.body) {
        Ok(r) => r,
        Err(_) => {
            return HttpResponse::new(
                200,
                "application/json",
                serialize(&Response::parse_error()).into_bytes(),
            )
        }
    };
    let is_initialize = rpc_req.method == "initialize";
    let session_id = if is_initialize {
        None
    } else {
        match require_active_session(&req, ctx) {
            Ok(id) => Some(id),
            Err(resp) => return resp,
        }
    };
    apply_transport_session_identity(&mut rpc_req, session_id.as_deref());
    if rpc_req.id.is_none() {
        return HttpResponse::new(202, "application/json", Vec::new());
    }
    let id = rpc_req.id.clone().unwrap_or(Value::Null);
    let mut resp = HttpResponse::new(
        200,
        "application/json",
        serialize(&handle_request(rpc_req, id, &ctx.registry).await).into_bytes(),
    );
    if is_initialize {
        let sid = ctx.sessions.create();
        cua_driver_core::session::touch_session(&sid);
        resp.headers.push(("MCP-Session-Id".into(), sid));
    }
    resp
}

fn handle_mcp_get(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let session_id = match require_active_session(req, ctx) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if !accepts_sse(req) {
        return json_response(
            406,
            json!({"error": "not_acceptable", "error_description": "GET /mcp requires Accept: text/event-stream"}),
        );
    }
    cua_driver_core::session::touch_session(&session_id);
    let mut resp = HttpResponse::new(
        200,
        "text/event-stream",
        b": cua-driver mcp-oauth stream ready\n\n".to_vec(),
    );
    resp.headers.push(("MCP-Session-Id".into(), session_id));
    resp.headers
        .push(("Cache-Control".into(), "no-cache".into()));
    resp
}

fn handle_mcp_delete(req: &HttpRequest, ctx: &HandlerContext) -> HttpResponse {
    let session_id = match require_active_session(req, ctx) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    ctx.sessions.end(&session_id);
    cua_driver_core::session::fire_session_end(&session_id);
    HttpResponse::new(202, "application/json", Vec::new())
}

fn validate_protocol_version(req: &HttpRequest) -> Result<(), HttpResponse> {
    match req.headers.get("mcp-protocol-version").map(String::as_str) {
        None | Some("2025-06-18") | Some("2025-03-26") => Ok(()),
        Some(version) => Err(json_response(
            400,
            json!({"error": "unsupported_protocol_version", "error_description": format!("unsupported MCP-Protocol-Version: {version}")}),
        )),
    }
}

fn require_active_session(req: &HttpRequest, ctx: &HandlerContext) -> Result<String, HttpResponse> {
    let sid = req
        .headers
        .get("mcp-session-id")
        .filter(|v| !v.is_empty())
        .cloned()
        .ok_or_else(|| {
            json_response(
                400,
                json!({"error": "missing_session", "error_description": "MCP-Session-Id is required after initialize"}),
            )
        })?;
    if !ctx.sessions.contains(&sid) {
        return Err(json_response(
            404,
            json!({"error": "unknown_session", "error_description": "MCP session is unknown or has ended"}),
        ));
    }
    Ok(sid)
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
    if !is_valid_scope(&stored.scope) {
        return Err(bearer_error("Token does not carry mcp:tools scope"));
    }
    Ok(())
}

fn accepts_post_response(req: &HttpRequest) -> bool {
    match req.headers.get("accept") {
        None => true,
        Some(accept) if accept.contains("*/*") => true,
        Some(accept) => {
            let accept = accept.to_ascii_lowercase();
            accept.contains("application/json") && accept.contains("text/event-stream")
        }
    }
}

fn accepts_sse(req: &HttpRequest) -> bool {
    match req.headers.get("accept") {
        None => false,
        Some(accept) if accept.contains("*/*") => true,
        Some(accept) => accept.to_ascii_lowercase().contains("text/event-stream"),
    }
}

fn apply_transport_session_identity(req: &mut Request, session_id: Option<&str>) {
    let Some(params) = req.params.as_mut() else {
        return;
    };
    let Some(args) = params.get_mut("arguments").and_then(|a| a.as_object_mut()) else {
        return;
    };
    let explicit_session = args
        .get("session")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(sess) = explicit_session {
        args.entry("_session_id")
            .or_insert_with(|| Value::String(sess.clone()));
        cua_driver_core::session::touch_session(&sess);
    } else if let Some(sid) = session_id {
        args.entry("_session_id")
            .or_insert_with(|| Value::String(sid.to_owned()));
        cua_driver_core::session::touch_session(sid);
    }
}

fn serialize(resp: &Response) -> String {
    serde_json::to_string(resp).unwrap_or_else(|e| {
        format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#)
    })
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

fn optional_form_value<'a>(form: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    form.get(key).map(String::as_str).filter(|v| !v.is_empty())
}

fn authorize_page(pending: &PendingAuthorization, public_url: &str, nonce: &str) -> String {
    let display_resource = pending
        .resource
        .clone()
        .unwrap_or_else(|| format!("{public_url}/mcp"));
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

#[derive(Default, Debug)]
struct SessionStore {
    active: Mutex<HashMap<String, ()>>,
}

impl SessionStore {
    fn create(&self) -> String {
        let id = random_id();
        let mut active = self.active.lock().expect("session store mutex poisoned");
        active.insert(id.clone(), ());
        id
    }

    fn contains(&self, id: &str) -> bool {
        self.active
            .lock()
            .expect("session store mutex poisoned")
            .contains_key(id)
    }

    fn end(&self, id: &str) {
        self.active
            .lock()
            .expect("session store mutex poisoned")
            .remove(id);
    }
}

#[derive(Clone, Debug)]
struct JsonStore {
    dir: PathBuf,
    token_ttl_seconds: u64,
    code_ttl_seconds: u64,
    lock: Arc<Mutex<()>>,
}

impl JsonStore {
    fn new(dir: PathBuf, token_ttl_seconds: u64, code_ttl_seconds: u64) -> Self {
        Self {
            dir,
            token_ttl_seconds,
            code_ttl_seconds,
            lock: Arc::new(Mutex::new(())),
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

    fn upsert_client(&self, id: &str, client: Client) -> io::Result<()> {
        let _guard = self.lock.lock().expect("oauth store mutex poisoned");
        let mut clients = self.load_clients()?;
        clients.insert(id.to_owned(), client);
        write_json_atomic(&self.clients_path(), &clients)
    }

    fn upsert_code(&self, code: &str, value: AuthCode) -> io::Result<()> {
        let _guard = self.lock.lock().expect("oauth store mutex poisoned");
        let mut codes = self.load_codes()?;
        prune_codes(&mut codes, self.code_ttl_seconds);
        codes.insert(code.to_owned(), value);
        write_json_atomic(&self.codes_path(), &codes)
    }

    #[cfg(test)]
    fn upsert_token(&self, token: &str, value: AccessToken) -> io::Result<()> {
        let _guard = self.lock.lock().expect("oauth store mutex poisoned");
        let mut tokens = self.load_tokens()?;
        self.prune_tokens(&mut tokens);
        tokens.insert(token.to_owned(), value);
        write_json_atomic(&self.tokens_path(), &tokens)
    }

    fn upsert_consent(&self, nonce: &str, pending: &PendingAuthorization) -> io::Result<()> {
        let _guard = self.lock.lock().expect("oauth store mutex poisoned");
        let mut consents = read_json_map(&self.consents_path())?;
        prune_consents(&mut consents, self.code_ttl_seconds);
        consents.insert(
            nonce.to_owned(),
            ConsentTransaction {
                pending: pending.clone(),
                created_at: now_secs(),
            },
        );
        write_json_atomic(&self.consents_path(), &consents)
    }

    fn consume_consent(&self, nonce: &str) -> io::Result<Option<PendingAuthorization>> {
        let _guard = self.lock.lock().expect("oauth store mutex poisoned");
        let mut consents: HashMap<String, ConsentTransaction> =
            read_json_map(&self.consents_path())?;
        prune_consents(&mut consents, self.code_ttl_seconds);
        let pending = consents.remove(nonce).map(|c| c.pending);
        write_json_atomic(&self.consents_path(), &consents)?;
        Ok(pending)
    }

    fn redeem_code_for_token(
        &self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> io::Result<TokenRedeem> {
        let _guard = self.lock.lock().expect("oauth store mutex poisoned");
        let mut codes = self.load_codes()?;
        let Some(stored) = codes.get_mut(code) else {
            return Ok(TokenRedeem::InvalidGrant("unknown authorization code"));
        };
        if stored.used {
            return Ok(TokenRedeem::InvalidGrant(
                "authorization code has already been used",
            ));
        }
        if now_secs().saturating_sub(stored.created_at) > self.code_ttl_seconds {
            return Ok(TokenRedeem::InvalidGrant("authorization code has expired"));
        }
        if stored.client_id != client_id || stored.redirect_uri != redirect_uri {
            return Ok(TokenRedeem::InvalidGrant(
                "authorization code binding mismatch",
            ));
        }
        if !verify_pkce(
            verifier,
            &stored.code_challenge,
            &stored.code_challenge_method,
        ) {
            return Ok(TokenRedeem::InvalidGrant("PKCE verification failed"));
        }
        if !is_valid_scope(&stored.scope) {
            return Ok(TokenRedeem::InvalidScope);
        }

        let access_token = random_id();
        let now = now_secs();
        let token_scope = stored.scope.clone();
        let token = AccessToken {
            client_id: client_id.to_owned(),
            scope: token_scope.clone(),
            resource: stored.resource.clone(),
            created_at: now,
            expires_at: now.saturating_add(self.token_ttl_seconds),
        };
        stored.used = true;
        write_json_atomic(&self.codes_path(), &codes)?;

        let mut tokens = self.load_tokens()?;
        self.prune_tokens(&mut tokens);
        tokens.insert(access_token.clone(), token);
        write_json_atomic(&self.tokens_path(), &tokens)?;

        Ok(TokenRedeem::Issued(TokenIssue {
            status: 200,
            access_token,
            scope: token_scope,
        }))
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
struct ConsentTransaction {
    pending: PendingAuthorization,
    created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccessToken {
    client_id: String,
    scope: String,
    resource: Option<String>,
    created_at: u64,
    expires_at: u64,
}

#[derive(Debug)]
struct TokenIssue {
    status: u16,
    access_token: String,
    scope: String,
}

#[derive(Debug)]
enum TokenRedeem {
    Issued(TokenIssue),
    InvalidGrant(&'static str),
    InvalidScope,
}

fn prune_codes(codes: &mut HashMap<String, AuthCode>, ttl: u64) {
    let now = now_secs();
    codes.retain(|_, c| !c.used && now.saturating_sub(c.created_at) <= ttl);
}

fn prune_consents(consents: &mut HashMap<String, ConsentTransaction>, ttl: u64) {
    let now = now_secs();
    consents.retain(|_, c| now.saturating_sub(c.created_at) <= ttl);
}

fn read_json_map<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<HashMap<String, T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e),
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    write_private_file(&tmp, &bytes)?;
    fs::rename(tmp, path)
}

fn is_valid_scope(scope: &str) -> bool {
    scope == DEFAULT_SCOPE
}

fn is_allowed_redirect_uri(uri: &str) -> bool {
    if uri.is_empty()
        || uri.bytes().any(|b| b.is_ascii_control())
        || uri.contains('#')
        || uri.contains(' ')
    {
        return false;
    }
    if let Some(rest) = uri.strip_prefix("https://") {
        return valid_uri_authority(rest).is_some();
    }
    if let Some(rest) = uri.strip_prefix("http://") {
        let Some(authority) = valid_uri_authority(rest) else {
            return false;
        };
        return authority == "localhost"
            || authority.starts_with("localhost:")
            || authority == "127.0.0.1"
            || authority.starts_with("127.0.0.1:")
            || authority == "[::1]"
            || authority.starts_with("[::1]:");
    }
    false
}

fn valid_uri_authority(rest: &str) -> Option<&str> {
    let authority = rest.split(['/', '?']).next().unwrap_or("").trim();
    if authority.is_empty() || authority.contains('@') {
        None
    } else {
        Some(authority)
    }
}

#[cfg(unix)]
fn ensure_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(not(any(unix, windows)))]
fn ensure_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    std::io::Write::write_all(&mut file, bytes)
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
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
        let n = time::timeout(HTTP_HEADER_TIMEOUT, stream.read(&mut byte))
            .await
            .map_err(|_| anyhow::anyhow!("HTTP header read timed out"))??;
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
        time::timeout(HTTP_BODY_TIMEOUT, stream.read_exact(&mut body))
            .await
            .map_err(|_| anyhow::anyhow!("HTTP body read timed out"))??;
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
        "Content-Type, Authorization, MCP-Session-Id, MCP-Protocol-Version, Last-Event-ID".into(),
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
        406 => "Not Acceptable",
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

    #[test]
    fn redirect_uri_validation_allows_https_and_loopback_http_only() {
        assert!(is_allowed_redirect_uri("https://client.test/callback"));
        assert!(is_allowed_redirect_uri("http://localhost:3000/callback"));
        assert!(is_allowed_redirect_uri("http://127.0.0.1/callback"));
        assert!(is_allowed_redirect_uri("http://[::1]:3000/callback"));

        assert!(!is_allowed_redirect_uri("http://evil.test/callback"));
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
    fn authorize_requires_code_response_type_and_supported_scope() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = HandlerContext {
            options: Options {
                public_url: "https://example.test".to_owned(),
                listen: DEFAULT_LISTEN.to_owned(),
                storage_dir: Some(dir.path().to_path_buf()),
                token_ttl_seconds: 3600,
                code_ttl_seconds: 300,
                require_user_consent: true,
            },
            registry: Arc::new(ToolRegistry::new()),
            store: Arc::new(JsonStore::new(dir.path().to_path_buf(), 3600, 300)),
            sessions: Arc::new(SessionStore::default()),
        };
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
            sessions: Arc::new(SessionStore::default()),
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

    #[tokio::test]
    async fn mcp_streamable_http_session_lifecycle() {
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
            sessions: Arc::new(SessionStore::default()),
        };
        ctx.store
            .upsert_token(
                "token",
                AccessToken {
                    client_id: "client".to_owned(),
                    scope: DEFAULT_SCOPE.to_owned(),
                    resource: Some("https://example.test/mcp".to_owned()),
                    created_at: now_secs(),
                    expires_at: now_secs() + 3600,
                },
            )
            .unwrap();

        let mut headers = HashMap::from([
            ("authorization".to_owned(), "Bearer token".to_owned()),
            (
                "accept".to_owned(),
                "application/json, text/event-stream".to_owned(),
            ),
            ("mcp-protocol-version".to_owned(), "2025-06-18".to_owned()),
        ]);
        let initialize = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/mcp".to_owned(),
                headers: headers.clone(),
                body: br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(initialize.status, 200);
        let session_id = initialize
            .headers
            .iter()
            .find(|(k, _)| k == "MCP-Session-Id")
            .map(|(_, v)| v.clone())
            .expect("initialize response includes MCP-Session-Id");
        assert!(ctx.sessions.contains(&session_id));

        let missing_session = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/mcp".to_owned(),
                headers: headers.clone(),
                body: br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#.to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(missing_session.status, 400);

        headers.insert("mcp-session-id".to_owned(), session_id.clone());
        let tools_list = handle_http(
            HttpRequest {
                method: "POST".to_owned(),
                path: "/mcp".to_owned(),
                headers: headers.clone(),
                body: br#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#.to_vec(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(tools_list.status, 200);

        headers.insert("accept".to_owned(), "text/event-stream".to_owned());
        let stream = handle_http(
            HttpRequest {
                method: "GET".to_owned(),
                path: "/mcp".to_owned(),
                headers: headers.clone(),
                body: Vec::new(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(stream.status, 200);
        assert!(stream
            .headers
            .iter()
            .any(|(k, v)| k == "Content-Type" && v == "text/event-stream"));

        let delete = handle_http(
            HttpRequest {
                method: "DELETE".to_owned(),
                path: "/mcp".to_owned(),
                headers: headers.clone(),
                body: Vec::new(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(delete.status, 202);
        assert!(!ctx.sessions.contains(&session_id));

        let ended = handle_http(
            HttpRequest {
                method: "GET".to_owned(),
                path: "/mcp".to_owned(),
                headers,
                body: Vec::new(),
                keep_alive: false,
            },
            &ctx,
        )
        .await;
        assert_eq!(ended.status, 404);
    }

    fn test_ctx(dir: &Path, require_user_consent: bool) -> HandlerContext {
        HandlerContext {
            options: Options {
                public_url: "https://example.test".to_owned(),
                listen: DEFAULT_LISTEN.to_owned(),
                storage_dir: Some(dir.to_path_buf()),
                token_ttl_seconds: 3600,
                code_ttl_seconds: 300,
                require_user_consent,
            },
            registry: Arc::new(ToolRegistry::new()),
            store: Arc::new(JsonStore::new(dir.to_path_buf(), 3600, 300)),
            sessions: Arc::new(SessionStore::default()),
        }
    }

    fn extract_hidden_value(page: &str, name: &str) -> Option<String> {
        let needle = format!(r#"name="{name}" value=""#);
        let start = page.find(&needle)? + needle.len();
        let end = page[start..].find('"')?;
        Some(page[start..start + end].to_owned())
    }
}
