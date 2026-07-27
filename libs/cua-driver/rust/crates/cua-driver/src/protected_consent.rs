//! Built-in protected consent host for standalone Cua Driver runtimes.
//!
//! The surface runs in a separate instance of the signed driver executable.
//! Its request and result travel only through inherited anonymous pipes.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use cua_driver_core::consent::{
    ConsentAction, ConsentRequest, IndicatorLease, ProtectedConsentProvider, ProviderDecision,
};
use overlay_ui::{ConsentCard, HelperDecision, HelperEvent, HelperRequest, IndicatorCard};

const HELPER_ENV: &str = "CUA_INTERNAL_PROTECTED_CONSENT_HELPER";

pub(crate) fn native_provider() -> Option<Arc<dyn ProtectedConsentProvider>> {
    if !interactive_surface_available() {
        return None;
    }
    Some(Arc::new(NativeProtectedConsentProvider::default()))
}

fn interactive_surface_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        platform_macos::session::has_graphic_access()
    }
    #[cfg(target_os = "windows")]
    {
        platform_windows::protected_consent::interactive_surface_available()
    }
    #[cfg(target_os = "linux")]
    {
        platform_linux::protected_consent::interactive_surface_available()
    }
}

/// Run before telemetry, CLI parsing, or platform runtime initialization.
pub(crate) fn run_helper_if_requested() -> Option<i32> {
    if std::env::var_os(HELPER_ENV).is_none() {
        return None;
    }
    Some(match run_helper() {
        Ok(()) => 0,
        Err(error) => {
            let _ = write_event(&HelperEvent::Failed {
                reason: error.to_string(),
            });
            1
        }
    })
}

fn run_helper() -> anyhow::Result<()> {
    let request: HelperRequest = serde_json::from_reader(std::io::stdin().lock())?;
    #[cfg(target_os = "macos")]
    {
        platform_macos::protected_consent::run(request)
    }
    #[cfg(target_os = "windows")]
    {
        platform_windows::protected_consent::run(request)
    }
    #[cfg(target_os = "linux")]
    {
        platform_linux::protected_consent::run(request)
    }
}

pub(crate) fn write_event(event: &HelperEvent) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, event)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

#[derive(Default)]
struct NativeProtectedConsentProvider {
    indicators: Mutex<HashMap<String, IndicatorProcess>>,
}

struct IndicatorProcess {
    child: Arc<Mutex<Child>>,
    revoked: Arc<AtomicBool>,
}

#[async_trait]
impl ProtectedConsentProvider for NativeProtectedConsentProvider {
    fn provider_id(&self) -> &'static str {
        "cua_native_overlay_v1"
    }

    async fn request_consent(&self, request: &ConsentRequest) -> Result<ProviderDecision, String> {
        let expires_unix_ms = u64::try_from(request.expires_unix_ms)
            .map_err(|_| "protected consent expiry exceeds the helper wire range".to_owned())?;
        let card = ConsentCard {
            operation: request.operation.clone(),
            risk_label: format!("{:?}", request.risk_class),
            summary: request.human_summary.clone(),
            request_digest: request.request_digest.clone(),
            expires_unix_ms,
        };
        let expected_digest = request.request_digest.clone();
        tokio::task::spawn_blocking(move || {
            let event = run_one_shot(HelperRequest::Consent(card))?;
            let HelperEvent::Decision {
                action,
                request_digest,
            } = event
            else {
                return Err("protected helper did not return a decision".to_owned());
            };
            if request_digest != expected_digest {
                return Err("protected helper returned a mismatched request digest".to_owned());
            }
            Ok(ProviderDecision {
                action: match action {
                    HelperDecision::Accept => ConsentAction::Accept,
                    HelperDecision::Decline => ConsentAction::Decline,
                    HelperDecision::Cancel => ConsentAction::Cancel,
                },
                request_digest,
            })
        })
        .await
        .map_err(|error| format!("protected helper task failed: {error}"))?
    }

    async fn activate_indicator(&self, request: &ConsentRequest) -> Result<IndicatorLease, String> {
        let id = format!("indicator-{}", request.nonce);
        let summary = request.human_summary.clone();
        let indicator_id = id.clone();
        let (process, reader) = tokio::task::spawn_blocking(move || {
            spawn_indicator(HelperRequest::Indicator(IndicatorCard {
                indicator_id,
                summary,
            }))
        })
        .await
        .map_err(|error| format!("indicator helper task failed: {error}"))??;

        let revoked = process.revoked.clone();
        let child = process.child.clone();
        let watch_id = id.clone();
        std::thread::Builder::new()
            .name("cua-protected-indicator".to_owned())
            .spawn(move || watch_indicator(reader, child, revoked, &watch_id))
            .map_err(|error| format!("could not monitor protected indicator: {error}"))?;
        let lease = IndicatorLease::new(id.clone(), process.revoked.clone());
        self.indicators.lock().unwrap().insert(id, process);
        Ok(lease)
    }

    async fn deactivate_indicator(&self, indicator_id: &str) {
        let process = self.indicators.lock().unwrap().remove(indicator_id);
        if let Some(process) = process {
            process
                .revoked
                .store(true, std::sync::atomic::Ordering::Release);
            if let Ok(mut child) = process.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn spawn_child(request: &HelperRequest) -> Result<Child, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("resolve helper executable: {error}"))?;
    let mut child = Command::new(executable)
        .env(HELPER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("spawn protected helper: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "protected helper stdin was unavailable".to_owned())?;
    serde_json::to_writer(&mut stdin, request)
        .map_err(|error| format!("serialize protected helper request: {error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("flush protected helper request: {error}"))?;
    drop(stdin);
    Ok(child)
}

fn run_one_shot(request: HelperRequest) -> Result<HelperEvent, String> {
    let mut child = spawn_child(&request)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "protected helper stdout was unavailable".to_owned())?;
    let mut reader = BufReader::new(stdout);
    expect_ready(&mut reader)?;
    let event = read_event(&mut reader)?;
    let status = child
        .wait()
        .map_err(|error| format!("wait for protected helper: {error}"))?;
    if !status.success() {
        return Err(format!("protected helper exited with {status}"));
    }
    Ok(event)
}

fn spawn_indicator(
    request: HelperRequest,
) -> Result<(IndicatorProcess, BufReader<std::process::ChildStdout>), String> {
    let mut child = spawn_child(&request)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "indicator helper stdout was unavailable".to_owned())?;
    let mut reader = BufReader::new(stdout);
    expect_ready(&mut reader)?;
    Ok((
        IndicatorProcess {
            child: Arc::new(Mutex::new(child)),
            revoked: Arc::new(AtomicBool::new(false)),
        },
        reader,
    ))
}

fn watch_indicator(
    mut reader: BufReader<std::process::ChildStdout>,
    child: Arc<Mutex<Child>>,
    revoked: Arc<AtomicBool>,
    expected_id: &str,
) {
    let stopped = matches!(
        read_event(&mut reader),
        Ok(HelperEvent::Stop { indicator_id }) if indicator_id == expected_id
    );
    revoked.store(true, std::sync::atomic::Ordering::Release);
    if !stopped {
        tracing::warn!("protected indicator helper exited without an authenticated Stop event");
    }
    if let Ok(mut child) = child.lock() {
        let _ = child.wait();
    }
}

fn expect_ready(reader: &mut impl BufRead) -> Result<(), String> {
    match read_event(reader)? {
        HelperEvent::Ready => Ok(()),
        HelperEvent::Failed { reason } => Err(reason),
        _ => Err("protected helper did not become ready".to_owned()),
    }
}

fn read_event(reader: &mut impl BufRead) -> Result<HelperEvent, String> {
    let mut line = String::new();
    let count = reader
        .read_line(&mut line)
        .map_err(|error| format!("read protected helper event: {error}"))?;
    if count == 0 {
        return Err("protected helper closed its private channel".to_owned());
    }
    serde_json::from_str(&line).map_err(|error| format!("invalid protected helper event: {error}"))
}
