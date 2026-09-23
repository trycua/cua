use cua_driver_core::capture_runtime::{
    CaptureActionError, CaptureActionRequest, CaptureIdParseError, CaptureLookupError,
    CapturePublication, CaptureService, CaptureTarget, EncodedScreenshotDimensions,
    NativeActionDimensions, ScreenshotToActionTransform,
};
use serde_json::Value;

fn window_target(pid: u32, window_id: u64) -> CaptureTarget {
    CaptureTarget::Window { pid, window_id }
}

pub fn resolve_max_image_dimension(
    configured: u32,
    legacy_max_dimension: Option<u32>,
    max_image_dimension: Option<u32>,
) -> u32 {
    max_image_dimension.unwrap_or_else(|| match legacy_max_dimension {
        Some(value) if configured == 0 => value,
        Some(value) => configured.min(value),
        None => configured,
    })
}

fn publish(
    service: &CaptureService,
    args: &Value,
    png_bytes: &[u8],
    target: CaptureTarget,
    encoded_dimensions: (u32, u32),
    native_action_dimensions: (u32, u32),
    screenshot_to_action: ScreenshotToActionTransform,
) -> anyhow::Result<String> {
    let binding = service.binding_from_args(args)?;
    let capture_id = service.publish(CapturePublication {
        png_bytes: png_bytes.to_vec(),
        target,
        encoded_dimensions: EncodedScreenshotDimensions::new(
            encoded_dimensions.0,
            encoded_dimensions.1,
        )?,
        native_action_dimensions: NativeActionDimensions::new(
            native_action_dimensions.0,
            native_action_dimensions.1,
        )?,
        screenshot_to_action,
        session_id: binding.session_id().into(),
        session_generation: binding.session_generation(),
    })?;
    Ok(capture_id.to_string())
}

pub fn publish_window(
    service: &CaptureService,
    args: &Value,
    png_bytes: &[u8],
    pid: u32,
    window_id: u64,
    encoded_dimensions: (u32, u32),
    native_action_dimensions: (u32, u32),
) -> anyhow::Result<String> {
    // The resizer preserves aspect ratio before rounding each encoded axis.
    // Derive both ratios independently so a one-pixel rounded height does not
    // skew Y coordinates in the native window frame.
    let scale_x = f64::from(native_action_dimensions.0) / f64::from(encoded_dimensions.0);
    let scale_y = f64::from(native_action_dimensions.1) / f64::from(encoded_dimensions.1);
    let screenshot_to_action =
        ScreenshotToActionTransform::new(scale_x, 0.0, 0.0, scale_y, 0.0, 0.0)?;
    publish(
        service,
        args,
        png_bytes,
        window_target(pid, window_id),
        encoded_dimensions,
        native_action_dimensions,
        screenshot_to_action,
    )
}

pub fn publish_desktop(
    service: &CaptureService,
    args: &Value,
    png_bytes: &[u8],
    encoded_dimensions: (u32, u32),
    native_action_dimensions: (u32, u32),
) -> anyhow::Result<String> {
    // The desktop screenshot can be downsized below the action frame; map its
    // pixels back per axis, as for a window capture.
    let scale_x = f64::from(native_action_dimensions.0) / f64::from(encoded_dimensions.0);
    let scale_y = f64::from(native_action_dimensions.1) / f64::from(encoded_dimensions.1);
    let screenshot_to_action =
        ScreenshotToActionTransform::new(scale_x, 0.0, 0.0, scale_y, 0.0, 0.0)?;
    publish(
        service,
        args,
        png_bytes,
        CaptureTarget::PrimaryDesktop,
        encoded_dimensions,
        native_action_dimensions,
        screenshot_to_action,
    )
}

fn admit(
    service: &CaptureService,
    args: &Value,
    capture_id: &str,
    target: CaptureTarget,
    screenshot_x: f64,
    screenshot_y: f64,
) -> anyhow::Result<(f64, f64)> {
    let current_native_action_dimensions = live_action_dimensions(&target)?;
    admit_with_live_dimensions(
        service,
        args,
        capture_id,
        target,
        current_native_action_dimensions,
        screenshot_x,
        screenshot_y,
    )
}

fn admit_with_live_dimensions(
    service: &CaptureService,
    args: &Value,
    capture_id: &str,
    target: CaptureTarget,
    current_native_action_dimensions: NativeActionDimensions,
    screenshot_x: f64,
    screenshot_y: f64,
) -> anyhow::Result<(f64, f64)> {
    let binding = service.binding_from_args(args)?;
    let capture_id = service.parse_capture_id(capture_id)?;
    let admission = service.admit_action(CaptureActionRequest {
        capture_id,
        binding,
        target,
        current_native_action_dimensions,
        screenshot_x,
        screenshot_y,
    })?;
    Ok((admission.action_x, admission.action_y))
}

#[cfg(target_os = "linux")]
fn live_action_dimensions(target: &CaptureTarget) -> anyhow::Result<NativeActionDimensions> {
    let dimensions = match target {
        CaptureTarget::Window { pid, window_id } => {
            let identity_matches = if crate::wayland::is_wayland() {
                crate::wayland::window_was_listed_for_pid(*pid, *window_id)
            } else {
                crate::x11::window_belongs_to_pid(*window_id, *pid)
            };
            anyhow::ensure!(
                identity_matches,
                "native window identity changed after capture"
            );
            let png = crate::wayland::screenshot_dispatch_with_pid(*window_id, *pid)?;
            crate::capture::png_dimensions_pub(&png)?
        }
        CaptureTarget::PrimaryDesktop => {
            let png = crate::capture::screenshot_display_bytes()?;
            let native = crate::capture::png_dimensions_pub(&png)?;
            desktop_action_dimensions(native)?
        }
    };
    Ok(NativeActionDimensions::new(dimensions.0, dimensions.1)?)
}

#[cfg(not(target_os = "linux"))]
fn live_action_dimensions(_target: &CaptureTarget) -> anyhow::Result<NativeActionDimensions> {
    anyhow::bail!("live Linux capture validation is unavailable on this platform")
}

#[cfg(target_os = "linux")]
pub(crate) fn desktop_action_dimensions(native: (u32, u32)) -> anyhow::Result<(u32, u32)> {
    let logical = if crate::wayland::is_wayland() && crate::wayland::hyprland::is_session() {
        let (width, height, _) = crate::wayland::hyprland::screen_size()?;
        Some((width, height))
    } else {
        None
    };
    select_desktop_action_dimensions(native, logical)
}

fn select_desktop_action_dimensions(
    native: (u32, u32),
    compositor_logical: Option<(u32, u32)>,
) -> anyhow::Result<(u32, u32)> {
    let dimensions = compositor_logical.unwrap_or(native);
    anyhow::ensure!(
        dimensions.0 > 0 && dimensions.1 > 0,
        "desktop action frame is empty: {}x{}",
        dimensions.0,
        dimensions.1
    );
    Ok(dimensions)
}

pub(crate) fn admission_error_code(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<CaptureIdParseError>().is_some() {
        return "capture_id_invalid";
    }
    match error.downcast_ref::<CaptureActionError>() {
        Some(CaptureActionError::Lookup(CaptureLookupError::Unknown)) => "capture_not_found",
        Some(CaptureActionError::Lookup(CaptureLookupError::Expired)) => "capture_expired",
        Some(CaptureActionError::Lookup(CaptureLookupError::GenerationMismatch)) => {
            "capture_generation_mismatch"
        }
        Some(CaptureActionError::Lookup(CaptureLookupError::TargetMismatch)) => {
            "capture_target_mismatch"
        }
        Some(
            CaptureActionError::InvalidScreenshotPoint | CaptureActionError::InvalidMappedPoint,
        ) => "capture_coordinate_invalid",
        Some(CaptureActionError::NativeActionFrameMismatch) => "capture_frame_mismatch",
        None => "capture_action_refused",
    }
}

pub fn admit_window_click(
    service: &CaptureService,
    args: &Value,
    capture_id: &str,
    pid: u32,
    window_id: u64,
    screenshot_x: f64,
    screenshot_y: f64,
) -> anyhow::Result<(f64, f64)> {
    admit(
        service,
        args,
        capture_id,
        window_target(pid, window_id),
        screenshot_x,
        screenshot_y,
    )
}

pub fn admit_desktop_click(
    service: &CaptureService,
    args: &Value,
    capture_id: &str,
    screenshot_x: f64,
    screenshot_y: f64,
) -> anyhow::Result<(f64, f64)> {
    admit(
        service,
        args,
        capture_id,
        CaptureTarget::PrimaryDesktop,
        screenshot_x,
        screenshot_y,
    )
}

pub fn retire_runtime(service: &CaptureService) {
    service.retire_runtime();
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn png(width: u32, height: u32, value: u8) -> Vec<u8> {
        let rgba = vec![value; (width * height * 4) as usize];
        cua_driver_core::image_utils::encode_rgba_to_png(&rgba, width, height)
            .expect("encode fixture")
    }

    fn args(session: &str) -> Value {
        serde_json::json!({"_session_id": session})
    }

    fn admit_window(
        service: &CaptureService,
        args: &Value,
        capture_id: &str,
        pid: u32,
        window_id: u64,
        point: (f64, f64),
        native: (u32, u32),
    ) -> anyhow::Result<(f64, f64)> {
        admit_with_live_dimensions(
            service,
            args,
            capture_id,
            window_target(pid, window_id),
            NativeActionDimensions::new(native.0, native.1).unwrap(),
            point.0,
            point.1,
        )
    }

    fn admit_desktop(
        service: &CaptureService,
        args: &Value,
        capture_id: &str,
        point: (f64, f64),
        native: (u32, u32),
    ) -> anyhow::Result<(f64, f64)> {
        admit_with_live_dimensions(
            service,
            args,
            capture_id,
            CaptureTarget::PrimaryDesktop,
            NativeActionDimensions::new(native.0, native.1).unwrap(),
            point.0,
            point.1,
        )
    }

    #[test]
    fn window_publication_digests_the_exact_returned_png() {
        let service = CaptureService::default();
        let delivered = png(2, 2, 0x41);
        let id = publish_window(
            &service,
            &args("digest"),
            &delivered,
            41,
            99,
            (2, 2),
            (4, 4),
        )
        .expect("publish window capture");
        let binding = service.binding_from_args(&args("digest")).unwrap();
        let capture = service
            .read_for_perception(service.parse_capture_id(&id).unwrap(), &binding)
            .expect("read published capture");

        assert_eq!(capture.png_bytes().as_ref(), delivered.as_slice());
        let expected: [u8; 32] = Sha256::digest(&delivered).into();
        assert_eq!(capture.digest().as_bytes(), &expected);
    }

    #[test]
    fn target_mismatch_does_not_consume_but_successful_admission_does() {
        let service = CaptureService::default();
        let call_args = args("one-action");
        let id = publish_window(
            &service,
            &call_args,
            &png(10, 10, 0x22),
            7,
            11,
            (10, 10),
            (20, 20),
        )
        .unwrap();

        assert!(admit_window(&service, &call_args, &id, 7, 12, (2.0, 3.0), (20, 20)).is_err());
        assert_eq!(
            admit_window(&service, &call_args, &id, 7, 11, (2.0, 3.0), (20, 20)).unwrap(),
            (4.0, 6.0)
        );
        assert!(admit_window(&service, &call_args, &id, 7, 11, (2.0, 3.0), (20, 20)).is_err());
    }

    #[test]
    fn retired_session_capture_is_stale() {
        let service = CaptureService::default();
        let call_args = args("retired");
        let id = publish_desktop(&service, &call_args, &png(3, 2, 0x77), (3, 2), (3, 2)).unwrap();

        let binding = service.binding_from_args(&call_args).unwrap();
        service.retire_session(&binding);

        assert!(admit_desktop(&service, &call_args, &id, (1.0, 1.0), (3, 2)).is_err());
    }

    #[test]
    fn window_transform_preserves_each_rounded_axis() {
        let service = CaptureService::default();
        let call_args = args("rounding");
        let id = publish_window(
            &service,
            &call_args,
            &png(3, 2, 0x33),
            8,
            13,
            (3, 2),
            (5, 3),
        )
        .unwrap();

        let (x, y) = admit_window(&service, &call_args, &id, 8, 13, (1.0, 1.0), (5, 3)).unwrap();
        assert_eq!(x, 5.0 / 3.0);
        assert_eq!(y, 3.0 / 2.0);
    }

    #[test]
    fn another_session_cannot_admit_or_consume_a_capture() {
        let service = CaptureService::default();
        let owner = args("owner");
        let id = publish_desktop(&service, &owner, &png(4, 3, 0x21), (4, 3), (4, 3)).unwrap();

        let refusal = admit_desktop(&service, &args("other"), &id, (2.0, 1.0), (4, 3))
            .expect_err("cross-session admission must fail");
        assert_eq!(
            admission_error_code(&refusal),
            "capture_generation_mismatch"
        );
        assert_eq!(
            admit_desktop(&service, &owner, &id, (2.0, 1.0), (4, 3)).unwrap(),
            (2.0, 1.0)
        );
    }

    #[test]
    fn hidpi_desktop_uses_the_compositors_logical_action_frame() {
        let native = (3200, 2000);
        let logical = select_desktop_action_dimensions(native, Some((1600, 1000))).unwrap();
        assert_eq!(logical, (1600, 1000));

        let service = CaptureService::default();
        let call_args = args("hidpi");
        let id = publish_desktop(
            &service,
            &call_args,
            &png(logical.0, logical.1, 0x31),
            logical,
            logical,
        )
        .unwrap();
        assert_eq!(
            admit_desktop(&service, &call_args, &id, (800.0, 500.0), logical).unwrap(),
            (800.0, 500.0)
        );
    }

    #[test]
    fn downsized_desktop_capture_maps_back_to_the_action_frame() {
        let service = CaptureService::default();
        let call_args = args("downsized");
        let id = publish_desktop(&service, &call_args, &png(4, 3, 0x42), (4, 3), (8, 6)).unwrap();
        assert_eq!(
            admit_desktop(&service, &call_args, &id, (2.0, 1.5), (8, 6)).unwrap(),
            (4.0, 3.0)
        );
    }

    #[test]
    fn capture_action_refusals_have_stable_specific_codes() {
        for (error, code) in [
            (
                CaptureActionError::Lookup(CaptureLookupError::Unknown),
                "capture_not_found",
            ),
            (
                CaptureActionError::Lookup(CaptureLookupError::Expired),
                "capture_expired",
            ),
            (
                CaptureActionError::Lookup(CaptureLookupError::GenerationMismatch),
                "capture_generation_mismatch",
            ),
            (
                CaptureActionError::Lookup(CaptureLookupError::TargetMismatch),
                "capture_target_mismatch",
            ),
            (
                CaptureActionError::InvalidScreenshotPoint,
                "capture_coordinate_invalid",
            ),
            (
                CaptureActionError::InvalidMappedPoint,
                "capture_coordinate_invalid",
            ),
            (
                CaptureActionError::NativeActionFrameMismatch,
                "capture_frame_mismatch",
            ),
        ] {
            assert_eq!(admission_error_code(&anyhow::Error::new(error)), code);
        }
    }

    #[test]
    fn desktop_publication_uses_the_post_normalization_bytes() {
        let service = CaptureService::default();
        let normalized = png(4, 3, 0x18);
        let id = publish_desktop(&service, &args("desktop"), &normalized, (4, 3), (4, 3)).unwrap();
        let binding = service.binding_from_args(&args("desktop")).unwrap();
        let capture = service
            .read_for_perception(service.parse_capture_id(&id).unwrap(), &binding)
            .unwrap();

        assert_eq!(capture.png_bytes().as_ref(), normalized.as_slice());
        assert_eq!(
            admit_desktop(&service, &args("desktop"), &id, (2.0, 1.0), (4, 3)).unwrap(),
            (2.0, 1.0)
        );
    }

    #[test]
    fn explicit_image_dimension_override_wins_including_native_zero() {
        assert_eq!(
            resolve_max_image_dimension(1568, Some(800), Some(2048)),
            2048
        );
        assert_eq!(resolve_max_image_dimension(1568, Some(800), Some(0)), 0);
    }

    #[test]
    fn omitted_image_dimension_override_preserves_existing_behavior() {
        assert_eq!(resolve_max_image_dimension(1568, None, None), 1568);
        assert_eq!(resolve_max_image_dimension(0, None, None), 0);
        assert_eq!(resolve_max_image_dimension(1568, Some(800), None), 800);
    }

    #[test]
    fn live_resize_refuses_before_dispatch_without_consuming_capture() {
        let service = CaptureService::default();
        let call_args = args("resize");
        let id = publish_window(
            &service,
            &call_args,
            &png(4, 3, 0x44),
            8,
            13,
            (4, 3),
            (8, 6),
        )
        .unwrap();
        let mut dispatches = 0;
        let refusal = admit_window(&service, &call_args, &id, 8, 13, (1.25, 1.5), (9, 6));
        if refusal.is_ok() {
            dispatches += 1;
        }
        assert_eq!(dispatches, 0);
        assert!(refusal
            .unwrap_err()
            .downcast_ref::<cua_driver_core::capture_runtime::CaptureActionError>()
            .is_some_and(|error| {
                *error
                == cua_driver_core::capture_runtime::CaptureActionError::NativeActionFrameMismatch
            }));
        assert_eq!(
            admit_window(&service, &call_args, &id, 8, 13, (1.25, 1.5), (8, 6),).unwrap(),
            (2.5, 3.0)
        );
    }
}
