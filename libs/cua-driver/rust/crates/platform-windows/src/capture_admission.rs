use std::sync::Arc;

use cua_driver_core::capture_runtime::{
    CaptureActionError, CaptureActionRequest, CaptureIdParseError, CaptureLookupError,
    CapturePublication, CaptureService, CaptureTarget, EncodedScreenshotDimensions,
    NativeActionDimensions, ScreenshotToActionTransform,
};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowsCaptureTarget {
    Window { pid: u32, window_id: u64 },
    PrimaryDesktop,
}

impl WindowsCaptureTarget {
    fn core(self) -> CaptureTarget {
        match self {
            Self::Window { pid, window_id } => CaptureTarget::Window { pid, window_id },
            Self::PrimaryDesktop => CaptureTarget::PrimaryDesktop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaptureGeometry {
    pub encoded_width: u32,
    pub encoded_height: u32,
    pub native_width: u32,
    pub native_height: u32,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl CaptureGeometry {
    pub(crate) fn new(
        encoded_width: u32,
        encoded_height: u32,
        native_width: u32,
        native_height: u32,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            encoded_width > 0 && encoded_height > 0 && native_width > 0 && native_height > 0,
            "capture dimensions must be positive"
        );
        Ok(Self {
            encoded_width,
            encoded_height,
            native_width,
            native_height,
            scale_x: f64::from(native_width) / f64::from(encoded_width),
            scale_y: f64::from(native_height) / f64::from(encoded_height),
        })
    }
}

pub(crate) fn round_action_point(x: f64, y: f64) -> anyhow::Result<(i32, i32)> {
    fn round(value: f64) -> anyhow::Result<i32> {
        let rounded = value.round();
        anyhow::ensure!(
            rounded.is_finite() && rounded >= f64::from(i32::MIN) && rounded <= f64::from(i32::MAX),
            "capture action coordinate is outside the Windows pixel range"
        );
        Ok(rounded as i32)
    }
    Ok((round(x)?, round(y)?))
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
        Some(CaptureActionError::InvalidScreenshotPoint) => "capture_point_invalid",
        Some(CaptureActionError::InvalidMappedPoint) => "capture_mapping_invalid",
        Some(CaptureActionError::NativeActionFrameMismatch) => "capture_frame_mismatch",
        None => "capture_action_refused",
    }
}

pub(crate) struct WindowsCaptureBridge {
    service: Arc<CaptureService>,
}

impl WindowsCaptureBridge {
    pub(crate) fn new(service: Arc<CaptureService>) -> Arc<Self> {
        Arc::new(Self { service })
    }

    pub(crate) fn publish(
        &self,
        args: &Value,
        png_bytes: Vec<u8>,
        target: WindowsCaptureTarget,
        geometry: CaptureGeometry,
    ) -> anyhow::Result<Option<String>> {
        let binding = match self.service.binding_from_args(args) {
            Ok(binding) => binding,
            Err(_) if args.get("_session_id").is_none() => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let encoded_dimensions =
            EncodedScreenshotDimensions::new(geometry.encoded_width, geometry.encoded_height)?;
        let native_action_dimensions =
            NativeActionDimensions::new(geometry.native_width, geometry.native_height)?;
        let screenshot_to_action = ScreenshotToActionTransform::new(
            geometry.scale_x,
            0.0,
            0.0,
            geometry.scale_y,
            0.0,
            0.0,
        )?;
        let capture_id = self.service.publish(CapturePublication {
            png_bytes,
            target: target.core(),
            encoded_dimensions,
            native_action_dimensions,
            screenshot_to_action,
            session_id: binding.session_id().into(),
            session_generation: binding.session_generation(),
        })?;
        Ok(Some(capture_id.to_string()))
    }

    pub(crate) fn admit_click(
        &self,
        args: &Value,
        target: WindowsCaptureTarget,
        screenshot_x: f64,
        screenshot_y: f64,
    ) -> anyhow::Result<Option<(f64, f64)>> {
        if args.get("capture_id").and_then(Value::as_str).is_none() {
            return Ok(None);
        }
        let current_geometry = live_geometry(target)?;
        self.admit_click_with_geometry(args, target, current_geometry, screenshot_x, screenshot_y)
    }

    fn admit_click_with_geometry(
        &self,
        args: &Value,
        target: WindowsCaptureTarget,
        current_geometry: CaptureGeometry,
        screenshot_x: f64,
        screenshot_y: f64,
    ) -> anyhow::Result<Option<(f64, f64)>> {
        let Some(capture_id) = args.get("capture_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        let binding = self.service.binding_from_args(args)?;
        let admission = self.service.admit_action(CaptureActionRequest {
            capture_id: capture_id.parse()?,
            binding,
            target: target.core(),
            current_native_action_dimensions: NativeActionDimensions::new(
                current_geometry.native_width,
                current_geometry.native_height,
            )?,
            screenshot_x,
            screenshot_y,
        })?;
        Ok(Some((admission.action_x, admission.action_y)))
    }
}

#[cfg(target_os = "windows")]
fn live_geometry(target: WindowsCaptureTarget) -> anyhow::Result<CaptureGeometry> {
    let png = match target {
        WindowsCaptureTarget::Window { pid, window_id } => {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};
            let hwnd = HWND(window_id as usize as *mut std::ffi::c_void);
            anyhow::ensure!(
                unsafe { IsWindow(hwnd).as_bool() },
                "native window was replaced"
            );
            let mut owner_pid = 0_u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner_pid)) };
            anyhow::ensure!(
                owner_pid == pid,
                "native window owner changed after capture"
            );
            crate::capture::screenshot_window_bytes(window_id)?
        }
        WindowsCaptureTarget::PrimaryDesktop => crate::capture::screenshot_display_bytes()?,
    };
    let (width, height) = crate::capture::png_dimensions_pub(&png)?;
    CaptureGeometry::new(width, height, width, height)
}

#[cfg(not(target_os = "windows"))]
fn live_geometry(_target: WindowsCaptureTarget) -> anyhow::Result<CaptureGeometry> {
    anyhow::bail!("live Windows capture validation is unavailable on this platform")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cua_driver_core::capture_runtime::CaptureService;
    use serde_json::json;

    use super::{
        admission_error_code, round_action_point, CaptureGeometry, WindowsCaptureBridge,
        WindowsCaptureTarget,
    };

    fn png() -> Vec<u8> {
        cua_driver_core::image_utils::encode_rgba_to_png(&[10, 20, 30, 255], 1, 1).unwrap()
    }

    fn args() -> serde_json::Value {
        json!({
            "capture_id": "replaced-after-publication",
            "_session_id": "capture-test",
        })
    }

    fn publish(bridge: &WindowsCaptureBridge, target: WindowsCaptureTarget) -> String {
        let mut publication_args = args();
        publication_args
            .as_object_mut()
            .unwrap()
            .remove("capture_id");
        bridge
            .publish(
                &publication_args,
                png(),
                target,
                CaptureGeometry::new(1, 1, 1, 1).unwrap(),
            )
            .unwrap()
            .unwrap()
    }

    #[test]
    fn resized_capture_maps_each_axis_into_native_window_pixels() {
        let geometry = CaptureGeometry::new(800, 450, 1600, 900).unwrap();
        assert_eq!(geometry.scale_x, 2.0);
        assert_eq!(geometry.scale_y, 2.0);
    }

    #[test]
    fn non_uniform_dimensions_preserve_full_affine_scale() {
        let geometry = CaptureGeometry::new(640, 360, 1280, 1080).unwrap();
        assert_eq!(geometry.scale_x, 2.0);
        assert_eq!(geometry.scale_y, 3.0);
    }

    #[test]
    fn admitted_centers_round_once_at_native_dispatch_boundary() {
        assert_eq!(round_action_point(10.49, -20.49).unwrap(), (10, -20));
        assert_eq!(round_action_point(10.5, -20.5).unwrap(), (11, -21));
        assert!(round_action_point(f64::NAN, 0.0).is_err());
        assert!(round_action_point(f64::from(i32::MAX) + 1.0, 0.0).is_err());
    }

    #[test]
    fn target_mismatch_refuses_before_dispatch() {
        let bridge = WindowsCaptureBridge::new(Arc::new(CaptureService::default()));
        let capture_id = publish(
            &bridge,
            WindowsCaptureTarget::Window {
                pid: 7,
                window_id: 70,
            },
        );
        let mut request = args();
        request["capture_id"] = json!(capture_id);
        let admission = bridge.admit_click_with_geometry(
            &request,
            WindowsCaptureTarget::Window {
                pid: 7,
                window_id: 71,
            },
            CaptureGeometry::new(1, 1, 1, 1).unwrap(),
            0.0,
            0.0,
        );
        let mut dispatches = 0;
        if admission.is_ok() {
            dispatches += 1;
        }
        assert_eq!(
            admission_error_code(&admission.unwrap_err()),
            "capture_target_mismatch"
        );
        assert_eq!(dispatches, 0);
    }

    #[test]
    fn retired_capture_refuses_before_dispatch() {
        let bridge = WindowsCaptureBridge::new(Arc::new(CaptureService::default()));
        let target = WindowsCaptureTarget::PrimaryDesktop;
        let capture_id = publish(&bridge, target);
        bridge.service.retire_session_id("capture-test");
        let mut request = args();
        request["capture_id"] = json!(capture_id);
        let admission = bridge.admit_click_with_geometry(
            &request,
            target,
            CaptureGeometry::new(1, 1, 1, 1).unwrap(),
            0.0,
            0.0,
        );
        let mut dispatches = 0;
        if admission.is_ok() {
            dispatches += 1;
        }
        assert_eq!(
            admission_error_code(&admission.unwrap_err()),
            "capture_not_found"
        );
        assert_eq!(dispatches, 0);
    }

    #[test]
    fn live_resize_refuses_before_dispatch_without_consuming_capture() {
        let bridge = WindowsCaptureBridge::new(Arc::new(CaptureService::default()));
        let target = WindowsCaptureTarget::Window {
            pid: 7,
            window_id: 70,
        };
        let capture_id = publish(&bridge, target);
        let mut request = args();
        request["capture_id"] = json!(capture_id);
        let mut dispatches = 0;
        let refusal = bridge.admit_click_with_geometry(
            &request,
            target,
            CaptureGeometry::new(1, 1, 2, 1).unwrap(),
            0.25,
            0.25,
        );
        if refusal.is_ok() {
            dispatches += 1;
        }
        assert_eq!(dispatches, 0);
        assert_eq!(
            admission_error_code(&refusal.unwrap_err()),
            "capture_frame_mismatch"
        );
        assert_eq!(
            bridge
                .admit_click_with_geometry(
                    &request,
                    target,
                    CaptureGeometry::new(1, 1, 1, 1).unwrap(),
                    0.25,
                    0.25,
                )
                .unwrap(),
            Some((0.25, 0.25))
        );
    }

    #[test]
    fn ordinary_click_without_capture_id_skips_live_capture_validation() {
        let bridge = WindowsCaptureBridge::new(Arc::new(CaptureService::default()));
        let request = json!({"_session_id": "capture-test"});
        assert_eq!(
            bridge
                .admit_click(&request, WindowsCaptureTarget::PrimaryDesktop, 0.25, 0.25,)
                .unwrap(),
            None
        );
    }
}
