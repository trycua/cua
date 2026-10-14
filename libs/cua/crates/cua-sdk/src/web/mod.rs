//! Browser (wasm32) build: `@trycua/cua/browser`.
//!
//! The subset that makes sense in a browser tab, with the same names as the
//! native API:
//!
//! - `Cua.embedded(config)`: Fleet settings only (static bearer; OAuth client
//!   secrets never belong in a browser);
//! - `Cua.spacesd(url, token)` → [`SpacesdClient`] over gRPC-Web (`fetch`):
//!   capabilities, health, run/sh, screenshot, pointer and keyboard,
//!   clipboard and the proto3-JSON escape hatch for every unary RPC;
//! - `Cua.fleet()` → [`Fleet`]: pools, claims and service URLs through
//!   cyclops-sdk over `globalThis.fetch` (pages, workers and Node).
//!
//! Media in the browser goes through WebCodecs on the page, fed by the
//! daemon's media bridge or the spacesd's `/media` socket directly.

use crate::{
    CuaError, ExitInfo, ImageFormat, Point, ProcessOutput, Result, Screenshot, ScreenshotOptions,
    SpacesdCapabilities, SpacesdCommand,
};
use cua_proto::env::v1::{
    self as pb, computer_service_client::ComputerServiceClient,
    process_service_client::ProcessServiceClient, system_service_client::SystemServiceClient,
};
use std::{collections::HashMap, sync::Arc};

#[cfg(target_arch = "wasm32")]
mod fetch;

/// On wasm32 futures need not be `Send` (single-threaded). On the host this
/// module is only compiled for binding metadata (feature `web`), where
/// UniFFI still demands `Send`; the wrapper asserts it there. Host builds
/// of the web API are never executed.
#[cfg(target_arch = "wasm32")]
fn send<F: std::future::Future>(f: F) -> F {
    f
}

#[cfg(not(target_arch = "wasm32"))]
fn send<F: std::future::Future>(f: F) -> AssertSend<F> {
    AssertSend(f)
}

#[cfg(not(target_arch = "wasm32"))]
struct AssertSend<F>(F);

// SAFETY: host builds of the browser API exist only so UniFFI can read the
// export metadata; they are never run (see `send`).
#[cfg(not(target_arch = "wasm32"))]
unsafe impl<F> Send for AssertSend<F> {}

#[cfg(not(target_arch = "wasm32"))]
impl<F: std::future::Future> std::future::Future for AssertSend<F> {
    type Output = F::Output;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<F::Output> {
        // SAFETY: structural pinning of the only field.
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}

type Channel = tonic::service::interceptor::InterceptedService<tonic_web_wasm_client::Client, Auth>;

#[derive(Clone)]
struct Auth(Option<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>);

impl tonic::service::Interceptor for Auth {
    fn call(
        &mut self,
        mut req: tonic::Request<()>,
    ) -> std::result::Result<tonic::Request<()>, tonic::Status> {
        if let Some(v) = &self.0 {
            req.metadata_mut().insert("authorization", v.clone());
        }
        Ok(req)
    }
}

fn status(e: tonic::Status) -> CuaError {
    let m = e.message().to_string();
    match e.code() {
        tonic::Code::Unauthenticated => CuaError::Unauthenticated(m),
        tonic::Code::NotFound => CuaError::NotFound(m),
        tonic::Code::InvalidArgument => CuaError::InvalidArgument(m),
        tonic::Code::Unimplemented => CuaError::Unsupported(m),
        tonic::Code::PermissionDenied => CuaError::PermissionDenied(m),
        tonic::Code::DeadlineExceeded => CuaError::Timeout(m),
        tonic::Code::Unavailable => CuaError::Transport(m),
        _ => CuaError::Env(m),
    }
}

/// Fleet settings for the browser (static bearer only).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct FleetSettings {
    /// Fleet API base URL.
    #[uniffi(default = None)]
    pub base_url: Option<String>,
    /// Bearer token.
    #[uniffi(default = None)]
    pub token: Option<String>,
}

/// Configuration of the browser SDK.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct CuaConfig {
    /// Fleet settings.
    #[uniffi(default = None)]
    pub fleet: Option<FleetSettings>,
}

/// The cua SDK (browser build).
#[derive(uniffi::Object)]
pub struct Cua {
    config: CuaConfig,
}

#[uniffi::export]
impl Cua {
    /// The browser runtime.
    #[uniffi::constructor]
    pub fn embedded(config: CuaConfig) -> Arc<Self> {
        Arc::new(Self { config })
    }

    /// Connects to cua-spacesd at `url` over gRPC-Web.
    pub async fn spacesd(&self, url: String, token: Option<String>) -> Result<Arc<SpacesdClient>> {
        send(async move {
            let base = url.trim_end_matches('/').to_string();
            let auth = token
                .filter(|t| !t.is_empty())
                .map(|t| {
                    format!("Bearer {t}")
                        .parse()
                        .map_err(|_| CuaError::InvalidArgument("token".into()))
                })
                .transpose()?;
            let channel = tonic::service::interceptor::InterceptedService::new(
                tonic_web_wasm_client::Client::new(base),
                Auth(auth),
            );
            let client = SpacesdClient { channel };
            client.capabilities().await.map_err(|e| match e {
                CuaError::Unauthenticated(m) => CuaError::Unauthenticated(m),
                other => CuaError::SpacesdNotAvailable(format!("{url}: {other}")),
            })?;
            Ok(Arc::new(client))
        })
        .await
    }

    /// Fleet control plane.
    pub fn fleet(&self) -> Result<Arc<Fleet>> {
        let f = self.config.fleet.clone().unwrap_or_default();
        let token = f.token.filter(|t| !t.is_empty()).ok_or_else(|| {
            CuaError::ProviderNotConfigured("the browser Fleet client needs a token".into())
        })?;
        let cfg = cyclops_sdk::CyclopsTokenProviderConfiguration {
            base_url: f
                .base_url
                .unwrap_or_else(|| "https://run.cua.ai".into())
                .trim_end_matches('/')
                .to_string(),
            pool_poll_interval_ms: 2000,
            pool_poll_limit: 300,
            claim_poll_interval_ms: 2000,
            claim_poll_limit: 300,
        };
        // globalThis.fetch, not window.fetch: the same build runs in Node
        // and in workers.
        #[cfg(target_arch = "wasm32")]
        let sdk = cyclops_sdk::CyclopsClient::connect_with_access_token(
            cfg,
            token,
            Arc::new(fetch::GlobalFetch),
        )
        .map_err(|e| CuaError::Fleet(e.to_string()))?;
        #[cfg(not(target_arch = "wasm32"))]
        let sdk = cyclops_sdk::CyclopsClient::connect_browser_with_access_token(cfg, token)
            .map_err(|e| CuaError::Fleet(e.to_string()))?;
        Ok(Arc::new(Fleet { sdk }))
    }
}

/// cua-spacesd over gRPC-Web.
#[derive(uniffi::Object)]
pub struct SpacesdClient {
    channel: Channel,
}

impl SpacesdClient {
    fn system(&self) -> SystemServiceClient<Channel> {
        SystemServiceClient::new(self.channel.clone())
    }
    fn computer(&self) -> ComputerServiceClient<Channel> {
        ComputerServiceClient::new(self.channel.clone()).max_decoding_message_size(64 * 1024 * 1024)
    }
    fn process(&self) -> ProcessServiceClient<Channel> {
        ProcessServiceClient::new(self.channel.clone())
    }

    async fn pointer(&self, action: pb::pointer_request::Action) -> Result<()> {
        self.computer()
            .pointer(pb::PointerRequest {
                target: None,
                action: Some(action),
            })
            .await
            .map_err(status)?;
        Ok(())
    }

    async fn keyboard(&self, action: pb::keyboard_request::Action) -> Result<()> {
        self.computer()
            .keyboard(pb::KeyboardRequest {
                target: None,
                action: Some(action),
            })
            .await
            .map_err(status)?;
        Ok(())
    }
}

fn key(name: &str) -> pb::KeyInput {
    let upper = name
        .trim_start_matches("KEY_")
        .to_ascii_uppercase()
        .replace(['-', ' '], "_");
    let alias = match upper.as_str() {
        "CTRL" => "CONTROL",
        "CMD" | "COMMAND" | "SUPER" | "WIN" => "META",
        "OPTION" | "OPT" => "ALT",
        "ESC" => "ESCAPE",
        "RETURN" => "ENTER",
        "UP" => "ARROW_UP",
        "DOWN" => "ARROW_DOWN",
        "LEFT" => "ARROW_LEFT",
        "RIGHT" => "ARROW_RIGHT",
        other => other,
    };
    pb::KeyInput {
        key: Some(match pb::Key::from_str_name(&format!("KEY_{alias}")) {
            Some(k) => pb::key_input::Key::Named(k as i32),
            None => pb::key_input::Key::Character(name.to_string()),
        }),
    }
}

#[uniffi::export]
impl SpacesdClient {
    /// `GetCapabilities`.
    pub async fn capabilities(&self) -> Result<SpacesdCapabilities> {
        send(async move {
            Ok(self
                .system()
                .get_capabilities(pb::GetCapabilitiesRequest {})
                .await
                .map_err(status)?
                .into_inner()
                .into())
        })
        .await
    }

    /// `Health` as proto3 JSON.
    pub async fn health(&self) -> Result<String> {
        send(async move {
            let h = self
                .system()
                .health(pb::HealthRequest {})
                .await
                .map_err(status)?
                .into_inner();
            Ok(serde_json::to_string(&h)?)
        })
        .await
    }

    /// Runs a command to completion (at most 64 MiB of output).
    pub async fn run(&self, command: SpacesdCommand) -> Result<ProcessOutput> {
        send(async move {
            let req = pb::StartProcessRequest {
                config: Some(pb::ProcessConfig {
                    command: command.program,
                    args: command.args,
                    env: command.env,
                    cwd: command.cwd.unwrap_or_default(),
                    user: command.user.unwrap_or_default(),
                    timeout: command.timeout_ms.map(pbjson_types_duration),
                }),
                tag: command.tag.unwrap_or_default(),
                ..Default::default()
            };
            let mut stream = self
                .process()
                .start_process(req)
                .await
                .map_err(status)?
                .into_inner();
            let mut out = ProcessOutput {
                exit: ExitInfo {
                    code: None,
                    signal: None,
                    timed_out: false,
                    error: None,
                    success: false,
                },
                stdout: vec![],
                stderr: vec![],
                pty: vec![],
            };
            let mut total = 0usize;
            // Bounded by the output cap; the server ends the stream after
            // ProcessEnd.
            while let Some(msg) = stream.message().await.map_err(status)? {
                use pb::process_event::Event;
                match msg.event.and_then(|e| e.event) {
                    Some(Event::Data(d)) => {
                        use pb::process_data::Output;
                        let (buf, bytes) = match d.output {
                            Some(Output::Stdout(b)) => (&mut out.stdout, b),
                            Some(Output::Stderr(b)) => (&mut out.stderr, b),
                            Some(Output::Pty(b)) => (&mut out.pty, b),
                            None => continue,
                        };
                        total += bytes.len();
                        if total > 64 * 1024 * 1024 {
                            return Err(CuaError::Env("process output exceeds 64 MiB".into()));
                        }
                        buf.extend_from_slice(&bytes);
                    }
                    Some(Event::End(end)) => {
                        out.exit = ExitInfo {
                            success: end.exit_code == Some(0),
                            code: end.exit_code,
                            signal: pb::Signal::try_from(end.signal)
                                .ok()
                                .filter(|s| *s != pb::Signal::Unspecified)
                                .map(|s| crate::types::enum_suffix(format!("{s:?}"))),
                            timed_out: end.timed_out,
                            error: (!end.error.is_empty()).then_some(end.error),
                        };
                        break;
                    }
                    _ => {}
                }
            }
            Ok(out)
        })
        .await
    }

    /// Runs `line` with `/bin/sh -c`.
    pub async fn sh(&self, line: String, timeout_ms: Option<u32>) -> Result<ProcessOutput> {
        send(async move {
            self.run(SpacesdCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), line],
                env: HashMap::new(),
                cwd: None,
                user: None,
                timeout_ms,
                tag: None,
                stdin: false,
                pty: None,
            })
            .await
        })
        .await
    }

    /// Captures a display.
    pub async fn screenshot(&self, options: Option<ScreenshotOptions>) -> Result<Screenshot> {
        send(async move {
            let o = options.unwrap_or(ScreenshotOptions {
                display: None,
                format: None,
                quality: None,
                max_dimension: None,
                include_cursor: false,
            });
            let r = self
                .computer()
                .screenshot(pb::ScreenshotRequest {
                    source: o.display.map(pb::screenshot_request::Source::DisplayId),
                    region: None,
                    format: o
                        .format
                        .map(ImageFormat::to_pb)
                        .unwrap_or(pb::ImageFormat::Png) as i32,
                    quality: o.quality.unwrap_or(0),
                    max_dimension: o.max_dimension.unwrap_or(0),
                    include_cursor: o.include_cursor,
                })
                .await
                .map_err(status)?
                .into_inner();
            let size = r.image_size.unwrap_or_default();
            Ok(Screenshot {
                format: ImageFormat::from_pb(
                    pb::ImageFormat::try_from(r.format).unwrap_or_default(),
                ),
                width: size.width,
                height: size.height,
                scale: r.scale,
                screenshot_id: r.screenshot_id,
                image: r.image.to_vec(),
            })
        })
        .await
    }

    /// Left click.
    pub async fn click(&self, x: f64, y: f64) -> Result<()> {
        send(async move {
            self.pointer(pb::pointer_request::Action::Click(pb::PointerClick {
                position: Some(pb::Point { x, y }),
                button: pb::MouseButton::Left as i32,
                count: 1,
                modifiers: vec![],
            }))
            .await
        })
        .await
    }

    /// Moves the pointer.
    pub async fn move_to(&self, x: f64, y: f64) -> Result<()> {
        send(async move {
            self.pointer(pb::pointer_request::Action::Move(pb::PointerMove {
                position: Some(pb::Point { x, y }),
                duration: None,
            }))
            .await
        })
        .await
    }

    /// Scrolls by line deltas.
    pub async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        send(async move {
            self.pointer(pb::pointer_request::Action::Scroll(pb::PointerScroll {
                position: None,
                delta_x: dx,
                delta_y: dy,
                unit: pb::ScrollUnit::Line as i32,
            }))
            .await
        })
        .await
    }

    /// Types text.
    pub async fn type_text(&self, text: String) -> Result<()> {
        send(async move {
            self.keyboard(pb::keyboard_request::Action::Type(pb::KeyboardType {
                text,
                mode: pb::TextEntryMode::Keystrokes as i32,
                delay: None,
            }))
            .await
        })
        .await
    }

    /// Presses one key.
    pub async fn press(&self, key_name: String) -> Result<()> {
        send(async move {
            self.keyboard(pb::keyboard_request::Action::Press(pb::KeyboardPress {
                key: Some(key(&key_name)),
                modifiers: vec![],
                repeat: 1,
            }))
            .await
        })
        .await
    }

    /// Presses a chord.
    pub async fn hotkey(&self, keys: Vec<String>) -> Result<()> {
        send(async move {
            self.keyboard(pb::keyboard_request::Action::Hotkey(pb::KeyboardHotkey {
                keys: keys.iter().map(|k| key(k)).collect(),
            }))
            .await
        })
        .await
    }

    /// Clipboard text.
    pub async fn get_clipboard(&self) -> Result<Option<String>> {
        send(async move {
            Ok(self
                .computer()
                .get_clipboard(pb::GetClipboardRequest {})
                .await
                .map_err(status)?
                .into_inner()
                .content
                .and_then(|c| c.text))
        })
        .await
    }

    /// Sets clipboard text.
    pub async fn set_clipboard(&self, text: String) -> Result<u64> {
        send(async move {
            Ok(self
                .computer()
                .set_clipboard(pb::SetClipboardRequest {
                    content: Some(pb::ClipboardContent {
                        text: Some(text),
                        file_paths: vec![],
                        image_png: None,
                    }),
                })
                .await
                .map_err(status)?
                .into_inner()
                .generation)
        })
        .await
    }

    /// Pointer position.
    pub async fn cursor_position(&self) -> Result<Point> {
        send(async move {
            let p = self
                .computer()
                .get_cursor_position(pb::GetCursorPositionRequest {})
                .await
                .map_err(status)?
                .into_inner()
                .position
                .unwrap_or_default();
            Ok(Point { x: p.x, y: p.y })
        })
        .await
    }

    /// Any unary `cua.env.v1` RPC with proto3-JSON bodies.
    pub async fn call_json(&self, method: String, request_json: String) -> Result<String> {
        send(async move {
            let path = crate::json::normalize(&method)?;
            crate::json::call_unary(self.channel.clone(), &path, &request_json).await
        })
        .await
    }
}

fn pbjson_types_duration(ms: u32) -> cua_proto::wkt::Duration {
    cua_proto::wkt::Duration {
        seconds: i64::from(ms / 1000),
        nanos: ((ms % 1000) * 1_000_000) as i32,
    }
}

/// A bound Fleet sandbox.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FleetSandbox {
    /// Sandbox name.
    pub name: String,
    /// Namespace.
    pub namespace: String,
    /// Claim name.
    pub claim: String,
    /// Declared services.
    pub services: Vec<String>,
}

/// Fleet control plane (browser transport).
#[derive(uniffi::Object)]
pub struct Fleet {
    sdk: Arc<cyclops_sdk::CyclopsClient>,
}

fn fleet_err(e: cyclops_sdk::SdkError) -> CuaError {
    // Mirrors `cua_fleet::sdk_error_is_not_found` (cua-fleet is native-only):
    // 404, or 403 on a read in a deleted pool's namespace.
    if let cyclops_sdk::SdkError::Status {
        status, operation, ..
    } = &e
        && (*status == 404
            || (*status == 403
                && (operation.starts_with("get ") || operation.starts_with("list "))))
    {
        return CuaError::NotFound(e.to_string());
    }
    CuaError::Fleet(e.to_string())
}

#[uniffi::export]
impl Fleet {
    /// Pools in a namespace (JSON resources).
    pub async fn list_pools(&self, namespace: String) -> Result<Vec<String>> {
        send(async move {
            self.sdk
                .clone()
                .list_pools(namespace)
                .await
                .map_err(fleet_err)?
                .iter()
                .map(|p| serde_json::to_string(p).map_err(CuaError::from))
                .collect()
        })
        .await
    }

    /// Claims a sandbox from `pool` and waits for it to bind.
    pub async fn acquire(&self, pool: String, name: Option<String>) -> Result<FleetSandbox> {
        send(async move {
            let p = self.sdk.clone().get_pool(pool).await.map_err(fleet_err)?;
            let claim = self
                .sdk
                .clone()
                .create_claim(cyclops_sdk::CreateClaimRequest {
                    pool: p,
                    spec: None,
                    name,
                    labels: None,
                    secret_files: None,
                })
                .await
                .map_err(fleet_err)?;
            let b = self
                .sdk
                .clone()
                .wait_claim(claim)
                .await
                .map_err(fleet_err)?;
            Ok(FleetSandbox {
                name: b.name,
                namespace: b.namespace,
                claim: b.claim,
                services: b.services,
            })
        })
        .await
    }

    /// Claims in a namespace (JSON resources).
    pub async fn list_claims(&self, namespace: String) -> Result<Vec<String>> {
        send(async move {
            self.sdk
                .clone()
                .list_claims(namespace)
                .await
                .map_err(fleet_err)?
                .iter()
                .map(|c| serde_json::to_string(c).map_err(CuaError::from))
                .collect()
        })
        .await
    }

    /// Releases a claim.
    pub async fn release(&self, sandbox: FleetSandbox) -> Result<()> {
        send(async move {
            let claims = self
                .sdk
                .clone()
                .list_claims(sandbox.namespace.clone())
                .await
                .map_err(fleet_err)?;
            if let Some(c) = claims
                .into_iter()
                .find(|c| c.metadata.name == sandbox.claim)
            {
                self.sdk.clone().delete_claim(c).await.map_err(fleet_err)?;
            }
            Ok(())
        })
        .await
    }
}
