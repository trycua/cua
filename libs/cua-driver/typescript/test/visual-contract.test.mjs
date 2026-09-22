import assert from "node:assert/strict"
import test from "node:test"

import contractModule, {
  ActionTarget,
  ClickInput,
  ClickPosition,
  InputDeliveryMode,
  ParseVisualRegionsInput,
  ParseVisualRegionsOptions,
  ParseVisualRegionsOutput,
  VisualActionCoordinateSpace,
  VisualCaptureProvenance,
  VisualCaptureSource,
  VisualParseError,
  VisualParseErrorCode,
  VisualParseTiming,
  VisualParseWarning,
  VisualParserMetadata,
  VisualRegion,
  VisualRegionBounds,
  VisualRegionKind,
  VisualScreenshotReference,
} from "../dist/native/cua_driver_contract.js"

const roundTrip = (converter, value) =>
  converter.lift(converter.lower(value, size => new Uint8Array(size)))

test("visual contract records round-trip through generated UniFFI converters", () => {
  contractModule.initialize()

  const input = ParseVisualRegionsInput.create({
    captureId: "capture-1",
    options: ParseVisualRegionsOptions.create({
      kinds: [VisualRegionKind.Text, VisualRegionKind.Icon],
      minConfidence: 0.75,
      maxRegions: 50,
    }),
  })
  assert.deepEqual(
    roundTrip(contractModule.converters.FfiConverterTypeParseVisualRegionsInput, input),
    input,
  )

  const capturedClick = ClickInput.create({
    target: new ActionTarget.Desktop({ displayId: "primary" }),
    position: new ClickPosition.CapturedCoordinates({
      x: 12,
      y: 34,
      captureId: "capture-1",
    }),
    deliveryMode: InputDeliveryMode.Foreground,
    session: undefined,
    button: undefined,
    count: undefined,
  })
  assert.deepEqual(
    roundTrip(contractModule.converters.FfiConverterTypeClickInput, capturedClick),
    capturedClick,
  )

  const output = ParseVisualRegionsOutput.create({
    schema: "cua.visual_regions_v1",
    capture: VisualCaptureProvenance.create({
      captureId: "capture-1",
      source: new VisualCaptureSource.PrimaryDesktop({ displayId: "primary" }),
      screenshot: VisualScreenshotReference.create({
        reference: "sha256:abc123",
        width: 1440,
        height: 900,
        mimeType: "image/png",
        sha256: "abc123",
      }),
      actionCoordinateSpace: new VisualActionCoordinateSpace.Affine({
        m11: 0.5,
        m12: 0.25,
        m21: -0.5,
        m22: 2,
        tx: 10.25,
        ty: 20.75,
      }),
      capturedAt: "2026-09-17T12:00:00Z",
    }),
    parser: VisualParserMetadata.create({
      extensionId: "cua-perception",
      extensionVersion: "1.0.0",
      modelId: "visual-parser",
      modelVersion: "2026-09-17",
      runtime: "local",
      backend: "onnx_runtime_cpu",
      modelSourceRevision: "revision-1",
      modelManifestSha256: "a".repeat(64),
      onnxRuntimeVersion: "1.26.0",
      onnxRuntimeLibrarySha256: "b".repeat(64),
      fixtureSha256: undefined,
    }),
    regions: [VisualRegion.create({
      id: "region-1",
      kind: VisualRegionKind.Text,
      bounds: VisualRegionBounds.create({ x: 11, y: 20, width: 4, height: 3 }),
      text: "Open",
      label: "Open button",
      confidence: 0.99,
      interactive: true,
      parentId: "group-1",
      groupId: "toolbar",
      readingOrder: 1,
    })],
    warnings: [VisualParseWarning.create({
      code: "partial-ocr",
      message: "Some text could not be read",
      detail: "region-2",
    })],
    timing: VisualParseTiming.create({
      durationMs: 18n,
      preprocessMs: 3n,
      inferenceMs: 12n,
    }),
    requestId: "request-1",
  })
  const restored = roundTrip(
    contractModule.converters.FfiConverterTypeParseVisualRegionsOutput,
    output,
  )
  assert.deepEqual(restored, output)
  assert.equal(restored.capture.actionCoordinateSpace.inner.m11, 0.5)
  assert.equal(restored.regions[0].bounds.width, 4)
  assert.equal(restored.warnings[0].detail, "region-2")

  const error = VisualParseError.create({
    code: VisualParseErrorCode.CaptureExpired,
    message: "Capture expired",
    retryable: true,
    detail: "capture-1",
  })
  assert.deepEqual(
    roundTrip(contractModule.converters.FfiConverterTypeVisualParseError, error),
    error,
  )
})
