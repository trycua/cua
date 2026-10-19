//! Window capture: the output region a niri window occupies, and its capture.

use super::*;

// ── Window capture ───────────────────────────────────────────────────────────

/// Where a window is in a single-output display capture, and the output
/// geometry that mapping depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturePlan {
    /// Capture pixels, after the capture is oriented to the logical layout.
    pub pixels: Rect,
    /// The window's logical size, the size of the returned window image.
    pub width: u32,
    pub height: u32,
    /// The output the capture must come from, its logical rectangle and
    /// scale, and the oriented capture's size in pixels.
    pub output: String,
    pub area: Rect,
    pub scale: f64,
    pub transform: u32,
    pub image: (u32, u32),
}

/// An oriented display capture, the output it copied and the transform the
/// buffer was oriented with.
pub struct DisplayCapture {
    pub png: Vec<u8>,
    pub output: String,
    pub transform: u32,
    pub size: (u32, u32),
}

pub(super) fn capture_plan_in(
    scene: &Scene,
    id: u64,
    pid: Option<u32>,
) -> Result<CapturePlan, &'static str> {
    let window = scene
        .window(id)
        .map_err(|_| "niri reports no such window; refresh list_windows")?;
    if pid.is_some_and(|pid| window.pid != Some(pid)) {
        return Err("niri reports the window belongs to a different process");
    }
    let placed = window
        .placement
        .as_ref()
        .map_err(|unplaced| unplaced.reason())?;
    let (output, logical) = scene
        .single_output()
        .map_err(|_| "niri window capture supports a single output, and this layout has several")?;
    if placed.output != output.name {
        return Err("the window is not on the captured output");
    }
    if !placed.rect.inside(&placed.area) {
        return Err("the window extends past its output's edge");
    }
    if placed.overlapped() {
        return Err("another floating window overlaps it, and niri reports no stacking order");
    }
    let mode = output
        .current_mode
        .and_then(|index| output.modes.get(index))
        .ok_or("niri reports no current mode for the output")?;
    let transform =
        transform_code(&logical.transform).map_err(|_| "unknown niri output transform")?;
    let scale = logical.scale;
    // Round only in pixel space: the fractional origin is what niri drew.
    let pixels = Rect::from_f64(
        (placed.origin.0 - f64::from(logical.x)) * scale,
        (placed.origin.1 - f64::from(logical.y)) * scale,
        f64::from(placed.rect.width) * scale,
        f64::from(placed.rect.height) * scale,
    )
    .ok_or("invalid niri window geometry")?;
    Ok(CapturePlan {
        pixels,
        width: placed.rect.width,
        height: placed.rect.height,
        output: output.name.clone(),
        area: placed.area,
        scale,
        transform,
        image: super::super::logical_output_size((mode.width, mode.height), transform),
    })
}

/// Capture one floating window by cropping a settled display capture. The
/// layout must stay quiet through the capture, and the window and its
/// output unchanged. Errors are refusal reasons for
/// `surface_identity_unproven`.
pub fn capture_window(
    id: u64,
    pid: Option<u32>,
    capture_display: impl FnOnce() -> Result<DisplayCapture>,
) -> Result<(Vec<u8>, CapturePlan), String> {
    let mut ipc = ipc();
    capture_window_in(
        || ipc.scene(),
        settled,
        |mark| HISTORY.changed_since(mark, Expected::default()),
        id,
        pid,
        capture_display,
    )
}

fn capture_window_in(
    mut scene: impl FnMut() -> Result<Scene>,
    settle: impl FnOnce() -> Result<Mark>,
    changed_since: impl FnOnce(Mark) -> Result<bool>,
    id: u64,
    pid: Option<u32>,
    capture_display: impl FnOnce() -> Result<DisplayCapture>,
) -> Result<(Vec<u8>, CapturePlan), String> {
    let mut plan_now = || -> Result<CapturePlan, String> {
        Ok(capture_plan_in(
            &scene().map_err(|e| format!("{e:#}"))?,
            id,
            pid,
        )?)
    };
    let before = plan_now()?;
    let mark = settle().map_err(|e| format!("{e:#}"))?;
    let plan = plan_now()?;
    if plan != before {
        return Err("the window moved while niri was settling".into());
    }
    let capture = capture_display().map_err(|e| format!("{e:#}"))?;
    if changed_since(mark).map_err(|e| format!("{e:#}"))? {
        return Err("the niri layout changed during capture".into());
    }
    // niri has no output-change event: compare the output itself.
    if plan_now()? != plan {
        return Err("the niri window or its output changed during capture".into());
    }
    if capture.output != plan.output {
        return Err(format!(
            "the display capture copied output {:?}, not the window's output {:?}",
            capture.output, plan.output
        ));
    }
    if capture.transform != plan.transform {
        return Err(format!(
            "the capture was oriented for output transform {}, but the window's output has {}",
            capture.transform, plan.transform
        ));
    }
    if capture.size != plan.image {
        return Err(format!(
            "the capture is {}x{} pixels, but niri reports {}x{} for output {:?}",
            capture.size.0, capture.size.1, plan.image.0, plan.image.1, plan.output
        ));
    }
    Ok((capture.png, plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Capture plan ────────────────────────────────────────────────────────

    #[test]
    fn crop_maps_global_logical_geometry_to_output_pixels() {
        let scene = scene(
            vec![floating(7, 1, (100.0, 60.0), (300, 200))],
            vec![workspace(1, "DP-2", true, None)],
            vec![output("DP-2", 1000, 40, (1920, 1080), 1.5, "Normal")],
            false,
        );
        assert_eq!(
            capture_plan_in(&scene, 7, Some(107)),
            Ok(CapturePlan {
                pixels: Rect {
                    x: 150,
                    y: 90,
                    width: 450,
                    height: 300
                },
                width: 300,
                height: 200,
                output: "DP-2".into(),
                area: Rect {
                    x: 1000,
                    y: 40,
                    width: 1280,
                    height: 720
                },
                scale: 1.5,
                transform: 0,
                image: (1920, 1080),
            })
        );
    }

    #[test]
    fn a_one_physical_pixel_overlap_at_a_fractional_scale_refuses() {
        // At 1.5 the target spans physical x 152..602 and the other tile
        // starts at physical 601. Rounded to logical pixels (101 and 401)
        // they would only touch.
        let scene = one_output(vec![
            floating(7, 1, (152.0 / 1.5, 60.0), (300, 200)),
            floating(8, 1, (601.0 / 1.5, 60.0), (100, 100)),
        ]);
        assert_eq!(
            capture_plan_in(&scene, 7, None),
            Err("another floating window overlaps it, and niri reports no stacking order")
        );
        let placed = placement(&scene, 7).unwrap();
        assert!(placed.hits(400, 100));
        assert!(
            !placed.hits(401, 100),
            "x=401 is also on the tile from 400.667"
        );
    }

    #[test]
    fn crop_keeps_a_pixel_aligned_fractional_origin() {
        // At scale 1.5 niri places a window on physical x=151, which IPC
        // reports as logical 100.667; rounding it first would crop from 152.
        let scene = one_output(vec![floating(7, 1, (151.0 / 1.5, 60.0), (300, 200))]);
        let plan = capture_plan_in(&scene, 7, None).unwrap();
        assert_eq!((plan.pixels.x, plan.pixels.width), (151, 450));
        let placed = placement(&scene, 7).unwrap();
        assert!(!placed.covers(100, 100), "x=100 is left of 100.667");
        assert!(placed.covers(101, 100));
    }

    fn settled_now() -> Result<Mark> {
        Ok(Mark {
            generation: 1,
            at: 0,
        })
    }

    #[test]
    fn capture_refuses_an_output_change_niri_sends_no_event_for() {
        let window = || floating(7, 1, (100.0, 60.0), (300, 200));
        let mut calls = 0;
        let error = capture_window_in(
            || {
                calls += 1;
                // The scale changes during the capture; the layout does not.
                let scale = if calls <= 2 { 1.5 } else { 1.0 };
                Ok(scene(
                    vec![window()],
                    vec![workspace(1, "winit", true, None)],
                    vec![output("winit", 0, 0, (1280, 720), scale, "Normal")],
                    false,
                ))
            },
            settled_now,
            unchanged,
            7,
            None,
            || display("winit", (1280, 720)),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "the niri window or its output changed during capture"
        );
    }

    #[test]
    fn capture_refuses_a_transform_change_that_keeps_every_size() {
        // one_output's winit output is Flipped180 (6); Normal (0) keeps the
        // name, area, scale and image size.
        let one = || Ok(one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]));
        assert_eq!(
            capture_window_in(one, settled_now, unchanged, 7, None, || oriented(
                "winit",
                0,
                (1280, 720)
            ))
            .unwrap_err(),
            "the capture was oriented for output transform 0, but the window's output has 6"
        );
        let mut calls = 0;
        let error = capture_window_in(
            || {
                calls += 1;
                let transform = if calls <= 2 { "Flipped180" } else { "Normal" };
                Ok(scene(
                    vec![floating(7, 1, (100.0, 60.0), (300, 200))],
                    vec![workspace(1, "winit", true, None)],
                    vec![output("winit", 0, 0, (1280, 720), 1.5, transform)],
                    false,
                ))
            },
            settled_now,
            unchanged,
            7,
            None,
            || display("winit", (1280, 720)),
        )
        .unwrap_err();
        assert_eq!(
            error,
            "the niri window or its output changed during capture"
        );
    }

    #[test]
    fn capture_refuses_another_output_or_size() {
        let one = || Ok(one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]));
        assert_eq!(
            capture_window_in(one, settled_now, unchanged, 7, None, || display(
                "DP-1",
                (1280, 720)
            ))
            .unwrap_err(),
            "the display capture copied output \"DP-1\", not the window's output \"winit\""
        );
        assert_eq!(
            capture_window_in(one, settled_now, unchanged, 7, None, || display(
                "winit",
                (1920, 1080)
            ))
            .unwrap_err(),
            "the capture is 1920x1080 pixels, but niri reports 1280x720 for output \"winit\""
        );
        assert!(
            capture_window_in(one, settled_now, unchanged, 7, None, || display(
                "winit",
                (1280, 720)
            ))
            .is_ok()
        );
    }

    #[test]
    fn crop_refuses_other_outputs_overlap_and_other_pids() {
        let two_outputs = scene(
            vec![floating(7, 2, (100.0, 60.0), (300, 200))],
            vec![
                workspace(1, "DP-1", true, None),
                workspace(2, "DP-2", true, None),
            ],
            vec![
                output("DP-1", 0, 0, (1920, 1080), 1.0, "Normal"),
                output("DP-2", 1920, 0, (1920, 1080), 1.0, "Normal"),
            ],
            false,
        );
        assert_eq!(
            capture_plan_in(&two_outputs, 7, None),
            Err("niri window capture supports a single output, and this layout has several")
        );
        let overlapped = one_output(vec![
            floating(7, 1, (100.0, 60.0), (300, 200)),
            floating(8, 1, (390.0, 250.0), (100, 100)),
        ]);
        assert_eq!(
            capture_plan_in(&overlapped, 7, None),
            Err("another floating window overlaps it, and niri reports no stacking order")
        );
        let window = one_output(vec![floating(7, 1, (100.0, 60.0), (300, 200))]);
        assert_eq!(
            capture_plan_in(&window, 7, Some(1)),
            Err("niri reports the window belongs to a different process")
        );
        let past_edge = one_output(vec![floating(7, 1, (700.0, 60.0), (300, 200))]);
        assert_eq!(
            capture_plan_in(&past_edge, 7, None),
            Err("the window extends past its output's edge")
        );
    }
}
