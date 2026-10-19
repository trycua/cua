// SPDX-License-Identifier: MIT
// Cursor silhouette derived from Cua AI, Inc.'s MIT cursor-overlay default theme.
// Product presentation only: input delivery, coordinates and capture are unchanged.

use std::sync::OnceLock;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

#[cfg(test)]
std::thread_local! { static TEST_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if TEST_ENABLED.with(|value| value.get()) {
        return true;
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("COMPUTERUSE_CURSOR_STYLE").as_deref() == Ok("neutral"))
}

fn silhouette() -> Path {
    // Keep the upstream hotspot and silhouette so the artwork never moves the target.
    let points = [
        ((55.0, 30.0), (0.0, 0.0), (-7.0, -2.0)),
        ((43.0, 41.0), (-1.0, -8.0), (0.0, 0.0)),
        ((64.0, 98.0), (0.0, 0.0), (3.0, 8.0)),
        ((77.0, 99.0), (-4.0, 7.0), (0.0, 0.0)),
        ((86.0, 79.0), (0.0, 0.0), (2.0, -4.0)),
        ((95.0, 70.0), (-4.0, 2.0), (0.0, 0.0)),
        ((108.0, 63.0), (0.0, 0.0), (7.0, -4.0)),
        ((107.0, 50.0), (7.0, 3.0), (0.0, 0.0)),
    ];
    let mut path = PathBuilder::new();
    path.move_to(points[0].0 .0, points[0].0 .1);
    for index in 0..points.len() {
        let (from, _, outgoing) = points[index];
        let (to, incoming, _) = points[(index + 1) % points.len()];
        path.cubic_to(
            from.0 + outgoing.0,
            from.1 + outgoing.1,
            to.0 + incoming.0,
            to.1 + incoming.1,
            to.0,
            to.1,
        );
    }
    path.close();
    path.finish().expect("fixed cursor path")
}

pub(crate) fn paint(pm: &mut Pixmap, x: f32, y: f32, heading: f32, scale: f32, alpha: f32) {
    static PATH: OnceLock<Path> = OnceLock::new();
    let path = PATH.get_or_init(silhouette);
    let size = crate::theme::DISPLAY_SIZE * scale / crate::theme::CANVAS_SIZE;
    let transform = Transform::from_translate(-55.0, -30.0)
        .post_scale(size, size)
        .post_rotate((heading - std::f32::consts::FRAC_PI_4).to_degrees())
        .post_translate(x, y);
    let shadow = transform.post_translate(1.3 * scale, 2.6 * scale);
    let mut paint = Paint::default();
    paint.anti_alias = true;
    let mut stroke = Stroke {
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    // Wider than the original glow; layered alpha gives a soft neutral shadow.
    for (width, opacity) in [
        (56.0, 5),
        (46.0, 6),
        (36.0, 8),
        (28.0, 10),
        (21.0, 12),
        (16.0, 16),
        (12.0, 20),
        (8.0, 28),
    ] {
        stroke.width = width;
        paint.set_color_rgba8(0, 0, 0, (opacity as f32 * alpha) as u8);
        pm.stroke_path(path, &paint, &stroke, shadow, None);
    }
    let opacity = (255.0 * alpha.clamp(0.0, 1.0)) as u8;
    paint.set_color_rgba8(32, 33, 36, opacity); // #202124, constant across sessions and actions.
    pm.fill_path(path, &paint, FillRule::Winding, transform, None);
    paint.set_color_rgba8(255, 255, 255, opacity);
    stroke.width = 4.5;
    pm.stroke_path(path, &paint, &stroke, transform, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_cursor_retains_contrast_and_soft_shadow_at_both_scales() {
        for scale in [1.0, 2.0] {
            let mut pm = Pixmap::new(256, 256).unwrap();
            paint(&mut pm, 100.0, 100.0, 0.0, scale, 1.0);
            let pixels = pm.data().chunks_exact(4).collect::<Vec<_>>();
            assert!(pixels.iter().any(|p| *p == [32, 33, 36, 255]));
            assert!(pixels.iter().any(|p| *p == [255, 255, 255, 255]));
            assert!(pixels
                .iter()
                .any(|p| p[0..3] == [0, 0, 0] && p[3] > 0 && p[3] < 180));
        }
    }

    #[test]
    fn neutral_cursor_has_no_session_tint_or_action_badges() {
        TEST_ENABLED.with(|value| value.set(true));
        let mut core = crate::RenderStateCore::new(crate::CursorConfig::default());
        core.pos = (100.0, 100.0);
        let baseline = crate::render_frame(&core, 256, 256, 0.0, 0.0, None, 2.0);
        for session in ["agent-1", "agent-2", "agent-9"] {
            for action in crate::CursorAction::ALL {
                core.cfg.cursor_id = session.into();
                core.session_label = Some(session.into());
                core.session_badge_secs = 0.0;
                core.badge_modifiers = Some((
                    Some(crate::DeliveryModifier::Background),
                    Some(crate::TargetModifier::Ax),
                ));
                core.visual.begin(
                    action,
                    Some(crate::DeliveryModifier::Background),
                    Some(crate::TargetModifier::Ax),
                );
                let frame = crate::render_frame(&core, 256, 256, 0.0, 0.0, None, 2.0);
                assert_eq!(
                    baseline.data(),
                    frame.data(),
                    "session={session}, action={action:?}"
                );
            }
        }
        TEST_ENABLED.with(|value| value.set(false));
    }
}
