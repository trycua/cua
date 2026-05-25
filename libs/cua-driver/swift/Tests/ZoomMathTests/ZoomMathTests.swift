import XCTest
@testable import CuaDriverCore

final class ZoomMathTests: XCTestCase {
    // MARK: clamp01

    func testClamp01ClampsBelowZero() {
        XCTAssertEqual(clamp01(-1), 0)
    }

    func testClamp01ClampsAboveOne() {
        XCTAssertEqual(clamp01(2), 1)
    }

    func testClamp01PassesThroughInRange() {
        XCTAssertEqual(clamp01(0.5), 0.5)
    }

    // MARK: cubicBezier

    func testCubicBezierLinearIsIdentityAtHalf() {
        // (0,0)-(1,1) is the degenerate linear bezier.
        XCTAssertEqual(cubicBezier(x1: 0, y1: 0, x2: 1, y2: 1, t: 0.5), 0.5, accuracy: 1e-4)
    }

    func testCubicBezierCSSEaseMidpointIsRoughlyPointEight() {
        // CSS `ease` is cubic-bezier(0.25, 0.1, 0.25, 1.0). At t=0.5 the
        // curve output is ~0.8025 (see CSS Transitions / Easings specs).
        let y = cubicBezier(x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0, t: 0.5)
        XCTAssertGreaterThan(y, 0.7)
        XCTAssertLessThan(y, 0.9)
    }

    func testCubicBezierEndpoints() {
        XCTAssertEqual(cubicBezier(x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0, t: 0), 0, accuracy: 1e-4)
        XCTAssertEqual(cubicBezier(x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0, t: 1), 1, accuracy: 1e-4)
    }

    // MARK: easeOutExpo

    func testEaseOutExpoEndpoints() {
        XCTAssertEqual(easeOutExpo(0), 0, accuracy: 1e-6)
        XCTAssertEqual(easeOutExpo(1), 1, accuracy: 1e-12)
    }

    // MARK: lerp

    func testLerpMidpoint() {
        XCTAssertEqual(lerp(0, 10, 0.5), 5)
    }

    // MARK: positionAt

    func testPositionAtExactSampleReturnsThatSample() {
        let samples = [
            CursorSample(tMs: 0, x: 1, y: 2),
            CursorSample(tMs: 100, x: 3, y: 4),
            CursorSample(tMs: 200, x: 5, y: 6),
        ]
        let p = CursorTelemetry.positionAt(timeMs: 100, samples: samples)
        XCTAssertEqual(p, CursorPosition(x: 3, y: 4))
    }

    func testPositionAtBetweenSamplesLerps() {
        let samples = [
            CursorSample(tMs: 0, x: 0, y: 0),
            CursorSample(tMs: 100, x: 10, y: 20),
        ]
        let p = CursorTelemetry.positionAt(timeMs: 25, samples: samples)
        XCTAssertEqual(p?.x ?? .nan, 2.5, accuracy: 1e-9)
        XCTAssertEqual(p?.y ?? .nan, 5.0, accuracy: 1e-9)
    }

    func testPositionAtBeforeRangeClampsToFirst() {
        let samples = [
            CursorSample(tMs: 10, x: 1, y: 2),
            CursorSample(tMs: 20, x: 3, y: 4),
        ]
        let p = CursorTelemetry.positionAt(timeMs: 0, samples: samples)
        XCTAssertEqual(p, CursorPosition(x: 1, y: 2))
    }

    func testPositionAtAfterRangeClampsToLast() {
        let samples = [
            CursorSample(tMs: 10, x: 1, y: 2),
            CursorSample(tMs: 20, x: 3, y: 4),
        ]
        let p = CursorTelemetry.positionAt(timeMs: 999, samples: samples)
        XCTAssertEqual(p, CursorPosition(x: 3, y: 4))
    }

    func testPositionAtEmptyReturnsNil() {
        XCTAssertNil(CursorTelemetry.positionAt(timeMs: 0, samples: []))
    }

    // MARK: smoothing

    func testSmoothMovesFractionOfDelta() {
        let raw = CursorPosition(x: 100, y: 200)
        let prev = CursorPosition(x: 0, y: 0)
        let p = CursorTelemetry.smooth(raw, towards: prev, factor: 0.25)
        XCTAssertEqual(p, CursorPosition(x: 25, y: 50))
    }

    func testAdaptiveFactorSaturatesAtRampDistance() {
        let f = CursorTelemetry.adaptiveFactor(
            distance: 1.0, rampDistance: 0.15, minFactor: 0.1, maxFactor: 0.25
        )
        XCTAssertEqual(f, 0.25, accuracy: 1e-12)
    }

    func testAdaptiveFactorAtZeroDistanceIsMin() {
        let f = CursorTelemetry.adaptiveFactor(
            distance: 0, rampDistance: 0.15, minFactor: 0.1, maxFactor: 0.25
        )
        XCTAssertEqual(f, 0.1, accuracy: 1e-12)
    }

    // MARK: regions

    func testSampleCurveOutsideRegionIsIdentityAtCursor() {
        let regions: [ZoomRegion] = []
        let samples = [CursorSample(tMs: 0, x: 42, y: 24)]
        let (scale, fx, fy) = ZoomRegionGenerator.sampleCurve(atMs: 100, regions: regions, cursorSamples: samples)
        XCTAssertEqual(scale, 1.0)
        XCTAssertEqual(fx, 42)
        XCTAssertEqual(fy, 24)
    }

    func testSampleCurveInsideHoldPhaseIsFullScale() {
        let region = ZoomRegion(
            startMs: 0, endMs: 5000,
            focusX: 100, focusY: 200,
            scale: 2.0,
            zoomInDurationMs: 1000,
            zoomOutDurationMs: 1000
        )
        // Middle of the hold phase.
        let (scale, fx, fy) = ZoomRegionGenerator.sampleCurve(atMs: 2500, regions: [region], cursorSamples: [])
        XCTAssertEqual(scale, 2.0, accuracy: 1e-6)
        XCTAssertEqual(fx, 100)
        XCTAssertEqual(fy, 200)
    }

    func testGenerateSingleClickProducesOneRegion() {
        let clicks = [ClickEvent(tMs: 1000, x: 50, y: 60)]
        let regions = ZoomRegionGenerator.generate(clicks: clicks, cursorSamples: [], defaultScale: 2.0)
        XCTAssertEqual(regions.count, 1)
        XCTAssertEqual(regions[0].focusX, 50)
        XCTAssertEqual(regions[0].focusY, 60)
        XCTAssertEqual(regions[0].scale, 2.0)
    }

    func testGenerateMergesNearbyClicks() {
        // Two clicks close in time — the second's start (tMs-200) falls within
        // chainedZoomPanGapMs of the first's end (tMs+1500), so they merge.
        let clicks = [
            ClickEvent(tMs: 1000, x: 10, y: 10),
            ClickEvent(tMs: 2000, x: 20, y: 20),
        ]
        let regions = ZoomRegionGenerator.generate(clicks: clicks, cursorSamples: [])
        XCTAssertEqual(regions.count, 1)
    }

    func testGenerateKeepsDistantClicksSeparate() {
        let clicks = [
            ClickEvent(tMs: 1000, x: 10, y: 10),
            ClickEvent(tMs: 10_000, x: 20, y: 20),
        ]
        let regions = ZoomRegionGenerator.generate(clicks: clicks, cursorSamples: [])
        XCTAssertEqual(regions.count, 2)
    }

    // MARK: merged-region waypoints / pan

    func testGenerateMergedRegionCarriesWaypointPerClick() {
        // Two clicks 500ms apart → one merged region with 2 waypoints.
        let clicks = [
            ClickEvent(tMs: 1000, x: 100, y: 100),
            ClickEvent(tMs: 1500, x: 800, y: 500),
        ]
        let regions = ZoomRegionGenerator.generate(clicks: clicks, cursorSamples: [])
        XCTAssertEqual(regions.count, 1)
        let waypoints = regions[0].waypoints ?? []
        XCTAssertEqual(waypoints.count, 2)
        XCTAssertEqual(waypoints[0].tMs, 1000)
        XCTAssertEqual(waypoints[0].x, 100)
        XCTAssertEqual(waypoints[0].y, 100)
        XCTAssertEqual(waypoints[1].tMs, 1500)
        XCTAssertEqual(waypoints[1].x, 800)
        XCTAssertEqual(waypoints[1].y, 500)
    }

    func testGenerateDistantClicksProduceTwoRegions() {
        // Clicks far enough apart that `seed.startMs - last.endMs` exceeds
        // chainedZoomPanGapMs (1500ms) — so they do not merge.
        let clicks = [
            ClickEvent(tMs: 1000, x: 100, y: 100),
            ClickEvent(tMs: 5500, x: 800, y: 500),
        ]
        let regions = ZoomRegionGenerator.generate(clicks: clicks, cursorSamples: [])
        XCTAssertEqual(regions.count, 2)
        // Unmerged regions either have nil waypoints or a single-element array;
        // either way, they must not be multi-waypoint pans.
        XCTAssertLessThanOrEqual(regions[0].waypoints?.count ?? 0, 1)
        XCTAssertLessThanOrEqual(regions[1].waypoints?.count ?? 0, 1)
    }

    func testSampleCurveMidpointBetweenWaypointsIsBetween() {
        // Build a merged region manually so the test is independent of the
        // merge heuristic's exact arithmetic.
        let region = ZoomRegion(
            startMs: 0, endMs: 5000,
            focusX: 100, focusY: 100,
            scale: 2.0,
            zoomInDurationMs: 1000,
            zoomOutDurationMs: 1000,
            waypoints: [
                FocusWaypoint(tMs: 1000, x: 100, y: 100),
                FocusWaypoint(tMs: 2000, x: 800, y: 500),
            ]
        )
        // Midpoint between the two waypoints.
        let (_, fx, fy) = ZoomRegionGenerator.sampleCurve(
            atMs: 1500, regions: [region], cursorSamples: [])
        XCTAssertGreaterThan(fx, 100)
        XCTAssertLessThan(fx, 800)
        XCTAssertGreaterThan(fy, 100)
        XCTAssertLessThan(fy, 500)
    }

    func testSampleCurveAtWaypointTimestampReturnsWaypointFocus() {
        let region = ZoomRegion(
            startMs: 0, endMs: 5000,
            focusX: 100, focusY: 100,
            scale: 2.0,
            zoomInDurationMs: 1000,
            zoomOutDurationMs: 1000,
            waypoints: [
                FocusWaypoint(tMs: 1000, x: 100, y: 100),
                FocusWaypoint(tMs: 2000, x: 800, y: 500),
            ]
        )
        let (_, fx0, fy0) = ZoomRegionGenerator.sampleCurve(
            atMs: 1000, regions: [region], cursorSamples: [])
        XCTAssertEqual(fx0, 100, accuracy: 1e-9)
        XCTAssertEqual(fy0, 100, accuracy: 1e-9)

        let (_, fx1, fy1) = ZoomRegionGenerator.sampleCurve(
            atMs: 2000, regions: [region], cursorSamples: [])
        XCTAssertEqual(fx1, 800, accuracy: 1e-9)
        XCTAssertEqual(fy1, 500, accuracy: 1e-9)
    }

    func testSampleCurveBackCompatNoWaypointsUsesFocusXY() {
        let region = ZoomRegion(
            startMs: 0, endMs: 5000,
            focusX: 321, focusY: 654,
            scale: 2.0,
            zoomInDurationMs: 1000,
            zoomOutDurationMs: 1000,
            waypoints: nil
        )
        let (_, fx, fy) = ZoomRegionGenerator.sampleCurve(
            atMs: 2500, regions: [region], cursorSamples: [])
        XCTAssertEqual(fx, 321)
        XCTAssertEqual(fy, 654)
    }
}
