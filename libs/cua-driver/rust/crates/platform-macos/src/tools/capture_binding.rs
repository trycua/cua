//! Thin macOS adapter over the runtime-owned immutable capture service.
//!
//! The common runtime owns registry and session generations plus atomic action
//! admission. Keep its facade calls here so signature changes stay isolated.

use cua_driver_core::{
    capture_runtime::{
        CaptureActionError, CaptureActionRequest, CaptureLookupError, CapturePublication,
        CaptureService, CaptureTarget, EncodedScreenshotDimensions, NativeActionDimensions,
        ScreenshotToActionTransform,
    },
    protocol::ToolResult,
};
use std::sync::Arc;

pub(crate) struct MacCaptureBindings {
    service: Arc<CaptureService>,
}

impl MacCaptureBindings {
    pub(super) fn new(service: Arc<CaptureService>) -> Self {
        Self { service }
    }

    pub(super) fn publish_window(
        &self,
        args: &serde_json::Value,
        pid: i32,
        window_id: u32,
        png_bytes: Vec<u8>,
        encoded_dimensions: (u32, u32),
        native_dimensions: (u32, u32),
    ) -> Result<String, ToolResult> {
        let target = CaptureTarget::Window {
            pid: u32::try_from(pid)
                .map_err(|_| capture_error("invalid_target", "pid is negative"))?,
            window_id: u64::from(window_id),
        };
        self.publish(
            args,
            target,
            png_bytes,
            encoded_dimensions,
            native_dimensions,
        )
    }

    pub(super) fn publish_desktop(
        &self,
        args: &serde_json::Value,
        png_bytes: Vec<u8>,
        encoded_dimensions: (u32, u32),
        native_dimensions: (u32, u32),
    ) -> Result<String, ToolResult> {
        self.publish(
            args,
            CaptureTarget::PrimaryDesktop,
            png_bytes,
            encoded_dimensions,
            native_dimensions,
        )
    }

    fn publish(
        &self,
        args: &serde_json::Value,
        target: CaptureTarget,
        png_bytes: Vec<u8>,
        encoded_dimensions: (u32, u32),
        native_dimensions: (u32, u32),
    ) -> Result<String, ToolResult> {
        let binding = self
            .service
            .binding_from_args(args)
            .map_err(|error| capture_error("capture_binding_failed", error))?;
        let encoded = EncodedScreenshotDimensions::new(encoded_dimensions.0, encoded_dimensions.1)
            .map_err(|error| capture_error("capture_publication_failed", error))?;
        let native = NativeActionDimensions::new(native_dimensions.0, native_dimensions.1)
            .map_err(|error| capture_error("capture_publication_failed", error))?;
        let transform = scale_transform(encoded_dimensions, native_dimensions)?;
        self.service
            .publish(CapturePublication {
                png_bytes,
                target,
                encoded_dimensions: encoded,
                native_action_dimensions: native,
                screenshot_to_action: transform,
                session_id: Arc::<str>::from(binding.session_id()),
                session_generation: binding.session_generation(),
            })
            .map(|capture_id| capture_id.to_string())
            .map_err(|error| capture_error("capture_publication_failed", error))
    }

    pub(super) fn admit_window_click(
        &self,
        capture_id: &str,
        args: &serde_json::Value,
        pid: i32,
        window_id: u32,
        x: f64,
        y: f64,
    ) -> Result<(f64, f64), ToolResult> {
        let target = CaptureTarget::Window {
            pid: u32::try_from(pid)
                .map_err(|_| capture_error("invalid_target", "pid is negative"))?,
            window_id: u64::from(window_id),
        };
        self.admit(capture_id, args, target, x, y)
    }

    pub(super) fn admit_desktop_click(
        &self,
        capture_id: &str,
        args: &serde_json::Value,
        x: f64,
        y: f64,
    ) -> Result<(f64, f64), ToolResult> {
        self.admit(capture_id, args, CaptureTarget::PrimaryDesktop, x, y)
    }

    fn admit(
        &self,
        capture_id: &str,
        args: &serde_json::Value,
        target: CaptureTarget,
        x: f64,
        y: f64,
    ) -> Result<(f64, f64), ToolResult> {
        let current_native_action_dimensions = live_action_dimensions(&target)?;
        self.admit_with_live_dimensions(
            capture_id,
            args,
            target,
            current_native_action_dimensions,
            x,
            y,
        )
    }

    fn admit_with_live_dimensions(
        &self,
        capture_id: &str,
        args: &serde_json::Value,
        target: CaptureTarget,
        current_native_action_dimensions: NativeActionDimensions,
        x: f64,
        y: f64,
    ) -> Result<(f64, f64), ToolResult> {
        let binding = self
            .service
            .binding_from_args(args)
            .map_err(|error| capture_error("capture_binding_failed", error))?;
        let parsed = capture_id
            .parse()
            .map_err(|error| capture_error("capture_id_invalid", error))?;
        self.service
            .admit_action(CaptureActionRequest {
                capture_id: parsed,
                binding,
                target,
                current_native_action_dimensions,
                screenshot_x: x,
                screenshot_y: y,
            })
            .map(|admission| (admission.action_x, admission.action_y))
            .map_err(|error| capture_error(action_error_code(&error), error))
    }

    pub(super) fn retire_session(&self, session_id: &str) {
        self.service.retire_session_id(session_id);
    }

    pub(super) fn retire_runtime(&self) {
        self.service.retire_runtime();
    }
}

fn live_action_dimensions(target: &CaptureTarget) -> Result<NativeActionDimensions, ToolResult> {
    let (width, height) = match target {
        CaptureTarget::Window { pid, window_id } => {
            let pid = i32::try_from(*pid)
                .map_err(|_| capture_error("capture_target_mismatch", "pid is out of range"))?;
            let window_id = u32::try_from(*window_id).map_err(|_| {
                capture_error("capture_target_mismatch", "window id is out of range")
            })?;
            if crate::windows::resolve_window_owner(pid, window_id)
                != crate::windows::WindowOwner::SamePid
            {
                return Err(capture_error(
                    "capture_target_mismatch",
                    "native window identity changed after capture",
                ));
            }
            let png = crate::capture::screenshot_window_bytes(window_id).map_err(|error| {
                capture_error(
                    "capture_target_mismatch",
                    format!("live window capture failed: {error}"),
                )
            })?;
            crate::capture::png_dimensions(&png).map_err(|error| {
                capture_error(
                    "capture_target_mismatch",
                    format!("live window dimensions failed: {error}"),
                )
            })?
        }
        CaptureTarget::PrimaryDesktop => {
            let (width, height, _) =
                super::get_screen_size::main_screen_size().ok_or_else(|| {
                    capture_error("capture_target_mismatch", "primary display is unavailable")
                })?;
            (
                u32::try_from(width).map_err(|_| {
                    capture_error("capture_target_mismatch", "display width is out of range")
                })?,
                u32::try_from(height).map_err(|_| {
                    capture_error("capture_target_mismatch", "display height is out of range")
                })?,
            )
        }
    };
    NativeActionDimensions::new(width, height)
        .map_err(|error| capture_error("capture_target_mismatch", error))
}

fn scale_transform(
    encoded: (u32, u32),
    native: (u32, u32),
) -> Result<ScreenshotToActionTransform, ToolResult> {
    if encoded.0 == 0 || encoded.1 == 0 || native.0 == 0 || native.1 == 0 {
        return Err(capture_error(
            "capture_publication_failed",
            "capture dimensions must be non-zero",
        ));
    }
    ScreenshotToActionTransform::new(
        f64::from(native.0) / f64::from(encoded.0),
        0.0,
        0.0,
        f64::from(native.1) / f64::from(encoded.1),
        0.0,
        0.0,
    )
    .map_err(|error| capture_error("capture_publication_failed", error))
}

fn capture_error(code: &str, error: impl std::fmt::Display) -> ToolResult {
    ToolResult::error(format!(
        "capture binding failed: {error}. Not dispatching click."
    ))
    .with_structured(serde_json::json!({
        "code": code,
        "effect": "refused"
    }))
}

fn action_error_code(error: &CaptureActionError) -> &'static str {
    match error {
        CaptureActionError::Lookup(CaptureLookupError::Unknown | CaptureLookupError::Expired) => {
            "capture_stale"
        }
        CaptureActionError::Lookup(CaptureLookupError::GenerationMismatch) => {
            "capture_generation_mismatch"
        }
        CaptureActionError::Lookup(CaptureLookupError::TargetMismatch) => "capture_target_mismatch",
        CaptureActionError::InvalidScreenshotPoint | CaptureActionError::InvalidMappedPoint => {
            "capture_coordinate_invalid"
        }
        CaptureActionError::NativeActionFrameMismatch => "capture_frame_mismatch",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(session: &str) -> serde_json::Value {
        serde_json::json!({ "_session_id": session })
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn admit_desktop(
        bindings: &MacCaptureBindings,
        capture_id: &str,
        args: &serde_json::Value,
        x: f64,
        y: f64,
        native: (u32, u32),
    ) -> Result<(f64, f64), ToolResult> {
        bindings.admit_with_live_dimensions(
            capture_id,
            args,
            CaptureTarget::PrimaryDesktop,
            NativeActionDimensions::new(native.0, native.1).unwrap(),
            x,
            y,
        )
    }

    fn admit_window(
        bindings: &MacCaptureBindings,
        capture_id: &str,
        args: &serde_json::Value,
        pid: u32,
        window_id: u64,
        x: f64,
        y: f64,
        native: (u32, u32),
    ) -> Result<(f64, f64), ToolResult> {
        bindings.admit_with_live_dimensions(
            capture_id,
            args,
            CaptureTarget::Window { pid, window_id },
            NativeActionDimensions::new(native.0, native.1).unwrap(),
            x,
            y,
        )
    }

    #[test]
    fn resize_transform_preserves_fractional_coordinate_mapping() {
        let transform = scale_transform((800, 451), (1600, 902)).unwrap();
        assert_eq!(transform.apply(123.25, 17.75), (246.5, 35.5));
    }

    #[test]
    fn independent_axes_preserve_rounded_png_dimensions() {
        let transform = scale_transform((801, 450), (1600, 900)).unwrap();
        let (x, y) = transform.apply(800.0, 449.0);
        assert!((x - 1598.0024968789014).abs() < 1e-9);
        assert_eq!(y, 898.0);
    }

    #[test]
    fn zero_sized_capture_is_refused() {
        let error = scale_transform((0, 100), (100, 100)).unwrap_err();
        assert_eq!(
            error.structured_content.unwrap()["code"],
            "capture_publication_failed"
        );
    }

    #[test]
    fn action_errors_keep_stale_mismatch_and_coordinate_failures_distinct() {
        assert_eq!(
            action_error_code(&CaptureActionError::Lookup(CaptureLookupError::Expired)),
            "capture_stale"
        );
        assert_eq!(
            action_error_code(&CaptureActionError::Lookup(
                CaptureLookupError::GenerationMismatch
            )),
            "capture_generation_mismatch"
        );
        assert_eq!(
            action_error_code(&CaptureActionError::Lookup(
                CaptureLookupError::TargetMismatch
            )),
            "capture_target_mismatch"
        );
        assert_eq!(
            action_error_code(&CaptureActionError::InvalidScreenshotPoint),
            "capture_coordinate_invalid"
        );
    }

    #[test]
    fn publication_retains_exact_png_and_content_digest() {
        let service = Arc::new(CaptureService::default());
        let bindings = MacCaptureBindings::new(service.clone());
        let args = args("digest-session");
        let png = png(2, 2);
        let capture_id = bindings
            .publish_desktop(&args, png.clone(), (2, 2), (2, 2))
            .unwrap();
        let binding = service.binding_from_args(&args).unwrap();
        let capture = service
            .read_for_perception(capture_id.parse().unwrap(), &binding)
            .unwrap();
        assert_eq!(&*capture.png_bytes(), png.as_slice());
        assert_eq!(capture.digest().hex().len(), 64);
    }

    #[test]
    fn mismatch_and_invalid_coordinate_refuse_without_consuming_or_dispatching() {
        let service = Arc::new(CaptureService::default());
        let bindings = MacCaptureBindings::new(service);
        let args = args("click-session");
        let capture_id = bindings
            .publish_desktop(&args, png(4, 4), (4, 4), (2, 2))
            .unwrap();
        let mut dispatched = false;
        let mismatch = admit_window(&bindings, &capture_id, &args, 1, 1, 1.0, 1.0, (2, 2));
        if mismatch.is_ok() {
            dispatched = true;
        }
        assert!(!dispatched);
        assert_eq!(
            mismatch.unwrap_err().structured_content.unwrap()["code"],
            "capture_target_mismatch"
        );

        let invalid = admit_desktop(&bindings, &capture_id, &args, 5.0, 1.0, (2, 2));
        assert_eq!(
            invalid.unwrap_err().structured_content.unwrap()["code"],
            "capture_coordinate_invalid"
        );

        assert_eq!(
            admit_desktop(&bindings, &capture_id, &args, 2.0, 2.0, (2, 2)).unwrap(),
            (1.0, 1.0)
        );
        assert_eq!(
            admit_desktop(&bindings, &capture_id, &args, 2.0, 2.0, (2, 2))
                .unwrap_err()
                .structured_content
                .unwrap()["code"],
            "capture_stale"
        );
    }

    #[test]
    fn another_session_generation_cannot_admit_the_capture() {
        let service = Arc::new(CaptureService::default());
        let bindings = MacCaptureBindings::new(service);
        let owner = args("owner-session");
        let capture_id = bindings
            .publish_desktop(&owner, png(2, 2), (2, 2), (2, 2))
            .unwrap();
        let error = admit_desktop(
            &bindings,
            &capture_id,
            &args("other-session"),
            1.0,
            1.0,
            (2, 2),
        )
        .unwrap_err();
        assert_eq!(
            error.structured_content.unwrap()["code"],
            "capture_generation_mismatch"
        );
    }

    #[test]
    fn session_retirement_makes_published_capture_stale() {
        let service = Arc::new(CaptureService::default());
        let bindings = MacCaptureBindings::new(service);
        let args = args("retired-session");
        let capture_id = bindings
            .publish_desktop(&args, png(2, 2), (2, 2), (2, 2))
            .unwrap();
        bindings.retire_session("retired-session");
        let error = admit_desktop(&bindings, &capture_id, &args, 1.0, 1.0, (2, 2)).unwrap_err();
        assert_eq!(error.structured_content.unwrap()["code"], "capture_stale");
    }

    #[test]
    fn live_resize_refuses_before_dispatch_and_preserves_capture() {
        let service = Arc::new(CaptureService::default());
        let bindings = MacCaptureBindings::new(service.clone());
        let args = args("resize-session");
        let capture_id = bindings
            .publish_window(&args, 8, 13, png(4, 3), (4, 3), (8, 6))
            .unwrap();
        let mut dispatches = 0;
        let refusal = admit_window(&bindings, &capture_id, &args, 8, 13, 1.25, 1.5, (9, 6));
        if refusal.is_ok() {
            dispatches += 1;
        }
        assert_eq!(dispatches, 0);
        assert_eq!(
            refusal.unwrap_err().structured_content.unwrap()["code"],
            "capture_frame_mismatch"
        );
        assert_eq!(
            admit_window(&bindings, &capture_id, &args, 8, 13, 1.25, 1.5, (8, 6)).unwrap(),
            (2.5, 3.0)
        );
        assert!(service
            .read_for_perception(
                service.parse_capture_id(&capture_id).unwrap(),
                &service.binding_from_args(&args).unwrap()
            )
            .is_err());
    }
}
