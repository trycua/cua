//! Common `parse_visual_regions` tool implementation.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use cua_driver_contract::{
    ParseVisualRegionsInput, ParseVisualRegionsOutput, VisualActionCoordinateSpace,
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
        Self {
            def: ToolDef {
                name: "parse_visual_regions".into(),
                description: "Parse text and icon regions from an immutable Driver capture.".into(),
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["capture_id"],
                    "properties": {
                        "capture_id": {"type": "string", "minLength": 1},
                        "options": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "kinds": {
                                    "type": ["array", "null"],
                                    "minItems": 1,
                                    "maxItems": 2,
                                    "uniqueItems": true,
                                    "items": {"enum": ["text", "icon"]}
                                },
                                "min_confidence": {"type": ["number", "null"], "minimum": 0, "maximum": 1},
                                "max_regions": {"type": ["integer", "null"], "minimum": 1}
                            }
                        }
                    }
                }),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
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
        let input: ParseVisualRegionsInput = match parse_typed_input(&self.def.name, args) {
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

fn build_output(
    input: &ParseVisualRegionsInput,
    capture: &PerceptionCapture,
    worker: Value,
    elapsed_ms: u128,
) -> Result<ParseVisualRegionsOutput, VisualParseError> {
    validate_worker_echo(input, capture, &worker)?;
    if let Ok(mut output) = serde_json::from_value::<ParseVisualRegionsOutput>(worker.clone()) {
        // The Driver, rather than the extension, is authoritative for capture provenance.
        output.capture = capture_provenance(capture)?;
        output.capture.capture_id = input.capture_id.clone();
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
        parser: VisualParserMetadata {
            extension_id: "cua-perception".into(),
            extension_version: "1".into(),
            model_id: "visual-regions-default".into(),
            model_version: "1".into(),
            runtime: worker
                .get("runtime")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
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
    } else if m12 == 0.0 && m21 == 0.0 && m11 > 0.0 && m22 > 0.0 {
        VisualActionCoordinateSpace::ScaledTopLeft {
            action_origin_x: tx,
            action_origin_y: ty,
            action_units_per_pixel_x: m11,
            action_units_per_pixel_y: m22,
        }
    } else {
        return Err(error(
            VisualParseErrorCode::UnsupportedTarget,
            "the public visual-region contract cannot represent this capture transform",
            false,
            None,
        ));
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
    use crate::capture_runtime::{
        CapturePublication, EncodedScreenshotDimensions, NativeActionDimensions,
        ScreenshotToActionTransform,
    };
    use crate::image_utils::encode_rgba_to_png;

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

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_sends_registry_owned_png_and_validates_worker_regions() {
        use std::os::unix::fs::PermissionsExt;

        let service = Arc::new(CaptureService::default());
        let (capture_id, binding) = capture(&service);
        let expected = service
            .read_for_perception(service.parse_capture_id(&capture_id).unwrap(), &binding)
            .unwrap()
            .digest()
            .hex();
        let directory = tempfile::tempdir().unwrap();
        let worker = directory.path().join("worker.py");
        let script = format!(
            r#"#!/usr/bin/env python3
import base64, hashlib, json, struct, sys
def read():
 p=sys.stdin.buffer.read(4); n=struct.unpack('>I',p)[0]; return json.loads(sys.stdin.buffer.read(n))
def write(v):
 p=json.dumps(v,separators=(',',':')).encode(); sys.stdout.buffer.write(struct.pack('>I',len(p))+p); sys.stdout.buffer.flush()
h=read(); write({{'protocol':'cua-perception/1','request_id':h['request_id'],'status':'ok','result':{{'ready':True,'protocol':'cua-perception/1'}}}})
r=read(); raw=base64.b64decode(r['params']['image']['data_base64'])
if hashlib.sha256(raw).hexdigest() != '{expected}':
 write({{'protocol':'cua-perception/1','request_id':r['request_id'],'status':'error','error':{{'code':'invalid_image','message':'wrong capture bytes'}}}})
else:
 write({{'protocol':'cua-perception/1','request_id':r['request_id'],'status':'ok','result':{{'runtime':'fixture','regions':[{{'id':'one','kind':'text','bounds':{{'x':0,'y':0,'width':1,'height':1}},'text':'pixel','confidence':0.9}}]}}}})
"#
        );
        std::fs::write(&worker, script).unwrap();
        let mut permissions = std::fs::metadata(&worker).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&worker, permissions).unwrap();
        let client = PerceptionClient::new(crate::perception_client::PerceptionWorkerConfig::new(
            worker,
        ))
        .unwrap();
        let tool =
            ParseVisualRegionsTool::new(service, client, Arc::new(move |_| Ok(binding.clone())));
        let result = tool.invoke(json!({"capture_id": capture_id})).await;
        assert_ne!(result.is_error, Some(true));
        let output: ParseVisualRegionsOutput =
            serde_json::from_value(result.structured_content.unwrap()).unwrap();
        assert_eq!(output.regions.len(), 1);
        assert_eq!(output.regions[0].text.as_deref(), Some("pixel"));
        assert_eq!(
            output.capture.screenshot.sha256.as_deref(),
            Some(expected.as_str())
        );
        output.validate().unwrap();
    }
}
