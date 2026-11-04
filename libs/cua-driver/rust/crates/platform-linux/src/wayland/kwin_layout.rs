//! KWin's desktop layout in logical pixels.
//!
//! Portal/libei pointer input takes logical desktop coordinates on KWin, while
//! its screenshots are physical pixels. On a scaled output those two frames
//! differ, so a coordinate read off the screenshot lands in the wrong place.
//! `compositor_logical_frame` reads this layout so both share one frame.

use wayland_client::{
    protocol::{wl_output, wl_registry},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};

/// One output's logical rectangle plus its current mode, as announced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OutputRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub mode_width: i32,
    pub mode_height: i32,
}

/// What the compositor reported: whether it is KWin, and its outputs.
#[derive(Debug, Default)]
struct Report {
    is_kwin: bool,
    manager: Option<ZxdgOutputManagerV1>,
    outputs: Vec<(wl_output::WlOutput, Option<ZxdgOutputV1>, OutputRect)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Report {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match interface.as_str() {
            kwin_output if announces_kwin(kwin_output) => state.is_kwin = true,
            "wl_output" => {
                let index = state.outputs.len();
                let output =
                    registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, index);
                state.outputs.push((output, None, OutputRect::default()));
            }
            "zxdg_output_manager_v1" => {
                state.manager =
                    Some(registry.bind::<ZxdgOutputManagerV1, _, _>(name, version.min(3), qh, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Report {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Mode {
            flags,
            width,
            height,
            ..
        } = event
        {
            let current = matches!(
                flags,
                wayland_client::WEnum::Value(f) if f.contains(wl_output::Mode::Current)
            );
            if current {
                if let Some((_, _, rect)) = state.outputs.get_mut(*index) {
                    rect.mode_width = width;
                    rect.mode_height = height;
                }
            }
        }
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for Report {
    fn event(
        _: &mut Self,
        _: &ZxdgOutputManagerV1,
        _: <ZxdgOutputManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgOutputV1, usize> for Report {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, _, rect)) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                rect.x = x;
                rect.y = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                rect.width = width;
                rect.height = height;
            }
            _ => {}
        }
    }
}

/// Whether a global is one of KWin's output globals. Matched by prefix because
/// the names change between releases: 6.8 replaced `kde_output_device_v2` with
/// `kde_output_device_registry_v2`, and an exact match then looks like another
/// compositor and silently loses the logical frame.
fn announces_kwin(interface: &str) -> bool {
    interface.starts_with("kde_output_")
}

/// Bounding box of every output, which must start at the desktop origin
/// because libei coordinates are absolute desktop positions.
pub(crate) fn bounding_frame(outputs: &[OutputRect]) -> anyhow::Result<(u32, u32)> {
    anyhow::ensure!(!outputs.is_empty(), "KWin announced no output layout");
    let min_x = outputs.iter().map(|o| o.x).min().unwrap_or(0);
    let min_y = outputs.iter().map(|o| o.y).min().unwrap_or(0);
    anyhow::ensure!(
        min_x == 0 && min_y == 0,
        "KWin desktop layout starts at ({min_x}, {min_y}), not the origin; \
         refusing a desktop frame that would offset every coordinate"
    );
    let max_x = outputs.iter().map(|o| o.x + o.width).max().unwrap_or(0);
    let max_y = outputs.iter().map(|o| o.y + o.height).max().unwrap_or(0);
    anyhow::ensure!(max_x > 0 && max_y > 0, "KWin desktop layout is empty");
    Ok((max_x as u32, max_y as u32))
}

/// Physical pixels per logical pixel. Compared by area, so a rotated output
/// (whose mode is reported before the rotation) cannot skew the result.
pub(crate) fn output_scale(output: &OutputRect) -> f64 {
    let logical = f64::from(output.width) * f64::from(output.height);
    let physical = f64::from(output.mode_width) * f64::from(output.mode_height);
    if logical <= 0.0 || physical <= 0.0 {
        return 1.0;
    }
    ((physical / logical).sqrt() * 100.0).round() / 100.0
}

/// `Ok(None)` on a compositor that is not KWin.
pub(crate) fn query() -> anyhow::Result<Option<Vec<OutputRect>>> {
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue::<Report>();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut report = Report::default();
    queue.roundtrip(&mut report)?;
    if !report.is_kwin {
        return Ok(None);
    }
    let manager = report
        .manager
        .clone()
        .ok_or_else(|| anyhow::anyhow!("KWin did not announce zxdg_output_manager_v1"))?;
    for index in 0..report.outputs.len() {
        let xdg = manager.get_xdg_output(&report.outputs[index].0, &qh, index);
        report.outputs[index].1 = Some(xdg);
    }
    queue.roundtrip(&mut report)?;
    queue.roundtrip(&mut report)?;
    Ok(Some(report.outputs.iter().map(|o| o.2).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(x: i32, y: i32, width: i32, height: i32, mode: (i32, i32)) -> OutputRect {
        OutputRect {
            x,
            y,
            width,
            height,
            mode_width: mode.0,
            mode_height: mode.1,
        }
    }

    // A portrait 1080p at 1.25 beside a landscape 1440p at 1.25.
    fn two_monitors() -> [OutputRect; 2] {
        [
            output(0, 0, 864, 1536, (1080, 1920)),
            output(864, 0, 2048, 1152, (2560, 1440)),
        ]
    }

    #[test]
    fn frame_is_the_bounding_box_of_every_output_in_logical_pixels() {
        assert_eq!(bounding_frame(&two_monitors()).unwrap(), (2912, 1536));
    }

    #[test]
    fn a_layout_away_from_the_origin_is_refused_rather_than_offset() {
        let err = bounding_frame(&[output(100, 0, 800, 600, (800, 600))]).unwrap_err();
        assert!(err.to_string().contains("origin"), "{err}");
    }

    #[test]
    fn every_kwin_release_tested_is_recognised_by_its_output_globals() {
        // Announced by KWin 6.3.6 and by 6.8.2, which replaced the first name.
        assert!(announces_kwin("kde_output_device_v2"));
        assert!(announces_kwin("kde_output_device_registry_v2"));
    }

    #[test]
    fn globals_other_compositors_also_announce_are_not_taken_for_kwin() {
        for interface in [
            "wl_output",
            "zxdg_output_manager_v1",
            "org_kde_kwin_idle",
            "org_kde_kwin_server_decoration_manager",
            "zwlr_output_manager_v1",
        ] {
            assert!(!announces_kwin(interface), "{interface}");
        }
    }

    #[test]
    fn no_outputs_is_an_error() {
        assert!(bounding_frame(&[]).is_err());
    }

    #[test]
    fn scale_comes_from_area_so_rotation_cannot_skew_it() {
        assert_eq!(output_scale(&output(0, 0, 864, 1536, (1920, 1080))), 1.25);
        assert_eq!(output_scale(&output(0, 0, 2048, 1152, (2560, 1440))), 1.25);
        assert_eq!(output_scale(&output(0, 0, 0, 0, (0, 0))), 1.0);
    }
}
