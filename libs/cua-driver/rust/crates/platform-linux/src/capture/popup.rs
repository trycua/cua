//! Conservative X11 composition of exact-window combo popups.
//! Never reads desktop pixels, activates windows, waits for UI, or expands the canvas.
use anyhow::{anyhow, bail, Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, MapState, Window, WindowClass};
use x11rb::rust_connection::RustConnection;

const MAX_ROOT_CHILDREN: usize = 256;
const MAX_ANCESTORS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}
impl Rect {
    fn overlaps(self, other: Self) -> bool {
        i64::from(self.x) < i64::from(other.x) + i64::from(other.width)
            && i64::from(other.x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(self.y) < i64::from(other.y) + i64::from(other.height)
            && i64::from(other.y) < i64::from(self.y) + i64::from(self.height)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Surface {
    xid: Window,
    rect: Rect,
    depth: u8,
    border: u16,
    visual: u32,
    override_redirect: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    target: Surface,
    // Root-child order is X11 stacking order, from bottom to top.
    popups: Vec<Surface>,
    pid: u32,
    root_children: Vec<Window>,
    target_ancestry: Vec<Window>,
    // Every mapped foreign drawable above the lowest popup, including geometry,
    // is checked. Owned combo popups are captured in stack order instead.
    above: Vec<Surface>,
}

struct Inspector {
    conn: RustConnection,
    root: Window,
    active: u32,
    pid: u32,
    transient: u32,
    window_type: u32,
    combo: u32,
    opacity: u32,
}
impl Inspector {
    fn new() -> Result<Self> {
        let (conn, screen) = RustConnection::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atom =
            |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
        let active = atom(b"_NET_ACTIVE_WINDOW")?;
        let pid = atom(b"_NET_WM_PID")?;
        let transient = atom(b"WM_TRANSIENT_FOR")?;
        let window_type = atom(b"_NET_WM_WINDOW_TYPE")?;
        let combo = atom(b"_NET_WM_WINDOW_TYPE_COMBO")?;
        let opacity = atom(b"_NET_WM_WINDOW_OPACITY")?;
        Ok(Self {
            conn,
            root,
            active,
            pid,
            transient,
            window_type,
            combo,
            opacity,
        })
    }

    fn property(&self, xid: Window, atom: u32, kind: AtomEnum) -> Result<Vec<u32>> {
        let reply = self
            .conn
            .get_property(false, xid, atom, kind, 0, 16)?
            .reply()?;
        if reply.type_ == x11rb::NONE {
            return Ok(Vec::new());
        }
        if reply.type_ != u32::from(kind) || reply.format != 32 || reply.bytes_after != 0 {
            bail!("unsupported X11 popup property format");
        }
        let values = reply
            .value32()
            .ok_or_else(|| anyhow!("missing X11 property values"))?
            .collect();
        Ok(values)
    }

    fn singleton(&self, xid: Window, atom: u32, kind: AtomEnum) -> Result<Option<u32>> {
        let values = self.property(xid, atom, kind)?;
        match values.as_slice() {
            [] => Ok(None),
            [value] => Ok(Some(*value)),
            _ => bail!("ambiguous X11 popup property"),
        }
    }

    fn surface(&self, xid: Window) -> Result<Option<Surface>> {
        let attributes = self.conn.get_window_attributes(xid)?.reply()?;
        if attributes.map_state != MapState::VIEWABLE || attributes.class == WindowClass::INPUT_ONLY
        {
            return Ok(None);
        }
        let geometry = self.conn.get_geometry(xid)?.reply()?;
        let position = self
            .conn
            .translate_coordinates(xid, self.root, 0, 0)?
            .reply()?;
        if !position.same_screen {
            bail!("X11 popup crosses screens");
        }
        Ok(Some(Surface {
            xid,
            rect: Rect {
                x: i32::from(position.dst_x),
                y: i32::from(position.dst_y),
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
            },
            depth: geometry.depth,
            border: geometry.border_width,
            visual: attributes.visual,
            override_redirect: attributes.override_redirect,
        }))
    }

    fn inspect(&self, target: Window) -> Result<Option<Snapshot>> {
        // Background capture retains its original behavior.
        if self.singleton(self.root, self.active, AtomEnum::WINDOW)? != Some(target) {
            return Ok(None);
        }
        let Some(pid) = self.singleton(target, self.pid, AtomEnum::CARDINAL)? else {
            return Ok(None);
        };
        let children = self.conn.query_tree(self.root)?.reply()?.children;
        if children.len() > MAX_ROOT_CHILDREN {
            bail!("X11 popup inventory exceeds bounded root child limit");
        }
        let mut eligible = Vec::new();
        for &xid in &children {
            if xid == target {
                continue;
            }
            // A window may vanish while enumerating unrelated root children.
            // A direct transient with proven owner/type becomes a known popup;
            // errors after that point are never silently ignored.
            if self
                .singleton(xid, self.transient, AtomEnum::WINDOW)
                .ok()
                .flatten()
                != Some(target)
            {
                continue;
            }
            if self.singleton(xid, self.pid, AtomEnum::CARDINAL)? != Some(pid) {
                continue;
            }
            let types = self.property(xid, self.window_type, AtomEnum::ATOM)?;
            if !types.contains(&self.combo) {
                continue;
            }
            if types != vec![self.combo] {
                bail!("ambiguous same-window combo popup type");
            }
            let Some(popup) = self.surface(xid)? else {
                continue;
            };
            eligible.push(popup);
        }
        if eligible.is_empty() {
            return Ok(None);
        }
        let target_surface = self
            .surface(target)?
            .ok_or_else(|| anyhow!("foreground popup target is not viewable"))?;
        for popup in &eligible {
            validate_format(popup)?;
            let opacity = self.singleton(popup.xid, self.opacity, AtomEnum::CARDINAL)?;
            if opacity.is_some_and(|value| value != u32::MAX) {
                bail!("translucent X11 combo popup is unsupported");
            }
            let shape = self.conn.shape_query_extents(popup.xid)?.reply()?;
            if shape.bounding_shaped || shape.clip_shaped {
                bail!("shaped X11 combo popup is unsupported");
            }
        }
        let mut ancestry = vec![target];
        let mut parent = target;
        loop {
            let next = self.conn.query_tree(parent)?.reply()?.parent;
            if next == self.root {
                break;
            }
            if next == x11rb::NONE || ancestry.contains(&next) || ancestry.len() >= MAX_ANCESTORS {
                bail!("X11 popup target ancestry is unproven");
            }
            ancestry.push(next);
            parent = next;
        }
        let target_stack = children
            .iter()
            .position(|xid| *xid == parent)
            .ok_or_else(|| anyhow!("X11 target frame missing from root stack"))?;
        let popup_stack = eligible
            .iter()
            .map(|popup| {
                children
                    .iter()
                    .position(|xid| *xid == popup.xid)
                    .ok_or_else(|| anyhow!("X11 combo popup missing from root stack"))
            })
            .collect::<Result<Vec<_>>>()?;
        if popup_stack.iter().any(|stack| *stack <= target_stack) {
            bail!("X11 combo popup is below its target frame");
        }
        let lowest_popup_stack = *popup_stack
            .first()
            .ok_or_else(|| anyhow!("X11 combo popup stack is empty"))?;
        let mut above = Vec::new();
        for &xid in &children[lowest_popup_stack + 1..] {
            if eligible.iter().any(|popup| popup.xid == xid) {
                continue;
            }
            if let Some(surface) = self.surface(xid)? {
                if eligible
                    .iter()
                    .any(|popup| surface.rect.overlaps(popup.rect))
                {
                    bail!("X11 combo popup is occluded by another window");
                }
                above.push(surface);
            }
        }
        Ok(Some(Snapshot {
            target: target_surface,
            popups: eligible,
            pid,
            root_children: children,
            target_ancestry: ancestry,
            above,
        }))
    }
}

fn validate_format(popup: &Surface) -> Result<()> {
    if !popup.override_redirect
        || popup.depth != 24
        || popup.border != 0
        || popup.rect.width == 0
        || popup.rect.height == 0
        || popup.rect.width > super::MAX_CAPTURE_DIM
        || popup.rect.height > super::MAX_CAPTURE_DIM
    {
        bail!("X11 combo popup requires override-redirect, opaque depth24, border0 and bounded rectangular geometry");
    }
    Ok(())
}

fn paint(base: &[u8], popups: &[Vec<u8>], snapshot: &Snapshot) -> Result<Vec<u8>> {
    let mut canvas = image::load_from_memory_with_format(base, image::ImageFormat::Png)?.to_rgba8();
    if canvas.dimensions() != (snapshot.target.rect.width, snapshot.target.rect.height) {
        bail!("X11 combo capture dimensions changed");
    }
    if popups.len() != snapshot.popups.len() {
        bail!("X11 combo capture popup inventory changed");
    }
    for (popup, bytes) in snapshot.popups.iter().zip(popups) {
        let pixels =
            image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.to_rgba8();
        if pixels.dimensions() != (popup.rect.width, popup.rect.height) {
            bail!("X11 combo capture dimensions changed");
        }
        if pixels.pixels().any(|pixel| pixel.0[3] != 255) {
            bail!("X11 combo capture has unsupported alpha");
        }
        // The image API clips negative offsets and pixels outside the original canvas.
        // Popups are overlaid in root-stack order, so later windows remain on top.
        image::imageops::overlay(
            &mut canvas,
            &pixels,
            i64::from(popup.rect.x) - i64::from(snapshot.target.rect.x),
            i64::from(popup.rect.y) - i64::from(snapshot.target.rect.y),
        );
    }
    let mut png = std::io::Cursor::new(Vec::new());
    canvas.write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

pub(super) fn capture(xid: u64, raw: impl Fn(u64) -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    let target = u32::try_from(xid).context("X11 capture target is out of range")?;
    let inspector = Inspector::new().context("X11 popup inspection connection")?;
    let Some(before) = inspector
        .inspect(target)
        .map_err(|error| anyhow!("X11 popup capture inspection: {error:#}"))?
    else {
        return raw(xid);
    };
    let base = raw(xid)?;
    // Direct drawable capture only; no recursive composition or desktop pixels.
    let popups = before
        .popups
        .iter()
        .map(|popup| raw(u64::from(popup.xid)))
        .collect::<Result<Vec<_>>>()?;
    let after = inspector
        .inspect(target)
        .map_err(|error| anyhow!("X11 popup capture revalidation: {error:#}"))?;
    if after.as_ref() != Some(&before) {
        bail!("X11 popup capture became unstable; observe again");
    }
    paint(&base, &popups, &before)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(xid: u32, x: i32, y: i32, width: u32, height: u32) -> Surface {
        Surface {
            xid,
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            depth: 24,
            border: 0,
            visual: 21,
            override_redirect: true,
        }
    }
    fn snapshot(popups: Vec<Surface>) -> Snapshot {
        Snapshot {
            target: surface(1, 100, 200, 4, 3),
            popups,
            pid: 10,
            root_children: vec![1, 2],
            target_ancestry: vec![1],
            above: vec![],
        }
    }
    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn clips_popup_without_expanding_or_shifting_original_canvas() {
        let base = png(4, 3, [0, 0, 255, 255]);
        let popup = png(3, 2, [255, 0, 0, 255]);
        let result = paint(&base, &[popup], &snapshot(vec![surface(2, 99, 202, 3, 2)])).unwrap();
        let actual = image::load_from_memory(&result).unwrap().to_rgba8();
        assert_eq!(actual.dimensions(), (4, 3));
        for (x, y, pixel) in actual.enumerate_pixels() {
            assert_eq!(
                pixel.0,
                if y == 2 && x < 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            );
        }
    }

    #[test]
    fn outside_popup_does_not_expose_pixels_outside_target() {
        let base = png(4, 3, [0, 0, 255, 255]);
        let result = paint(
            &base,
            &[png(2, 2, [255, 0, 0, 255])],
            &snapshot(vec![surface(2, 105, 200, 2, 2)]),
        )
        .unwrap();
        assert_eq!(
            image::load_from_memory(&result).unwrap().to_rgba8(),
            image::load_from_memory(&base).unwrap().to_rgba8()
        );
    }

    #[test]
    fn changed_image_dimensions_and_alpha_are_explicit_errors() {
        let state = snapshot(vec![surface(2, 101, 201, 2, 2)]);
        assert!(paint(
            &png(5, 3, [0, 0, 0, 255]),
            &[png(2, 2, [1, 2, 3, 255])],
            &state
        )
        .is_err());
        assert!(paint(
            &png(4, 3, [0, 0, 0, 255]),
            &[png(2, 1, [1, 2, 3, 255])],
            &state
        )
        .is_err());
        assert!(paint(
            &png(4, 3, [0, 0, 0, 255]),
            &[png(2, 2, [1, 2, 3, 0])],
            &state
        )
        .is_err());
    }

    #[test]
    fn overlays_owned_popups_in_root_stack_order() {
        let base = png(4, 3, [0, 0, 255, 255]);
        let result = paint(
            &base,
            &[png(3, 2, [255, 0, 0, 255]), png(2, 2, [0, 255, 0, 255])],
            &snapshot(vec![surface(2, 100, 200, 3, 2), surface(3, 101, 201, 2, 2)]),
        )
        .unwrap();
        let actual = image::load_from_memory(&result).unwrap().to_rgba8();
        assert_eq!(actual.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(actual.get_pixel(1, 1).0, [0, 255, 0, 255]);
        assert_eq!(actual.get_pixel(2, 2).0, [0, 255, 0, 255]);
        assert_eq!(actual.get_pixel(3, 2).0, [0, 0, 255, 255]);
    }

    #[test]
    fn only_supported_opaque_borderless_override_redirect_geometry_passes() {
        let original = surface(2, -10, 0, 20, 30);
        assert!(validate_format(&original).is_ok());
        let mut wrong = original.clone();
        wrong.depth = 32;
        assert!(validate_format(&wrong).is_err());
        wrong = original.clone();
        wrong.border = 1;
        assert!(validate_format(&wrong).is_err());
        wrong = original.clone();
        wrong.override_redirect = false;
        assert!(validate_format(&wrong).is_err());
        wrong = original.clone();
        wrong.rect.width = super::super::MAX_CAPTURE_DIM + 1;
        assert!(validate_format(&wrong).is_err());
        wrong = original;
        wrong.rect.height = 0;
        assert!(validate_format(&wrong).is_err());
    }

    #[test]
    fn stacking_overlap_uses_root_coordinates_and_excludes_touching_edges() {
        let popup = surface(2, -10, 200, 20, 30).rect;
        assert!(popup.overlaps(surface(3, 9, 229, 5, 5).rect));
        assert!(!popup.overlaps(surface(3, 10, 200, 5, 5).rect));
        assert!(!popup.overlaps(surface(3, -10, 230, 5, 5).rect));
    }

    #[test]
    fn metadata_changes_do_not_match_the_captured_snapshot() {
        let original = snapshot(vec![surface(2, 101, 201, 2, 2)]);
        let mut after = original.clone();
        after.popups[0].rect.height += 1;
        assert_ne!(original, after);
        after = original.clone();
        after.pid += 1;
        assert_ne!(original, after);
        after = original.clone();
        after.root_children.reverse();
        assert_ne!(original, after);
        after = original.clone();
        after.popups[0].xid = 3;
        assert_ne!(original, after);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use x11rb::protocol::shape::{SK, SO};
    use x11rb::protocol::xproto::{
        ClipOrdering, ConfigureWindowAux, CreateWindowAux, PropMode, Rectangle,
    };
    use x11rb::wrapper::ConnectionExt as _;

    // These are real X11 drawable fixtures and real WM activation, not captured
    // image mocks. Run only in a separate Xvfb+EWMH-WM session, serially.
    struct Fixture {
        inspector: Inspector,
        windows: Vec<Window>,
        target: Window,
        sibling: Window,
        popup: Window,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for xid in self.windows.iter().rev() {
                if let Ok(cookie) = self.inspector.conn.destroy_window(*xid) {
                    let _ = cookie.check();
                }
            }
            let _ = self.inspector.conn.flush();
        }
    }
    impl Fixture {
        fn new() -> Self {
            assert_eq!(
                std::env::var("CUA_POPUP_CAPTURE_DISPOSABLE_X11").as_deref(),
                Ok("1"),
                "requires a dedicated disposable Xvfb+EWMH WM; never run on a user desktop"
            );
            let inspector = Inspector::new().unwrap();
            let mut fixture = Self {
                inspector,
                windows: Vec::new(),
                target: 0,
                sibling: 0,
                popup: 0,
            };
            fixture.target = fixture.window(100, 100, 96, 72, false, 0x0011_2233);
            fixture.sibling = fixture.window(400, 300, 96, 72, false, 0x0011_2233);
            fixture.activate(fixture.target);
            let target = fixture.inspector.surface(fixture.target).unwrap().unwrap();
            fixture.popup = fixture.combo_popup(
                (target.rect.x + 10) as i16,
                (target.rect.y + 10) as i16,
                24,
                20,
                0x00dd_2244,
            );
            fixture.sync();
            fixture
        }
        fn combo_popup(&mut self, x: i16, y: i16, width: u16, height: u16, pixel: u32) -> Window {
            let popup = self.window(x, y, width, height, true, pixel);
            self.property(
                popup,
                self.inspector.transient,
                AtomEnum::WINDOW,
                &[self.target],
            );
            self.property(
                popup,
                self.inspector.window_type,
                AtomEnum::ATOM,
                &[self.inspector.combo],
            );
            popup
        }
        fn window(
            &mut self,
            x: i16,
            y: i16,
            width: u16,
            height: u16,
            override_redirect: bool,
            pixel: u32,
        ) -> Window {
            let conn = &self.inspector.conn;
            let screen = conn
                .setup()
                .roots
                .iter()
                .find(|screen| screen.root == self.inspector.root)
                .unwrap();
            assert_eq!(screen.root_depth, 24, "requires Xvfb depth24");
            let xid = conn.generate_id().unwrap();
            conn.create_window(
                screen.root_depth,
                xid,
                screen.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .override_redirect(u32::from(override_redirect))
                    .background_pixel(pixel),
            )
            .unwrap()
            .check()
            .unwrap();
            self.windows.push(xid);
            self.property(
                xid,
                self.inspector.pid,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            );
            conn.change_property8(
                PropMode::REPLACE,
                xid,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                b"OpenSky popup capture fixture",
            )
            .unwrap()
            .check()
            .unwrap();
            conn.map_window(xid).unwrap().check().unwrap();
            conn.flush().unwrap();
            xid
        }
        fn property(&self, xid: Window, atom: u32, kind: AtomEnum, values: &[u32]) {
            self.inspector
                .conn
                .change_property32(PropMode::REPLACE, xid, atom, kind, values)
                .unwrap()
                .check()
                .unwrap();
        }
        fn activate(&self, xid: Window) {
            // Wait only for real fixture/WM setup; production capture has no sleep.
            for _ in 0..100 {
                if self.inspector.surface(xid).unwrap().is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            crate::input::x11_activate_window_persistent(u64::from(xid)).unwrap();
            for _ in 0..100 {
                if self
                    .inspector
                    .singleton(self.inspector.root, self.inspector.active, AtomEnum::WINDOW)
                    .unwrap()
                    == Some(xid)
                {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("real EWMH WM did not activate owned fixture");
        }
        fn sync(&self) {
            self.inspector
                .conn
                .get_input_focus()
                .unwrap()
                .reply()
                .unwrap();
        }
        fn capture(&self) -> Result<Vec<u8>> {
            super::super::screenshot_window_bytes(u64::from(self.target))
        }
        fn raw(&self) -> Vec<u8> {
            super::super::screenshot_window_bytes_raw(u64::from(self.target)).unwrap()
        }
        fn assert_baseline(&self) {
            let baseline = image::load_from_memory(&self.raw()).unwrap().to_rgba8();
            let actual = image::load_from_memory(&self.capture().unwrap())
                .unwrap()
                .to_rgba8();
            assert_eq!(
                actual, baseline,
                "ineligible/background popup must not change raw window pixels"
            );
        }
    }

    #[test]
    #[ignore = "requires disposable depth24 Xvfb with real EWMH WM; run --test-threads=1"]
    fn live_exact_owner_composes_and_wrong_sibling_missing_hidden_background_keep_raw() {
        let f = Fixture::new();
        let image = image::load_from_memory(&f.capture().unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(image.dimensions(), (96, 72));
        assert_eq!(image.get_pixel(12, 12).0, [0xdd, 0x22, 0x44, 255]);
        assert_eq!(image.get_pixel(0, 0).0, [0x11, 0x22, 0x33, 255]);
        f.property(
            f.popup,
            f.inspector.pid,
            AtomEnum::CARDINAL,
            &[std::process::id().wrapping_add(1)],
        );
        f.assert_baseline();
        f.property(
            f.popup,
            f.inspector.pid,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        );
        f.property(
            f.popup,
            f.inspector.transient,
            AtomEnum::WINDOW,
            &[f.sibling],
        );
        f.assert_baseline();
        f.inspector
            .conn
            .delete_property(f.popup, f.inspector.transient)
            .unwrap()
            .check()
            .unwrap();
        f.assert_baseline();
        f.property(
            f.popup,
            f.inspector.transient,
            AtomEnum::WINDOW,
            &[f.target],
        );
        f.inspector
            .conn
            .unmap_window(f.popup)
            .unwrap()
            .check()
            .unwrap();
        f.assert_baseline();
        f.inspector
            .conn
            .map_window(f.popup)
            .unwrap()
            .check()
            .unwrap();
        f.activate(f.sibling);
        f.assert_baseline();
    }

    #[test]
    #[ignore = "requires disposable depth24 Xvfb with real EWMH WM; run --test-threads=1"]
    fn live_multiple_owned_combos_compose_in_root_stack_order() {
        let mut f = Fixture::new();
        let target = f.inspector.surface(f.target).unwrap().unwrap();
        let upper = f.combo_popup(
            (target.rect.x + 16) as i16,
            (target.rect.y + 16) as i16,
            24,
            20,
            0x0022_dd44,
        );
        f.sync();

        let image = image::load_from_memory(&f.capture().unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(image.get_pixel(12, 12).0, [0xdd, 0x22, 0x44, 255]);
        assert_eq!(image.get_pixel(18, 18).0, [0x22, 0xdd, 0x44, 255]);

        // An otherwise matching but wrong-owner popup remains a foreign
        // occluder rather than being included in the composition.
        f.property(
            upper,
            f.inspector.pid,
            AtomEnum::CARDINAL,
            &[std::process::id().wrapping_add(1)],
        );
        assert!(f.capture().is_err());
    }

    #[test]
    #[ignore = "requires disposable depth24 Xvfb with real EWMH WM; run --test-threads=1"]
    fn live_popup_clips_to_original_canvas() {
        let f = Fixture::new();
        let target = f.inspector.surface(f.target).unwrap().unwrap();
        f.inspector
            .conn
            .configure_window(
                f.popup,
                &ConfigureWindowAux::new()
                    .x(target.rect.x - 4)
                    .y(target.rect.y + 68),
            )
            .unwrap()
            .check()
            .unwrap();
        f.sync();
        let actual = image::load_from_memory(&f.capture().unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(actual.dimensions(), (96, 72));
        assert_eq!(actual.get_pixel(0, 68).0, [0xdd, 0x22, 0x44, 255]);
        assert_eq!(actual.get_pixel(19, 71).0, [0xdd, 0x22, 0x44, 255]);
        assert_eq!(actual.get_pixel(20, 71).0, [0x11, 0x22, 0x33, 255]);
    }

    #[test]
    #[ignore = "requires disposable depth24 Xvfb with real EWMH WM; run --test-threads=1"]
    fn live_shape_opacity_occlusion_and_resize_refuse_without_pixel_fallback() {
        let mut f = Fixture::new();
        f.property(
            f.popup,
            f.inspector.opacity,
            AtomEnum::CARDINAL,
            &[u32::MAX / 2],
        );
        assert!(f
            .capture()
            .unwrap_err()
            .to_string()
            .contains("popup capture inspection"));
        f.inspector
            .conn
            .delete_property(f.popup, f.inspector.opacity)
            .unwrap()
            .check()
            .unwrap();
        f.inspector
            .conn
            .shape_rectangles(
                SO::SET,
                SK::BOUNDING,
                ClipOrdering::UNSORTED,
                f.popup,
                0,
                0,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                }],
            )
            .unwrap()
            .check()
            .unwrap();
        assert!(f.capture().is_err());
        f.inspector
            .conn
            .shape_mask(SO::SET, SK::BOUNDING, f.popup, 0, 0, x11rb::NONE)
            .unwrap()
            .check()
            .unwrap();
        let popup = f.inspector.surface(f.popup).unwrap().unwrap();
        let occluder = f.window(
            popup.rect.x as i16,
            popup.rect.y as i16,
            10,
            10,
            true,
            0x00ff_ffff,
        );
        assert!(f.capture().is_err());
        f.inspector
            .conn
            .unmap_window(occluder)
            .unwrap()
            .check()
            .unwrap();
        // A real application resize delivered between actual captures probes
        // the race guard. Both pixel reads still use the production raw backend.
        let result = capture(u64::from(f.target), |xid| {
            let image = super::super::screenshot_window_bytes_raw(xid)?;
            if xid == u64::from(f.target) {
                f.inspector
                    .conn
                    .configure_window(f.popup, &ConfigureWindowAux::new().width(25))?
                    .check()?;
            }
            Ok(image)
        });
        assert!(result.unwrap_err().to_string().contains("unstable"));
    }
}
