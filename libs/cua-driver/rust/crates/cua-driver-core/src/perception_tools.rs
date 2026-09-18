//! Common `parse_visual_regions` tool implementation.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use cua_driver_contract::{
    ParseVisualRegionsInput, ParseVisualRegionsOutput, ToolInput, VisualActionCoordinateSpace,
    VisualCaptureProvenance, VisualCaptureSource, VisualParseError, VisualParseErrorCode,
    VisualParseTiming, VisualParserMetadata, VisualRegion, VisualRegionBounds, VisualRegionKind,
    VisualScreenshotReference, VISUAL_REGIONS_SCHEMA,
};

use crate::capture_registry::{CaptureLookupError, PerceptionCapture};
use crate::capture_runtime::{CaptureBinding, CaptureService, CaptureTarget};
use crate::perception_client::{error, PerceptionCancellation, PerceptionClient};
use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef, ToolRegistry};
use crate::tool_args::parse_typed_input;

pub type CaptureBindingResolver =
    Arc<dyn Fn(&Value) -> Result<CaptureBinding, VisualParseError> + Send + Sync + 'static>;

pub fn register_perception_tool(
    registry: &mut ToolRegistry,
    client: PerceptionClient,
    resolve_binding: CaptureBindingResolver,
) {
    let shutdown_client = client.clone();
    registry.retain_session_end_hook(crate::session::register_scoped_session_end_hook(
        move |_| shutdown_client.shutdown_now(),
    ));
    registry.register(Box::new(ParseVisualRegionsTool::new(
        registry.capture_service(),
        client,
        resolve_binding,
    )));
}

struct ParseVisualRegionsTool {
    def: ToolDef,
    captures: Arc<CaptureService>,
    client: PerceptionClient,
    resolve_binding: CaptureBindingResolver,
}

impl ParseVisualRegionsTool {
    fn new(
        captures: Arc<CaptureService>,
        client: PerceptionClient,
        resolve_binding: CaptureBindingResolver,
    ) -> Self {
        let contract = cua_driver_contract::tool_contract(ParseVisualRegionsInput::TOOL_NAME)
            .expect("parse_visual_regions has a canonical contract");
        Self {
            def: ToolDef::from_contract(&contract),
            captures,
            client,
            resolve_binding,
        }
    }
}

#[async_trait]
impl Tool for ParseVisualRegionsTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn protected_resource_scope(
        &self,
        _adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        let binding = (self.resolve_binding)(args).map_err(|error| error.message)?;
        let capture_id = args
            .get("capture_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "capture_id is required".to_owned())?;
        let capture_id = self
            .captures
            .parse_capture_id(capture_id)
            .map_err(|error| error.to_string())?;
        let capture = self
            .captures
            .read_for_perception(capture_id, &binding)
            .map_err(|error| error.to_string())?;
        Ok(Some(match capture.target() {
            CaptureTarget::Window { pid, window_id } => {
                json!({"pid": pid, "window_id": window_id, "capture_id": capture.id().to_string()})
            }
            CaptureTarget::PrimaryDesktop => {
                json!({"display": "primary", "capture_id": capture.id().to_string()})
            }
        }))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let binding = match (self.resolve_binding)(&args) {
            Ok(binding) => binding,
            Err(error) => return tool_error(error),
        };
        let mut input: ParseVisualRegionsInput = match parse_typed_input(&self.def.name, args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        if let Err(cause) = input.validate() {
            return tool_error(error(
                VisualParseErrorCode::ArtifactInvalid,
                "parse_visual_regions arguments are invalid",
                false,
                Some(cause.to_string()),
            ));
        }
        let capture_id = match self.captures.parse_capture_id(&input.capture_id) {
            Ok(id) => id,
            Err(cause) => {
                return tool_error(error(
                    VisualParseErrorCode::CaptureNotFound,
                    "capture_id is unknown",
                    false,
                    Some(cause.to_string()),
                ))
            }
        };
        input.capture_id = capture_id.to_string();
        let capture = match self.captures.read_for_perception(capture_id, &binding) {
            Ok(capture) => capture,
            Err(cause) => return tool_error(map_capture_error(cause)),
        };
        let started = Instant::now();
        let dimensions = capture.encoded_dimensions();
        let result = self
            .client
            .parse(
                &input.capture_id,
                dimensions.width(),
                dimensions.height(),
                &capture.png_bytes(),
                &PerceptionCancellation::default(),
            )
            .await;
        let worker = match result {
            Ok(result) => result,
            Err(error) => return tool_error(error),
        };
        let output = match build_output(&input, &capture, worker, started.elapsed().as_millis()) {
            Ok(output) => output,
            Err(error) => return tool_error(error),
        };
        let structured = serde_json::to_value(&output).expect("visual output serializes");
        ToolResult::text(format!("Parsed {} visual regions", output.regions.len()))
            .with_structured(structured)
    }
}

#[derive(Deserialize)]
struct WorkerRegion {
    id: String,
    kind: VisualRegionKind,
    bounds: VisualRegionBounds,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    label: Option<String>,
    confidence: f64,
    #[serde(default)]
    interactive: bool,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    group_id: Option<String>,
    #[serde(default)]
    reading_order: Option<u32>,
}

#[derive(Deserialize)]
struct WorkerIdentity {
    extension: WorkerExtensionIdentity,
    backend: String,
    #[serde(default)]
    model: Option<WorkerModelIdentity>,
    #[serde(default)]
    onnx_runtime: Option<WorkerRuntimeIdentity>,
    #[serde(default)]
    fixture_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerExtensionIdentity {
    id: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerModelIdentity {
    name: String,
    version: String,
    source_revision: String,
    manifest_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRuntimeIdentity {
    version: String,
    library_sha256: String,
    intra_threads: usize,
}

fn build_output(
    input: &ParseVisualRegionsInput,
    capture: &PerceptionCapture,
    worker: Value,
    elapsed_ms: u128,
) -> Result<ParseVisualRegionsOutput, VisualParseError> {
    validate_worker_echo(input, capture, &worker)?;
    let parser = worker_parser_metadata(&worker)?;
    if let Ok(mut output) = serde_json::from_value::<ParseVisualRegionsOutput>(worker.clone()) {
        // The Driver, rather than the extension, is authoritative for capture provenance.
        output.capture = capture_provenance(capture)?;
        output.parser = parser;
        apply_options(&mut output.regions, input);
        output.validate().map_err(invalid_worker_artifact)?;
        return Ok(output);
    }

    let regions_value = worker.get("regions").cloned().ok_or_else(|| {
        error(
            VisualParseErrorCode::ArtifactInvalid,
            "perception worker result omitted regions",
            false,
            None,
        )
    })?;
    let worker_regions: Vec<WorkerRegion> =
        serde_json::from_value(regions_value).map_err(|cause| {
            error(
                VisualParseErrorCode::ArtifactInvalid,
                "perception worker returned invalid regions",
                false,
                Some(cause.to_string()),
            )
        })?;
    let mut regions = worker_regions
        .into_iter()
        .map(|region| VisualRegion {
            id: region.id,
            kind: region.kind,
            bounds: region.bounds,
            text: region.text,
            label: region.label,
            confidence: region.confidence,
            interactive: region.interactive,
            parent_id: region.parent_id,
            group_id: region.group_id,
            reading_order: region.reading_order,
        })
        .collect::<Vec<_>>();
    apply_options(&mut regions, input);
    let output = ParseVisualRegionsOutput {
        schema: VISUAL_REGIONS_SCHEMA.into(),
        capture: capture_provenance(capture)?,
        parser,
        regions,
        warnings: Vec::new(),
        timing: VisualParseTiming {
            duration_ms: u64::try_from(elapsed_ms).unwrap_or(u64::MAX),
            preprocess_ms: None,
            inference_ms: None,
        },
        request_id: None,
    };
    output.validate().map_err(invalid_worker_artifact)?;
    Ok(output)
}

fn worker_parser_metadata(worker: &Value) -> Result<VisualParserMetadata, VisualParseError> {
    let identity: WorkerIdentity = serde_json::from_value(
        worker
            .get("identity")
            .cloned()
            .ok_or_else(|| invalid_worker_artifact("worker result omitted parser identity"))?,
    )
    .map_err(|cause| invalid_worker_artifact(format!("worker identity is malformed: {cause}")))?;
    let runtime = worker
        .get("runtime")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_worker_artifact("worker result omitted runtime identity"))?;
    if identity.extension.id != "cua-perception"
        || identity.extension.version.trim().is_empty()
        || identity.backend.trim().is_empty()
    {
        return Err(invalid_worker_artifact(
            "worker extension or backend identity is invalid",
        ));
    }

    let mut parser = VisualParserMetadata {
        extension_id: identity.extension.id,
        extension_version: identity.extension.version,
        model_id: String::new(),
        model_version: String::new(),
        runtime: Some(runtime.to_owned()),
        backend: Some(identity.backend.clone()),
        model_source_revision: None,
        model_manifest_sha256: None,
        onnx_runtime_version: None,
        onnx_runtime_library_sha256: None,
        fixture_sha256: None,
    };
    match identity.backend.as_str() {
        "onnx_runtime_cpu" => {
            if runtime != "onnx_runtime_cpu" || identity.fixture_sha256.is_some() {
                return Err(invalid_worker_artifact(
                    "worker runtime and production backend identity disagree",
                ));
            }
            let model = identity.model.ok_or_else(|| {
                invalid_worker_artifact("production worker identity omitted model provenance")
            })?;
            let onnx_runtime = identity.onnx_runtime.ok_or_else(|| {
                invalid_worker_artifact(
                    "production worker identity omitted ONNX Runtime provenance",
                )
            })?;
            if model.name.trim().is_empty()
                || model.version.trim().is_empty()
                || model.source_revision.trim().is_empty()
                || !valid_sha256(&model.manifest_sha256)
                || onnx_runtime.version.trim().is_empty()
                || !valid_sha256(&onnx_runtime.library_sha256)
                || !(1..=64).contains(&onnx_runtime.intra_threads)
            {
                return Err(invalid_worker_artifact(
                    "production worker model or runtime identity is invalid",
                ));
            }
            parser.model_id = model.name;
            parser.model_version = model.version;
            parser.model_source_revision = Some(model.source_revision);
            parser.model_manifest_sha256 = Some(model.manifest_sha256);
            parser.onnx_runtime_version = Some(onnx_runtime.version);
            parser.onnx_runtime_library_sha256 = Some(onnx_runtime.library_sha256);
        }
        "deterministic_fixture" => {
            if runtime != "fixture_only"
                || identity.model.is_some()
                || identity.onnx_runtime.is_some()
            {
                return Err(invalid_worker_artifact(
                    "worker runtime and fixture backend identity disagree",
                ));
            }
            let fixture_sha256 = identity
                .fixture_sha256
                .filter(|hash| valid_sha256(hash))
                .ok_or_else(|| {
                    invalid_worker_artifact("fixture worker identity omitted a valid fixture hash")
                })?;
            parser.model_id = "deterministic_fixture".into();
            parser.model_version = "1".into();
            parser.fixture_sha256 = Some(fixture_sha256);
        }
        _ => {
            return Err(invalid_worker_artifact(
                "worker reported an unsupported backend identity",
            ))
        }
    }
    Ok(parser)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn capture_provenance(
    capture: &PerceptionCapture,
) -> Result<VisualCaptureProvenance, VisualParseError> {
    let dimensions = capture.encoded_dimensions();
    let digest = capture.digest().hex();
    let source = match capture.target() {
        CaptureTarget::Window { pid, window_id } => VisualCaptureSource::Window {
            pid: *pid,
            window_id: *window_id,
        },
        CaptureTarget::PrimaryDesktop => VisualCaptureSource::PrimaryDesktop {
            display_id: "primary".into(),
        },
    };
    let [m11, m12, m21, m22, tx, ty] = capture.screenshot_to_action().coefficients();
    let action_coordinate_space = if [m11, m12, m21, m22, tx, ty] == [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    {
        VisualActionCoordinateSpace::ScreenshotPixels
    } else {
        VisualActionCoordinateSpace::Affine {
            m11,
            m12,
            m21,
            m22,
            tx,
            ty,
        }
    };
    Ok(VisualCaptureProvenance {
        capture_id: capture.id().to_string(),
        source,
        screenshot: VisualScreenshotReference {
            reference: format!("png-sha256:{digest}"),
            width: dimensions.width(),
            height: dimensions.height(),
            mime_type: "image/png".into(),
            sha256: Some(digest),
        },
        action_coordinate_space,
        captured_at: None,
    })
}

fn apply_options(regions: &mut Vec<VisualRegion>, input: &ParseVisualRegionsInput) {
    regions.retain(|region| {
        input
            .options
            .kinds
            .as_ref()
            .is_none_or(|kinds| kinds.contains(&region.kind))
            && input
                .options
                .min_confidence
                .is_none_or(|minimum| region.confidence >= minimum)
    });
    if let Some(maximum) = input.options.max_regions {
        regions.truncate(maximum as usize);
    }
    let retained = regions
        .iter()
        .map(|region| region.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for region in regions.iter_mut() {
        if region
            .parent_id
            .as_deref()
            .is_some_and(|id| !retained.contains(id))
        {
            region.parent_id = None;
        }
        if region
            .group_id
            .as_deref()
            .is_some_and(|id| !retained.contains(id))
        {
            region.group_id = None;
        }
    }
}

fn validate_worker_echo(
    input: &ParseVisualRegionsInput,
    capture: &PerceptionCapture,
    worker: &Value,
) -> Result<(), VisualParseError> {
    if let Some(capture_id) = worker.get("capture_id").and_then(Value::as_str) {
        if capture_id != input.capture_id {
            return Err(invalid_worker_artifact(
                "worker capture_id does not match the requested capture",
            ));
        }
    }
    let Some(image) = worker.get("image") else {
        return Ok(());
    };
    let dimensions = capture.encoded_dimensions();
    let digest_matches = image
        .get("sha256")
        .and_then(Value::as_str)
        .is_some_and(|digest| digest == capture.digest().hex());
    let dimensions_match = image.get("width").and_then(Value::as_u64)
        == Some(u64::from(dimensions.width()))
        && image.get("height").and_then(Value::as_u64) == Some(u64::from(dimensions.height()));
    if !digest_matches || !dimensions_match {
        return Err(invalid_worker_artifact(
            "worker image identity does not match the retained capture",
        ));
    }
    if worker
        .get("coordinate_space")
        .and_then(Value::as_str)
        .is_some_and(|space| space != "image_pixels")
    {
        return Err(invalid_worker_artifact(
            "worker regions are not expressed in source image pixels",
        ));
    }
    Ok(())
}

fn map_capture_error(cause: CaptureLookupError) -> VisualParseError {
    let code = match cause {
        CaptureLookupError::Unknown | CaptureLookupError::TargetMismatch => {
            VisualParseErrorCode::CaptureNotFound
        }
        CaptureLookupError::Expired => VisualParseErrorCode::CaptureExpired,
        CaptureLookupError::GenerationMismatch => VisualParseErrorCode::CaptureGenerationMismatch,
    };
    error(code, cause.to_string(), false, None)
}

fn invalid_worker_artifact(cause: impl ToString) -> VisualParseError {
    error(
        VisualParseErrorCode::ArtifactInvalid,
        "perception worker output failed contract validation",
        false,
        Some(cause.to_string()),
    )
}

fn tool_error(error: VisualParseError) -> ToolResult {
    let message = error.message.clone();
    ToolResult::error(message)
        .with_structured(serde_json::to_value(error).expect("visual parse error serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_registry::{CaptureRegistryConfig, MonotonicClock};
    use crate::capture_runtime::{
        CapturePublication, EncodedScreenshotDimensions, NativeActionDimensions,
        ScreenshotToActionTransform,
    };
    use crate::image_utils::encode_rgba_to_png;
    use sha2::Digest as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[derive(Default)]
    struct ManualClock(AtomicU64);

    impl MonotonicClock for ManualClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

    fn capture(service: &Arc<CaptureService>) -> (String, CaptureBinding) {
        let png = encode_rgba_to_png(&[7, 8, 9, 255], 1, 1).unwrap();
        let id = service
            .publish(CapturePublication {
                png_bytes: png,
                target: CaptureTarget::PrimaryDesktop,
                encoded_dimensions: EncodedScreenshotDimensions::new(1, 1).unwrap(),
                native_action_dimensions: NativeActionDimensions::new(1, 1).unwrap(),
                screenshot_to_action: ScreenshotToActionTransform::identity(),
                session_id: Arc::from("session-a"),
                session_generation: 1,
            })
            .unwrap();
        (id.to_string(), service.binding("session-a", 1).unwrap())
    }

    #[tokio::test]
    async fn absent_extension_returns_stable_not_installed_error() {
        let service = Arc::new(CaptureService::default());
        let (capture_id, binding) = capture(&service);
        let tool = ParseVisualRegionsTool::new(
            service,
            PerceptionClient::unavailable(),
            Arc::new(move |_| Ok(binding.clone())),
        );
        let result = tool.invoke(json!({"capture_id": capture_id})).await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap()["code"], "not_installed");
    }

    #[tokio::test]
    async fn invalid_retired_and_cross_session_captures_fail_before_worker_launch() {
        let service = Arc::new(CaptureService::default());
        let (capture_id, binding) = capture(&service);
        let missing_worker =
            PerceptionClient::new(crate::perception_client::PerceptionWorkerConfig::new(
                "/definitely/not/a/perception-worker",
            ))
            .unwrap();

        let other_binding = service.current_binding("session-b").unwrap();
        let cross_session = ParseVisualRegionsTool::new(
            service.clone(),
            missing_worker.clone(),
            Arc::new(move |_| Ok(other_binding.clone())),
        )
        .invoke(json!({"capture_id": capture_id.clone()}))
        .await;
        assert_eq!(
            cross_session.structured_content.unwrap()["code"],
            "capture_generation_mismatch"
        );

        let malformed = ParseVisualRegionsTool::new(
            service.clone(),
            missing_worker.clone(),
            Arc::new({
                let binding = binding.clone();
                move |_| Ok(binding.clone())
            }),
        )
        .invoke(json!({"capture_id": "not-a-capture"}))
        .await;
        assert_eq!(
            malformed.structured_content.unwrap()["code"],
            "capture_not_found"
        );

        let id = service.parse_capture_id(&capture_id).unwrap();
        service.retire_capture(id, &binding).unwrap();
        let retired = ParseVisualRegionsTool::new(
            service,
            missing_worker,
            Arc::new(move |_| Ok(binding.clone())),
        )
        .invoke(json!({"capture_id": capture_id}))
        .await;
        assert_eq!(
            retired.structured_content.unwrap()["code"],
            "capture_not_found"
        );
    }

    #[tokio::test]
    async fn expired_capture_fails_at_the_tool_boundary_before_worker_launch() {
        let clock = Arc::new(ManualClock::default());
        let mut config = CaptureRegistryConfig::default();
        config.ttl = Duration::from_millis(10);
        let service = Arc::new(CaptureService::with_clock(config, clock.clone()).unwrap());
        let (capture_id, binding) = capture(&service);
        clock.0.store(10, Ordering::SeqCst);
        let tool = ParseVisualRegionsTool::new(
            service,
            PerceptionClient::new(crate::perception_client::PerceptionWorkerConfig::new(
                "/definitely/not/a/perception-worker",
            ))
            .unwrap(),
            Arc::new(move |_| Ok(binding.clone())),
        );
        let result = tool.invoke(json!({"capture_id": capture_id})).await;
        assert_eq!(
            result.structured_content.unwrap()["code"],
            "capture_expired"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_maps_worker_icon_output_into_the_driver_contract() {
        use std::os::unix::fs::PermissionsExt;

        #[cfg(target_os = "macos")]
        const SHEBANG: &str = "#!/Applications/Xcode.app/Contents/Developer/usr/bin/python3";
        #[cfg(not(target_os = "macos"))]
        const SHEBANG: &str = "#!/usr/bin/env python3";

        let service = Arc::new(CaptureService::default());
        let (capture_id, binding) = capture(&service);
        let expected = service
            .read_for_perception(service.parse_capture_id(&capture_id).unwrap(), &binding)
            .unwrap()
            .digest()
            .hex();
        let extension_version = "0.1.0";
        let directory = tempfile::tempdir().unwrap();
        let worker = directory.path().join("worker.py");
        let script = format!(
            r#"{SHEBANG}
import base64, hashlib, json, struct, sys
def read():
 p=sys.stdin.buffer.read(4); n=struct.unpack('>I',p)[0]; return json.loads(sys.stdin.buffer.read(n))
def write(v):
 p=json.dumps(v,separators=(',',':')).encode(); sys.stdout.buffer.write(struct.pack('>I',len(p))+p); sys.stdout.buffer.flush()
h=read()
health={{'protocol':'cua-perception/1','request_id':h['request_id'],'status':'ok','result':{{
 'ready':True,
 'protocol':'cua-perception/1',
 'identity':{{'extension':{{'id':'cua-perception','version':'{extension_version}'}}}}
}}}}
write(health)
r=read(); raw=base64.b64decode(r['params']['image']['data_base64'])
if hashlib.sha256(raw).hexdigest() != '{expected}':
 write({{'protocol':'cua-perception/1','request_id':r['request_id'],'status':'error','error':{{'code':'invalid_image','message':'wrong capture bytes'}}}})
else:
 write({{'protocol':'cua-perception/1','request_id':r['request_id'],'status':'ok','result':{{'runtime':'fixture_only','identity':{{'extension':{{'id':'cua-perception','version':'{extension_version}'}},'backend':'deterministic_fixture','fixture_sha256':'{expected}'}},'regions':[{{'id':'one','kind':'icon','bounds':{{'x':0,'y':0,'width':1,'height':1}},'label':'icon-class-4','confidence':0.9}}]}}}})
"#
        );
        std::fs::write(&worker, script).unwrap();
        let mut permissions = std::fs::metadata(&worker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&worker, permissions).unwrap();
        let containment = crate::perception_client::containment::ContainmentLimits {
            additional_readable_paths: [
                "/usr",
                "/bin",
                "/lib",
                "/lib64",
                "/etc",
                "/opt",
                "/System",
                "/Library",
                "/private/var/db",
                "/private/var/select",
                "/Applications/Xcode.app",
            ]
            .into_iter()
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_dir())
            .collect(),
            additional_executable_paths: [
                "/usr/bin/env",
                "/usr/bin/python3",
                "/Applications/Xcode.app/Contents/Developer/usr/bin/python3",
                "/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/bin/python3.9",
                "/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/Python3",
                "/Applications/Xcode.app/Contents/Developer/Library/Frameworks/Python3.framework/Versions/3.9/Resources/Python.app/Contents/MacOS/Python",
            ]
            .into_iter()
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_file())
            .collect(),
            ..Default::default()
        };
        let client = PerceptionClient::new(
            crate::perception_client::PerceptionWorkerConfig::installed_with_identity(
                worker,
                "cua-perception",
                extension_version,
            )
            .with_test_containment(containment),
        )
        .unwrap();
        let action_binding = binding.clone();
        let tool = ParseVisualRegionsTool::new(
            service.clone(),
            client,
            Arc::new(move |_| Ok(binding.clone())),
        );
        let result = tool.invoke(json!({"capture_id": capture_id.clone()})).await;
        assert_ne!(
            result.is_error,
            Some(true),
            "worker mapping failed: {:?}",
            result.structured_content
        );
        let output: ParseVisualRegionsOutput =
            serde_json::from_value(result.structured_content.unwrap()).unwrap();
        assert_eq!(output.regions.len(), 1);
        assert_eq!(output.capture.capture_id, capture_id);
        assert_eq!(output.regions[0].label.as_deref(), Some("icon-class-4"));
        assert_eq!(output.parser.extension_id, "cua-perception");
        assert_eq!(output.parser.extension_version, extension_version);
        assert_eq!(
            output.parser.backend.as_deref(),
            Some("deterministic_fixture")
        );
        assert_eq!(
            output.parser.fixture_sha256.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            output.capture.screenshot.sha256.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            output.capture.source,
            VisualCaptureSource::PrimaryDesktop {
                display_id: "primary".into()
            }
        );
        let admitted = service
            .admit_action(crate::capture_runtime::CaptureActionRequest {
                capture_id: service.parse_capture_id(&capture_id).unwrap(),
                binding: action_binding.clone(),
                target: CaptureTarget::PrimaryDesktop,
                current_native_action_dimensions: NativeActionDimensions::new(1, 1).unwrap(),
                screenshot_x: 0.5,
                screenshot_y: 0.5,
            })
            .unwrap();
        assert_eq!((admitted.action_x, admitted.action_y), (0.5, 0.5));
        assert_eq!(admitted.digest.hex(), expected);
        assert_eq!(
            service
                .admit_action(crate::capture_runtime::CaptureActionRequest {
                    capture_id: service.parse_capture_id(&capture_id).unwrap(),
                    binding: action_binding,
                    target: CaptureTarget::PrimaryDesktop,
                    current_native_action_dimensions: NativeActionDimensions::new(1, 1).unwrap(),
                    screenshot_x: 0.5,
                    screenshot_y: 0.5,
                })
                .unwrap_err(),
            crate::capture_runtime::CaptureActionError::Lookup(CaptureLookupError::Unknown)
        );
        output.validate().unwrap();
    }

    fn production_worker_identity() -> Value {
        json!({
            "extension": {
                "id": "cua-perception",
                "version": "0.1.0"
            },
            "backend": "onnx_runtime_cpu",
            "model": {
                "name": "omniparser-v2-ppocrv5-en",
                "version": "2026-09-17",
                "source_revision": "6600256cb0f1b07651e3bc86166196307bad7e2d",
                "manifest_sha256": "a".repeat(64)
            },
            "onnx_runtime": {
                "version": "1.26.0",
                "library_sha256": "b".repeat(64),
                "intra_threads": 2
            }
        })
    }

    #[test]
    fn production_worker_icon_and_verified_identity_reach_the_driver_contract() {
        let service = Arc::new(CaptureService::default());
        let (capture_id, binding) = capture(&service);
        let capture = service
            .read_for_perception(service.parse_capture_id(&capture_id).unwrap(), &binding)
            .unwrap();
        let input = ParseVisualRegionsInput {
            capture_id: capture_id
                .to_ascii_uppercase()
                .replacen("CAPTURE_", "capture_", 1),
            options: Default::default(),
        };
        let output = build_output(
            &input,
            &capture,
            json!({
                "runtime": "onnx_runtime_cpu",
                "identity": production_worker_identity(),
                "regions": [{
                    "id": "icon-1",
                    "kind": "icon",
                    "bounds": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "label": "icon-class-4",
                    "confidence": 0.9
                }]
            }),
            7,
        )
        .unwrap();

        output.validate().unwrap();
        assert_eq!(output.capture.capture_id, capture_id);
        assert_eq!(output.regions[0].label.as_deref(), Some("icon-class-4"));
        assert_eq!(output.parser.extension_id, "cua-perception");
        assert_eq!(output.parser.extension_version, "0.1.0");
        assert_eq!(output.parser.model_id, "omniparser-v2-ppocrv5-en");
        assert_eq!(output.parser.model_version, "2026-09-17");
        assert_eq!(
            output.parser.model_source_revision.as_deref(),
            Some("6600256cb0f1b07651e3bc86166196307bad7e2d")
        );
        assert_eq!(output.parser.model_manifest_sha256, Some("a".repeat(64)));
        assert_eq!(
            output.parser.onnx_runtime_version.as_deref(),
            Some("1.26.0")
        );
        assert_eq!(
            output.parser.onnx_runtime_library_sha256,
            Some("b".repeat(64))
        );
    }

    #[test]
    fn malformed_or_missing_production_worker_identity_is_rejected() {
        let service = Arc::new(CaptureService::default());
        let (capture_id, binding) = capture(&service);
        let capture = service
            .read_for_perception(service.parse_capture_id(&capture_id).unwrap(), &binding)
            .unwrap();
        let input = ParseVisualRegionsInput {
            capture_id,
            options: Default::default(),
        };
        let regions = json!([{
            "id": "icon-1",
            "kind": "icon",
            "bounds": {"x": 0, "y": 0, "width": 1, "height": 1},
            "label": "icon-class-4",
            "confidence": 0.9
        }]);
        let missing = build_output(
            &input,
            &capture,
            json!({"runtime": "onnx_runtime_cpu", "regions": regions}),
            0,
        )
        .unwrap_err();
        assert_eq!(missing.code, VisualParseErrorCode::ArtifactInvalid);

        let mut identity = production_worker_identity();
        identity["model"]["manifest_sha256"] = json!("not-a-sha256");
        let malformed = build_output(
            &input,
            &capture,
            json!({
                "runtime": "onnx_runtime_cpu",
                "identity": identity,
                "regions": regions
            }),
            0,
        )
        .unwrap_err();
        assert_eq!(malformed.code, VisualParseErrorCode::ArtifactInvalid);
    }

    #[test]
    fn parsed_window_provenance_keeps_native_ids_and_full_affine_mapping() {
        let service = Arc::new(CaptureService::default());
        let png = encode_rgba_to_png(&vec![17; 4 * 3 * 4], 4, 3).unwrap();
        let transform = ScreenshotToActionTransform::new(1.5, 0.25, -0.5, 2.0, 7.25, -3.5).unwrap();
        let target = CaptureTarget::Window {
            pid: 4242,
            window_id: 0xfeed,
        };
        let id = service
            .publish(CapturePublication {
                png_bytes: png.clone(),
                target: target.clone(),
                encoded_dimensions: EncodedScreenshotDimensions::new(4, 3).unwrap(),
                native_action_dimensions: NativeActionDimensions::new(9, 7).unwrap(),
                screenshot_to_action: transform,
                session_id: Arc::from("window-parse"),
                session_generation: 1,
            })
            .unwrap();
        let binding = service.binding("window-parse", 1).unwrap();
        let capture = service.read_for_perception(id, &binding).unwrap();
        let input = ParseVisualRegionsInput {
            capture_id: id.to_string(),
            options: Default::default(),
        };
        let output = build_output(
            &input,
            &capture,
            json!({
                "runtime": "fixture_only",
                "identity": {
                    "extension": {"id": "cua-perception", "version": "0.1.0"},
                    "backend": "deterministic_fixture",
                    "fixture_sha256": "a".repeat(64)
                },
                "regions": [{
                    "id": "target",
                    "kind": "icon",
                    "bounds": {"x": 1, "y": 1, "width": 1, "height": 1},
                    "label": "target",
                    "confidence": 1.0
                }]
            }),
            0,
        )
        .unwrap();
        assert_eq!(
            output.capture.source,
            VisualCaptureSource::Window {
                pid: 4242,
                window_id: 0xfeed
            }
        );
        assert_eq!(output.capture.capture_id, id.to_string());
        assert_eq!(
            output.capture.screenshot.sha256,
            Some(
                sha2::Sha256::digest(&png)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            )
        );
        assert_eq!(
            output.capture.action_coordinate_space.map_point(1.5, 1.5),
            transform.apply(1.5, 1.5)
        );
    }
}
