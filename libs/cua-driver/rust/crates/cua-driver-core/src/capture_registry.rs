//! Bounded, immutable screenshot storage for later coordinate-bound actions.
//!
//! This registry deliberately does not store accessibility elements or mirror
//! the element cache. A later integration with the snapshot-owned transforms
//! from #3630 should keep the snapshot as the authority for element tokens and
//! screenshot freshness. At capture publication, compose the snapshot's
//! delivered-image-to-native transform with any native-to-action transform and
//! store that single screenshot-to-action transform here. Retire the capture
//! when the owning snapshot, session, or runtime generation is retired.
//!
//! The registry grants no screenshot or action permission. Callers must pass
//! already-authorized PNG bytes and must independently authorize every action.
//! In particular, this foundation is not wired into platform adapters, action
//! dispatch, session hooks, or permission checks. That P1 integration remains
//! required before a capture ID can authorize or drive any production action.
//! Target-specific bounds and coordinate semantics must be revalidated there.
//!
//! TTL cleanup is bounded and lazy: [`CaptureRegistry::prune_expired`] scans at
//! most `max_captures` entries and drops expired PNG references. Registry size
//! metrics call it automatically. An eventual runtime owner must call it from
//! its maintenance/metrics path if no store or lookup traffic is occurring.

use image::{ImageDecoder, Limits};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::io::Cursor;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct CaptureId {
    namespace: [u8; 16],
    sequence: u64,
}

impl fmt::Debug for CaptureId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CaptureId(")?;
        fmt::Display::fmt(self, formatter)?;
        formatter.write_str(")")
    }
}

impl fmt::Display for CaptureId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("capture_")?;
        for byte in self.namespace {
            write!(formatter, "{byte:02x}")?;
        }
        write!(formatter, "_{:016x}", self.sequence)
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CaptureIdParseError {
    #[error("capture id has an invalid format")]
    InvalidFormat,
}

impl FromStr for CaptureId {
    type Err = CaptureIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value
            .strip_prefix("capture_")
            .ok_or(CaptureIdParseError::InvalidFormat)?;
        let (namespace, sequence) = value
            .split_once('_')
            .ok_or(CaptureIdParseError::InvalidFormat)?;
        if namespace.len() != 32 || sequence.len() != 16 {
            return Err(CaptureIdParseError::InvalidFormat);
        }
        let mut namespace_bytes = [0_u8; 16];
        for (index, byte) in namespace_bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&namespace[index * 2..index * 2 + 2], 16)
                .map_err(|_| CaptureIdParseError::InvalidFormat)?;
        }
        let sequence =
            u64::from_str_radix(sequence, 16).map_err(|_| CaptureIdParseError::InvalidFormat)?;
        Ok(Self {
            namespace: namespace_bytes,
            sequence,
        })
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CaptureTarget {
    Window { pid: u32, window_id: u64 },
    PrimaryDesktop,
}

impl CaptureTarget {
    fn validate(&self) -> Result<(), CaptureStoreError> {
        match self {
            Self::Window { pid, window_id } if *pid == 0 || *window_id == 0 => {
                Err(CaptureStoreError::InvalidTarget)
            }
            Self::Window { .. } | Self::PrimaryDesktop => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedScreenshotDimensions {
    width: u32,
    height: u32,
}

impl EncodedScreenshotDimensions {
    pub fn new(width: u32, height: u32) -> Result<Self, CaptureStoreError> {
        if width == 0 || height == 0 {
            return Err(CaptureStoreError::InvalidEncodedDimensions);
        }
        Ok(Self { width, height })
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }
}

/// Dimensions of the full-resolution native frame used by action coordinates.
///
/// These may differ from [`EncodedScreenshotDimensions`] when a screenshot is
/// downscaled. They are metadata for later integration; this inert registry
/// does not claim that the action frame matches the target's current bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeActionDimensions {
    width: u32,
    height: u32,
}

impl NativeActionDimensions {
    pub fn new(width: u32, height: u32) -> Result<Self, CaptureStoreError> {
        if width == 0 || height == 0 {
            return Err(CaptureStoreError::InvalidNativeActionDimensions);
        }
        Ok(Self { width, height })
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }
}

/// Continuous affine mapping from screenshot pixels to action coordinates.
///
/// For a screenshot point `(x, y)`, the action point is:
/// `((m11*x) + (m12*y) + tx, (m21*x) + (m22*y) + ty)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenshotToActionTransform {
    m11: f64,
    m12: f64,
    m21: f64,
    m22: f64,
    tx: f64,
    ty: f64,
}

impl ScreenshotToActionTransform {
    pub fn new(
        m11: f64,
        m12: f64,
        m21: f64,
        m22: f64,
        tx: f64,
        ty: f64,
    ) -> Result<Self, CaptureStoreError> {
        let values = [m11, m12, m21, m22, tx, ty];
        let determinant = m11.mul_add(m22, -(m12 * m21));
        if values.iter().any(|value| !value.is_finite())
            || !determinant.is_finite()
            || determinant.abs() <= f64::EPSILON
        {
            return Err(CaptureStoreError::InvalidTransform);
        }
        Ok(Self {
            m11,
            m12,
            m21,
            m22,
            tx,
            ty,
        })
    }

    pub fn identity() -> Self {
        Self {
            m11: 1.0,
            m12: 0.0,
            m21: 0.0,
            m22: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    pub fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.m11.mul_add(x, self.m12.mul_add(y, self.tx)),
            self.m21.mul_add(x, self.m22.mul_add(y, self.ty)),
        )
    }

    pub fn coefficients(self) -> [f64; 6] {
        [self.m11, self.m12, self.m21, self.m22, self.tx, self.ty]
    }

    fn validate_extent(
        self,
        dimensions: EncodedScreenshotDimensions,
    ) -> Result<(), CaptureStoreError> {
        let max_x = f64::from(dimensions.width.saturating_sub(1));
        let max_y = f64::from(dimensions.height.saturating_sub(1));
        for (x, y) in [(0.0, 0.0), (max_x, 0.0), (0.0, max_y), (max_x, max_y)] {
            let (action_x, action_y) = self.apply(x, y);
            if !action_x.is_finite() || !action_y.is_finite() {
                return Err(CaptureStoreError::InvalidTransformExtent);
            }
        }
        Ok(())
    }

    /// Compose two mappings, applying `self` first and `next` second.
    pub fn then(self, next: Self) -> Result<Self, CaptureStoreError> {
        Self::new(
            next.m11.mul_add(self.m11, next.m12 * self.m21),
            next.m11.mul_add(self.m12, next.m12 * self.m22),
            next.m21.mul_add(self.m11, next.m22 * self.m21),
            next.m21.mul_add(self.m12, next.m22 * self.m22),
            next.m11
                .mul_add(self.tx, next.m12.mul_add(self.ty, next.tx)),
            next.m21
                .mul_add(self.tx, next.m22.mul_add(self.ty, next.ty)),
        )
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct CaptureBinding {
    runtime_generation: u64,
    session_id: Arc<str>,
    session_generation: u64,
}

impl fmt::Debug for CaptureBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptureBinding")
            .field("runtime_generation", &self.runtime_generation)
            .field("session_id", &"[redacted]")
            .field("session_generation", &self.session_generation)
            .finish()
    }
}

impl CaptureBinding {
    pub fn new(
        runtime_generation: u64,
        session_id: impl Into<Arc<str>>,
        session_generation: u64,
    ) -> Result<Self, CaptureStoreError> {
        let session_id = session_id.into();
        if runtime_generation == 0 || session_generation == 0 || session_id.trim().is_empty() {
            return Err(CaptureStoreError::InvalidBinding);
        }
        Ok(Self {
            runtime_generation,
            session_id,
            session_generation,
        })
    }

    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }
}

pub trait MonotonicClock: Send + Sync + 'static {
    fn now(&self) -> Duration;
}

#[derive(Debug)]
pub struct SystemMonotonicClock {
    origin: Instant,
}

impl SystemMonotonicClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureRegistryConfig {
    pub max_captures: usize,
    pub max_total_bytes: usize,
    pub max_encoded_width: u32,
    pub max_encoded_height: u32,
    pub max_decoded_bytes: u64,
    pub ttl: Duration,
}

impl CaptureRegistryConfig {
    fn validate(self) -> Result<Self, CaptureStoreError> {
        if self.max_captures == 0
            || self.max_total_bytes == 0
            || self.max_encoded_width == 0
            || self.max_encoded_height == 0
            || self.max_decoded_bytes == 0
            || self.ttl.is_zero()
        {
            return Err(CaptureStoreError::InvalidConfig);
        }
        Ok(self)
    }
}

impl Default for CaptureRegistryConfig {
    fn default() -> Self {
        Self {
            max_captures: 32,
            max_total_bytes: 64 * 1024 * 1024,
            max_encoded_width: 16_384,
            max_encoded_height: 16_384,
            max_decoded_bytes: 512 * 1024 * 1024,
            ttl: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum CaptureStoreError {
    #[error("capture registry limits and ttl must be non-zero")]
    InvalidConfig,
    #[error("capture binding is invalid")]
    InvalidBinding,
    #[error("capture target is invalid")]
    InvalidTarget,
    #[error("encoded screenshot dimensions are invalid or do not match the PNG")]
    InvalidEncodedDimensions,
    #[error("native action dimensions are invalid")]
    InvalidNativeActionDimensions,
    #[error("screenshot-to-action transform is invalid")]
    InvalidTransform,
    #[error("screenshot-to-action transform is not finite across the encoded image extent")]
    InvalidTransformExtent,
    #[error("capture bytes are not a valid PNG")]
    InvalidPng,
    #[error("PNG dimensions or decoded allocation exceed registry limits")]
    PngDecodeLimitExceeded,
    #[error("capture exceeds the registry byte limit")]
    CaptureTooLarge,
    #[error("capture id space is exhausted")]
    IdExhausted,
    #[error("capture expiry cannot be represented")]
    ExpiryOverflow,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CaptureLookupError {
    #[error("capture id is unknown")]
    Unknown,
    #[error("capture has expired")]
    Expired,
    #[error("capture belongs to another runtime or session generation")]
    GenerationMismatch,
    #[error("capture belongs to another target")]
    TargetMismatch,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PruneOutcome {
    pub captures_removed: usize,
    pub encoded_bytes_released: usize,
}

pub struct CaptureRegistration {
    pub png_bytes: Vec<u8>,
    pub target: CaptureTarget,
    pub encoded_dimensions: EncodedScreenshotDimensions,
    pub native_action_dimensions: NativeActionDimensions,
    pub screenshot_to_action: ScreenshotToActionTransform,
    pub binding: CaptureBinding,
}

pub struct CapturePublication {
    pub png_bytes: Vec<u8>,
    pub target: CaptureTarget,
    pub encoded_dimensions: EncodedScreenshotDimensions,
    pub native_action_dimensions: NativeActionDimensions,
    pub screenshot_to_action: ScreenshotToActionTransform,
    pub session_id: Arc<str>,
    pub session_generation: u64,
}

pub struct CaptureActionRequest {
    pub capture_id: CaptureId,
    pub binding: CaptureBinding,
    /// Target identity observed immediately before native dispatch.
    pub target: CaptureTarget,
    /// Native action frame observed immediately before native dispatch.
    pub current_native_action_dimensions: NativeActionDimensions,
    pub screenshot_x: f64,
    pub screenshot_y: f64,
}

#[derive(Debug)]
pub struct CaptureActionAdmission {
    pub capture_id: CaptureId,
    pub target: CaptureTarget,
    pub action_x: f64,
    pub action_y: f64,
    pub native_action_dimensions: NativeActionDimensions,
    pub digest: ContentDigest,
}

#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum CaptureActionError {
    #[error(transparent)]
    Lookup(#[from] CaptureLookupError),
    #[error("screenshot point must be finite and within the encoded capture")]
    InvalidScreenshotPoint,
    #[error("mapped action point is not finite")]
    InvalidMappedPoint,
    #[error("capture native action frame no longer matches the live target")]
    NativeActionFrameMismatch,
}

#[derive(Clone)]
struct StoredCapture {
    id: CaptureId,
    png_bytes: Arc<[u8]>,
    digest: ContentDigest,
    target: CaptureTarget,
    encoded_dimensions: EncodedScreenshotDimensions,
    native_action_dimensions: NativeActionDimensions,
    screenshot_to_action: ScreenshotToActionTransform,
    binding: CaptureBinding,
    created_at: Duration,
    expires_at: Duration,
}

impl fmt::Debug for StoredCapture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredCapture")
            .field("id", &self.id)
            .field("byte_len", &self.png_bytes.len())
            .field("digest", &self.digest)
            .field("target", &self.target)
            .field("encoded_dimensions", &self.encoded_dimensions)
            .field("native_action_dimensions", &self.native_action_dimensions)
            .field("screenshot_to_action", &self.screenshot_to_action)
            .field("binding", &self.binding)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct PerceptionCapture(StoredCapture);

impl PerceptionCapture {
    pub fn id(&self) -> CaptureId {
        self.0.id
    }

    pub fn png_bytes(&self) -> Arc<[u8]> {
        self.0.png_bytes.clone()
    }

    pub fn digest(&self) -> ContentDigest {
        self.0.digest
    }

    pub fn target(&self) -> &CaptureTarget {
        &self.0.target
    }

    pub fn encoded_dimensions(&self) -> EncodedScreenshotDimensions {
        self.0.encoded_dimensions
    }

    pub(crate) fn screenshot_to_action(&self) -> ScreenshotToActionTransform {
        self.0.screenshot_to_action
    }

    pub fn created_at(&self) -> Duration {
        self.0.created_at
    }

    pub fn expires_at(&self) -> Duration {
        self.0.expires_at
    }
}

/// A capture atomically removed from the registry for one action attempt.
///
/// The action transform and native action dimensions are exposed only through
/// this consumed form, preventing action integrations from accidentally using
/// a reusable perception read. Failed actions must acquire a fresh capture.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ActionCapture(StoredCapture);

#[allow(dead_code)]
impl ActionCapture {
    pub fn id(&self) -> CaptureId {
        self.0.id
    }

    pub fn png_bytes(&self) -> Arc<[u8]> {
        self.0.png_bytes.clone()
    }

    pub fn digest(&self) -> ContentDigest {
        self.0.digest
    }

    pub fn target(&self) -> &CaptureTarget {
        &self.0.target
    }

    pub fn encoded_dimensions(&self) -> EncodedScreenshotDimensions {
        self.0.encoded_dimensions
    }

    pub fn native_action_dimensions(&self) -> NativeActionDimensions {
        self.0.native_action_dimensions
    }

    pub fn screenshot_to_action(&self) -> ScreenshotToActionTransform {
        self.0.screenshot_to_action
    }

    pub fn created_at(&self) -> Duration {
        self.0.created_at
    }

    pub fn expires_at(&self) -> Duration {
        self.0.expires_at
    }
}

struct RegistryInner {
    captures: HashMap<CaptureId, StoredCapture>,
    insertion_order: VecDeque<CaptureId>,
    expired_ids: HashSet<CaptureId>,
    expired_order: VecDeque<CaptureId>,
    total_bytes: usize,
    next_sequence: u64,
}

pub struct CaptureRegistry {
    config: CaptureRegistryConfig,
    clock: Arc<dyn MonotonicClock>,
    namespace: [u8; 16],
    inner: Mutex<RegistryInner>,
}

#[allow(dead_code)]
impl CaptureRegistry {
    pub fn new(config: CaptureRegistryConfig) -> Result<Self, CaptureStoreError> {
        Self::with_clock(config, Arc::new(SystemMonotonicClock::new()))
    }

    pub fn with_clock(
        config: CaptureRegistryConfig,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, CaptureStoreError> {
        let config = config.validate()?;
        Ok(Self {
            config,
            clock,
            namespace: *Uuid::new_v4().as_bytes(),
            inner: Mutex::new(RegistryInner {
                captures: HashMap::new(),
                insertion_order: VecDeque::new(),
                expired_ids: HashSet::new(),
                expired_order: VecDeque::new(),
                total_bytes: 0,
                next_sequence: 0,
            }),
        })
    }

    pub fn store(&self, registration: CaptureRegistration) -> Result<CaptureId, CaptureStoreError> {
        registration.target.validate()?;
        if registration.png_bytes.len() > self.config.max_total_bytes {
            return Err(CaptureStoreError::CaptureTooLarge);
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(self.config.max_encoded_width);
        limits.max_image_height = Some(self.config.max_encoded_height);
        limits.max_alloc = Some(self.config.max_decoded_bytes);
        let decoder = image::codecs::png::PngDecoder::with_limits(
            Cursor::new(&registration.png_bytes),
            limits,
        )
        .map_err(map_png_admission_error)?;
        let decoded_dimensions = decoder.dimensions();
        if decoded_dimensions
            != (
                registration.encoded_dimensions.width,
                registration.encoded_dimensions.height,
            )
        {
            return Err(CaptureStoreError::InvalidEncodedDimensions);
        }
        if decoder.total_bytes() > self.config.max_decoded_bytes {
            return Err(CaptureStoreError::PngDecodeLimitExceeded);
        }
        registration
            .screenshot_to_action
            .validate_extent(registration.encoded_dimensions)?;
        image::DynamicImage::from_decoder(decoder).map_err(map_png_admission_error)?;
        let png_bytes: Arc<[u8]> = registration.png_bytes.into();
        let byte_len = png_bytes.len();
        let digest = ContentDigest(Sha256::digest(&png_bytes).into());
        let created_at = self.clock.now();
        let expires_at = created_at
            .checked_add(self.config.ttl)
            .ok_or(CaptureStoreError::ExpiryOverflow)?;

        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        self.prune_expired_locked(&mut inner, created_at);
        if inner.next_sequence == u64::MAX {
            return Err(CaptureStoreError::IdExhausted);
        }
        while inner.captures.len() >= self.config.max_captures
            || inner.total_bytes.saturating_add(byte_len) > self.config.max_total_bytes
        {
            self.evict_oldest(&mut inner);
        }
        let sequence = inner.next_sequence;
        inner.next_sequence += 1;
        let id = CaptureId {
            namespace: self.namespace,
            sequence,
        };
        let capture = StoredCapture {
            id,
            png_bytes,
            digest,
            target: registration.target,
            encoded_dimensions: registration.encoded_dimensions,
            native_action_dimensions: registration.native_action_dimensions,
            screenshot_to_action: registration.screenshot_to_action,
            binding: registration.binding,
            created_at,
            expires_at,
        };
        inner.total_bytes += byte_len;
        inner.insertion_order.push_back(id);
        inner.captures.insert(id, capture);
        Ok(id)
    }

    /// Read immutable screenshot material without making it action-capable.
    pub fn read_for_perception(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
        target: &CaptureTarget,
    ) -> Result<PerceptionCapture, CaptureLookupError> {
        self.resolve_capture(id, binding, target, false)
            .map(PerceptionCapture)
    }

    fn read_for_perception_bound(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
    ) -> Result<PerceptionCapture, CaptureLookupError> {
        let now = self.clock.now();
        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        if inner
            .captures
            .get(&id)
            .is_some_and(|capture| capture.expires_at <= now)
        {
            self.remove_capture(&mut inner, id);
            self.remember_expired(&mut inner, id);
            return Err(CaptureLookupError::Expired);
        }
        let capture = inner.captures.get(&id).ok_or_else(|| {
            if inner.expired_ids.contains(&id) {
                CaptureLookupError::Expired
            } else {
                CaptureLookupError::Unknown
            }
        })?;
        if capture.binding != *binding {
            return Err(CaptureLookupError::GenerationMismatch);
        }
        Ok(PerceptionCapture(capture.clone()))
    }

    fn admit_action(
        &self,
        request: CaptureActionRequest,
    ) -> Result<CaptureActionAdmission, CaptureActionError> {
        let now = self.clock.now();
        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        if inner
            .captures
            .get(&request.capture_id)
            .is_some_and(|capture| capture.expires_at <= now)
        {
            self.remove_capture(&mut inner, request.capture_id);
            self.remember_expired(&mut inner, request.capture_id);
            return Err(CaptureLookupError::Expired.into());
        }
        let capture = inner.captures.get(&request.capture_id).ok_or_else(|| {
            if inner.expired_ids.contains(&request.capture_id) {
                CaptureLookupError::Expired
            } else {
                CaptureLookupError::Unknown
            }
        })?;
        if capture.binding != request.binding {
            return Err(CaptureLookupError::GenerationMismatch.into());
        }
        if capture.target != request.target {
            return Err(CaptureLookupError::TargetMismatch.into());
        }
        if capture.native_action_dimensions != request.current_native_action_dimensions {
            return Err(CaptureActionError::NativeActionFrameMismatch);
        }
        let max_x = f64::from(capture.encoded_dimensions.width);
        let max_y = f64::from(capture.encoded_dimensions.height);
        if !request.screenshot_x.is_finite()
            || !request.screenshot_y.is_finite()
            || request.screenshot_x < 0.0
            || request.screenshot_y < 0.0
            || request.screenshot_x >= max_x
            || request.screenshot_y >= max_y
        {
            return Err(CaptureActionError::InvalidScreenshotPoint);
        }
        let (action_x, action_y) = capture
            .screenshot_to_action
            .apply(request.screenshot_x, request.screenshot_y);
        if !action_x.is_finite() || !action_y.is_finite() {
            return Err(CaptureActionError::InvalidMappedPoint);
        }
        let admission = CaptureActionAdmission {
            capture_id: capture.id,
            target: capture.target.clone(),
            action_x,
            action_y,
            native_action_dimensions: capture.native_action_dimensions,
            digest: capture.digest,
        };
        self.remove_capture(&mut inner, request.capture_id);
        Ok(admission)
    }

    /// Atomically remove and return the capture for exactly one action attempt.
    pub fn consume_for_action(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
        target: &CaptureTarget,
    ) -> Result<ActionCapture, CaptureLookupError> {
        self.resolve_capture(id, binding, target, true)
            .map(ActionCapture)
    }

    fn resolve_capture(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
        target: &CaptureTarget,
        consume: bool,
    ) -> Result<StoredCapture, CaptureLookupError> {
        let now = self.clock.now();
        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        if inner
            .captures
            .get(&id)
            .is_some_and(|capture| capture.expires_at <= now)
        {
            self.remove_capture(&mut inner, id);
            self.remember_expired(&mut inner, id);
            return Err(CaptureLookupError::Expired);
        }
        let capture = inner.captures.get(&id).ok_or_else(|| {
            if inner.expired_ids.contains(&id) {
                CaptureLookupError::Expired
            } else {
                CaptureLookupError::Unknown
            }
        })?;
        if capture.binding != *binding {
            return Err(CaptureLookupError::GenerationMismatch);
        }
        if capture.target != *target {
            return Err(CaptureLookupError::TargetMismatch);
        }
        let capture = capture.clone();
        if consume {
            self.remove_capture(&mut inner, id);
        }
        Ok(capture)
    }

    pub fn retire(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
    ) -> Result<(), CaptureLookupError> {
        let now = self.clock.now();
        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        if inner
            .captures
            .get(&id)
            .is_some_and(|capture| capture.expires_at <= now)
        {
            self.remove_capture(&mut inner, id);
            self.remember_expired(&mut inner, id);
            return Err(CaptureLookupError::Expired);
        }
        let capture = inner.captures.get(&id).ok_or_else(|| {
            if inner.expired_ids.contains(&id) {
                CaptureLookupError::Expired
            } else {
                CaptureLookupError::Unknown
            }
        })?;
        if capture.binding != *binding {
            return Err(CaptureLookupError::GenerationMismatch);
        }
        self.remove_capture(&mut inner, id);
        Ok(())
    }

    pub fn retire_session(&self, binding: &CaptureBinding) -> usize {
        self.retire_matching(|capture| capture.binding == *binding)
    }

    pub fn retire_runtime(&self, runtime_generation: u64) -> usize {
        self.retire_matching(|capture| capture.binding.runtime_generation == runtime_generation)
    }

    /// Drop all registry-owned references whose TTL has elapsed.
    ///
    /// Work is bounded by `max_captures`. The returned byte count is the sum of
    /// encoded PNG lengths removed from the registry; independently cloned
    /// perception results may keep their own immutable references alive.
    pub fn prune_expired(&self) -> PruneOutcome {
        let now = self.clock.now();
        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        self.prune_expired_locked(&mut inner, now)
    }

    pub fn len(&self) -> usize {
        self.prune_expired();
        self.inner
            .lock()
            .expect("capture registry lock poisoned")
            .captures
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn total_bytes(&self) -> usize {
        self.prune_expired();
        self.inner
            .lock()
            .expect("capture registry lock poisoned")
            .total_bytes
    }

    fn retire_matching(&self, matches: impl Fn(&StoredCapture) -> bool) -> usize {
        let mut inner = self.inner.lock().expect("capture registry lock poisoned");
        let ids = inner
            .insertion_order
            .iter()
            .copied()
            .filter(|id| inner.captures.get(id).is_some_and(&matches))
            .collect::<Vec<_>>();
        for id in &ids {
            self.remove_capture(&mut inner, *id);
        }
        ids.len()
    }

    fn prune_expired_locked(&self, inner: &mut RegistryInner, now: Duration) -> PruneOutcome {
        let expired = inner
            .insertion_order
            .iter()
            .copied()
            .filter(|id| {
                inner
                    .captures
                    .get(id)
                    .is_some_and(|capture| capture.expires_at <= now)
            })
            .collect::<Vec<_>>();
        let mut outcome = PruneOutcome::default();
        for id in expired {
            outcome.encoded_bytes_released += self.remove_capture(inner, id);
            outcome.captures_removed += 1;
            self.remember_expired(inner, id);
        }
        outcome
    }

    fn evict_oldest(&self, inner: &mut RegistryInner) {
        if let Some(id) = inner.insertion_order.front().copied() {
            self.remove_capture(inner, id);
        }
    }

    fn remove_capture(&self, inner: &mut RegistryInner, id: CaptureId) -> usize {
        let mut removed_bytes = 0;
        if let Some(capture) = inner.captures.remove(&id) {
            removed_bytes = capture.png_bytes.len();
            inner.total_bytes -= removed_bytes;
        }
        if let Some(index) = inner
            .insertion_order
            .iter()
            .position(|candidate| *candidate == id)
        {
            inner.insertion_order.remove(index);
        }
        removed_bytes
    }

    fn remember_expired(&self, inner: &mut RegistryInner, id: CaptureId) {
        if inner.expired_ids.insert(id) {
            inner.expired_order.push_back(id);
        }
        while inner.expired_order.len() > self.config.max_captures {
            if let Some(oldest) = inner.expired_order.pop_front() {
                inner.expired_ids.remove(&oldest);
            }
        }
    }
}

static NEXT_RUNTIME_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Runtime-owned facade for immutable capture publication, perception reads,
/// and one-shot action admission.
pub struct CaptureService {
    runtime_generation: u64,
    registry: CaptureRegistry,
    session_generations: Mutex<HashMap<Arc<str>, u64>>,
}

impl CaptureService {
    pub fn new(config: CaptureRegistryConfig) -> Result<Self, CaptureStoreError> {
        let runtime_generation = NEXT_RUNTIME_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0 && current != u64::MAX).then_some(current + 1)
            })
            .map_err(|_| CaptureStoreError::IdExhausted)?;
        Ok(Self {
            runtime_generation,
            registry: CaptureRegistry::new(config)?,
            session_generations: Mutex::new(HashMap::new()),
        })
    }

    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }

    pub fn binding(
        &self,
        session_id: impl Into<Arc<str>>,
        session_generation: u64,
    ) -> Result<CaptureBinding, CaptureStoreError> {
        CaptureBinding::new(self.runtime_generation, session_id, session_generation)
    }

    /// Return the current private generation for a runtime-scoped session.
    pub fn current_binding(
        &self,
        session_id: impl Into<Arc<str>>,
    ) -> Result<CaptureBinding, CaptureStoreError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(CaptureStoreError::InvalidBinding);
        }
        let generation = *self
            .session_generations
            .lock()
            .expect("capture session generation lock poisoned")
            .entry(session_id.clone())
            .or_insert(1);
        self.binding(session_id, generation)
    }

    /// Resolve trusted runtime arguments after the dispatch boundary has
    /// replaced the public session label with `_session_id`.
    pub fn binding_from_args(
        &self,
        args: &serde_json::Value,
    ) -> Result<CaptureBinding, CaptureStoreError> {
        let session_id = args
            .get("_session_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or(CaptureStoreError::InvalidBinding)?;
        self.current_binding(Arc::<str>::from(session_id))
    }

    pub fn publish(&self, publication: CapturePublication) -> Result<CaptureId, CaptureStoreError> {
        let binding = self.binding(publication.session_id, publication.session_generation)?;
        self.registry.store(CaptureRegistration {
            png_bytes: publication.png_bytes,
            target: publication.target,
            encoded_dimensions: publication.encoded_dimensions,
            native_action_dimensions: publication.native_action_dimensions,
            screenshot_to_action: publication.screenshot_to_action,
            binding,
        })
    }

    pub fn parse_capture_id(&self, value: &str) -> Result<CaptureId, CaptureIdParseError> {
        value.parse()
    }

    pub fn read_for_perception(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
    ) -> Result<PerceptionCapture, CaptureLookupError> {
        self.registry.read_for_perception_bound(id, binding)
    }

    pub fn admit_action(
        &self,
        request: CaptureActionRequest,
    ) -> Result<CaptureActionAdmission, CaptureActionError> {
        self.registry.admit_action(request)
    }

    pub fn retire_session(&self, binding: &CaptureBinding) -> usize {
        let removed = self.registry.retire_session(binding);
        if binding.runtime_generation == self.runtime_generation {
            let mut generations = self
                .session_generations
                .lock()
                .expect("capture session generation lock poisoned");
            let generation = generations
                .entry(binding.session_id.clone())
                .or_insert(binding.session_generation);
            if *generation == binding.session_generation {
                *generation = generation.saturating_add(1).max(1);
            }
        }
        removed
    }

    /// Retire one capture when its owning snapshot is superseded.
    pub fn retire_capture(
        &self,
        id: CaptureId,
        binding: &CaptureBinding,
    ) -> Result<(), CaptureLookupError> {
        self.registry.retire(id, binding)
    }

    pub fn retire_session_id(&self, session_id: &str) -> usize {
        let Ok(binding) = self.current_binding(Arc::<str>::from(session_id)) else {
            return 0;
        };
        self.retire_session(&binding)
    }

    pub fn retire_runtime(&self) -> usize {
        self.registry.retire_runtime(self.runtime_generation)
    }

    pub fn prune_expired(&self) -> PruneOutcome {
        self.registry.prune_expired()
    }
}

impl Default for CaptureService {
    fn default() -> Self {
        Self::new(CaptureRegistryConfig::default()).expect("default capture config is valid")
    }
}

impl Drop for CaptureService {
    fn drop(&mut self) {
        self.registry.retire_runtime(self.runtime_generation);
    }
}

fn map_png_admission_error(error: image::ImageError) -> CaptureStoreError {
    if matches!(error, image::ImageError::Limits(_)) {
        CaptureStoreError::PngDecodeLimitExceeded
    } else {
        CaptureStoreError::InvalidPng
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_utils::encode_rgba_to_png;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Default)]
    struct ManualClock(AtomicU64);

    impl ManualClock {
        fn advance(&self, duration: Duration) {
            self.0.fetch_add(
                u64::try_from(duration.as_millis()).expect("test duration fits"),
                Ordering::SeqCst,
            );
        }
    }

    impl MonotonicClock for ManualClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
    }

    fn config(max_captures: usize, max_total_bytes: usize) -> CaptureRegistryConfig {
        CaptureRegistryConfig {
            max_captures,
            max_total_bytes,
            max_encoded_width: 4096,
            max_encoded_height: 4096,
            max_decoded_bytes: 64 * 1024 * 1024,
            ttl: Duration::from_secs(10),
        }
    }

    fn binding(runtime: u64, session: &str, generation: u64) -> CaptureBinding {
        CaptureBinding::new(runtime, Arc::<str>::from(session), generation).unwrap()
    }

    fn png(width: u32, height: u32, value: u8) -> Vec<u8> {
        encode_rgba_to_png(
            &vec![value; usize::try_from(width * height * 4).unwrap()],
            width,
            height,
        )
        .unwrap()
    }

    fn registration(
        png_bytes: Vec<u8>,
        target: CaptureTarget,
        binding: CaptureBinding,
    ) -> CaptureRegistration {
        CaptureRegistration {
            png_bytes,
            target,
            encoded_dimensions: EncodedScreenshotDimensions::new(2, 2).unwrap(),
            native_action_dimensions: NativeActionDimensions::new(2, 2).unwrap(),
            screenshot_to_action: ScreenshotToActionTransform::identity(),
            binding,
        }
    }

    fn action_request(
        capture_id: CaptureId,
        binding: CaptureBinding,
        target: CaptureTarget,
        native: (u32, u32),
    ) -> CaptureActionRequest {
        CaptureActionRequest {
            capture_id,
            binding,
            target,
            current_native_action_dimensions: NativeActionDimensions::new(native.0, native.1)
                .unwrap(),
            screenshot_x: 0.5,
            screenshot_y: 0.5,
        }
    }

    #[test]
    fn stores_immutable_png_metadata_and_never_reuses_ids() {
        let registry = CaptureRegistry::new(config(2, 10_000)).unwrap();
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::Window {
            pid: 7,
            window_id: 9,
        };
        let bytes = png(2, 2, 42);
        let expected_digest = Sha256::digest(&bytes);
        let first = registry
            .store(registration(bytes.clone(), target.clone(), owner.clone()))
            .unwrap();
        let capture = registry
            .read_for_perception(first, &owner, &target)
            .unwrap();
        assert_eq!(capture.png_bytes().as_ref(), bytes.as_slice());
        assert_eq!(capture.digest().as_bytes(), expected_digest.as_slice());
        assert_eq!(
            capture.encoded_dimensions(),
            EncodedScreenshotDimensions::new(2, 2).unwrap()
        );

        registry.retire(first, &owner).unwrap();
        let second = registry.store(registration(bytes, target, owner)).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn rejects_invalid_png_dimensions_target_and_transform_before_storage() {
        let registry = CaptureRegistry::new(config(4, 10_000)).unwrap();
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::PrimaryDesktop;
        let mut invalid_png = registration(b"not a png".to_vec(), target.clone(), owner.clone());
        assert_eq!(
            registry.store(invalid_png),
            Err(CaptureStoreError::InvalidPng)
        );

        invalid_png = registration(png(3, 2, 1), target, owner.clone());
        assert_eq!(
            registry.store(invalid_png),
            Err(CaptureStoreError::InvalidEncodedDimensions)
        );

        let invalid_target = CaptureTarget::Window {
            pid: 0,
            window_id: 9,
        };
        assert_eq!(
            registry.store(registration(png(2, 2, 1), invalid_target, owner)),
            Err(CaptureStoreError::InvalidTarget)
        );
        assert_eq!(registry.len(), 0);
        assert_eq!(
            ScreenshotToActionTransform::new(f64::NAN, 0.0, 0.0, 1.0, 0.0, 0.0),
            Err(CaptureStoreError::InvalidTransform)
        );
        assert_eq!(
            ScreenshotToActionTransform::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0),
            Err(CaptureStoreError::InvalidTransform)
        );
    }

    #[test]
    fn rejects_pngs_before_decode_when_dimensions_or_allocation_exceed_limits() {
        let bytes = png(2, 2, 1);
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::PrimaryDesktop;

        let mut dimension_config = config(2, 10_000);
        dimension_config.max_encoded_width = 1;
        let registry = CaptureRegistry::new(dimension_config).unwrap();
        assert_eq!(
            registry.store(registration(bytes.clone(), target.clone(), owner.clone())),
            Err(CaptureStoreError::PngDecodeLimitExceeded)
        );

        let mut allocation_config = config(2, 10_000);
        allocation_config.max_decoded_bytes = 8;
        let registry = CaptureRegistry::new(allocation_config).unwrap();
        assert_eq!(
            registry.store(registration(bytes, target, owner)),
            Err(CaptureStoreError::PngDecodeLimitExceeded)
        );
    }

    #[test]
    fn transform_composition_preserves_continuous_coordinates() {
        let image_to_native =
            ScreenshotToActionTransform::new(2.0, 0.0, 0.0, 3.0, 4.0, 5.0).unwrap();
        let native_to_action =
            ScreenshotToActionTransform::new(1.0, 0.5, 0.0, 1.0, -2.0, 7.0).unwrap();
        let composed = image_to_native.then(native_to_action).unwrap();
        let intermediate = image_to_native.apply(1.25, 2.5);
        assert_eq!(
            composed.apply(1.25, 2.5),
            native_to_action.apply(intermediate.0, intermediate.1)
        );
    }

    #[test]
    fn downscaled_capture_keeps_encoded_and_native_action_dimensions_distinct() {
        let registry = CaptureRegistry::new(config(2, 10_000)).unwrap();
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::PrimaryDesktop;
        let transform = ScreenshotToActionTransform::new(2.0, 0.0, 0.0, 3.0, 0.0, 0.0).unwrap();
        let id = registry
            .store(CaptureRegistration {
                png_bytes: png(2, 2, 1),
                target: target.clone(),
                encoded_dimensions: EncodedScreenshotDimensions::new(2, 2).unwrap(),
                native_action_dimensions: NativeActionDimensions::new(4, 6).unwrap(),
                screenshot_to_action: transform,
                binding: owner.clone(),
            })
            .unwrap();

        let capture = registry.consume_for_action(id, &owner, &target).unwrap();
        assert_eq!(
            capture.encoded_dimensions(),
            EncodedScreenshotDimensions::new(2, 2).unwrap()
        );
        assert_eq!(
            capture.native_action_dimensions(),
            NativeActionDimensions::new(4, 6).unwrap()
        );
        assert_eq!(capture.screenshot_to_action().apply(1.0, 1.0), (2.0, 3.0));
    }

    #[test]
    fn rejects_transform_that_overflows_across_encoded_extent() {
        let registry = CaptureRegistry::new(config(2, 10_000)).unwrap();
        let mut capture = registration(
            png(3, 2, 1),
            CaptureTarget::PrimaryDesktop,
            binding(1, "session-a", 1),
        );
        capture.encoded_dimensions = EncodedScreenshotDimensions::new(3, 2).unwrap();
        capture.screenshot_to_action =
            ScreenshotToActionTransform::new(f64::MAX, 0.0, 0.0, 1.0, 0.0, 0.0).unwrap();
        assert_eq!(
            registry.store(capture),
            Err(CaptureStoreError::InvalidTransformExtent)
        );
    }

    #[test]
    fn lookup_errors_are_specific_and_action_consumption_is_atomic() {
        let registry = CaptureRegistry::new(config(4, 10_000)).unwrap();
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::PrimaryDesktop;
        let id = registry
            .store(registration(png(2, 2, 1), target.clone(), owner.clone()))
            .unwrap();
        let unknown = CaptureId {
            namespace: [0; 16],
            sequence: 99,
        };
        assert_eq!(
            registry
                .read_for_perception(unknown, &owner, &target)
                .unwrap_err(),
            CaptureLookupError::Unknown
        );
        assert_eq!(
            registry
                .read_for_perception(id, &binding(1, "session-a", 2), &target)
                .unwrap_err(),
            CaptureLookupError::GenerationMismatch
        );
        assert_eq!(
            registry
                .read_for_perception(
                    id,
                    &owner,
                    &CaptureTarget::Window {
                        pid: 7,
                        window_id: 9,
                    },
                )
                .unwrap_err(),
            CaptureLookupError::TargetMismatch
        );
        registry.consume_for_action(id, &owner, &target).unwrap();
        assert_eq!(
            registry
                .consume_for_action(id, &owner, &target)
                .unwrap_err(),
            CaptureLookupError::Unknown
        );
    }

    #[test]
    fn unknown_expired_session_runtime_target_replacement_eviction_and_resize_refuse_dispatch() {
        let dispatched = AtomicU64::new(0);
        let attempt = |registry: &CaptureRegistry, request: CaptureActionRequest| {
            let result = registry.admit_action(request);
            if result.is_ok() {
                dispatched.fetch_add(1, Ordering::SeqCst);
            }
            result
        };
        let owner = binding(11, "session-a", 3);
        let target = CaptureTarget::Window {
            pid: 42,
            window_id: 7,
        };

        let registry = CaptureRegistry::new(config(8, 100_000)).unwrap();
        let unknown = CaptureId {
            namespace: [0; 16],
            sequence: 99,
        };
        assert_eq!(
            attempt(
                &registry,
                action_request(unknown, owner.clone(), target.clone(), (2, 2))
            )
            .unwrap_err(),
            CaptureActionError::Lookup(CaptureLookupError::Unknown)
        );

        for (wrong_binding, expected) in [
            (
                binding(11, "session-b", 3),
                CaptureActionError::Lookup(CaptureLookupError::GenerationMismatch),
            ),
            (
                binding(12, "session-a", 3),
                CaptureActionError::Lookup(CaptureLookupError::GenerationMismatch),
            ),
        ] {
            let id = registry
                .store(registration(png(2, 2, 1), target.clone(), owner.clone()))
                .unwrap();
            assert_eq!(
                attempt(
                    &registry,
                    action_request(id, wrong_binding, target.clone(), (2, 2))
                )
                .unwrap_err(),
                expected
            );
            registry.retire(id, &owner).unwrap();
        }

        let id = registry
            .store(registration(png(2, 2, 2), target.clone(), owner.clone()))
            .unwrap();
        assert_eq!(
            attempt(
                &registry,
                action_request(id, owner.clone(), CaptureTarget::PrimaryDesktop, (2, 2))
            )
            .unwrap_err(),
            CaptureActionError::Lookup(CaptureLookupError::TargetMismatch)
        );
        registry.retire(id, &owner).unwrap();

        let replaced = registry
            .store(registration(png(2, 2, 3), target.clone(), owner.clone()))
            .unwrap();
        registry.retire(replaced, &owner).unwrap();
        let _replacement = registry
            .store(registration(png(2, 2, 4), target.clone(), owner.clone()))
            .unwrap();
        assert_eq!(
            attempt(
                &registry,
                action_request(replaced, owner.clone(), target.clone(), (2, 2))
            )
            .unwrap_err(),
            CaptureActionError::Lookup(CaptureLookupError::Unknown)
        );

        let resized = registry
            .store(registration(png(2, 2, 5), target.clone(), owner.clone()))
            .unwrap();
        assert_eq!(
            attempt(
                &registry,
                action_request(resized, owner.clone(), target.clone(), (3, 2))
            )
            .unwrap_err(),
            CaptureActionError::NativeActionFrameMismatch
        );

        let evicting = CaptureRegistry::new(config(1, 100_000)).unwrap();
        let evicted = evicting
            .store(registration(png(2, 2, 6), target.clone(), owner.clone()))
            .unwrap();
        let _newest = evicting
            .store(registration(png(2, 2, 7), target.clone(), owner.clone()))
            .unwrap();
        assert_eq!(
            attempt(
                &evicting,
                action_request(evicted, owner.clone(), target.clone(), (2, 2))
            )
            .unwrap_err(),
            CaptureActionError::Lookup(CaptureLookupError::Unknown)
        );

        let clock = Arc::new(ManualClock::default());
        let expiring = CaptureRegistry::with_clock(config(2, 100_000), clock.clone()).unwrap();
        let expired = expiring
            .store(registration(png(2, 2, 8), target.clone(), owner.clone()))
            .unwrap();
        clock.advance(Duration::from_secs(10));
        assert_eq!(
            attempt(&expiring, action_request(expired, owner, target, (2, 2))).unwrap_err(),
            CaptureActionError::Lookup(CaptureLookupError::Expired)
        );
        assert_eq!(dispatched.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn encoded_pixel_edges_are_half_open_and_refuse_without_consuming() {
        let registry = CaptureRegistry::new(config(2, 10_000)).unwrap();
        let owner = binding(1, "edge", 1);
        let target = CaptureTarget::PrimaryDesktop;
        let id = registry
            .store(registration(png(2, 2, 1), target.clone(), owner.clone()))
            .unwrap();
        let mut request = action_request(id, owner.clone(), target.clone(), (2, 2));
        request.screenshot_x = 2.0;
        assert_eq!(
            registry.admit_action(request).unwrap_err(),
            CaptureActionError::InvalidScreenshotPoint
        );
        assert!(registry.read_for_perception(id, &owner, &target).is_ok());

        let mut request = action_request(id, owner.clone(), target.clone(), (2, 2));
        request.screenshot_y = 2.0;
        assert_eq!(
            registry.admit_action(request).unwrap_err(),
            CaptureActionError::InvalidScreenshotPoint
        );
        assert!(registry.read_for_perception(id, &owner, &target).is_ok());

        let mut request = action_request(id, owner, target, (2, 2));
        request.screenshot_x = 2.0 - f64::EPSILON;
        request.screenshot_y = 2.0 - f64::EPSILON;
        assert!(registry.admit_action(request).is_ok());
    }

    #[test]
    fn bounded_maintenance_releases_expired_png_without_lookup_or_store() {
        let clock = Arc::new(ManualClock::default());
        let registry = CaptureRegistry::with_clock(config(2, 10_000), clock.clone()).unwrap();
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::PrimaryDesktop;
        let id = registry
            .store(registration(png(2, 2, 1), target.clone(), owner.clone()))
            .unwrap();
        let bytes = registry
            .read_for_perception(id, &owner, &target)
            .unwrap()
            .png_bytes();
        let weak_bytes = Arc::downgrade(&bytes);
        let byte_len = bytes.len();
        drop(bytes);
        clock.advance(Duration::from_secs(10));
        assert_eq!(
            registry.prune_expired(),
            PruneOutcome {
                captures_removed: 1,
                encoded_bytes_released: byte_len,
            }
        );
        assert!(weak_bytes.upgrade().is_none());
        assert_eq!(
            registry
                .read_for_perception(id, &owner, &target)
                .unwrap_err(),
            CaptureLookupError::Expired
        );
        assert!(registry.is_empty());
        assert_eq!(registry.total_bytes(), 0);
    }

    #[test]
    fn count_and_byte_limits_evict_oldest_deterministically() {
        let sample = png(2, 2, 1);
        let byte_limit = sample.len() * 2;
        let registry = CaptureRegistry::new(config(2, byte_limit)).unwrap();
        let owner = binding(1, "session-a", 1);
        let target = CaptureTarget::PrimaryDesktop;
        let first = registry
            .store(registration(sample.clone(), target.clone(), owner.clone()))
            .unwrap();
        let second = registry
            .store(registration(sample.clone(), target.clone(), owner.clone()))
            .unwrap();
        let third = registry
            .store(registration(sample, target.clone(), owner.clone()))
            .unwrap();
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.total_bytes(), byte_limit);
        assert_eq!(
            registry
                .read_for_perception(first, &owner, &target)
                .unwrap_err(),
            CaptureLookupError::Unknown
        );
        assert!(registry
            .read_for_perception(second, &owner, &target)
            .is_ok());
        assert!(registry.read_for_perception(third, &owner, &target).is_ok());
    }

    #[test]
    fn session_and_runtime_cleanup_are_generation_exact() {
        let registry = CaptureRegistry::new(config(4, 10_000)).unwrap();
        let target = CaptureTarget::PrimaryDesktop;
        let first = binding(1, "same-label", 1);
        let second = binding(1, "same-label", 2);
        let other_runtime = binding(2, "same-label", 1);
        registry
            .store(registration(png(2, 2, 1), target.clone(), first.clone()))
            .unwrap();
        registry
            .store(registration(png(2, 2, 2), target.clone(), second.clone()))
            .unwrap();
        registry
            .store(registration(png(2, 2, 3), target.clone(), other_runtime))
            .unwrap();

        assert_eq!(registry.retire_session(&first), 1);
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.retire_runtime(1), 1);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.retire_runtime(2), 1);
        assert!(registry.is_empty());
    }

    #[test]
    fn service_round_trips_ids_and_applies_full_affine_once() {
        let service = CaptureService::new(config(4, 10_000)).unwrap();
        let target = CaptureTarget::Window {
            pid: 42,
            window_id: 7,
        };
        let transform = ScreenshotToActionTransform::new(2.0, 0.5, -0.25, 3.0, 11.0, -4.0).unwrap();
        let id = service
            .publish(CapturePublication {
                png_bytes: png(4, 3, 9),
                target: target.clone(),
                encoded_dimensions: EncodedScreenshotDimensions::new(4, 3).unwrap(),
                native_action_dimensions: NativeActionDimensions::new(40, 30).unwrap(),
                screenshot_to_action: transform,
                session_id: Arc::from("session-a"),
                session_generation: 2,
            })
            .unwrap();
        assert_eq!(service.parse_capture_id(&id.to_string()).unwrap(), id);
        let binding = service.binding("session-a", 2).unwrap();
        let admission = service
            .admit_action(CaptureActionRequest {
                capture_id: id,
                binding: binding.clone(),
                target: target.clone(),
                current_native_action_dimensions: NativeActionDimensions::new(40, 30).unwrap(),
                screenshot_x: 2.5,
                screenshot_y: 1.5,
            })
            .unwrap();
        assert_eq!((admission.action_x, admission.action_y), (16.75, -0.125));
        assert_eq!(admission.target, target);
        assert_eq!(
            service
                .admit_action(CaptureActionRequest {
                    capture_id: id,
                    binding,
                    target: CaptureTarget::Window {
                        pid: 42,
                        window_id: 7,
                    },
                    current_native_action_dimensions: NativeActionDimensions::new(40, 30).unwrap(),
                    screenshot_x: 2.5,
                    screenshot_y: 1.5,
                })
                .unwrap_err(),
            CaptureActionError::Lookup(CaptureLookupError::Unknown)
        );
    }

    #[test]
    fn invalid_action_point_does_not_consume_capture() {
        let service = CaptureService::new(config(4, 10_000)).unwrap();
        let target = CaptureTarget::PrimaryDesktop;
        let id = service
            .publish(CapturePublication {
                png_bytes: png(2, 2, 5),
                target: target.clone(),
                encoded_dimensions: EncodedScreenshotDimensions::new(2, 2).unwrap(),
                native_action_dimensions: NativeActionDimensions::new(2, 2).unwrap(),
                screenshot_to_action: ScreenshotToActionTransform::identity(),
                session_id: Arc::from("session-a"),
                session_generation: 1,
            })
            .unwrap();
        let binding = service.binding("session-a", 1).unwrap();
        assert_eq!(
            service
                .admit_action(CaptureActionRequest {
                    capture_id: id,
                    binding: binding.clone(),
                    target: target.clone(),
                    current_native_action_dimensions: NativeActionDimensions::new(2, 2).unwrap(),
                    screenshot_x: f64::NAN,
                    screenshot_y: 0.0,
                })
                .unwrap_err(),
            CaptureActionError::InvalidScreenshotPoint
        );
        assert!(service.read_for_perception(id, &binding).is_ok());
    }

    #[test]
    fn capture_id_parser_rejects_noncanonical_ids() {
        let id = CaptureId {
            namespace: [0xab; 16],
            sequence: 15,
        };
        assert_eq!(id.to_string().parse::<CaptureId>().unwrap(), id);
        for invalid in [
            "",
            "capture_deadbeef_000000000000000f",
            "capture_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz_000000000000000f",
            "capture_abababababababababababababababab_0_f",
        ] {
            assert_eq!(
                invalid.parse::<CaptureId>().unwrap_err(),
                CaptureIdParseError::InvalidFormat
            );
        }
    }

    #[test]
    fn service_owns_session_generation_and_advances_it_on_retirement() {
        let service = CaptureService::new(config(4, 10_000)).unwrap();
        let first = service
            .binding_from_args(&serde_json::json!({"_session_id": "runtime/session-a"}))
            .unwrap();
        assert_eq!(first.session_generation(), 1);
        assert_eq!(service.retire_session_id("runtime/session-a"), 0);
        let second = service.current_binding("runtime/session-a").unwrap();
        assert_eq!(second.session_generation(), 2);
        assert_eq!(first.runtime_generation(), second.runtime_generation());
        assert_eq!(
            service
                .binding_from_args(&serde_json::json!({"session": "public-forgery"}))
                .unwrap_err(),
            CaptureStoreError::InvalidBinding
        );
    }
}
