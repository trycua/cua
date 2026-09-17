// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Model-neutral DTOs for parsing an immutable screenshot into visual regions.
//!
//! This module is a transport-free public contract. It registers the callable
//! tool schema without implementing dispatch. A Driver-owned capture registry
//! must retain an immutable PNG together with its exact
//! target and screenshot-to-action transform for every `capture_id`. Current
//! `WindowStateOutput::snapshot_id` values do not retain screenshot pixels, and
//! `DesktopStateOutput` has no capture ID, so neither satisfies this contract.
//!
//! V1 deliberately refers to registry captures instead of accepting inline
//! bytes or file paths. Existing MCP images carry base64 data in the envelope,
//! while file paths are local-process metadata and are not portable. The V1
//! DTOs use explicit fields compatible with UniFFI; future additive data should
//! use optional typed fields or a new versioned schema rather than dynamic JSON.

use crate::{
    CursorAction, CursorSemantics, Platform, SchemaMode, ToolAnnotations, ToolContract, ToolInput,
    ToolOutput,
};
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const VISUAL_REGIONS_SCHEMA: &str = "cua.visual_regions_v1";

fn positive_integer_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 1 })
}

fn pixel_integer_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 0 })
}

fn confidence_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "number", "minimum": 0, "maximum": 1 })
}

fn png_mime_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "const": "image/png" })
}

fn primary_display_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "const": "primary" })
}

fn visual_regions_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "const": VISUAL_REGIONS_SCHEMA })
}

fn region_kinds_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "anyOf": [
            {
                "type": "array",
                "minItems": 1,
                "maxItems": 2,
                "uniqueItems": true,
                "items": { "enum": ["text", "icon"] }
            },
            { "type": "null" }
        ]
    })
}

/// Exact screen content that produced the immutable capture.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VisualCaptureSource {
    Window {
        pid: u32,
        window_id: u64,
    },
    PrimaryDesktop {
        #[schemars(schema_with = "primary_display_schema")]
        display_id: String,
    },
}

/// Screenshot identity and geometry retained by the future capture registry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VisualScreenshotReference {
    /// Opaque identity for these exact encoded pixels, such as a content digest.
    pub reference: String,
    #[schemars(schema_with = "positive_integer_schema")]
    pub width: u32,
    #[schemars(schema_with = "positive_integer_schema")]
    pub height: u32,
    #[schemars(schema_with = "png_mime_schema")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Normative mapping from source screenshot pixels to Driver action coordinates.
///
/// Screenshot coordinates always originate at the encoded PNG's top-left; X
/// increases right and Y increases down. The affine form maps a source point
/// `(px, py)` exactly as:
///
/// `action_x = m11 * px + m12 * py + tx`
///
/// `action_y = m21 * px + m22 * py + ty`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Enum)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VisualActionCoordinateSpace {
    /// Action coordinates are the same physical pixel coordinates as the PNG.
    ScreenshotPixels,
    /// Lossless six-coefficient affine transform retained by the capture registry.
    Affine {
        m11: f64,
        m12: f64,
        m21: f64,
        m22: f64,
        tx: f64,
        ty: f64,
    },
}

impl VisualActionCoordinateSpace {
    pub fn map_point(&self, pixel_x: f64, pixel_y: f64) -> (f64, f64) {
        match self {
            Self::ScreenshotPixels => (pixel_x, pixel_y),
            Self::Affine {
                m11,
                m12,
                m21,
                m22,
                tx,
                ty,
            } => (
                m11.mul_add(pixel_x, m12.mul_add(pixel_y, *tx)),
                m21.mul_add(pixel_x, m22.mul_add(pixel_y, *ty)),
            ),
        }
    }

    fn validate(&self) -> Result<(), VisualContractValidationError> {
        match self {
            Self::ScreenshotPixels => Ok(()),
            Self::Affine {
                m11,
                m12,
                m21,
                m22,
                tx,
                ty,
            } if [m11, m12, m21, m22, tx, ty]
                .into_iter()
                .all(|value| value.is_finite())
                && m11.mul_add(*m22, -(*m12 * *m21)).abs() > f64::EPSILON =>
            {
                Ok(())
            }
            Self::Affine { .. } => Err(VisualContractValidationError::InvalidCoordinateSpace),
        }
    }
}

/// Immutable capture provenance echoed by a parse result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct VisualCaptureProvenance {
    pub capture_id: String,
    pub source: VisualCaptureSource,
    pub screenshot: VisualScreenshotReference,
    pub action_coordinate_space: VisualActionCoordinateSpace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
}

/// Optional, model-neutral controls for one bounded parse.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct ParseVisualRegionsOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "region_kinds_schema")]
    pub kinds: Option<Vec<VisualRegionKind>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "confidence_schema")]
    pub min_confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "positive_integer_schema")]
    pub max_regions: Option<u32>,
}

/// Transport-free request. Runtime use requires the future capture registry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct ParseVisualRegionsInput {
    pub capture_id: String,
    #[serde(default)]
    pub options: ParseVisualRegionsOptions,
}

impl ToolInput for ParseVisualRegionsInput {
    const TOOL_NAME: &'static str = "parse_visual_regions";

    fn validate(&self) -> Result<(), String> {
        ParseVisualRegionsInput::validate(self).map_err(|error| error.to_string())
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    uniffi::Enum,
)]
#[serde(rename_all = "snake_case")]
pub enum VisualRegionKind {
    Text,
    Icon,
}

/// Integer pixel-edge bounds `[x, x + width) x [y, y + height)`.
///
/// `x` and `y` name the left and top pixel edges. The right and bottom edges
/// are excluded. A region therefore covers every pixel whose center lies in
/// this half-open box; its geometric center may lie between pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VisualRegionBounds {
    #[schemars(schema_with = "pixel_integer_schema")]
    pub x: u32,
    #[schemars(schema_with = "pixel_integer_schema")]
    pub y: u32,
    #[schemars(schema_with = "positive_integer_schema")]
    pub width: u32,
    #[schemars(schema_with = "positive_integer_schema")]
    pub height: u32,
}

impl VisualRegionBounds {
    /// Geometric center in screenshot pixels. Odd-sized boxes intentionally
    /// produce half-pixel centers; no redundant rounded integer center is sent.
    /// The `f64` point is preserved through Driver mapping. Native dispatch may
    /// round only under the affected platform action contract, outside V1.
    pub fn center(&self) -> (f64, f64) {
        (
            f64::from(self.x) + f64::from(self.width) / 2.0,
            f64::from(self.y) + f64::from(self.height) / 2.0,
        )
    }
}

/// One parser-observed region in the exact source screenshot pixel space.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct VisualRegion {
    /// Stable identifier unique within this parse result.
    pub id: String,
    pub kind: VisualRegionKind,
    pub bounds: VisualRegionBounds,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[schemars(schema_with = "confidence_schema")]
    pub confidence: f64,
    pub interactive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_order: Option<u32>,
}

impl VisualRegion {
    /// Map the unrounded geometric center into action coordinates. This DTO
    /// layer never applies integer rounding or platform dispatch policy.
    pub fn action_center(&self, space: &VisualActionCoordinateSpace) -> (f64, f64) {
        let (pixel_x, pixel_y) = self.bounds.center();
        space.map_point(pixel_x, pixel_y)
    }
}

/// Identity of the extension and model that produced a result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VisualParserMetadata {
    pub extension_id: String,
    pub extension_version: String,
    pub model_id: String,
    pub model_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VisualParseWarning {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VisualParseTiming {
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprocess_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct ParseVisualRegionsOutput {
    #[schemars(schema_with = "visual_regions_schema")]
    pub schema: String,
    pub capture: VisualCaptureProvenance,
    pub parser: VisualParserMetadata,
    pub regions: Vec<VisualRegion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<VisualParseWarning>,
    pub timing: VisualParseTiming,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ToolOutput for ParseVisualRegionsOutput {
    fn validate(&self) -> Result<(), String> {
        ParseVisualRegionsOutput::validate(self).map_err(|error| error.to_string())
    }
}

/// Stable machine-readable failures for capture lookup and visual parsing.
#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    uniffi::Enum,
)]
#[serde(rename_all = "snake_case")]
pub enum VisualParseErrorCode {
    NotInstalled,
    CaptureNotFound,
    CaptureExpired,
    CaptureStale,
    CaptureGenerationMismatch,
    UnsupportedTarget,
    UnsupportedPlatform,
    IncompatibleProtocol,
    InvalidFrame,
    WorkerLaunchFailed,
    WorkerCrashed,
    WorkerCancelled,
    Timeout,
    ResourceLimitExceeded,
    ArtifactInvalid,
    InferenceFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VisualParseError {
    pub code: VisualParseErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

fn error_schema() -> Value {
    crate::outputs::output_schema_with_additional_properties::<VisualParseError>(false)
}

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub(crate) fn contracts() -> Vec<ToolContract> {
    vec![ToolContract {
        name: ParseVisualRegionsInput::TOOL_NAME.into(),
        description:
            "Parse one immutable registered capture into model-neutral text and icon regions."
                .into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: vec![
            "visual.regions.parse".into(),
            "screen.capture.registry.read".into(),
        ],
        annotations: ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        schema_mode: SchemaMode::CanonicalRuntime,
        cursor_semantics: Some(CursorSemantics::new(CursorAction::Observe)),
        input_schema: ParseVisualRegionsInput::input_schema(),
        success_output_schema: Some(ParseVisualRegionsOutput::output_schema()),
        error_output_schema: Some(error_schema()),
        output_validator: crate::validate_typed_output::<ParseVisualRegionsOutput>,
    }]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualContractValidationError {
    EmptyCaptureId,
    InvalidCaptureSource,
    EmptyScreenshotReference,
    InvalidScreenshotDimensions,
    UnsupportedScreenshotMimeType,
    InvalidScreenshotDigest,
    InvalidCaptureMetadata,
    InvalidCoordinateSpace,
    InvalidMinimumConfidence,
    InvalidMaximumRegions,
    InvalidRegionKinds,
    InvalidResultSchema,
    InvalidRegionSize,
    RegionOutsideScreenshot,
    InvalidRegionConfidence,
    EmptyRegionId,
    DuplicateRegionId,
    InvalidRegionContent,
    MissingRegionReference,
    SelfRegionReference,
    CyclicParentReference,
    InvalidParserMetadata,
    InvalidWarning,
    InvalidTiming,
    InvalidRequestId,
    InvalidError,
}

impl std::fmt::Display for VisualContractValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::EmptyCaptureId => "capture_id must not be empty",
            Self::InvalidCaptureSource => {
                "capture source must identify a positive window or the primary display"
            }
            Self::EmptyScreenshotReference => "screenshot reference must not be empty",
            Self::InvalidScreenshotDimensions => "screenshot dimensions must be positive",
            Self::UnsupportedScreenshotMimeType => "screenshot mime_type must be image/png",
            Self::InvalidScreenshotDigest => "screenshot sha256 must be a 64-character hex digest",
            Self::InvalidCaptureMetadata => "optional capture metadata must not be empty",
            Self::InvalidCoordinateSpace => {
                "action coordinate transform must be finite and invertible"
            }
            Self::InvalidMinimumConfidence => "min_confidence must be finite and between 0 and 1",
            Self::InvalidMaximumRegions => "max_regions must be positive",
            Self::InvalidRegionKinds => "kinds must contain unique text or icon entries",
            Self::InvalidResultSchema => "visual result schema is not cua.visual_regions_v1",
            Self::InvalidRegionSize => "visual region width and height must be positive",
            Self::RegionOutsideScreenshot => "visual region bounds exceed the screenshot",
            Self::InvalidRegionConfidence => "region confidence must be finite and between 0 and 1",
            Self::EmptyRegionId => "visual region id must not be empty",
            Self::DuplicateRegionId => "visual region ids must be unique within a result",
            Self::InvalidRegionContent => "text and icon regions require matching content",
            Self::MissingRegionReference => "parent_id and group_id must reference a result region",
            Self::SelfRegionReference => "a region cannot parent or group itself",
            Self::CyclicParentReference => "parent_id references must not form a cycle",
            Self::InvalidParserMetadata => {
                "parser and model identifiers and versions must not be empty"
            }
            Self::InvalidWarning => "warning code and message must not be empty",
            Self::InvalidTiming => "timing components must not exceed total duration",
            Self::InvalidRequestId => "request_id must not be empty when present",
            Self::InvalidError => {
                "visual parse error message and optional detail must not be empty"
            }
        })
    }
}

impl std::error::Error for VisualContractValidationError {}

fn nonempty(value: &str) -> bool {
    !value.trim().is_empty()
}

impl VisualScreenshotReference {
    fn validate(&self) -> Result<(), VisualContractValidationError> {
        if !nonempty(&self.reference) {
            return Err(VisualContractValidationError::EmptyScreenshotReference);
        }
        if self.width == 0 || self.height == 0 {
            return Err(VisualContractValidationError::InvalidScreenshotDimensions);
        }
        if self.mime_type != "image/png" {
            return Err(VisualContractValidationError::UnsupportedScreenshotMimeType);
        }
        if self.sha256.as_ref().is_some_and(|digest| {
            digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Err(VisualContractValidationError::InvalidScreenshotDigest);
        }
        Ok(())
    }
}

impl ParseVisualRegionsInput {
    pub fn validate(&self) -> Result<(), VisualContractValidationError> {
        if !nonempty(&self.capture_id) {
            return Err(VisualContractValidationError::EmptyCaptureId);
        }
        if self
            .options
            .min_confidence
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(VisualContractValidationError::InvalidMinimumConfidence);
        }
        if self.options.max_regions == Some(0) {
            return Err(VisualContractValidationError::InvalidMaximumRegions);
        }
        if self.options.kinds.as_ref().is_some_and(|kinds| {
            kinds.is_empty()
                || kinds.len() > 2
                || kinds.iter().collect::<BTreeSet<_>>().len() != kinds.len()
        }) {
            return Err(VisualContractValidationError::InvalidRegionKinds);
        }
        Ok(())
    }
}

impl ParseVisualRegionsOutput {
    pub fn validate(&self) -> Result<(), VisualContractValidationError> {
        if self.schema != VISUAL_REGIONS_SCHEMA {
            return Err(VisualContractValidationError::InvalidResultSchema);
        }
        if !nonempty(&self.capture.capture_id) {
            return Err(VisualContractValidationError::EmptyCaptureId);
        }
        match &self.capture.source {
            VisualCaptureSource::Window { pid, window_id } if *pid == 0 || *window_id == 0 => {
                return Err(VisualContractValidationError::InvalidCaptureSource);
            }
            VisualCaptureSource::PrimaryDesktop { display_id } if display_id != "primary" => {
                return Err(VisualContractValidationError::InvalidCaptureSource);
            }
            _ => {}
        }
        self.capture.screenshot.validate()?;
        self.capture.action_coordinate_space.validate()?;
        if self
            .capture
            .captured_at
            .as_ref()
            .is_some_and(|value| !nonempty(value))
        {
            return Err(VisualContractValidationError::InvalidCaptureMetadata);
        }
        if !nonempty(&self.parser.extension_id)
            || !nonempty(&self.parser.extension_version)
            || !nonempty(&self.parser.model_id)
            || !nonempty(&self.parser.model_version)
            || self
                .parser
                .runtime
                .as_ref()
                .is_some_and(|value| !nonempty(value))
        {
            return Err(VisualContractValidationError::InvalidParserMetadata);
        }
        if self
            .request_id
            .as_ref()
            .is_some_and(|value| !nonempty(value))
        {
            return Err(VisualContractValidationError::InvalidRequestId);
        }
        if self.warnings.iter().any(|warning| {
            !nonempty(&warning.code)
                || !nonempty(&warning.message)
                || warning
                    .detail
                    .as_ref()
                    .is_some_and(|value| !nonempty(value))
        }) {
            return Err(VisualContractValidationError::InvalidWarning);
        }
        if self
            .timing
            .preprocess_ms
            .unwrap_or(0)
            .checked_add(self.timing.inference_ms.unwrap_or(0))
            .is_none_or(|value| value > self.timing.duration_ms)
        {
            return Err(VisualContractValidationError::InvalidTiming);
        }

        if self.regions.iter().any(|region| !nonempty(&region.id)) {
            return Err(VisualContractValidationError::EmptyRegionId);
        }
        let region_ids: BTreeSet<&str> = self
            .regions
            .iter()
            .map(|region| region.id.as_str())
            .collect();
        if region_ids.len() != self.regions.len() {
            return Err(VisualContractValidationError::DuplicateRegionId);
        }

        for region in &self.regions {
            if region.bounds.width == 0 || region.bounds.height == 0 {
                return Err(VisualContractValidationError::InvalidRegionSize);
            }
            let right = region
                .bounds
                .x
                .checked_add(region.bounds.width)
                .ok_or(VisualContractValidationError::RegionOutsideScreenshot)?;
            let bottom = region
                .bounds
                .y
                .checked_add(region.bounds.height)
                .ok_or(VisualContractValidationError::RegionOutsideScreenshot)?;
            if right > self.capture.screenshot.width || bottom > self.capture.screenshot.height {
                return Err(VisualContractValidationError::RegionOutsideScreenshot);
            }
            if !region.confidence.is_finite() || !(0.0..=1.0).contains(&region.confidence) {
                return Err(VisualContractValidationError::InvalidRegionConfidence);
            }
            if region.text.as_ref().is_some_and(|value| !nonempty(value))
                || region.label.as_ref().is_some_and(|value| !nonempty(value))
            {
                return Err(VisualContractValidationError::InvalidRegionContent);
            }
            let content_valid = match region.kind {
                VisualRegionKind::Text => region.text.as_deref().is_some_and(nonempty),
                VisualRegionKind::Icon => region.label.as_deref().is_some_and(nonempty),
            };
            if !content_valid {
                return Err(VisualContractValidationError::InvalidRegionContent);
            }
            for reference in [&region.parent_id, &region.group_id].into_iter().flatten() {
                if reference == &region.id {
                    return Err(VisualContractValidationError::SelfRegionReference);
                }
                if !region_ids.contains(reference.as_str()) {
                    return Err(VisualContractValidationError::MissingRegionReference);
                }
            }
        }

        for region in &self.regions {
            let mut visited = BTreeSet::from([region.id.as_str()]);
            let mut parent = region.parent_id.as_deref();
            while let Some(parent_id) = parent {
                if !visited.insert(parent_id) {
                    return Err(VisualContractValidationError::CyclicParentReference);
                }
                parent = self
                    .regions
                    .iter()
                    .find(|candidate| candidate.id == parent_id)
                    .and_then(|candidate| candidate.parent_id.as_deref());
            }
        }
        Ok(())
    }
}

impl VisualParseError {
    pub fn validate(&self) -> Result<(), VisualContractValidationError> {
        if !nonempty(&self.message) || self.detail.as_ref().is_some_and(|value| !nonempty(value)) {
            return Err(VisualContractValidationError::InvalidError);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn capture() -> VisualCaptureProvenance {
        VisualCaptureProvenance {
            capture_id: "capture-1".into(),
            source: VisualCaptureSource::Window {
                pid: 7,
                window_id: 9,
            },
            screenshot: VisualScreenshotReference {
                reference: "snapshot-1".into(),
                width: 1280,
                height: 720,
                mime_type: "image/png".into(),
                sha256: None,
            },
            action_coordinate_space: VisualActionCoordinateSpace::Affine {
                m11: 0.5,
                m12: 0.25,
                m21: -0.5,
                m22: 2.0,
                tx: 100.0,
                ty: 200.0,
            },
            captured_at: Some("2026-09-17T12:00:00Z".into()),
        }
    }

    fn parser() -> VisualParserMetadata {
        VisualParserMetadata {
            extension_id: "visual-parser".into(),
            extension_version: "1.0.0".into(),
            model_id: "default".into(),
            model_version: "1".into(),
            runtime: Some("cpu".into()),
        }
    }

    fn region(id: &str) -> VisualRegion {
        VisualRegion {
            id: id.into(),
            kind: VisualRegionKind::Text,
            bounds: VisualRegionBounds {
                x: 11,
                y: 20,
                width: 101,
                height: 21,
            },
            text: Some("Apply".into()),
            label: None,
            confidence: 0.95,
            interactive: true,
            parent_id: None,
            group_id: None,
            reading_order: Some(1),
        }
    }

    fn output() -> ParseVisualRegionsOutput {
        let output = ParseVisualRegionsOutput {
            schema: VISUAL_REGIONS_SCHEMA.into(),
            capture: capture(),
            parser: parser(),
            regions: vec![region("region-1")],
            warnings: vec![VisualParseWarning {
                code: "low_contrast".into(),
                message: "contrast was low".into(),
                detail: None,
            }],
            timing: VisualParseTiming {
                duration_ms: 12,
                preprocess_ms: Some(2),
                inference_ms: Some(8),
            },
            request_id: Some("parse-1".into()),
        };
        output.validate().unwrap();
        output
    }

    #[test]
    fn v1_refers_to_a_registry_capture_without_bytes_or_file_paths() {
        let input = ParseVisualRegionsInput {
            capture_id: "capture-1".into(),
            options: ParseVisualRegionsOptions::default(),
        };
        input.validate().unwrap();
        let value = serde_json::to_value(input).unwrap();
        assert_eq!(value["capture_id"], "capture-1");
        assert!(!value.to_string().contains("data_base64"));
        assert!(!value.to_string().contains("file_path"));
    }

    #[test]
    fn capture_sources_are_exact_window_or_primary_desktop() {
        assert_eq!(
            serde_json::to_value(VisualCaptureSource::Window {
                pid: 7,
                window_id: 9
            })
            .unwrap(),
            json!({"kind":"window","pid":7,"window_id":9})
        );
        assert_eq!(
            serde_json::to_value(VisualCaptureSource::PrimaryDesktop {
                display_id: "primary".into()
            })
            .unwrap(),
            json!({"kind":"primary_desktop","display_id":"primary"})
        );
        let schema = serde_json::to_string(&schemars::schema_for!(VisualCaptureSource)).unwrap();
        assert!(schema.contains("\"const\":\"primary\""));
    }

    #[test]
    fn region_center_is_geometric_then_mapped_by_the_normative_equation() {
        let region = region("region-1");
        assert_eq!(region.bounds.center(), (61.5, 30.5));
        assert_eq!(
            region.action_center(&capture().action_coordinate_space),
            (138.375, 230.25)
        );
        assert!(serde_json::to_value(region)
            .unwrap()
            .get("center")
            .is_none());
    }

    #[test]
    fn parse_options_schema_and_validation_require_unique_kinds() {
        let mut input = ParseVisualRegionsInput {
            capture_id: "capture-1".into(),
            options: ParseVisualRegionsOptions {
                kinds: Some(vec![VisualRegionKind::Text, VisualRegionKind::Icon]),
                min_confidence: Some(0.5),
                max_regions: Some(100),
            },
        };
        input.validate().unwrap();
        input.options.kinds = Some(vec![VisualRegionKind::Text, VisualRegionKind::Text]);
        assert_eq!(
            input.validate(),
            Err(VisualContractValidationError::InvalidRegionKinds)
        );

        let schema =
            serde_json::to_string(&schemars::schema_for!(ParseVisualRegionsInput)).unwrap();
        assert!(schema.contains("\"uniqueItems\":true"));
    }

    #[test]
    fn result_validates_metadata_content_and_region_references() {
        let mut result = output();
        result.regions.push(VisualRegion {
            id: "group-1".into(),
            kind: VisualRegionKind::Icon,
            bounds: VisualRegionBounds {
                x: 5,
                y: 5,
                width: 10,
                height: 10,
            },
            text: None,
            label: Some("toolbar".into()),
            confidence: 0.8,
            interactive: false,
            parent_id: None,
            group_id: None,
            reading_order: None,
        });
        result.regions[0].parent_id = Some("group-1".into());
        result.regions[0].group_id = Some("group-1".into());
        result.validate().unwrap();

        result.regions[0].parent_id = Some("missing".into());
        assert_eq!(
            result.validate(),
            Err(VisualContractValidationError::MissingRegionReference)
        );
        result.regions[0].parent_id = None;
        result.parser.model_id.clear();
        assert_eq!(
            result.validate(),
            Err(VisualContractValidationError::InvalidParserMetadata)
        );
    }

    #[test]
    fn output_schema_is_versioned_integer_pixel_data_without_dynamic_extensions() {
        let schema = serde_json::to_value(schemars::schema_for!(ParseVisualRegionsOutput)).unwrap();
        assert_eq!(
            schema["properties"]["schema"]["const"],
            VISUAL_REGIONS_SCHEMA
        );
        let schema_text = serde_json::to_string(&schema).unwrap();
        assert!(schema_text.contains("\"type\":\"integer\""));
        assert!(!schema_text.contains("data_base64"));
        assert!(!schema_text.contains("file_path"));
        assert!(!schema_text.contains("extensions"));
        output().validate().unwrap();
    }

    #[test]
    fn errors_cover_capture_worker_artifact_and_inference_failures() {
        let codes = [
            (VisualParseErrorCode::NotInstalled, "not_installed"),
            (VisualParseErrorCode::CaptureNotFound, "capture_not_found"),
            (VisualParseErrorCode::CaptureExpired, "capture_expired"),
            (VisualParseErrorCode::CaptureStale, "capture_stale"),
            (
                VisualParseErrorCode::CaptureGenerationMismatch,
                "capture_generation_mismatch",
            ),
            (
                VisualParseErrorCode::UnsupportedTarget,
                "unsupported_target",
            ),
            (
                VisualParseErrorCode::UnsupportedPlatform,
                "unsupported_platform",
            ),
            (
                VisualParseErrorCode::IncompatibleProtocol,
                "incompatible_protocol",
            ),
            (VisualParseErrorCode::InvalidFrame, "invalid_frame"),
            (
                VisualParseErrorCode::WorkerLaunchFailed,
                "worker_launch_failed",
            ),
            (VisualParseErrorCode::WorkerCrashed, "worker_crashed"),
            (VisualParseErrorCode::WorkerCancelled, "worker_cancelled"),
            (VisualParseErrorCode::Timeout, "timeout"),
            (
                VisualParseErrorCode::ResourceLimitExceeded,
                "resource_limit_exceeded",
            ),
            (VisualParseErrorCode::ArtifactInvalid, "artifact_invalid"),
            (VisualParseErrorCode::InferenceFailed, "inference_failed"),
        ];
        for (code, expected) in codes {
            assert_eq!(serde_json::to_value(code).unwrap(), expected);
        }
        VisualParseError {
            code: VisualParseErrorCode::CaptureNotFound,
            message: "capture is no longer registered".into(),
            retryable: true,
            detail: None,
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn production_type_names_remain_model_neutral() {
        let schemas = [
            serde_json::to_string(&schemars::schema_for!(ParseVisualRegionsInput)).unwrap(),
            serde_json::to_string(&schemars::schema_for!(ParseVisualRegionsOutput)).unwrap(),
            serde_json::to_string(&schemars::schema_for!(VisualParseError)).unwrap(),
        ]
        .join(" ")
        .to_lowercase();
        for forbidden in ["jev", "typesafe", "omniparser", "prompt", "provider"] {
            assert!(!schemas.contains(forbidden));
        }
    }
}
