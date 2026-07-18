//! Approval-gated downloads for exact browser tab and element capabilities.
//!
//! Chromium's download behavior is browser-wide, so this tool enables it only
//! for the bounded interval around one exact ref mutation and restores the
//! default before returning. Output deliberately omits URLs, filenames, and
//! local paths.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use crate::tool_args::ArgsExt;

use super::cdp_ws::CdpConnection;
use super::engine::BrowserEngine;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::store::BrowserActionKind;

/// Injected by an MCP host only after its destructive-tool approval flow.
/// It is intentionally absent from the public input schema.
pub const MCP_HOST_DOWNLOAD_APPROVAL_ARG: &str = "_cua_browser_download_mcp_host_approved";

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

fn browser_download_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub struct BrowserDownloadTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserDownloadTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_download".into(),
                description: "Trigger one download through an exact live browser ref and save it inside an explicitly approved directory. Requires MCP-host destructive-tool approval, refuses ambiguous or stale capabilities, and never returns the source URL, filename, or destination path.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "session": {
                            "type": "string",
                            "description": "Explicit caller session owning the browser capabilities."
                        },
                        "target_id": {
                            "type": "string",
                            "description": "Opaque exact browser target id from get_browser_state."
                        },
                        "tab_id": {
                            "type": "string",
                            "description": "Opaque exact tab id from get_browser_state."
                        },
                        "ref": {
                            "type": "string",
                            "description": "Live page ref whose activation initiates the download."
                        },
                        "destination_root": {
                            "type": "string",
                            "description": "Absolute, existing, canonical directory approved to receive the download."
                        }
                    },
                    "required": ["session", "target_id", "tab_id", "ref", "destination_root"],
                    "additionalProperties": true
                }),
                read_only: false,
                destructive: true,
                idempotent: false,
                open_world: true,
            },
            engine,
        }
    }
}

fn consent_required() -> ToolResult {
    BrowserRefusal::new(
        BrowserRefusalCode::BrowserConsentRequired,
        "browser_download requires approval through the MCP host's destructive-tool confirmation flow",
    )
    .to_tool_result()
}

fn explicit_session(args: &Value) -> Result<String, ToolResult> {
    let session = args.require_str("session")?;
    if session.trim().is_empty() || session == "default" {
        return Err(ToolResult::error(
            "browser_download requires an explicit non-default session",
        ));
    }
    Ok(session)
}

fn validate_destination_root(raw: &str) -> Result<PathBuf, ToolResult> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(ToolResult::error("destination_root must be absolute"));
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| ToolResult::error("destination_root must be an existing directory"))?;
    if metadata.file_type().is_symlink() {
        return Err(ToolResult::error(
            "destination_root must name the directory directly, not a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(ToolResult::error(
            "destination_root must be an existing directory",
        ));
    }

    let canonical = std::fs::canonicalize(path)
        .map_err(|_| ToolResult::error("destination_root could not be canonicalized"))?;
    if canonical != path {
        return Err(ToolResult::error(
            "destination_root must already be in canonical form",
        ));
    }
    Ok(canonical)
}

fn download_path_for_cdp(path: &Path) -> String {
    let raw = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    {
        // `canonicalize` deliberately gives containment checks a verbatim
        // Windows path. Chromium's download API does not accept that transport
        // spelling, so remove only the namespace prefix after validation.
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(local) = raw.strip_prefix(r"\\?\") {
            return local.to_owned();
        }
    }
    raw.into_owned()
}

fn main_frame_id(frame_tree: &Value) -> Option<&str> {
    frame_tree
        .get("frameTree")?
        .get("frame")?
        .get("id")?
        .as_str()
}

fn is_safe_guid(guid: &str) -> bool {
    let mut components = Path::new(guid).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn completed_download_size(root: &Path, guid: &str) -> Result<u64, BrowserRefusal> {
    if !is_safe_guid(guid) {
        return Err(BrowserRefusal::new(
            BrowserRefusalCode::BrowserActionUnavailable,
            "the browser returned an invalid opaque download id",
        ));
    }
    let file = root.join(guid);
    let metadata = std::fs::symlink_metadata(&file).map_err(|_| {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserActionUnavailable,
            "the completed download could not be proven inside the approved directory",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(BrowserRefusal::new(
            BrowserRefusalCode::BrowserActionUnavailable,
            "the completed download is not a direct regular file",
        ));
    }
    let canonical = std::fs::canonicalize(&file).map_err(|_| {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserActionUnavailable,
            "the completed download could not be canonicalized",
        )
    })?;
    if canonical.parent() != Some(root) {
        return Err(BrowserRefusal::new(
            BrowserRefusalCode::BrowserActionUnavailable,
            "the completed download escaped the approved directory",
        ));
    }
    Ok(metadata.len())
}

async fn reset_download_behavior(conn: &CdpConnection) {
    let _ = conn
        .call(
            None,
            "Browser.setDownloadBehavior",
            json!({ "behavior": "default", "eventsEnabled": false }),
        )
        .await;
}

enum DownloadEventOutcome {
    Completed { guid: String },
    Canceled { guid: String },
    EventStreamClosed,
}

async fn await_download(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<super::cdp_ws::CdpEvent>,
    expected_frame_id: &str,
    observed_guid: &mut Option<String>,
) -> DownloadEventOutcome {
    while let Some(event) = events.recv().await {
        match event.method.as_str() {
            "Browser.downloadWillBegin" if observed_guid.is_none() => {
                if event.params.get("frameId").and_then(Value::as_str) != Some(expected_frame_id) {
                    continue;
                }
                *observed_guid = event
                    .params
                    .get("guid")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "Browser.downloadProgress" => {
                let Some(guid) = observed_guid.as_deref() else {
                    continue;
                };
                if event.params.get("guid").and_then(Value::as_str) != Some(guid) {
                    continue;
                }
                match event.params.get("state").and_then(Value::as_str) {
                    Some("completed") => {
                        return DownloadEventOutcome::Completed {
                            guid: guid.to_owned(),
                        }
                    }
                    Some("canceled") => {
                        return DownloadEventOutcome::Canceled {
                            guid: guid.to_owned(),
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    DownloadEventOutcome::EventStreamClosed
}

fn remove_proven_partial(root: &Path, guid: Option<&str>) {
    let Some(guid) = guid.filter(|guid| is_safe_guid(guid)) else {
        return;
    };
    let path = root.join(guid);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return;
    }
    let Ok(canonical) = std::fs::canonicalize(&path) else {
        return;
    };
    if canonical.parent() == Some(root) {
        let _ = std::fs::remove_file(canonical);
    }
}

#[async_trait]
impl Tool for BrowserDownloadTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (session, target_id, tab_id, ext_ref, destination_raw) = match (
            explicit_session(&args),
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("ref"),
            args.require_str("destination_root"),
        ) {
            (Ok(session), Ok(target), Ok(tab), Ok(ext_ref), Ok(destination)) => {
                (session, target, tab, ext_ref, destination)
            }
            (Err(error), _, _, _, _)
            | (_, Err(error), _, _, _)
            | (_, _, Err(error), _, _)
            | (_, _, _, Err(error), _)
            | (_, _, _, _, Err(error)) => return error,
        };

        if args
            .get(MCP_HOST_DOWNLOAD_APPROVAL_ARG)
            .and_then(Value::as_bool)
            != Some(true)
        {
            return consent_required();
        }
        let destination_root = match validate_destination_root(&destination_raw) {
            Ok(root) => root,
            Err(error) => return error,
        };

        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        // Download routing is browser-wide rather than tab-scoped. Serialize
        // these bounded mutations process-wide so two exact tabs cannot race
        // to replace each other's approved destination.
        let _download_mutation = browser_download_gate().lock().await;
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(validated) => validated,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let entry = match self
            .engine
            .store
            .resolve_ref(&session, &target_id, &tab_id, &ext_ref)
        {
            Ok(entry) => entry,
            Err(refusal) => return refusal.to_tool_result(),
        };
        if entry.semantic && !entry.actions.contains(&BrowserActionKind::Click) {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                "the semantic ref does not declare an activation action",
            )
            .to_tool_result();
        }
        let ref_session = match self
            .engine
            .frame_session_for_mutation(&session, &target_id, &tab_id, &validated, &entry.frame)
            .await
        {
            Ok(session) => session,
            Err(refusal) => return refusal.to_tool_result(),
        };

        let frame_tree = match validated
            .conn
            .call(Some(&validated.cdp_session), "Page.getFrameTree", json!({}))
            .await
        {
            Ok(tree) => tree,
            Err(_) => {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRouteUnavailable,
                    "the exact tab's main frame could not be proven",
                )
                .to_tool_result()
            }
        };
        let Some(main_frame_id) = main_frame_id(&frame_tree).map(str::to_owned) else {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserRouteUnavailable,
                "Page.getFrameTree did not identify the exact tab's main frame",
            )
            .to_tool_result();
        };
        let expected_frame_id = entry
            .frame
            .identity
            .as_ref()
            .map(|identity| identity.frame_id.clone())
            .unwrap_or(main_frame_id);

        let resolved = match validated
            .conn
            .call(
                Some(&ref_session),
                "DOM.resolveNode",
                json!({ "backendNodeId": entry.backend_node_id }),
            )
            .await
        {
            Ok(resolved) => resolved,
            Err(_) => {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    "the download ref no longer resolves in the live page",
                )
                .to_tool_result()
            }
        };
        let Some(object_id) = resolved
            .get("object")
            .and_then(|object| object.get("objectId"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserRefStale,
                "the download ref has no live page object",
            )
            .to_tool_result();
        };

        // Browser events are endpoint-wide. Subscribe before enabling and
        // triggering so no matching event can be lost between command replies.
        let mut events = validated.conn.subscribe();
        if validated
            .conn
            .call(
                None,
                "Browser.setDownloadBehavior",
                json!({
                    "behavior": "allowAndName",
                    "downloadPath": download_path_for_cdp(&destination_root),
                    "eventsEnabled": true
                }),
            )
            .await
            .is_err()
        {
            reset_download_behavior(&validated.conn).await;
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                "the browser could not configure the approved download",
            )
            .to_tool_result();
        }

        let trigger = validated
            .conn
            .call(
                Some(&ref_session),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": "function() { this.click(); }",
                    "userGesture": true,
                    "awaitPromise": false
                }),
            )
            .await;
        let trigger_failed = trigger
            .as_ref()
            .map_or(true, |result| result.get("exceptionDetails").is_some());
        if trigger_failed {
            reset_download_behavior(&validated.conn).await;
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserRefStale,
                "the exact download ref could not be activated",
            )
            .to_tool_result();
        }

        let mut observed_guid = None;
        let outcome = tokio::time::timeout(
            DOWNLOAD_TIMEOUT,
            await_download(&mut events, &expected_frame_id, &mut observed_guid),
        )
        .await;
        reset_download_behavior(&validated.conn).await;

        match outcome {
            Ok(DownloadEventOutcome::Completed { guid }) => {
                match completed_download_size(&destination_root, &guid) {
                    Ok(bytes) => ToolResult::text("browser download completed").with_structured(
                        json!({ "status": "completed", "download_id": guid, "bytes": bytes }),
                    ),
                    Err(refusal) => refusal.to_tool_result(),
                }
            }
            Ok(DownloadEventOutcome::Canceled { guid }) => {
                remove_proven_partial(&destination_root, Some(&guid));
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserActionUnavailable,
                    "the approved browser download was canceled",
                )
                .to_tool_result()
            }
            Ok(DownloadEventOutcome::EventStreamClosed) => {
                remove_proven_partial(&destination_root, observed_guid.as_deref());
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserActionUnavailable,
                    "the browser connection closed before the approved download completed",
                )
                .to_tool_result()
            }
            Err(_) => {
                remove_proven_partial(&destination_root, observed_guid.as_deref());
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserActionUnavailable,
                    "the approved browser download did not complete within 30 seconds",
                )
                .to_tool_result()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_must_be_absolute_existing_canonical_directory() {
        assert!(validate_destination_root("relative").is_err());
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("does-not-exist");
        assert!(validate_destination_root(missing.to_str().unwrap()).is_err());

        let root = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(root.path()).unwrap();
        assert_eq!(
            validate_destination_root(canonical.to_str().unwrap()).unwrap(),
            canonical
        );
    }

    #[test]
    fn destination_rejects_regular_files() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not-a-directory");
        std::fs::write(&file, b"x").unwrap();
        assert!(validate_destination_root(file.to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn destination_rejects_symlink_root() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("real");
        let link = parent.path().join("link");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &link).unwrap();
        assert!(validate_destination_root(link.to_str().unwrap()).is_err());
    }

    #[test]
    fn completion_proof_accepts_only_direct_regular_guid_file() {
        let root = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(root.path()).unwrap();
        std::fs::write(canonical.join("opaque-guid"), b"abc").unwrap();
        assert_eq!(
            completed_download_size(&canonical, "opaque-guid").unwrap(),
            3
        );
        assert!(completed_download_size(&canonical, "../escape").is_err());
        assert!(completed_download_size(&canonical, "missing").is_err());
    }

    #[test]
    fn partial_cleanup_is_confined_to_a_direct_guid_file() {
        let root = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(root.path()).unwrap();
        let partial = canonical.join("opaque-guid");
        std::fs::write(&partial, b"partial").unwrap();

        remove_proven_partial(&canonical, Some("../escape"));
        assert!(partial.exists());

        remove_proven_partial(&canonical, Some("opaque-guid"));
        assert!(!partial.exists());
    }

    #[test]
    fn cdp_download_path_preserves_ordinary_paths() {
        let path = Path::new("/tmp/approved-downloads");
        assert_eq!(download_path_for_cdp(path), path.to_string_lossy());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn cdp_download_path_removes_only_windows_verbatim_prefixes() {
        assert_eq!(
            download_path_for_cdp(Path::new(r"\\?\C:\approved\downloads")),
            r"C:\approved\downloads"
        );
        assert_eq!(
            download_path_for_cdp(Path::new(r"\\?\UNC\server\share\downloads")),
            r"\\server\share\downloads"
        );
    }
}
