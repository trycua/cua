//! Public runtime facade for capture publication and capture-bound actions.

pub use crate::capture_registry::{
    CaptureActionAdmission, CaptureActionError, CaptureActionRequest, CaptureBinding, CaptureId,
    CaptureIdParseError, CaptureLookupError, CapturePublication, CaptureService, CaptureStoreError,
    CaptureTarget, EncodedScreenshotDimensions, NativeActionDimensions, PruneOutcome,
    ScreenshotToActionTransform,
};
