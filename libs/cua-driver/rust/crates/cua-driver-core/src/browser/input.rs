//! Shared trusted browser-input policy and ref-point proof.

use serde_json::{json, Value};

use super::cdp_ws::CdpConnection;
use super::platform::BrowserPlatform;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::store::TargetRecord;
use super::types::BrowserInputAction;

pub(crate) fn trusted_input_refusal(
    platform: &dyn BrowserPlatform,
    record: &TargetRecord,
    action: BrowserInputAction,
) -> Option<BrowserRefusal> {
    if record.cdp_window_id.is_none() {
        return None;
    }
    let limitation =
        platform.standalone_trusted_input_background_limitation(record.product_kind, action)?;
    Some(
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserInputTrustUnavailable,
            format!(
                "{limitation}; use input_route=\"dom_event\" with refs to explicitly request synthetic full-background delivery"
            ),
        )
        .with_detail(json!({
            "requested_route": "trusted",
            "action": action.as_str(),
            "limitation": limitation,
            "alternative_route": "dom_event",
            "alternative_requires_ref": true,
            "trusted_delivery_attempted": false,
        })),
    )
}

pub(crate) async fn ref_point(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<(f64, f64), BrowserRefusal> {
    conn.call(
        Some(cdp_session),
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await
    .map_err(|error| {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserActionUnavailable,
            format!("the ref could not be scrolled into view: {error}"),
        )
    })?;
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
                "the ref's node has no live layout box",
            )
        })?;
    quad_center(&model).ok_or_else(|| {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserRefStale,
            "the ref's node returned an unusable layout box",
        )
    })
}

pub(crate) async fn trusted_ref_point(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<(f64, f64), BrowserRefusal> {
    let point = integer_css_point(ref_point(conn, cdp_session, backend_node_id).await?);
    prove_ref_hit(conn, cdp_session, backend_node_id, point).await?;
    Ok(point)
}

async fn prove_ref_hit(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
    point: (f64, f64),
) -> Result<(), BrowserRefusal> {
    let wrong_target = |reason: &'static str| {
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserWrongTargetRefused,
            "the trusted input point is outside the viewport or does not hit the referenced node",
        )
        .with_detail(json!({
            "hit_test": reason,
            "x": point.0,
            "y": point.1,
            "trusted_delivery_attempted": false,
        }))
    };
    let metrics = conn
        .call(Some(cdp_session), "Page.getLayoutMetrics", json!({}))
        .await
        .map_err(|error| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("the trusted input viewport check could not run: {error}"),
            )
        })?;
    let viewport = metrics.get("cssVisualViewport");
    let width = viewport
        .and_then(|value| value.get("clientWidth"))
        .and_then(Value::as_f64);
    let height = viewport
        .and_then(|value| value.get("clientHeight"))
        .and_then(Value::as_f64);
    if !point.0.is_finite()
        || !point.1.is_finite()
        || width.is_none_or(|value| !value.is_finite() || value <= 0.0 || point.0 >= value)
        || height.is_none_or(|value| !value.is_finite() || value <= 0.0 || point.1 >= value)
        || point.0 < 0.0
        || point.1 < 0.0
    {
        return Err(wrong_target("outside_viewport"));
    }

    let hit = conn
        .call(
            Some(cdp_session),
            "DOM.getNodeForLocation",
            json!({
                "x": point.0,
                "y": point.1,
                "includeUserAgentShadowDOM": true,
                "ignorePointerEventsNone": false,
            }),
        )
        .await
        .map_err(|_| wrong_target("no_hit"))?;
    let hit_backend = hit
        .get("backendNodeId")
        .and_then(Value::as_i64)
        .ok_or_else(|| wrong_target("no_hit"))?;
    if hit_backend == backend_node_id {
        return Ok(());
    }

    let target_object = resolve_object(conn, cdp_session, backend_node_id)
        .await
        .ok_or_else(|| wrong_target("target_unresolved"))?;
    let hit_object = resolve_object(conn, cdp_session, hit_backend)
        .await
        .ok_or_else(|| wrong_target("hit_unresolved"))?;
    let checked = conn
        .call(
            Some(cdp_session),
            "Runtime.callFunctionOn",
            json!({
                "objectId": target_object,
                "functionDeclaration": "function(node) { /* cua_trusted_ref_hit_test */ while (node) { if (node === this) return true; const root=node.getRootNode ? node.getRootNode() : null; node=node.parentNode || (root && root.host) || null; } return false; }",
                "arguments": [{ "objectId": hit_object }],
                "returnByValue": true,
            }),
        )
        .await
        .map_err(|error| {
            BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("the trusted input ancestry check could not run: {error}"),
            )
        })?;
    if checked.pointer("/result/value").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(wrong_target("covered"))
    }
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

fn integer_css_point(point: (f64, f64)) -> (f64, f64) {
    (point.0.floor(), point.1.floor())
}

fn quad_center(box_model: &Value) -> Option<(f64, f64)> {
    let quad = box_model.get("model")?.get("content")?.as_array()?;
    if quad.len() < 8 {
        return None;
    }
    let values = quad
        .iter()
        .take(8)
        .map(Value::as_f64)
        .collect::<Option<Vec<_>>>()?;
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
    }

    #[test]
    fn trusted_points_use_integer_css_coordinates() {
        assert_eq!(integer_css_point((10.75, 20.25)), (10.0, 20.0));
    }
}
