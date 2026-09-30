use std::collections::HashMap;
use std::os::fd::AsFd;
use std::os::unix::fs::MetadataExt;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{ensure, Context};
use tokio::io::AsyncReadExt;
use wayland_client::{protocol::{wl_callback, wl_registry}, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::{
    self as management, OrgKdePlasmaWindowManagement,
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
    roundtrips: u32,
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
            if interface == OrgKdePlasmaWindowManagement::interface().name && version >= 17 {
                state.manager = Some(registry.bind(name, 17, qh, ()));
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
        if let management::Event::WindowWithUuid { uuid, .. } = event {
            state.uuids.push(uuid);
        }
    }
}

fn window_uuids() -> anyhow::Result<Vec<String>> {
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue::<Windows>();
    connection.display().get_registry(&queue.handle(), ());
    let mut state = Windows::default();
    let deadline = std::time::Instant::now() + TIMEOUT;
    connection.display().sync(&queue.handle(), ());
    super::hyprland_capture::dispatch_until(
        &mut queue,
        &mut state,
        deadline,
        "KWin registry",
        |state| state.roundtrips == 1,
    )?;
    ensure!(
        state.manager.is_some(),
        "KWin window management protocol unavailable"
    );
    connection.display().sync(&queue.handle(), ());
    super::hyprland_capture::dispatch_until(
        &mut queue,
        &mut state,
        deadline,
        "KWin windows",
        |state| state.roundtrips == 2,
    )?;
    state.uuids.sort();
    state.uuids.dedup();
    Ok(state.uuids)
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

pub(super) fn knows_window(window_id: u64) -> bool {
    ids()
        .lock()
        .is_ok_and(|ids| ids.uuid_for(window_id).is_some())
}

fn run<T: Send + 'static>(
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
    let our_uid = std::fs::metadata(format!("/proc/{}", std::process::id()))?.uid();
    ensure!(
        uid == our_uid && super::kwin_helper::is_trusted_kwin_process(pid),
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

pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
    let uuids = window_uuids()?;
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
        let mut windows = Vec::new();
        for uuid in uuids {
            let properties = info(&connection, &owner, &uuid).await?;
            if properties.is_empty() {
                continue;
            }
            ensure!(
                text(&properties, "uuid")?.trim_matches(['{', '}'])
                    == uuid.trim_matches(['{', '}']),
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
                pid: Some(number(&properties, "pid")?),
                app_name: text(&properties, "resourceClass")?.to_owned(),
                title: text(&properties, "caption")?.to_owned(),
                is_on_screen: !minimized
                    && width > 0
                    && height > 0
                    && (desktops.is_empty() || desktops.contains(&desktop)),
                z_index: None,
                x: coordinate(&properties, "x")?.round() as i32,
                y: coordinate(&properties, "y")?.round() as i32,
                width,
                height,
            });
        }
        Ok(windows)
    })
}

pub fn capture(window_id: u64, pid: Option<u32>) -> anyhow::Result<Vec<u8>> {
    let uuid = ids()
        .lock()
        .map_err(|_| anyhow::anyhow!("KWin id registry poisoned"))?
        .uuid_for(window_id)
        .context("KWin window was not enumerated in this connection")?;
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
            text(&before, "uuid")?.trim_matches(['{', '}']) == uuid.trim_matches(['{', '}']),
            "KWin window identity mismatch"
        );
        let actual_pid = number(&before, "pid")?;
        ensure!(
            pid.is_none_or(|pid| pid == actual_pid),
            "KWin window PID mismatch"
        );
        let (reader, writer) = std::os::unix::net::UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        let mut reader = tokio::net::UnixStream::from_std(reader)?;
        let options: HashMap<&str, zbus::zvariant::Value<'_>> = [
            ("include-decoration", true.into()),
            ("include-shadow", false.into()),
            ("include-cursor", false.into()),
            ("native-resolution", false.into()),
        ]
        .into();
        let proxy = zbus::Proxy::new(
            &connection,
            owner.as_str(),
            "/org/kde/KWin/ScreenShot2",
            SCREENSHOT,
        )
        .await?;
        let result: Properties = proxy
            .call("CaptureWindow", &(&uuid, options, Fd::from(writer.as_fd())))
            .await?;
        drop(writer);
        ensure!(
            text(&result, "type")? == "raw",
            "KWin screenshot is not raw image data"
        );
        ensure!(
            text(&result, "windowId")?.trim_matches(['{', '}']) == uuid.trim_matches(['{', '}']),
            "KWin screenshot window identity mismatch"
        );
        let (width, height, stride, format) = (
            number(&result, "width")?,
            number(&result, "height")?,
            number(&result, "stride")?,
            number(&result, "format")?,
        );
        let size = image_size(width, height, stride, format)?;
        let mut pixels = vec![0; size];
        reader.read_exact(&mut pixels).await?;
        let after = info(&connection, &owner, &uuid).await?;
        ensure!(
            number(&after, "pid")? == actual_pid,
            "KWin window changed during capture"
        );
        decode(width, height, stride, format, &pixels)
    })
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
                    *channel = if alpha == 0 {
                        0
                    } else {
                        ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
                    };
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
