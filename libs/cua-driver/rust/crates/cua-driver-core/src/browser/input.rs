// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Ref-to-point resolution shared by the browser input tools.
//!
//! A trusted CDP mouse event lands on whatever the page renders at the point,
//! not on the ref it was computed from. [`trusted_ref_point`] proves the point
//! is inside the frame's visible viewport and hit-tests to the ref (or one of
//! its descendants) before any trusted event is sent, so a partly off-screen
//! or covered element is refused instead of silently missed.

use serde_json::{json, Value};

use super::cdp_ws::CdpConnection;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};

/// Walks from the hit node up through parents and shadow hosts. `this` is the
/// ref's node; nodes from another frame's JavaScript world fail to pass in,
/// which the caller treats as a miss.
const REF_HIT_ANCESTRY_CHECK: &str = "function(node) { /* cua_trusted_ref_hit */ \
    while (node) { \
        if (node === this) return true; \
        const root = node.getRootNode ? node.getRootNode() : null; \
        node = node.parentNode || (root && root.host) || null; \
    } \
    return false; \
}";

/// Why a trusted point did not provably hit its ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefHitMiss {
    /// The point is outside the frame's visible viewport after scrolling.
    OutsideViewport,
    /// Nothing, or another element, is rendered at the point.
    Covered,
}

impl RefHitMiss {
    fn as_str(self) -> &'static str {
        match self {
            Self::OutsideViewport => "outside_viewport",
            Self::Covered => "covered",
        }
    }
}

/// Scroll the ref into view (best effort) and return its content-box center
/// in the frame's viewport CSS pixels.
pub(crate) async fn ref_point(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<(f64, f64), BrowserRefusal> {
    // Older Chromium lacks this method; the trusted hit test below catches a
    // ref it leaves off screen.
    let _ = conn
        .call(
            Some(cdp_session),
            "DOM.scrollIntoViewIfNeeded",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
    let model = conn
        .call(
            Some(cdp_session),
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await
        .map_err(|_| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserRefStale,
                "the ref's node has no layout box; it left the DOM or is hidden",
            )
        })?;
    quad_center(&model).ok_or_else(|| {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserRefStale,
            "the ref's node returned an unusable layout box",
        )
    })
}

/// [`ref_point`], then prove a trusted mouse event at that point reaches the
/// ref. The returned point is the one that was hit-tested; dispatch there.
pub(crate) async fn trusted_ref_point(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<(f64, f64), BrowserRefusal> {
    let center = ref_point(conn, cdp_session, backend_node_id).await?;
    let point = (center.0.floor(), center.1.floor());
    match ref_hit(conn, cdp_session, backend_node_id, point).await? {
        None => Ok(point),
        Some(miss) => Err(miss_refusal(miss, point)),
    }
}

async fn ref_hit(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
    point: (f64, f64),
) -> Result<Option<RefHitMiss>, BrowserRefusal> {
    let metrics = conn
        .call(Some(cdp_session), "Page.getLayoutMetrics", json!({}))
        .await
        .map_err(|error| check_failed(format!("Page.getLayoutMetrics failed: {error}")))?;
    let viewport = FrameViewport::parse(&metrics).ok_or_else(|| {
        check_failed("Page.getLayoutMetrics returned no usable viewport".to_owned())
    })?;
    if !viewport.contains(point) {
        return Ok(Some(RefHitMiss::OutsideViewport));
    }

    // Chromium hit-tests this method in document coordinates: at a scrolled
    // offset, viewport coordinates report "No node found".
    let (doc_x, doc_y) = viewport.to_document(point);
    let hit = match conn
        .call(
            Some(cdp_session),
            "DOM.getNodeForLocation",
            json!({
                "x": doc_x as i64,
                "y": doc_y as i64,
                "includeUserAgentShadowDOM": false,
                "ignorePointerEventsNone": false,
            }),
        )
        .await
    {
        Ok(hit) => hit,
        // Chromium answers an empty point with an error, not an empty result.
        Err(_) => return Ok(Some(RefHitMiss::Covered)),
    };
    let Some(hit_backend) = hit.get("backendNodeId").and_then(Value::as_i64) else {
        return Ok(Some(RefHitMiss::Covered));
    };
    if hit_backend == backend_node_id {
        return Ok(None);
    }

    let (Some(target), Some(hit_object)) = (
        resolve_object(conn, cdp_session, backend_node_id).await,
        resolve_object(conn, cdp_session, hit_backend).await,
    ) else {
        return Ok(Some(RefHitMiss::Covered));
    };
    let contained = conn
        .call(
            Some(cdp_session),
            "Runtime.callFunctionOn",
            json!({
                "objectId": target,
                "functionDeclaration": REF_HIT_ANCESTRY_CHECK,
                "arguments": [{ "objectId": hit_object }],
                "returnByValue": true,
            }),
        )
        .await
        .ok()
        .and_then(|reply| reply.pointer("/result/value").and_then(Value::as_bool))
        == Some(true);
    Ok((!contained).then_some(RefHitMiss::Covered))
}

/// The frame's visible region, in the viewport CSS pixels `DOM.getBoxModel`
/// reports, plus the layout viewport's document scroll offset.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameViewport {
    visible_x: f64,
    visible_y: f64,
    width: f64,
    height: f64,
    scroll_x: f64,
    scroll_y: f64,
}

impl FrameViewport {
    fn parse(metrics: &Value) -> Option<Self> {
        let number = |path: &str| {
            metrics
                .pointer(path)
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
        };
        let viewport = Self {
            // Pinch zoom shifts the visible region inside the layout viewport.
            visible_x: number("/cssVisualViewport/offsetX").unwrap_or(0.0),
            visible_y: number("/cssVisualViewport/offsetY").unwrap_or(0.0),
            width: number("/cssVisualViewport/clientWidth")?,
            height: number("/cssVisualViewport/clientHeight")?,
            scroll_x: number("/cssLayoutViewport/pageX")?,
            scroll_y: number("/cssLayoutViewport/pageY")?,
        };
        (viewport.width > 0.0 && viewport.height > 0.0).then_some(viewport)
    }

    fn contains(self, (x, y): (f64, f64)) -> bool {
        x.is_finite()
            && y.is_finite()
            && x >= self.visible_x
            && y >= self.visible_y
            && x < self.visible_x + self.width
            && y < self.visible_y + self.height
    }

    fn to_document(self, (x, y): (f64, f64)) -> (f64, f64) {
        (x + self.scroll_x, y + self.scroll_y)
    }
}

fn miss_refusal(miss: RefHitMiss, (x, y): (f64, f64)) -> BrowserRefusal {
    let reason = match miss {
        RefHitMiss::OutsideViewport => "is outside the visible viewport",
        RefHitMiss::Covered => "does not hit the ref's element (another element covers it)",
    };
    BrowserRefusal::new(
        BrowserRefusalCode::BrowserInputTrustUnavailable,
        format!(
            "the ref's trusted input point ({x:.0}, {y:.0}) {reason}, so a trusted event would \
             land elsewhere; nothing was sent. Refresh page state, or use \
             input_route=\"dom_event\" to dispatch on the element itself"
        ),
    )
    .with_detail(json!({
        "requested_route": "trusted",
        "hit_test": miss.as_str(),
        "x": x,
        "y": y,
        "alternative_route": "dom_event",
        "alternative_requires_ref": true,
        "trusted_delivery_attempted": false,
    }))
}

fn check_failed(message: String) -> BrowserRefusal {
    BrowserRefusal::new(
        BrowserRefusalCode::BrowserInputTrustUnavailable,
        format!("the trusted input hit test could not run ({message}); nothing was sent"),
    )
    .with_detail(json!({ "trusted_delivery_attempted": false }))
}

async fn resolve_object(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Option<String> {
    conn.call(
        Some(cdp_session),
        "DOM.resolveNode",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await
    .ok()?
    .pointer("/object/objectId")?
    .as_str()
    .map(str::to_owned)
}

/// Center of the content quad from a `DOM.getBoxModel` result.
pub(crate) fn quad_center(box_model: &Value) -> Option<(f64, f64)> {
    let quad = box_model.get("model")?.get("content")?.as_array()?;
    let values = quad
        .iter()
        .take(8)
        .map(Value::as_f64)
        .collect::<Option<Vec<_>>>()?;
    if values.len() < 8 {
        return None;
    }
    Some((
        (values[0] + values[2] + values[4] + values[6]) / 4.0,
        (values[1] + values[3] + values[5] + values[7]) / 4.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_center_rejects_malformed_models() {
        assert_eq!(
            quad_center(&json!({ "model": { "content": [0, 0, 10, 0, 10, 20, 0, 20] } })),
            Some((5.0, 10.0))
        );
        assert_eq!(
            quad_center(&json!({ "model": { "content": [0, 0] } })),
            None
        );
        assert_eq!(
            quad_center(&json!({ "model": { "content": [0, 0, "x", 0, 10, 20, 0, 20] } })),
            None
        );
    }

    fn metrics(offset_y: f64, height: f64, page_y: f64) -> Value {
        json!({
            "cssVisualViewport": {
                "offsetX": 0.0, "offsetY": offset_y,
                "clientWidth": 800.0, "clientHeight": height,
            },
            "cssLayoutViewport": { "pageX": 0.0, "pageY": page_y },
        })
    }

    #[test]
    fn viewport_excludes_the_far_edges() {
        // The issue's case: a 725px viewport, a row whose center sits at 685
        // is inside, but a point at the bottom edge is not.
        let viewport = FrameViewport::parse(&metrics(0.0, 725.0, 0.0)).unwrap();
        assert!(viewport.contains((442.0, 685.0)));
        assert!(!viewport.contains((442.0, 725.0)));
        assert!(!viewport.contains((800.0, 10.0)));
        assert!(!viewport.contains((-1.0, 10.0)));
        assert!(!viewport.contains((f64::NAN, 10.0)));
    }

    #[test]
    fn pinch_zoom_moves_the_visible_region() {
        let viewport = FrameViewport::parse(&metrics(300.0, 319.0, 0.0)).unwrap();
        assert!(!viewport.contains((10.0, 100.0)));
        assert!(viewport.contains((10.0, 400.0)));
        assert!(!viewport.contains((10.0, 619.0)));
    }

    #[test]
    fn hit_test_points_are_document_coordinates() {
        let viewport = FrameViewport::parse(&metrics(0.0, 600.0, 1211.0)).unwrap();
        assert_eq!(viewport.to_document((100.0, 319.0)), (100.0, 1530.0));
    }

    #[test]
    fn viewport_metrics_without_a_size_are_unusable() {
        assert_eq!(FrameViewport::parse(&json!({})), None);
        assert_eq!(FrameViewport::parse(&metrics(0.0, 0.0, 0.0)), None);
    }
}
