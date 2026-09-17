use cua_driver_core::capture_runtime::{
    CaptureActionRequest, CapturePublication, CaptureService, CaptureTarget,
    EncodedScreenshotDimensions, NativeActionDimensions, ScreenshotToActionTransform,
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
    // Keep the established Linux screenshot coordinate contract: resize uses
    // a uniform long-edge scale and click conversion derives that scale from
    // width, then applies it to both axes before the existing integer cast.
    let scale = f64::from(native_action_dimensions.0) / f64::from(encoded_dimensions.0);
    let screenshot_to_action = ScreenshotToActionTransform::new(scale, 0.0, 0.0, scale, 0.0, 0.0)?;
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
    dimensions: (u32, u32),
) -> anyhow::Result<String> {
    publish(
        service,
        args,
        png_bytes,
        CaptureTarget::PrimaryDesktop,
        dimensions,
        dimensions,
        ScreenshotToActionTransform::identity(),
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
    let binding = service.binding_from_args(args)?;
    let capture_id = service.parse_capture_id(capture_id)?;
    let admission = service.admit_action(CaptureActionRequest {
        capture_id,
        binding,
        target,
        screenshot_x,
        screenshot_y,
    })?;
    Ok((admission.action_x, admission.action_y))
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

        assert!(admit_window_click(&service, &call_args, &id, 7, 12, 2.0, 3.0).is_err());
        assert_eq!(
            admit_window_click(&service, &call_args, &id, 7, 11, 2.0, 3.0).unwrap(),
            (4.0, 6.0)
        );
        assert!(admit_window_click(&service, &call_args, &id, 7, 11, 2.0, 3.0).is_err());
    }

    #[test]
    fn retired_session_capture_is_stale() {
        let service = CaptureService::default();
        let call_args = args("retired");
        let id = publish_desktop(&service, &call_args, &png(3, 2, 0x77), (3, 2)).unwrap();

        let binding = service.binding_from_args(&call_args).unwrap();
        service.retire_session(&binding);

        assert!(admit_desktop_click(&service, &call_args, &id, 1.0, 1.0).is_err());
    }

    #[test]
    fn window_transform_preserves_width_derived_uniform_scale() {
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

        let (x, y) = admit_window_click(&service, &call_args, &id, 8, 13, 1.0, 1.0).unwrap();
        assert_eq!(x, 5.0 / 3.0);
        assert_eq!(y, 5.0 / 3.0);
    }

    #[test]
    fn desktop_publication_uses_the_post_normalization_bytes() {
        let service = CaptureService::default();
        let normalized = png(4, 3, 0x18);
        let id = publish_desktop(&service, &args("desktop"), &normalized, (4, 3)).unwrap();
        let binding = service.binding_from_args(&args("desktop")).unwrap();
        let capture = service
            .read_for_perception(service.parse_capture_id(&id).unwrap(), &binding)
            .unwrap();

        assert_eq!(capture.png_bytes().as_ref(), normalized.as_slice());
        assert_eq!(
            admit_desktop_click(&service, &args("desktop"), &id, 2.0, 1.0).unwrap(),
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
}
