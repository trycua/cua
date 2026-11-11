// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! KWin's desktop layout in logical pixels.
//!
//! Portal/libei pointer input takes logical desktop coordinates on KWin, while
//! its screenshots are physical pixels. On a scaled output those two frames
//! differ, so a coordinate read off the screenshot lands in the wrong place.
//! This reads the layout into the [`DesktopFrame`] Hyprland and niri publish,
//! so capture and input share one frame, whatever the layout's origin.

use anyhow::Context as _;
use wayland_client::{
    protocol::{wl_output, wl_registry},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};

use super::hyprland::{bounding_box, DesktopFrame, FrameOutput};

/// One output's logical rectangle plus its current mode, as announced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct OutputRect {
    name: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    mode_width: i32,
    mode_height: i32,
    /// Turned by 90 or 270 degrees, flipped or not.
    quarter_turned: bool,
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
        let Some((_, _, rect)) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                let current = matches!(
                    flags,
                    wayland_client::WEnum::Value(f) if f.contains(wl_output::Mode::Current)
                );
                if current {
                    rect.mode_width = width;
                    rect.mode_height = height;
                }
            }
            wl_output::Event::Geometry { transform, .. } => {
                use wl_output::Transform as Turn;
                rect.quarter_turned = matches!(
                    transform,
                    wayland_client::WEnum::Value(
                        Turn::_90 | Turn::_270 | Turn::Flipped90 | Turn::Flipped270
                    )
                );
            }
            _ => {}
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
            zxdg_output_v1::Event::Name { name } => rect.name = name,
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

/// The desktop frame of these outputs: their logical bounding box, with frame
/// pixel (0, 0) at its top-left layout point, and the largest output scale,
/// which is the scale KWin renders a whole-desktop capture at.
fn frame_of(outputs: &[OutputRect]) -> anyhow::Result<DesktopFrame> {
    anyhow::ensure!(!outputs.is_empty(), NO_OUTPUTS);
    let frame_outputs = outputs
        .iter()
        .map(|output| {
            let (width, height) = (u32::try_from(output.width), u32::try_from(output.height));
            match (width, height) {
                (Ok(width), Ok(height)) if width > 0 && height > 0 => Ok(FrameOutput {
                    name: output.name.clone(),
                    x: output.x,
                    y: output.y,
                    width,
                    height,
                }),
                _ => anyhow::bail!(
                    "KWin output {:?} has no logical size: {}x{}",
                    output.name,
                    output.width,
                    output.height
                ),
            }
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let (x, y, width, height) = bounding_box(&frame_outputs)?;
    let scale = outputs.iter().map(output_scale).fold(f64::MIN, f64::max);
    Ok(DesktopFrame {
        x,
        y,
        width,
        height,
        scale,
        outputs: frame_outputs,
    })
}

const NO_OUTPUTS: &str = "KWin announced no output layout";

/// Physical pixels per logical pixel. Compared by area, so a rotated output
/// (whose mode is reported before the rotation) cannot skew the result.
fn output_scale(output: &OutputRect) -> f64 {
    let logical = f64::from(output.width) * f64::from(output.height);
    let physical = f64::from(output.mode_width) * f64::from(output.mode_height);
    if logical <= 0.0 || physical <= 0.0 {
        return 1.0;
    }
    ((physical / logical).sqrt() * 100.0).round() / 100.0
}

/// The first KWin release known to capture a quarter-turned output correctly.
/// Measured: 6.3.6 distorts it, even alone at scale 1; 6.6.6 and 6.8.2 do not.
/// 6.4 and 6.5 were not measured, so they count as distorting: a refused click
/// is better than one that silently misses.
const FIRST_UNDISTORTED_RELEASE: (u32, u32) = (6, 6);

/// Outputs whose part of a whole-desktop capture does not match the screen.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DistortedCapture {
    pub outputs: Vec<String>,
    pub kwin_version: String,
}

impl DistortedCapture {
    /// Refuse a layout point on one of these outputs: a pixel read off the
    /// screenshot there is not that point on the screen.
    pub(crate) fn admit(
        &self,
        frame: &DesktopFrame,
        frame_point: (i32, i32),
        layout_point: (i32, i32),
    ) -> anyhow::Result<()> {
        let (x, y) = layout_point;
        let on_output = |output: &&FrameOutput| {
            let (x, y) = (i64::from(x), i64::from(y));
            x >= i64::from(output.x)
                && y >= i64::from(output.y)
                && x < i64::from(output.x) + i64::from(output.width)
                && y < i64::from(output.y) + i64::from(output.height)
        };
        match frame
            .outputs
            .iter()
            .filter(|output| self.outputs.contains(&output.name))
            .find(on_output)
        {
            Some(output) => anyhow::bail!(
                "desktop point ({}, {}) is on {}, a rotated output that KWin {} captures \
                 distorted, so the screenshot does not match the screen there; KWin {}.{} \
                 and later capture it correctly",
                frame_point.0,
                frame_point.1,
                output.name,
                self.kwin_version,
                FIRST_UNDISTORTED_RELEASE.0,
                FIRST_UNDISTORTED_RELEASE.1,
            ),
            None => Ok(()),
        }
    }
}

/// KWin's desktop frame and what its capture of that frame gets wrong.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesktopLayout {
    pub frame: DesktopFrame,
    pub distorted: Option<DistortedCapture>,
}

/// `major.minor` and the full version from KWin's support report.
fn reported_version(support: &str) -> Option<((u32, u32), String)> {
    let (_, rest) = support.split_once("KWin version: ")?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let version = &rest[..end];
    let mut parts = version.split('.').map(str::parse::<u32>);
    let release = (parts.next()?.ok()?, parts.next()?.ok()?);
    Some((release, version.to_owned()))
}

fn distorted_capture(
    outputs: &[OutputRect],
    support: impl FnOnce() -> Option<String>,
) -> Option<DistortedCapture> {
    let turned: Vec<String> = outputs
        .iter()
        .filter(|output| output.quarter_turned)
        .map(|output| output.name.clone())
        .collect();
    if turned.is_empty() {
        return None;
    }
    let reported = support().as_deref().and_then(reported_version);
    match reported {
        Some((release, _)) if release >= FIRST_UNDISTORTED_RELEASE => None,
        Some((_, kwin_version)) => Some(DistortedCapture {
            outputs: turned,
            kwin_version,
        }),
        None => Some(DistortedCapture {
            outputs: turned,
            kwin_version: "of an unknown version".to_owned(),
        }),
    }
}

/// KWin's desktop layout. `Ok(None)` only when the compositor answered and is
/// not KWin. A compositor that cannot be asked might be KWin, and a layout KWin
/// cannot report is an error too: the physical frame would miss every click.
pub(crate) fn desktop_layout() -> anyhow::Result<Option<DesktopLayout>> {
    let Some(outputs) = query()? else {
        return Ok(None);
    };
    Ok(Some(DesktopLayout {
        frame: frame_of(&outputs)?,
        distorted: distorted_capture(&outputs, super::kwin_helper::support_information),
    }))
}

/// The frame plus the monitor list `get_screen_size` reports, both from one
/// read of the layout, in the shape the Hyprland and niri adapters publish.
pub(crate) fn screen_report() -> anyhow::Result<Option<(DesktopFrame, serde_json::Value)>> {
    let Some(outputs) = query()? else {
        return Ok(None);
    };
    let frame = frame_of(&outputs)?;
    let monitors = outputs
        .iter()
        .map(|output| {
            let (frame_x, frame_y) = frame.from_layout(output.x, output.y);
            serde_json::json!({
                "name": output.name,
                "width": output.width,
                "height": output.height,
                "scale": output_scale(output),
                "powered": true,
                "frame_x": frame_x,
                "frame_y": frame_y,
            })
        })
        .collect();
    Ok(Some((frame, serde_json::Value::Array(monitors))))
}

fn query() -> anyhow::Result<Option<Vec<OutputRect>>> {
    let connection = Connection::connect_to_env()
        .context("the Wayland compositor could not be asked whether it is KWin")?;
    query_on(&connection)
}

fn query_on(connection: &Connection) -> anyhow::Result<Option<Vec<OutputRect>>> {
    let mut queue = connection.new_event_queue::<Report>();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut report = Report::default();
    queue
        .roundtrip(&mut report)
        .context("the Wayland compositor could not be asked whether it is KWin")?;
    if !report.is_kwin {
        return Ok(None);
    }
    let manager = report
        .manager
        .clone()
        .context("KWin did not announce zxdg_output_manager_v1")?;
    for index in 0..report.outputs.len() {
        let xdg = manager.get_xdg_output(&report.outputs[index].0, &qh, index);
        report.outputs[index].1 = Some(xdg);
    }
    queue
        .roundtrip(&mut report)
        .context("KWin's output layout could not be read")?;
    Ok(Some(report.outputs.into_iter().map(|o| o.2).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use wayland_client::backend::Backend;

    fn output(x: i32, y: i32, width: i32, height: i32, mode: (i32, i32)) -> OutputRect {
        OutputRect {
            name: String::new(),
            x,
            y,
            width,
            height,
            mode_width: mode.0,
            mode_height: mode.1,
            quarter_turned: false,
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
        let frame = frame_of(&two_monitors()).unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (0, 0, 2912, 1536)
        );
    }

    #[test]
    fn a_layout_away_from_the_origin_maps_frame_pixels_onto_its_outputs() {
        // Only the output at (864, 0) is left on.
        let frame = frame_of(&[output(864, 0, 1920, 1080, (1920, 1080))]).unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (864, 0, 1920, 1080)
        );
        assert_eq!(frame.to_layout(0, 0), (864, 0));
        assert_eq!(frame.to_layout(1919, 1079), (2783, 1079));

        // An output above and left of the layout origin.
        let frame = frame_of(&[
            output(-1920, -200, 1920, 1080, (1920, 1080)),
            output(0, 0, 1920, 1080, (1920, 1080)),
        ])
        .unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (-1920, -200, 3840, 1280)
        );
        assert_eq!(frame.to_layout(1920, 200), (0, 0));
        assert!(frame.output_contains_layout(-1920, -200));
        assert!(!frame.output_contains_layout(-1, 900));
    }

    #[test]
    fn window_records_land_on_the_pixels_of_a_logical_workspace_capture() {
        // KWin's CaptureWorkspace without native resolution is the union of the
        // outputs' logical rectangles at one pixel per logical pixel, with pixel
        // (0, 0) at its top-left layout point. Here: a 1x output above and left
        // of the origin, a 2x output at it, and a 1.25x one off to the right.
        let frame = frame_of(&[
            output(-1920, -200, 1920, 1080, (1920, 1080)),
            output(0, 0, 1280, 720, (2560, 1440)),
            output(1280, 100, 2048, 1152, (2560, 1440)),
        ])
        .unwrap();
        assert_eq!((frame.width, frame.height), (5248, 1452));
        let window = |x, y| crate::x11::WindowInfo {
            xid: 0,
            pid: None,
            app_name: String::new(),
            title: String::new(),
            is_on_screen: true,
            z_index: None,
            x,
            y,
            width: 100,
            height: 100,
        };
        let mut windows = [window(-1920, -200), window(0, 0), window(1290, 110)];
        frame.rebase_windows(&mut windows);
        let corners: Vec<_> = windows.iter().map(|w| (w.x, w.y)).collect();
        assert_eq!(corners, [(0, 0), (1920, 200), (3210, 310)]);
    }

    #[test]
    fn scale_is_the_largest_output_scale_in_any_announcement_order() {
        let low = output(0, 0, 1920, 1080, (1920, 1080));
        let high = output(1920, 0, 960, 540, (1920, 1080));
        assert_eq!(frame_of(&[low.clone(), high.clone()]).unwrap().scale, 2.0);
        assert_eq!(frame_of(&[high, low]).unwrap().scale, 2.0);
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
        assert_eq!(frame_of(&[]).unwrap_err().to_string(), NO_OUTPUTS);
    }

    #[test]
    fn an_output_with_no_logical_size_is_an_error() {
        let mut unsized_output = output(0, 0, 0, 0, (1920, 1080));
        unsized_output.name = "DP-1".into();
        assert_eq!(
            frame_of(&[unsized_output]).unwrap_err().to_string(),
            "KWin output \"DP-1\" has no logical size: 0x0"
        );
    }

    #[test]
    fn scale_comes_from_area_so_rotation_cannot_skew_it() {
        assert_eq!(output_scale(&output(0, 0, 864, 1536, (1920, 1080))), 1.25);
        assert_eq!(output_scale(&output(0, 0, 2048, 1152, (2560, 1440))), 1.25);
        assert_eq!(output_scale(&output(0, 0, 0, 0, (0, 0))), 1.0);
    }

    fn turned_output(name: &str, x: i32) -> OutputRect {
        OutputRect {
            name: name.into(),
            quarter_turned: true,
            ..output(x, 0, 1080, 1920, (1920, 1080))
        }
    }

    // gdbus prints the report as a tuple with escaped newlines.
    fn support(version: &str) -> Option<String> {
        Some(format!(
            "('KWin Support Information:\\n\\nVersion\\n=======\\nKWin version: {version}\\nQt Version: 6.8.2\\n',)"
        ))
    }

    #[test]
    fn a_quarter_turned_output_is_distorted_below_the_first_good_release() {
        let outputs = [
            turned_output("DP-1", 0),
            output(1080, 0, 1920, 1080, (1920, 1080)),
        ];
        assert_eq!(
            distorted_capture(&outputs, || support("6.3.6")),
            Some(DistortedCapture {
                outputs: vec!["DP-1".into()],
                kwin_version: "6.3.6".into(),
            })
        );
        assert_eq!(distorted_capture(&outputs, || support("6.6.0")), None);
        assert_eq!(distorted_capture(&outputs, || support("6.8.2")), None);
        assert_eq!(distorted_capture(&outputs, || support("7.0")), None);
    }

    #[test]
    fn an_unknown_version_with_a_quarter_turned_output_counts_as_distorted() {
        let distorted = distorted_capture(&[turned_output("DP-1", 0)], || None).unwrap();
        assert_eq!(distorted.kwin_version, "of an unknown version");
    }

    #[test]
    fn no_quarter_turned_output_never_needs_the_version() {
        let outputs = [output(0, 0, 1920, 1080, (1920, 1080))];
        let distorted = distorted_capture(&outputs, || unreachable!("no version needed"));
        assert_eq!(distorted, None);
    }

    #[test]
    fn only_a_point_on_a_distorted_output_is_refused() {
        let outputs = [
            turned_output("DP-1", 0),
            output(1080, 0, 1920, 1080, (1920, 1080)),
        ];
        let frame = frame_of(&outputs).unwrap();
        let distorted = distorted_capture(&outputs, || support("6.3.6")).unwrap();
        assert_eq!(
            distorted
                .admit(&frame, (5, 6), (5, 6))
                .unwrap_err()
                .to_string(),
            "desktop point (5, 6) is on DP-1, a rotated output that KWin 6.3.6 captures \
             distorted, so the screenshot does not match the screen there; KWin 6.6 and later \
             capture it correctly"
        );
        assert!(distorted.admit(&frame, (1080, 0), (1080, 0)).is_ok());
    }

    /// What a scripted compositor does on the wire.
    #[derive(Clone, Default)]
    struct Script {
        globals: Vec<&'static str>,
        /// Logical position and size sent for each `get_xdg_output`, in order.
        logical: Vec<(i32, i32, i32, i32)>,
        /// Hang up instead of answering this sync: 1 before the client has
        /// the globals, 2 once it has them.
        hang_up_at_sync: Option<u32>,
    }

    fn message(object: u32, opcode: u16, args: &[u8]) -> Vec<u8> {
        let size = 8 + args.len() as u32;
        let mut out = object.to_ne_bytes().to_vec();
        out.extend(((size << 16) | u32::from(opcode)).to_ne_bytes());
        out.extend(args);
        out
    }

    fn global_args(name: u32, interface: &str, version: u32) -> Vec<u8> {
        let mut out = name.to_ne_bytes().to_vec();
        out.extend((interface.len() as u32 + 1).to_ne_bytes());
        out.extend(interface.as_bytes());
        out.resize(out.len() + 4 - interface.len() % 4, 0);
        out.extend(version.to_ne_bytes());
        out
    }

    fn word(bytes: &[u8], index: usize) -> u32 {
        let start = index * 4;
        u32::from_ne_bytes([
            bytes[start],
            bytes[start + 1],
            bytes[start + 2],
            bytes[start + 3],
        ])
    }

    /// Serves one client: enough of `wl_display`, `wl_registry`,
    /// `zxdg_output_manager_v1` and `wl_callback` for [`query_on`].
    fn serve(mut stream: UnixStream, script: Script) {
        let mut registry = 0;
        let mut manager = 0;
        let mut syncs = 0;
        let mut next_logical = script.logical.iter();
        let mut pending = Vec::new();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            while buffer.len() < 8 || buffer.len() < (word(&buffer, 1) >> 16) as usize {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(read) => buffer.extend(&chunk[..read]),
                }
            }
            let size = (word(&buffer, 1) >> 16) as usize;
            let request: Vec<u8> = buffer.drain(..size).collect();
            let (object, opcode) = (word(&request, 0), word(&request, 1) & 0xffff);
            match (object, opcode) {
                // wl_display.sync: flush what is pending, then answer.
                (1, 0) => {
                    syncs += 1;
                    if script.hang_up_at_sync == Some(syncs) {
                        return;
                    }
                    pending.extend(message(word(&request, 2), 0, &0u32.to_ne_bytes()));
                    if stream.write_all(&pending).is_err() {
                        return;
                    }
                    pending.clear();
                }
                // wl_display.get_registry: announce every global.
                (1, 1) => {
                    registry = word(&request, 2);
                    for (index, interface) in script.globals.iter().enumerate() {
                        let args = global_args(index as u32 + 1, interface, 1);
                        pending.extend(message(registry, 0, &args));
                    }
                }
                // wl_registry.bind: the last word is the new object's id.
                (object, 0) if object == registry => {
                    let name = word(&request, 2) as usize - 1;
                    if script.globals[name] == "zxdg_output_manager_v1" {
                        manager = word(&request, request.len() / 4 - 1);
                    }
                }
                // zxdg_output_manager_v1.get_xdg_output.
                (object, 1) if object == manager => {
                    let xdg = word(&request, 2);
                    if let Some((x, y, width, height)) = next_logical.next() {
                        let position = [x.to_ne_bytes(), y.to_ne_bytes()].concat();
                        let size = [width.to_ne_bytes(), height.to_ne_bytes()].concat();
                        pending.extend(message(xdg, 0, &position));
                        pending.extend(message(xdg, 1, &size));
                    }
                }
                _ => {}
            }
        }
    }

    fn query_script(script: Script) -> anyhow::Result<Option<Vec<OutputRect>>> {
        let (client, server) = UnixStream::pair().unwrap();
        let compositor = std::thread::spawn(move || serve(server, script));
        let connection = Connection::from_backend(Backend::connect(client).unwrap());
        let result = query_on(&connection);
        drop(connection);
        compositor.join().unwrap();
        result
    }

    const KWIN_GLOBALS: [&str; 4] = [
        "kde_output_device_v2",
        "wl_output",
        "wl_output",
        "zxdg_output_manager_v1",
    ];

    #[test]
    fn query_reads_every_output_of_kwin() {
        let outputs = query_script(Script {
            globals: KWIN_GLOBALS.to_vec(),
            logical: vec![(864, 0, 1920, 1080), (-100, 50, 1280, 720)],
            ..Script::default()
        })
        .unwrap()
        .unwrap();
        let frame = frame_of(&outputs).unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (-100, 0, 2884, 1080)
        );
    }

    #[test]
    fn a_compositor_that_is_not_kwin_has_no_layout_here() {
        let outputs = query_script(Script {
            globals: vec!["wl_output", "zxdg_output_manager_v1"],
            ..Script::default()
        })
        .unwrap();
        assert_eq!(outputs, None);
    }

    #[test]
    fn kwin_without_xdg_output_is_an_error_not_the_physical_frame() {
        let error = query_script(Script {
            globals: vec!["kde_output_device_registry_v2", "wl_output"],
            ..Script::default()
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "KWin did not announce zxdg_output_manager_v1"
        );
    }

    #[test]
    fn a_kwin_connection_lost_mid_read_is_an_error_not_the_physical_frame() {
        let error = query_script(Script {
            globals: KWIN_GLOBALS.to_vec(),
            hang_up_at_sync: Some(2),
            ..Script::default()
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "KWin's output layout could not be read");
    }

    #[test]
    fn a_compositor_lost_before_its_globals_is_an_error_not_a_non_kwin_session() {
        let error = query_script(Script {
            globals: KWIN_GLOBALS.to_vec(),
            hang_up_at_sync: Some(1),
            ..Script::default()
        })
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the Wayland compositor could not be asked whether it is KWin"
        );
    }
}
