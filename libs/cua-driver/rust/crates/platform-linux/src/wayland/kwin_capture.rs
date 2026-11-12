// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{ensure, Context};
use tokio::io::AsyncReadExt;
use wayland_client::{protocol::{wl_callback, wl_registry}, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::{
    self as management, OrgKdePlasmaWindowManagement,
};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::{
    self as plasma_window, OrgKdePlasmaWindow,
};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_stacking_order::{
    self as stacking, OrgKdePlasmaStackingOrder,
};
use zbus::zvariant::{Fd, OwnedValue};

use crate::x11::WindowInfo;

const KWIN: &str = "org.kde.KWin";
const SCREENSHOT: &str = "org.kde.KWin.ScreenShot2";
const TIMEOUT: Duration = Duration::from_secs(5);
type Properties = HashMap<String, OwnedValue>;

#[derive(Default)]
struct Windows {
    manager: Option<OrgKdePlasmaWindowManagement>,
    uuids: Vec<String>,
    stacking: Vec<String>,
    stacking_complete: bool,
    roundtrips: u32,
    target_state: Option<u32>,
    pids: HashMap<String, u32>,
}

impl Dispatch<wl_callback::WlCallback, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.roundtrips += 1;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Windows {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == OrgKdePlasmaWindowManagement::interface().name && version >= 13 {
                state.manager = Some(registry.bind(name, version.min(17), qh, ()));
            }
        }
    }
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &OrgKdePlasmaWindowManagement,
        event: management::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            management::Event::WindowWithUuid { uuid, .. } => state.uuids.push(uuid),
            management::Event::StackingOrderUuidChanged { uuids } => {
                state.stacking = uuids
                    .split(';')
                    .filter(|uuid| !uuid.is_empty())
                    .map(str::to_owned)
                    .collect();
                state.stacking_complete = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<OrgKdePlasmaWindow, String> for Windows {
    fn event(
        state: &mut Self,
        _: &OrgKdePlasmaWindow,
        event: plasma_window::Event,
        uuid: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            plasma_window::Event::StateChanged { flags } => state.target_state = Some(flags),
            plasma_window::Event::PidChanged { pid } if pid != 0 => {
                state.pids.insert(uuid.clone(), pid);
            }
            _ => {}
        }
    }
}

impl Dispatch<OrgKdePlasmaStackingOrder, ()> for Windows {
    fn event(
        state: &mut Self,
        _: &OrgKdePlasmaStackingOrder,
        event: stacking::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            stacking::Event::Window { uuid } => state.stacking.push(uuid),
            stacking::Event::Done => state.stacking_complete = true,
            _ => {}
        }
    }
}

/// A Plasma window-management connection, past its registry roundtrip.
struct Plasma {
    connection: Connection,
    queue: EventQueue<Windows>,
    state: Windows,
    manager: OrgKdePlasmaWindowManagement,
    deadline: Instant,
}

impl Plasma {
    fn connect(connection: Connection) -> anyhow::Result<Self> {
        let mut queue = connection.new_event_queue::<Windows>();
        connection.display().get_registry(&queue.handle(), ());
        connection.display().sync(&queue.handle(), ());
        let mut state = Windows::default();
        let deadline = Instant::now() + TIMEOUT;
        super::hyprland_capture::dispatch_until(
            &mut queue,
            &mut state,
            deadline,
            "KWin registry",
            |state| state.roundtrips == 1,
        )?;
        let manager = state
            .manager
            .clone()
            .context("KWin window management protocol unavailable")?;
        Ok(Self {
            connection,
            queue,
            state,
            manager,
            deadline,
        })
    }

    /// One more server roundtrip, then until `done` holds.
    fn roundtrip(&mut self, what: &str, done: impl Fn(&Windows) -> bool) -> anyhow::Result<()> {
        self.connection.display().sync(&self.queue.handle(), ());
        let target = self.state.roundtrips + 1;
        super::hyprland_capture::dispatch_until(
            &mut self.queue,
            &mut self.state,
            self.deadline,
            what,
            |state| state.roundtrips == target && done(state),
        )
    }
}

fn window_snapshot(connection: Connection) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let mut plasma = Plasma::connect(connection)?;
    if plasma.manager.version() >= 17 {
        plasma
            .manager
            .get_stacking_order(&plasma.queue.handle(), ());
    }
    plasma.roundtrip("KWin windows", |state| state.stacking_complete)?;
    let Windows {
        mut uuids,
        stacking,
        ..
    } = plasma.state;
    uuids.sort();
    uuids.dedup();
    Ok((uuids, stacking))
}

/// The owning process of each window, as Plasma window management announces it.
/// `getWindowInfo` only started reporting a pid in later KWin releases.
fn window_pids(connection: Connection, uuids: &[String]) -> anyhow::Result<HashMap<String, u32>> {
    let mut plasma = Plasma::connect(connection)?;
    let windows: Vec<OrgKdePlasmaWindow> = uuids
        .iter()
        .map(|uuid| {
            plasma
                .manager
                .get_window_by_uuid(uuid.clone(), &plasma.queue.handle(), uuid.clone())
        })
        .collect();
    plasma.roundtrip("KWin window owners", |_| true)?;
    for window in windows {
        window.destroy();
    }
    let _ = plasma.connection.flush();
    Ok(plasma.state.pids)
}

/// The window's owner: KWin's own answer when it gives one, else the announced pid.
fn owning_pid(properties: &Properties, announced: Option<u32>) -> anyhow::Result<u32> {
    if properties.contains_key("pid") {
        return number(properties, "pid");
    }
    announced.context("KWin omitted pid")
}

#[derive(Default)]
struct WindowIds {
    uuids: Vec<String>,
}

impl WindowIds {
    fn id_for(&mut self, uuid: &str) -> anyhow::Result<u64> {
        let index = match self.uuids.iter().position(|known| known == uuid) {
            Some(index) => index,
            None => {
                self.uuids.push(uuid.to_owned());
                self.uuids.len() - 1
            }
        };
        ensure!(index < 0x1000_0000, "KWin window id space exhausted");
        Ok(0xE000_0000 + index as u64)
    }

    fn uuid_for(&self, id: u64) -> Option<String> {
        self.uuids
            .get(usize::try_from(id.checked_sub(0xE000_0000)?).ok()?)
            .cloned()
    }
}

fn ids() -> &'static Mutex<WindowIds> {
    static IDS: OnceLock<Mutex<WindowIds>> = OnceLock::new();
    IDS.get_or_init(|| Mutex::new(WindowIds::default()))
}

/// Whether this session could be KWin, so other compositors skip its probes.
/// A daemon started without a desktop name still probes: an unset name says
/// nothing, and skipping would lose the exact-window route on a real KWin.
pub(crate) fn may_be_session() -> bool {
    desktop_may_be_kde(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref())
}

fn desktop_may_be_kde(desktop: Option<&str>) -> bool {
    desktop.is_none_or(|desktop| {
        desktop.trim().is_empty()
            || desktop
                .split(':')
                .any(|name| name.trim().eq_ignore_ascii_case("kde"))
    })
}

/// A KWin handle the adapter may act on: listed by it, and still switched on.
/// Once switched off, its handles are unknown ids, as on any other route.
pub(crate) fn knows_window(window_id: u64) -> bool {
    listed(window_id) && super::kwin_access::allowed()
}

fn listed(window_id: u64) -> bool {
    ids()
        .lock()
        .is_ok_and(|ids| ids.uuid_for(window_id).is_some())
}

pub(super) fn run<T: Send + 'static>(
    work: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
) -> anyhow::Result<T> {
    // Window inspection also calls this from Tokio tasks; a separate thread avoids a nested runtime.
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                tokio::time::timeout(TIMEOUT, work)
                    .await
                    .context("KWin capture timed out")?
            })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("KWin capture worker panicked"))?
}

async fn owner(connection: &zbus::Connection) -> anyhow::Result<String> {
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    let owner = dbus.get_name_owner(KWIN.try_into()?).await?;
    let pid = dbus
        .get_connection_unix_process_id(owner.clone().into())
        .await?;
    let uid = dbus.get_connection_unix_user(owner.clone().into()).await?;
    ensure!(
        uid == super::kwin_helper::current_uid()
            && super::kwin_helper::is_trusted_kwin_process(pid),
        "KWin D-Bus owner is not the trusted session compositor"
    );
    Ok(owner.to_string())
}

async fn info(
    connection: &zbus::Connection,
    owner: &str,
    uuid: &str,
) -> anyhow::Result<Properties> {
    let proxy = zbus::Proxy::new(connection, owner, "/KWin", KWIN).await?;
    proxy
        .call("getWindowInfo", &(uuid,))
        .await
        .context("KWin window lookup failed")
}

/// KWin writes a UUID with or without braces depending on the call.
fn same_uuid(left: &str, right: &str) -> bool {
    left.trim_matches(['{', '}']) == right.trim_matches(['{', '}'])
}

fn number(properties: &Properties, name: &str) -> anyhow::Result<u32> {
    let value = properties
        .get(name)
        .with_context(|| format!("KWin omitted {name}"))?;
    u32::try_from(value)
        .ok()
        .or_else(|| i32::try_from(value).ok().and_then(|n| n.try_into().ok()))
        .with_context(|| format!("KWin returned invalid {name}"))
}

fn text<'a>(properties: &'a Properties, name: &str) -> anyhow::Result<&'a str> {
    <&str>::try_from(
        properties
            .get(name)
            .context(format!("KWin omitted {name}"))?,
    )
    .with_context(|| format!("KWin returned invalid {name}"))
}

fn coordinate(properties: &Properties, name: &str) -> anyhow::Result<f64> {
    let value = properties
        .get(name)
        .with_context(|| format!("KWin omitted {name}"))?;
    f64::try_from(value)
        .ok()
        .or_else(|| i32::try_from(value).ok().map(f64::from))
        .filter(|number| {
            number.is_finite() && *number >= i32::MIN as f64 && *number <= i32::MAX as f64
        })
        .with_context(|| format!("KWin returned invalid {name}"))
}

async fn current_activity(connection: &zbus::Connection) -> anyhow::Result<Option<String>> {
    const SERVICE: &str = "org.kde.ActivityManager";
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    if !dbus.name_has_owner(SERVICE.try_into()?).await? {
        return Ok(None);
    }
    let proxy = zbus::Proxy::new(
        connection,
        SERVICE,
        "/ActivityManager/Activities",
        "org.kde.ActivityManager.Activities",
    )
    .await?;
    let activity: String = proxy.call("CurrentActivity", &()).await?;
    Ok((!activity.is_empty()).then_some(activity))
}

fn on_current_activity(properties: &Properties, current: Option<&str>) -> anyhow::Result<bool> {
    let Some(activities) = properties.get("activities") else {
        return Ok(true);
    };
    let activities = Vec::<String>::try_from(activities.try_clone()?)
        .context("KWin returned invalid activities")?;
    Ok(activities.is_empty()
        || current.is_some_and(|current| activities.iter().any(|id| id == current)))
}

pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
    let (uuids, stacking) = window_snapshot(Connection::connect_to_env()?)?;
    let pids = window_pids(Connection::connect_to_env()?, &uuids)?;
    let stacking: HashMap<_, _> = stacking
        .into_iter()
        .enumerate()
        .map(|(index, uuid)| (uuid, index))
        .collect();
    run(async move {
        let connection = zbus::Connection::session().await?;
        let owner = owner(&connection).await?;
        let desktop = zbus::Proxy::new(
            &connection,
            owner.as_str(),
            "/VirtualDesktopManager",
            "org.kde.KWin.VirtualDesktopManager",
        )
        .await?
        .get_property::<String>("current")
        .await?;
        let activity = current_activity(&connection).await?;
        let mut windows = Vec::new();
        for uuid in uuids {
            let properties = info(&connection, &owner, &uuid).await?;
            if properties.is_empty() {
                continue;
            }
            ensure!(
                same_uuid(text(&properties, "uuid")?, &uuid),
                "KWin window identity mismatch"
            );
            let width = coordinate(&properties, "width")?;
            let height = coordinate(&properties, "height")?;
            ensure!(
                width >= 0.0 && height >= 0.0,
                "KWin returned negative dimensions"
            );
            let (width, height) = (width.round() as u32, height.round() as u32);
            let desktops = Vec::<String>::try_from(
                properties
                    .get("desktops")
                    .context("KWin omitted desktops")?
                    .try_clone()?,
            )?;
            let minimized = bool::try_from(
                properties
                    .get("minimized")
                    .context("KWin omitted minimized")?,
            )?;
            windows.push(WindowInfo {
                xid: ids()
                    .lock()
                    .map_err(|_| anyhow::anyhow!("KWin id registry poisoned"))?
                    .id_for(&uuid)?,
                pid: owning_pid(&properties, pids.get(&uuid).copied()).ok(),
                app_name: text(&properties, "resourceClass")?.to_owned(),
                title: text(&properties, "caption")?.to_owned(),
                is_on_screen: !minimized
                    && width > 0
                    && height > 0
                    && (desktops.is_empty() || desktops.contains(&desktop))
                    && on_current_activity(&properties, activity.as_deref())?,
                z_index: stacking.get(&uuid).copied(),
                x: coordinate(&properties, "x")?.round() as i32,
                y: coordinate(&properties, "y")?.round() as i32,
                width,
                height,
            });
        }
        Ok(windows)
    })
}

fn is_active(state: Option<u32>) -> bool {
    state.is_some_and(|flags| flags & management::State::Active as u32 != 0)
}

/// Make one exact window the active window, past focus stealing prevention,
/// and confirm it from KWin's own state. Input sent afterwards still goes to
/// whatever holds focus at that moment; the caller must not treat this as a
/// binding between the window and later global input.
pub fn activate(window_id: u64, pid: Option<u32>) -> anyhow::Result<()> {
    let uuid = ids()
        .lock()
        .map_err(|_| anyhow::anyhow!("KWin id registry poisoned"))?
        .uuid_for(window_id)
        .context("KWin window was not enumerated in this connection")?;
    ensure!(
        list_windows()?
            .iter()
            .any(|window| window.xid == window_id && pid.is_none_or(|pid| window.pid == Some(pid))),
        "foreground_unavailable: window {window_id} no longer exists or belongs to another process"
    );
    let plasma = Plasma::connect(Connection::connect_to_env()?)
        .context("foreground_unavailable: KWin window management protocol unavailable")?;
    request_active(plasma, uuid)
        .context("foreground_unavailable: KWin did not confirm the window became active")
}

/// Ask KWin to activate `uuid` and wait until it reports Active as of after the
/// request. A flag cached before KWin read `set_state` may be stale, so nothing
/// counts until the reply to a sync queued behind the request: by then every
/// state change KWin made up to that point has been dispatched.
fn request_active(mut plasma: Plasma, uuid: String) -> anyhow::Result<()> {
    let window = plasma
        .manager
        .get_window_by_uuid(uuid.clone(), &plasma.queue.handle(), uuid);
    plasma.roundtrip("KWin window state", |_| true)?;
    let active = management::State::Active as u32;
    window.set_state(active, active);
    plasma.connection.display().sync(&plasma.queue.handle(), ());
    let after_request = plasma.state.roundtrips + 1;
    let confirmed = super::hyprland_capture::dispatch_until(
        &mut plasma.queue,
        &mut plasma.state,
        Instant::now() + Duration::from_secs(1),
        "KWin activation",
        |state| state.roundtrips >= after_request && is_active(state.target_state),
    );
    window.destroy();
    let _ = plasma.connection.flush();
    confirmed
}

pub fn capture(window_id: u64, pid: Option<u32>) -> anyhow::Result<Vec<u8>> {
    let uuid = ids()
        .lock()
        .map_err(|_| anyhow::anyhow!("KWin id registry poisoned"))?
        .uuid_for(window_id)
        .context("KWin window was not enumerated in this connection")?;
    let announced = window_pids(Connection::connect_to_env()?, std::slice::from_ref(&uuid))?
        .get(&uuid)
        .copied();
    run(async move {
        let connection = zbus::Connection::session().await?;
        let owner = owner(&connection).await?;
        let before = info(&connection, &owner, &uuid).await?;
        ensure!(!before.is_empty(), "KWin window no longer exists");
        let dbus = zbus::fdo::DBusProxy::new(&connection).await?;
        ensure!(
            dbus.get_name_owner(SCREENSHOT.try_into()?).await?.as_str() == owner,
            "KWin screenshot service has a different owner"
        );
        ensure!(
            same_uuid(text(&before, "uuid")?, &uuid),
            "KWin window identity mismatch"
        );
        let actual_pid = owning_pid(&before, announced)?;
        ensure!(
            pid.is_none_or(|pid| pid == actual_pid),
            "KWin window PID mismatch"
        );
        let (result, mut reader) = request_image(
            &connection,
            &owner,
            Some(&uuid),
            [
                ("include-decoration", true.into()),
                ("include-shadow", false.into()),
                ("include-cursor", false.into()),
                ("native-resolution", false.into()),
            ]
            .into(),
        )
        .await?;
        ensure!(
            same_uuid(text(&result, "windowId")?, &uuid),
            "KWin screenshot window identity mismatch"
        );
        let png = read_image(&result, &mut reader).await?;
        let after = info(&connection, &owner, &uuid).await?;
        ensure!(
            owning_pid(&after, announced)? == actual_pid,
            "KWin window changed during capture"
        );
        let (width, height) = logical_size(
            (number(&result, "width")?, number(&result, "height")?),
            coordinate(&result, "scale")?,
            (
                coordinate(&after, "width")?.round() as u32,
                coordinate(&after, "height")?.round() as u32,
            ),
        )?;
        to_window_frame(png, width, height)
    })
}

/// The logical size of an image KWin captured: its pixels over KWin's scale for
/// it. A fractional scale can round that a pixel off the frame KWin reports, so
/// the frame wins within a pixel. Further off, the window was resized around the
/// capture and the image knows its own size: the frame would stretch it.
fn logical_size(pixels: (u32, u32), scale: f64, frame: (u32, u32)) -> anyhow::Result<(u32, u32)> {
    ensure!(scale > 0.0, "KWin returned invalid scale");
    let size = (
        (f64::from(pixels.0) / scale).round() as u32,
        (f64::from(pixels.1) / scale).round() as u32,
    );
    Ok(
        if size.0.abs_diff(frame.0) <= 1 && size.1.abs_diff(frame.1) <= 1 {
            frame
        } else {
            size
        },
    )
}

/// KWin renders a window at its output's scale even when asked for logical
/// resolution on some releases. Window-local coordinates are logical, so the
/// image must be too or a pixel read off it names a different spot.
fn to_window_frame(png: Vec<u8>, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    if crate::capture::png_dimensions_pub(&png)? == (width, height) {
        return Ok(png);
    }
    ensure!(width > 0 && height > 0, "KWin window has no size");
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .resize_exact(width, height, image::imageops::FilterType::Lanczos3)
        .write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

async fn read_image(
    result: &Properties,
    reader: &mut tokio::net::UnixStream,
) -> anyhow::Result<Vec<u8>> {
    let (width, height, stride, format) = (
        number(result, "width")?,
        number(result, "height")?,
        number(result, "stride")?,
        number(result, "format")?,
    );
    let size = image_size(width, height, stride, format)?;
    let mut pixels = vec![0; size];
    reader.read_exact(&mut pixels).await?;
    decode(width, height, stride, format, &pixels)
}

/// The whole desktop, every output included, in logical pixels: KWin's layout
/// bounding box, the frame `kwin_layout` reads. KWin answers directly, with no
/// portal request or prompt, and never consults a Deny saved in the portal, so
/// the caller checks the operator's switch first. KWin 6.7 and older refuse an
/// executable without the ScreenShot2 grant; KWin 6.8 has no such check.
pub fn capture_workspace() -> anyhow::Result<Vec<u8>> {
    run(async move {
        let connection = zbus::Connection::session().await?;
        let owner = owner(&connection).await?;
        workspace_image(&connection, &owner).await
    })
}

async fn workspace_image(connection: &zbus::Connection, owner: &str) -> anyhow::Result<Vec<u8>> {
    let (result, mut reader) = request_image(
        connection,
        owner,
        None,
        [
            ("include-cursor", false.into()),
            // The action frame is logical. Asking for it here avoids encoding
            // a full-size image only to decode and shrink it afterwards.
            ("native-resolution", false.into()),
        ]
        .into(),
    )
    .await?;
    read_image(&result, &mut reader).await
}

/// Ask ScreenShot2 for one window, or the workspace with no `window`. The image
/// comes back on the returned pipe, described by the returned properties.
async fn request_image(
    connection: &zbus::Connection,
    owner: &str,
    window: Option<&str>,
    options: HashMap<&str, zbus::zvariant::Value<'_>>,
) -> anyhow::Result<(Properties, tokio::net::UnixStream)> {
    let (reader, writer) = std::os::unix::net::UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    let reader = tokio::net::UnixStream::from_std(reader)?;
    let proxy =
        zbus::Proxy::new(connection, owner, "/org/kde/KWin/ScreenShot2", SCREENSHOT).await?;
    let pipe = Fd::from(writer.as_fd());
    let result: Properties = match window {
        Some(window) => {
            proxy
                .call("CaptureWindow", &(window, options, pipe))
                .await?
        }
        None => proxy.call("CaptureWorkspace", &(options, pipe)).await?,
    };
    drop(writer);
    ensure!(
        text(&result, "type")? == "raw",
        "KWin screenshot is not raw image data"
    );
    Ok((result, reader))
}

fn image_size(width: u32, height: u32, stride: u32, format: u32) -> anyhow::Result<usize> {
    ensure!(
        width > 0 && height > 0 && width <= 16384 && height <= 16384,
        "KWin screenshot dimensions out of range"
    );
    ensure!(
        matches!(format, 4 | 5 | 6 | 16 | 17 | 18),
        "Unsupported KWin screenshot format {format}"
    );
    ensure!(stride >= width * 4, "KWin screenshot stride is too small");
    let size = u64::from(stride) * u64::from(height);
    ensure!(size <= 128 * 1024 * 1024, "KWin screenshot exceeds 128 MiB");
    Ok(size as usize)
}

fn decode(
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
    pixels: &[u8],
) -> anyhow::Result<Vec<u8>> {
    ensure!(
        pixels.len() == image_size(width, height, stride, format)?,
        "KWin screenshot data length mismatch"
    );
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in pixels.chunks_exact(stride as usize) {
        for pixel in row[..width as usize * 4].chunks_exact(4) {
            let mut color = if format <= 6 {
                let argb = u32::from_ne_bytes(pixel.try_into()?);
                [
                    (argb >> 16) as u8,
                    (argb >> 8) as u8,
                    argb as u8,
                    (argb >> 24) as u8,
                ]
            } else {
                pixel.try_into()?
            };
            if matches!(format, 4 | 16) {
                color[3] = 255;
            }
            if matches!(format, 6 | 18) {
                let alpha = u32::from(color[3]);
                for channel in &mut color[..3] {
                    *channel = (u32::from(*channel) * 255 + alpha / 2)
                        .checked_div(alpha)
                        .map_or(0, |value| value.min(255) as u8);
                }
            }
            rgba.extend_from_slice(&color);
        }
    }
    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_capture_is_delivered_in_the_window_frame() {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(1000, 900)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let png = encoded.into_inner();
        let scaled = to_window_frame(png.clone(), 800, 720).unwrap();
        assert_eq!(
            crate::capture::png_dimensions_pub(&scaled).unwrap(),
            (800, 720)
        );
        // Already in the frame: the same bytes, no needless recompression.
        assert_eq!(to_window_frame(png.clone(), 1000, 900).unwrap(), png);
        assert_eq!(
            to_window_frame(png, 0, 0).unwrap_err().to_string(),
            "KWin window has no size"
        );
    }

    #[test]
    fn the_owner_comes_from_the_protocol_when_getwindowinfo_has_no_pid() {
        // KWin 6.6 answers getWindowInfo without a pid field.
        let without: Properties = [("caption".to_string(), OwnedValue::from(1i32))].into();
        assert_eq!(owning_pid(&without, Some(42)).unwrap(), 42);
        assert_eq!(
            owning_pid(&without, None).unwrap_err().to_string(),
            "KWin omitted pid"
        );

        // A newer KWin's own answer wins over the announcement.
        let with: Properties = [("pid".to_string(), OwnedValue::from(7i32))].into();
        assert_eq!(owning_pid(&with, Some(42)).unwrap(), 7);
    }

    #[test]
    fn active_is_read_from_the_state_flags_only_when_kwin_reported_them() {
        let active = management::State::Active as u32;
        assert!(!is_active(None));
        assert!(!is_active(Some(0)));
        assert!(!is_active(Some(!active)));
        assert!(is_active(Some(active)));
        assert!(is_active(Some(
            active | management::State::Maximized as u32
        )));
    }

    #[test]
    fn a_capture_keeps_its_own_size_when_the_window_was_resized_around_it() {
        // Scale 1.25, 800x720 window: the physical image lands on its frame.
        assert_eq!(
            logical_size((1000, 900), 1.25, (800, 720)).unwrap(),
            (800, 720)
        );
        // Rounding at a fractional scale leaves it a pixel off: the frame wins.
        assert_eq!(
            logical_size((1002, 900), 1.25, (801, 720)).unwrap(),
            (801, 720)
        );
        // Resized to 600x400 after the image was taken: not stretched to fit.
        assert_eq!(
            logical_size((1000, 900), 1.25, (600, 400)).unwrap(),
            (800, 720)
        );
        assert_eq!(
            logical_size((800, 720), 1.0, (600, 400)).unwrap(),
            (800, 720)
        );
        assert_eq!(
            logical_size((800, 720), 0.0, (800, 720))
                .unwrap_err()
                .to_string(),
            "KWin returned invalid scale"
        );
    }

    #[test]
    fn only_a_session_that_could_be_kde_gets_the_kwin_probes() {
        for desktop in [None, Some(""), Some("KDE"), Some("kde"), Some("ubuntu:KDE")] {
            assert!(desktop_may_be_kde(desktop), "{desktop:?}");
        }
        for desktop in ["sway", "wlroots", "GNOME", "ubuntu:GNOME", "niri", "COSMIC"] {
            assert!(!desktop_may_be_kde(Some(desktop)), "{desktop}");
        }
    }

    #[test]
    fn raw_input_at_a_kwin_handle_gets_the_refusal_any_compositor_gives() {
        // Global input after activating would reach whatever holds focus when it
        // lands, so a KWin handle is refused like one with no activation adapter.
        let window = ids().lock().unwrap().id_for("raw-input-refusal").unwrap();
        assert!(listed(window));
        assert_eq!(
            super::super::no_input_activation(window).to_string(),
            format!(
                "foreground_unavailable: this Wayland compositor does not expose a verified, \
                 target-addressable activation adapter for window {window}; refusing global \
                 input because it could affect the wrong application"
            )
        );
    }

    /// KWin's `ScreenShot2.CaptureWorkspace`: writes a 3x2 image to the pipe
    /// and records the options it was asked for.
    struct FakeScreenShot2 {
        kind: &'static str,
        asked: std::sync::Arc<Mutex<Option<Properties>>>,
    }

    #[zbus::interface(name = "org.kde.KWin.ScreenShot2")]
    impl FakeScreenShot2 {
        async fn capture_workspace(
            &self,
            options: Properties,
            pipe: zbus::zvariant::OwnedFd,
        ) -> Properties {
            use std::io::Write;
            *self.asked.lock().unwrap() = Some(options);
            // Format 5 (ARGB32), stride padded to 16 bytes: red, green, blue on
            // the first row, white on the second.
            let mut pixels = Vec::new();
            for row in [[0xFFFF_0000u32, 0xFF00_FF00, 0xFF00_00FF], [0xFFFF_FFFF; 3]] {
                row.iter()
                    .for_each(|pixel| pixels.extend(pixel.to_ne_bytes()));
                pixels.extend([0; 4]);
            }
            let mut pipe = std::fs::File::from(std::os::fd::OwnedFd::from(pipe));
            pipe.write_all(&pixels).unwrap();
            [
                (
                    "type",
                    OwnedValue::from(zbus::zvariant::Str::from(self.kind)),
                ),
                ("width", 3u32.into()),
                ("height", 2u32.into()),
                ("stride", 16u32.into()),
                ("format", 5u32.into()),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
        }
    }

    /// `workspace_image` against a fake KWin on a private bus, or None (skip)
    /// when `dbus-daemon` is not installed.
    fn workspace_from_fake(kind: &'static str) -> Option<(anyhow::Result<Vec<u8>>, Properties)> {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<busconfig><type>session</type><listen>unix:path={}</listen>\
                 <auth>EXTERNAL</auth><policy context=\"default\">\
                 <allow send_destination=\"*\"/><allow receive_sender=\"*\"/>\
                 <allow own=\"*\"/></policy></busconfig>",
                directory.path().join("bus").display()
            ),
        )
        .unwrap();
        let mut daemon = match std::process::Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(daemon) => daemon,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping: dbus-daemon is not installed");
                return None;
            }
            Err(error) => panic!("dbus-daemon: {error}"),
        };
        let mut address = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(daemon.stdout.take().unwrap()),
            &mut address,
        )
        .unwrap();
        let asked = std::sync::Arc::new(Mutex::new(None));
        let fake = FakeScreenShot2 {
            kind,
            asked: asked.clone(),
        };
        let exchange = async {
            let kwin = zbus::connection::Builder::address(address.trim())?
                .name(KWIN)?
                .serve_at("/org/kde/KWin/ScreenShot2", fake)?
                .build()
                .await?;
            let owner = kwin.unique_name().unwrap().to_string();
            let client = zbus::connection::Builder::address(address.trim())?
                .build()
                .await?;
            workspace_image(&client, &owner).await
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { tokio::time::timeout(TIMEOUT, exchange).await? });
        let _ = daemon.kill();
        let _ = daemon.wait();
        let asked = asked.lock().unwrap().take().unwrap_or_default();
        Some((result, asked))
    }

    #[test]
    fn the_workspace_is_asked_for_in_logical_pixels_without_the_cursor_and_decoded() {
        let Some((png, asked)) = workspace_from_fake("raw") else {
            return;
        };
        let flag = |name: &str| asked.get(name).and_then(|value| bool::try_from(value).ok());
        assert_eq!(flag("native-resolution"), Some(false));
        assert_eq!(flag("include-cursor"), Some(false));
        let image = image::load_from_memory(&png.unwrap()).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (3, 2));
        let pixels: Vec<[u8; 4]> = image.pixels().map(|pixel| pixel.0).collect();
        assert_eq!(
            pixels,
            [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
            ]
        );
    }

    #[test]
    fn a_workspace_reply_that_is_not_raw_pixels_is_an_error() {
        let Some((png, _)) = workspace_from_fake("png") else {
            return;
        };
        assert_eq!(
            png.unwrap_err().to_string(),
            "KWin screenshot is not raw image data"
        );
    }

    fn string(value: &str) -> Vec<u8> {
        let mut bytes = ((value.len() + 1) as u32).to_ne_bytes().to_vec();
        bytes.extend(value.as_bytes());
        bytes.push(0);
        bytes.resize(bytes.len().next_multiple_of(4), 0);
        bytes
    }

    /// A request from the client: object id, opcode and payload, or None at hang-up.
    fn receive(server: &mut std::os::unix::net::UnixStream) -> Option<(u32, u32, Vec<u8>)> {
        use std::io::Read;
        let mut header = [0; 8];
        match server.read_exact(&mut header) {
            Ok(()) => {}
            // A client that hangs up with unread replies resets the socket.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                return None
            }
            Err(error) => panic!("Wayland fixture: {error}"),
        }
        let word = u32::from_ne_bytes(header[4..].try_into().unwrap());
        let mut payload = vec![0; (word >> 16) as usize - 8];
        server.read_exact(&mut payload).unwrap();
        Some((
            u32::from_ne_bytes(header[..4].try_into().unwrap()),
            word & 0xffff,
            payload,
        ))
    }

    #[test]
    fn activation_is_confirmed_only_by_state_kwin_reports_after_the_request() {
        use std::os::unix::net::UnixStream;

        fn send(socket: &mut UnixStream, id: u32, opcode: u32, payload: &[u8]) {
            use std::io::Write;
            let mut message = id.to_ne_bytes().to_vec();
            message.extend(((payload.len() as u32 + 8) << 16 | opcode).to_ne_bytes());
            message.extend(payload);
            let _ = socket.write_all(&message);
        }
        let active = management::State::Active as u32;
        let state_changed = u32::from(plasma_window::EVT_STATE_CHANGED_OPCODE);
        // The window's state when bound, then any change KWin sends once it has
        // read set_state. Active at bind but inactive by then: the bind-time flag
        // is stale and must not count. Granted: confirmed. Active at bind with no
        // change: still active as of the sync reply, so confirmed.
        for (before, after, confirmed) in [
            (active, Some(0), false),
            (0, Some(active), true),
            (active, None, true),
        ] {
            let (client, mut server) = UnixStream::pair().unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let fixture = std::thread::spawn(move || {
                let (mut registry, mut manager, mut window) = (0, 0, 0);
                let mut requested = None;
                while let Some((id, opcode, payload)) = receive(&mut server) {
                    let argument = |offset: usize| {
                        u32::from_ne_bytes(payload[offset..offset + 4].try_into().unwrap())
                    };
                    if id == 1 && opcode == 1 {
                        registry = argument(0);
                        let mut global = 7u32.to_ne_bytes().to_vec();
                        global.extend(string(OrgKdePlasmaWindowManagement::interface().name));
                        global.extend(17u32.to_ne_bytes());
                        send(&mut server, registry, 0, &global);
                    } else if id == 1 && opcode == 0 {
                        let callback = argument(0);
                        send(&mut server, callback, 0, &0u32.to_ne_bytes());
                        send(&mut server, 1, 1, &callback.to_ne_bytes());
                    } else if id == registry && opcode == 0 {
                        manager = argument(payload.len() - 4);
                    } else if id == manager
                        && opcode == u32::from(management::REQ_GET_WINDOW_BY_UUID_OPCODE)
                    {
                        window = argument(0);
                        send(&mut server, window, state_changed, &before.to_ne_bytes());
                    } else if id == window
                        && opcode == u32::from(plasma_window::REQ_SET_STATE_OPCODE)
                    {
                        requested = Some((argument(0), argument(4)));
                        if let Some(after) = after {
                            send(&mut server, window, state_changed, &after.to_ne_bytes());
                        }
                    } else if id == window && opcode == u32::from(plasma_window::REQ_DESTROY_OPCODE)
                    {
                        break;
                    } else {
                        panic!("unexpected Wayland request {id}/{opcode}");
                    }
                }
                assert_eq!(requested, Some((active, active)));
            });
            let plasma = Plasma::connect(Connection::from_socket(client).unwrap()).unwrap();
            let result = request_active(plasma, "window".into());
            assert_eq!(
                result.is_ok(),
                confirmed,
                "{before} then {after:?}: {result:?}"
            );
            if !confirmed {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "wayland capture timed out waiting for KWin activation"
                );
            }
            fixture.join().unwrap();
        }
    }

    #[test]
    fn uuid_protocol_versions_preserve_compositor_stacking() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        fn send(socket: &mut UnixStream, id: u32, opcode: u32, payload: &[u8]) {
            let mut message = id.to_ne_bytes().to_vec();
            message.extend(((payload.len() as u32 + 8) << 16 | opcode).to_ne_bytes());
            message.extend(payload);
            // The client may finish and hang up before the trailing delete_id. A
            // real missing event still fails the client's own assertions.
            match socket.write_all(&message) {
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ) => {}
                result => result.unwrap(),
            }
        }
        for advertised in [12u32, 13, 14, 15, 16, 17, 20] {
            let (client, mut server) = UnixStream::pair().unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let fixture = std::thread::spawn(move || {
                let mut registry = 0;
                let mut manager = 0;
                let mut bound = None;
                let mut requested_stacking = false;
                while let Some((id, opcode, payload)) = receive(&mut server) {
                    let argument = |offset| {
                        u32::from_ne_bytes(payload[offset..offset + 4].try_into().unwrap())
                    };
                    if id == 1 && opcode == 1 {
                        registry = argument(0);
                        let mut global = 7u32.to_ne_bytes().to_vec();
                        global.extend(string(OrgKdePlasmaWindowManagement::interface().name));
                        global.extend(advertised.to_ne_bytes());
                        send(&mut server, registry, 0, &global);
                    } else if id == 1 && opcode == 0 {
                        let callback = argument(0);
                        send(&mut server, callback, 0, &0u32.to_ne_bytes());
                        send(&mut server, 1, 1, &callback.to_ne_bytes());
                    } else if id == registry && opcode == 0 {
                        let version = argument(payload.len() - 8);
                        assert_eq!(version, advertised.min(17));
                        bound = Some(version);
                        manager = argument(payload.len() - 4);
                        for (index, uuid) in ["bottom", "top"].into_iter().enumerate() {
                            let mut window = (index as u32 + 1).to_ne_bytes().to_vec();
                            window.extend(string(uuid));
                            send(&mut server, manager, 4, &window);
                        }
                        if version < 17 {
                            send(&mut server, manager, 3, &string("top;bottom"));
                        }
                    } else if id == manager && opcode == 3 {
                        assert_eq!(bound, Some(17));
                        requested_stacking = true;
                        let stacking = argument(0);
                        for uuid in ["top", "bottom"] {
                            send(&mut server, stacking, 0, &string(uuid));
                        }
                        send(&mut server, stacking, 1, &[]);
                        send(&mut server, 1, 1, &stacking.to_ne_bytes());
                    } else {
                        panic!("unexpected Wayland request {id}/{opcode}");
                    }
                }
                assert_eq!(bound, (advertised >= 13).then_some(advertised.min(17)));
                assert_eq!(requested_stacking, advertised >= 17);
            });
            let result = window_snapshot(Connection::from_socket(client).unwrap());
            if advertised < 13 {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "KWin window management protocol unavailable"
                );
            } else {
                let (windows, stacking) = result.unwrap();
                assert_eq!(windows, ["bottom", "top"]);
                assert_eq!(stacking, ["top", "bottom"]);
            }
            fixture.join().unwrap();
        }
    }

    #[test]
    fn activity_visibility_handles_disabled_all_current_and_inactive_membership() {
        let mut properties = Properties::new();
        assert!(on_current_activity(&properties, None).unwrap());
        for (activities, current, expected) in [
            (vec![], None, true),
            (vec![], Some("current"), true),
            (vec!["current"], Some("current"), true),
            (vec!["other", "current"], Some("current"), true),
            (vec!["other"], Some("current"), false),
            (vec!["other"], None, false),
        ] {
            properties.insert(
                "activities".into(),
                zbus::zvariant::Value::from(activities).try_into().unwrap(),
            );
            assert_eq!(on_current_activity(&properties, current).unwrap(), expected);
        }
        properties.insert("activities".into(), 42u32.into());
        assert_eq!(
            on_current_activity(&properties, Some("current"))
                .unwrap_err()
                .to_string(),
            "KWin returned invalid activities"
        );
    }

    #[test]
    fn qt_property_types_are_validated_before_use() {
        let properties: Properties = [
            ("pid".into(), 42i32.into()),
            ("width".into(), 400i32.into()),
            ("height".into(), 240.5f64.into()),
            ("x".into(), (-12.5f64).into()),
            ("invalid".into(), f64::NAN.into()),
            ("negative".into(), (-1i32).into()),
        ]
        .into();
        assert_eq!(number(&properties, "pid").unwrap(), 42);
        assert_eq!(coordinate(&properties, "width").unwrap(), 400.0);
        assert_eq!(coordinate(&properties, "height").unwrap(), 240.5);
        assert_eq!(coordinate(&properties, "x").unwrap(), -12.5);
        assert_eq!(
            number(&properties, "negative").unwrap_err().to_string(),
            "KWin returned invalid negative"
        );
        assert_eq!(
            coordinate(&properties, "invalid").unwrap_err().to_string(),
            "KWin returned invalid invalid"
        );
        assert_eq!(
            coordinate(&properties, "absent").unwrap_err().to_string(),
            "KWin omitted absent"
        );
    }

    #[test]
    fn window_ids_are_stable_and_do_not_accept_other_namespaces() {
        let mut ids = WindowIds::default();
        let first = ids.id_for("first").unwrap();
        let second = ids.id_for("second").unwrap();
        assert_eq!(ids.id_for("first").unwrap(), first);
        assert_ne!(first, second);
        assert_eq!(ids.uuid_for(first).as_deref(), Some("first"));
        for id in [0, 0xDFFF_FFFF, 0xF000_0000, u64::MAX] {
            assert_eq!(ids.uuid_for(id), None);
        }
    }

    #[test]
    fn screenshot_decoding_handles_padding_alpha_and_invalid_layouts() {
        for format in [4, 5, 6, 16, 17, 18] {
            let alpha = if matches!(format, 4 | 16) { 255 } else { 85 };
            let expected = [153, 102, 51, alpha];
            let color = if matches!(format, 6 | 18) {
                [51, 34, 17, alpha]
            } else {
                expected
            };
            let pixel = if format <= 6 {
                ((u32::from(color[3]) << 24)
                    | (u32::from(color[0]) << 16)
                    | (u32::from(color[1]) << 8)
                    | u32::from(color[2]))
                .to_ne_bytes()
            } else {
                color
            };
            let data = [pixel.as_slice(), &[99; 4], pixel.as_slice(), &[99; 4]].concat();
            let png = decode(1, 2, 8, format, &data).unwrap();
            let image = image::load_from_memory(&png).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), (1, 2));
            assert!(image.pixels().all(|pixel| pixel.0 == expected));
        }
        let transparent = decode(1, 1, 4, 6, &[0, 0, 0, 0]).unwrap();
        assert_eq!(
            image::load_from_memory(&transparent)
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0)
                .0,
            [0; 4]
        );
        for (args, error) in [
            ((0, 1, 4, 6), "KWin screenshot dimensions out of range"),
            ((1, 0, 4, 6), "KWin screenshot dimensions out of range"),
            (
                (16385, 1, 65540, 6),
                "KWin screenshot dimensions out of range",
            ),
            ((1, 1, 3, 6), "KWin screenshot stride is too small"),
            ((1, 1, 4, 99), "Unsupported KWin screenshot format 99"),
            ((1, 2, u32::MAX, 6), "KWin screenshot exceeds 128 MiB"),
        ] {
            assert_eq!(
                image_size(args.0, args.1, args.2, args.3)
                    .unwrap_err()
                    .to_string(),
                error
            );
        }
        assert_eq!(
            decode(1, 1, 4, 6, &[0; 3]).unwrap_err().to_string(),
            "KWin screenshot data length mismatch"
        );
    }
}
