//! Public runtime facade for capture publication and capture-bound actions.

pub use crate::capture_registry::{
    admission_error_code, CaptureActionAdmission, CaptureActionError, CaptureActionRequest,
    CaptureBinding, CaptureId, CaptureIdParseError, CaptureLookupError, CapturePublication,
    CaptureService, CaptureStoreError, CaptureTarget, EncodedScreenshotDimensions,
    NativeActionDimensions, PruneOutcome, ScreenshotToActionTransform, CAPTURE_ACTION_REFUSED_CODE,
    CAPTURE_ID_INVALID_CODE,
};
