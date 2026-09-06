//! Linux host probes shared by `cua-driver doctor` and `cua-driver diagnose`.
//!
//! Headless / Grok-Bot-style VMs (Xvfb, no `/dev/uinput`, no systemd user
//! session, no AT-SPI) are a supported *observation* environment: doctor and
//! diagnose must describe them honestly so an agent knows background MPX
//! pixel input cannot land and should retry with `delivery_mode: foreground`.
//! These probes never install packages and never start AT-SPI.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// `/dev/uinput` open result. Distinguishes "node missing" from "present but
/// this user cannot open it" so doctor can say which remediation applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UinputStatus {
    Accessible,
    Missing,
    Inaccessible,
}

impl UinputStatus {
    pub fn probe() -> Self {
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
        {
            Ok(_) => Self::Accessible,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Err(_) => Self::Inaccessible,
        }
    }

    pub fn is_accessible(self) -> bool {
        matches!(self, Self::Accessible)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Accessible => "accessible",
            Self::Missing => "missing",
            Self::Inaccessible => "inaccessible",
        }
    }
}

/// One doctor line produced from a [`LinuxHostSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProbe {
    pub label: &'static str,
    pub warn: bool,
    pub message: String,
    pub detail: Option<String>,
}

/// Read-only snapshot of the Linux input / session environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxHostSnapshot {
    pub display: Option<String>,
    pub wayland_display: Option<String>,
    pub x_session_type: Option<String>,
    pub x_server: Option<String>,
    pub compositors: Vec<String>,
    pub desktop: Option<String>,
    pub uinput: UinputStatus,
    pub xdg_runtime_dir: Option<String>,
    pub runtime_user_dir: String,
    pub runtime_user_dir_exists: bool,
    pub session_bus_socket: String,
    pub session_bus_socket_exists: bool,
    pub dbus_session_bus_address: Option<String>,
    pub at_spi_reachable: bool,
    pub background_mpx_available: bool,
}

impl LinuxHostSnapshot {
    /// Probe the calling process environment. Cheap and side-effect-free
    /// aside from opening the X display (for the MPX capability check) and
    /// a session-bus name lookup for `org.a11y.Bus`.
    pub fn collect() -> Self {
        let uid = current_uid();
        let runtime_user_dir = format!("/run/user/{uid}");
        let session_bus_socket = format!("{runtime_user_dir}/bus");
        Self {
            display: env_nonempty("DISPLAY"),
            wayland_display: env_nonempty("WAYLAND_DISPLAY"),
            x_session_type: env_nonempty("XDG_SESSION_TYPE"),
            x_server: crate::input::x_server_exe_name(),
            compositors: running_compositors(),
            desktop: env_nonempty("XDG_CURRENT_DESKTOP")
                .or_else(|| env_nonempty("DESKTOP_SESSION")),
            uinput: UinputStatus::probe(),
            xdg_runtime_dir: env_nonempty("XDG_RUNTIME_DIR"),
            runtime_user_dir_exists: Path::new(&runtime_user_dir).is_dir(),
            runtime_user_dir,
            session_bus_socket_exists: Path::new(&session_bus_socket).exists(),
            session_bus_socket,
            dbus_session_bus_address: env_nonempty("DBUS_SESSION_BUS_ADDRESS"),
            at_spi_reachable: crate::health_report::probe_a11y_bus(),
            background_mpx_available: crate::input::real_pointer_input_available(),
        }
    }

    /// Classify the display server for doctor / diagnose. Xvfb and Xtigervnc
    /// are called out by name because they cannot host an MPX/uinput slave.
    pub fn display_server_kind(&self) -> &'static str {
        if self.wayland_display.is_some() && self.display.is_none() {
            return "Wayland";
        }
        match self.x_server.as_deref() {
            Some("Xvfb") => "Xvfb",
            Some("Xorg") => "Xorg",
            Some("Xtigervnc") => "Xtigervnc",
            Some("Xwayland") => {
                if self.wayland_display.is_some() {
                    "Wayland+XWayland"
                } else {
                    "Xwayland"
                }
            }
            Some(_) | None if self.display.is_some() => "X11",
            _ => "none",
        }
    }

    /// True when this host is known not to expose a real X input slave for
    /// uinput (Xvfb has no udev/libinput hotplug; Xtigervnc exposes only its
    /// built-in VNC/XTEST devices).
    pub fn is_virtual_pointer_host(&self) -> bool {
        matches!(self.x_server.as_deref(), Some("Xvfb") | Some("Xtigervnc"))
    }

    pub fn format_diagnose(&self) -> String {
        let display = self.display.as_deref().unwrap_or("unset");
        let wayland = self.wayland_display.as_deref().unwrap_or("unset");
        let session_type = self.x_session_type.as_deref().unwrap_or("unset");
        let x_server = self.x_server.as_deref().unwrap_or("unknown");
        let compositors = if self.compositors.is_empty() {
            "none detected".to_owned()
        } else {
            self.compositors.join(", ")
        };
        let desktop = self.desktop.as_deref().unwrap_or("unset");
        let xdg_runtime = self.xdg_runtime_dir.as_deref().unwrap_or("unset");
        let dbus = self.dbus_session_bus_address.as_deref().unwrap_or("unset");
        let at_spi = if self.at_spi_reachable {
            "reachable"
        } else {
            "not reachable"
        };
        let mpx = if self.background_mpx_available {
            "available".to_owned()
        } else {
            format!("unavailable ({})", self.mpx_unavailable_reason())
        };

        format!(
            "## linux host\n\
             display server:            {kind}\n\
             DISPLAY:                   {display}\n\
             WAYLAND_DISPLAY:           {wayland}\n\
             XDG_SESSION_TYPE:          {session_type}\n\
             X server binary:           {x_server}\n\
             compositor:                {compositors}\n\
             desktop:                   {desktop}\n\
             /dev/uinput:               {uinput}\n\
             XDG_RUNTIME_DIR:           {xdg_runtime}\n\
             /run/user/<uid>:           {runtime} exists={runtime_exists}\n\
             session bus socket:        {bus} exists={bus_exists}\n\
             DBUS_SESSION_BUS_ADDRESS:  {dbus}\n\
             AT-SPI org.a11y.Bus:       {at_spi}\n\
             background MPX:            {mpx}\n\
             \n\
             {hint}",
            kind = self.display_server_kind(),
            uinput = self.uinput.label(),
            runtime = self.runtime_user_dir,
            runtime_exists = self.runtime_user_dir_exists,
            bus = self.session_bus_socket,
            bus_exists = self.session_bus_socket_exists,
            hint = self.background_pointer_hint(),
        )
    }

    pub fn mpx_unavailable_reason(&self) -> String {
        let mut parts = Vec::new();
        if self.is_virtual_pointer_host() {
            parts.push(format!(
                "{} is not a real pointer host",
                self.x_server.as_deref().unwrap_or("this X server")
            ));
        }
        if !self.uinput.is_accessible() {
            parts.push(format!("/dev/uinput is {}", self.uinput.label()));
        }
        if parts.is_empty() {
            "MPX/uinput pointer is not available on this X server".to_owned()
        } else {
            parts.join("; ")
        }
    }

    pub fn background_pointer_hint(&self) -> &'static str {
        if self.background_mpx_available {
            "Background pixel click/scroll/drag can use the MPX/uinput pointer."
        } else {
            "Background pixel click/scroll/drag cannot use MPX/uinput on this host. \
             When AT-SPI-at-point also misses, retry with delivery_mode:\"foreground\"."
        }
    }
}

/// Doctor probes for the Linux host. Warnings (never errors) when the X
/// server cannot host a real pointer or `/dev/uinput` is missing — CI and
/// headless VMs are valid places to run `doctor`.
pub fn doctor_host_probes(snapshot: &LinuxHostSnapshot) -> Vec<HostProbe> {
    let mut probes = Vec::new();

    let display_env = snapshot.display.as_deref();
    let wayland_env = snapshot.wayland_display.as_deref();
    match (display_env, wayland_env, snapshot.display_server_kind()) {
        (None, None, _) => probes.push(HostProbe {
            label: "display server",
            warn: true,
            message: "neither DISPLAY nor WAYLAND_DISPLAY set — window-driving tools will fail"
                .into(),
            detail: Some(
                "run from an interactive desktop session (X11 / Wayland with XWayland) or set DISPLAY explicitly.".into(),
            ),
        }),
        (_, _, "Xvfb") => probes.push(HostProbe {
            label: "display server",
            warn: true,
            message: format!(
                "Xvfb (DISPLAY={}) — not a real pointer host; background PX/scroll/drag cannot use MPX",
                snapshot.display.as_deref().unwrap_or("?")
            ),
            detail: Some(
                "Xvfb has no udev/libinput hotplug, so a uinput device never becomes an X input slave. \
                 Use delivery_mode:\"foreground\" for pixel click/scroll/drag when AT-SPI-at-point cannot land."
                    .into(),
            ),
        }),
        (_, _, "Xtigervnc") => probes.push(HostProbe {
            label: "display server",
            warn: true,
            message: format!(
                "Xtigervnc (DISPLAY={}) — not a real pointer host; background PX/scroll/drag cannot use MPX",
                snapshot.display.as_deref().unwrap_or("?")
            ),
            detail: Some(
                "Xtigervnc exposes only its built-in VNC/XTEST devices. \
                 Use delivery_mode:\"foreground\" for pixel click/scroll/drag when AT-SPI-at-point cannot land."
                    .into(),
            ),
        }),
        (None, Some(w), _) => probes.push(HostProbe {
            label: "display server",
            warn: true,
            message: format!("Wayland only (WAYLAND_DISPLAY={w}, DISPLAY unset)"),
            detail: Some(
                "X11 tools (list_windows, screenshot) need XWayland — start your session with XWayland enabled.".into(),
            ),
        }),
        (Some(d), Some(w), kind) => probes.push(HostProbe {
            label: "display server",
            warn: false,
            message: format!("{kind} (WAYLAND_DISPLAY={w}, DISPLAY={d})"),
            detail: None,
        }),
        (Some(d), None, kind) => probes.push(HostProbe {
            label: "display server",
            warn: false,
            message: format!("{kind} (DISPLAY={d})"),
            detail: None,
        }),
    }

    let compositor_msg = if snapshot.compositors.is_empty() {
        snapshot
            .desktop
            .as_deref()
            .map(|desktop| format!("none detected (desktop={desktop})"))
            .unwrap_or_else(|| "none detected".into())
    } else {
        snapshot.compositors.join(", ")
    };
    probes.push(HostProbe {
        label: "compositor",
        warn: false,
        message: compositor_msg,
        detail: None,
    });

    match snapshot.uinput {
        UinputStatus::Accessible => probes.push(HostProbe {
            label: "uinput",
            warn: false,
            message: "/dev/uinput is accessible".into(),
            detail: None,
        }),
        UinputStatus::Missing => probes.push(HostProbe {
            label: "uinput",
            warn: true,
            message: "/dev/uinput is missing — background MPX pointer cannot be created".into(),
            detail: Some(
                "background pixel click/scroll/drag needs a real Xorg plus /dev/uinput. \
                 Use delivery_mode:\"foreground\" on this host. cua-driver does not install kernel modules."
                    .into(),
            ),
        }),
        UinputStatus::Inaccessible => probes.push(HostProbe {
            label: "uinput",
            warn: true,
            message: "/dev/uinput exists but is not writable by this user".into(),
            detail: Some(
                "background MPX needs write access to /dev/uinput (usually the `input` group). \
                 Use delivery_mode:\"foreground\" until that is granted."
                    .into(),
            ),
        }),
    }

    let runtime = match (
        snapshot.xdg_runtime_dir.as_deref(),
        snapshot.session_bus_socket_exists,
        snapshot.dbus_session_bus_address.as_deref(),
    ) {
        (Some(dir), true, Some(addr)) => HostProbe {
            label: "session bus",
            warn: false,
            message: format!("XDG_RUNTIME_DIR={dir}; bus at {addr}"),
            detail: None,
        },
        (Some(dir), true, None) => HostProbe {
            label: "session bus",
            warn: false,
            message: format!(
                "XDG_RUNTIME_DIR={dir}; {} exists (DBUS_SESSION_BUS_ADDRESS unset)",
                snapshot.session_bus_socket
            ),
            detail: None,
        },
        _ => HostProbe {
            label: "session bus",
            warn: true,
            message: format!(
                "XDG_RUNTIME_DIR={}; {} exists={}; DBUS_SESSION_BUS_ADDRESS={}",
                snapshot.xdg_runtime_dir.as_deref().unwrap_or("unset"),
                snapshot.session_bus_socket,
                snapshot.session_bus_socket_exists,
                snapshot
                    .dbus_session_bus_address
                    .as_deref()
                    .unwrap_or("unset"),
            ),
            detail: Some(
                "AT-SPI lives on the session bus. A missing /run/user/<uid>/bus and unset \
                 DBUS_SESSION_BUS_ADDRESS is typical on headless VMs without systemd --user."
                    .into(),
            ),
        },
    };
    probes.push(runtime);

    if snapshot.at_spi_reachable {
        probes.push(HostProbe {
            label: "AT-SPI",
            warn: false,
            message: "org.a11y.Bus reachable via session bus".into(),
            detail: None,
        });
    } else {
        probes.push(HostProbe {
            label: "AT-SPI",
            warn: true,
            message: "accessibility bus not reachable".into(),
            detail: Some(
                "install at-spi2-core and ensure the user session has D-Bus running for get_window_state to work. \
                 cua-driver does not apt-install packages."
                    .into(),
            ),
        });
    }

    if snapshot.background_mpx_available {
        probes.push(HostProbe {
            label: "background MPX",
            warn: false,
            message: "real MPX/uinput pointer is available".into(),
            detail: None,
        });
    } else {
        probes.push(HostProbe {
            label: "background MPX",
            warn: true,
            message: format!(
                "unavailable ({}) — background PX/scroll/drag should use delivery_mode:\"foreground\"",
                snapshot.mpx_unavailable_reason()
            ),
            detail: Some(snapshot.background_pointer_hint().into()),
        });
    }

    probes
}

/// macOS-only diagnose sections, printed as an explicit skip on Linux so the
/// command no longer looks like a broken TCC dump.
pub fn diagnose_macos_skipped_section() -> String {
    "## macOS TCC / codesign (skipped)\n\
     not applicable on Linux — no TCC, codesign, /Applications/CuaDriver.app, \
     or ~/Library TCC.db.\n\
     See the linux host section above. sqlite3 is not required."
        .to_owned()
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn current_uid() -> u32 {
    fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .unwrap_or(0)
}

const COMPOSITOR_COMMS: &[&str] = &[
    "xfwm4",
    "picom",
    "compton",
    "mutter",
    "gnome-shell",
    "kwin_x11",
    "kwin_wayland",
    "kwin",
    "sway",
    "labwc",
    "openbox",
    "i3",
    "Hyprland",
    "hyprland",
    "weston",
    "wayfire",
    "niri",
    "compiz",
    "marco",
];

/// Match `/proc/<pid>/comm` names against known compositor / compositor-helper
/// processes. Split out so the table is unit-tested without walking /proc.
pub fn compositors_from_comms(comms: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for comm in comms {
        if COMPOSITOR_COMMS.iter().any(|known| *known == *comm) && !found.iter().any(|s| s == comm)
        {
            found.push((*comm).to_owned());
        }
    }
    found
}

fn running_compositors() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut comms = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name
            .to_str()
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        if let Ok(comm) = fs::read_to_string(entry.path().join("comm")) {
            comms.push(comm.trim().to_owned());
        }
    }
    let refs: Vec<&str> = comms.iter().map(String::as_str).collect();
    compositors_from_comms(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grok_bot_snapshot() -> LinuxHostSnapshot {
        LinuxHostSnapshot {
            display: Some(":2".into()),
            wayland_display: None,
            x_session_type: None,
            x_server: Some("Xvfb".into()),
            compositors: vec!["xfwm4".into(), "picom".into()],
            desktop: None,
            uinput: UinputStatus::Missing,
            xdg_runtime_dir: None,
            runtime_user_dir: "/run/user/1000".into(),
            runtime_user_dir_exists: false,
            session_bus_socket: "/run/user/1000/bus".into(),
            session_bus_socket_exists: false,
            dbus_session_bus_address: None,
            at_spi_reachable: false,
            background_mpx_available: false,
        }
    }

    #[test]
    fn xvfb_is_classified_as_virtual_pointer_host() {
        let snap = grok_bot_snapshot();
        assert_eq!(snap.display_server_kind(), "Xvfb");
        assert!(snap.is_virtual_pointer_host());
        assert!(snap.mpx_unavailable_reason().contains("Xvfb"));
        assert!(snap.mpx_unavailable_reason().contains("/dev/uinput"));
    }

    #[test]
    fn diagnose_linux_host_section_lists_grok_bot_facts() {
        let text = grok_bot_snapshot().format_diagnose();
        assert!(text.starts_with("## linux host"), "{text}");
        assert!(text.contains("display server:            Xvfb"), "{text}");
        assert!(text.contains("DISPLAY:                   :2"), "{text}");
        assert!(
            text.contains("compositor:                xfwm4, picom"),
            "{text}"
        );
        assert!(
            text.contains("/dev/uinput:               missing"),
            "{text}"
        );
        assert!(text.contains("XDG_RUNTIME_DIR:           unset"), "{text}");
        assert!(text.contains("/run/user/1000 exists=false"), "{text}");
        assert!(text.contains("/run/user/1000/bus exists=false"), "{text}");
        assert!(
            text.contains("AT-SPI org.a11y.Bus:       not reachable"),
            "{text}"
        );
        assert!(
            text.contains("background MPX:            unavailable"),
            "{text}"
        );
        assert!(text.contains("delivery_mode:\"foreground\""), "{text}");
        assert!(!text.contains("TCC"), "{text}");
        assert!(!text.contains("/Applications/CuaDriver.app"), "{text}");
    }

    #[test]
    fn doctor_warns_on_xvfb_and_missing_uinput() {
        let probes = doctor_host_probes(&grok_bot_snapshot());
        let by_label: Vec<_> = probes
            .iter()
            .map(|p| (p.label, p.warn, &p.message))
            .collect();
        let display = probes.iter().find(|p| p.label == "display server").unwrap();
        assert!(display.warn, "{by_label:?}");
        assert!(display.message.contains("Xvfb"), "{}", display.message);
        assert!(
            display.message.contains("not a real pointer host"),
            "{}",
            display.message
        );

        let uinput = probes.iter().find(|p| p.label == "uinput").unwrap();
        assert!(uinput.warn, "{by_label:?}");
        assert!(
            uinput.message.contains("/dev/uinput is missing"),
            "{}",
            uinput.message
        );

        let mpx = probes.iter().find(|p| p.label == "background MPX").unwrap();
        assert!(mpx.warn, "{by_label:?}");
        assert!(
            mpx.message.contains("delivery_mode:\"foreground\""),
            "{}",
            mpx.message
        );

        let at_spi = probes.iter().find(|p| p.label == "AT-SPI").unwrap();
        assert!(at_spi.warn);
        assert!(at_spi
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("does not apt-install"));
    }

    #[test]
    fn doctor_ok_on_real_xorg_with_uinput() {
        let snap = LinuxHostSnapshot {
            display: Some(":0".into()),
            wayland_display: None,
            x_session_type: Some("x11".into()),
            x_server: Some("Xorg".into()),
            compositors: vec!["mutter".into()],
            desktop: Some("GNOME".into()),
            uinput: UinputStatus::Accessible,
            xdg_runtime_dir: Some("/run/user/1000".into()),
            runtime_user_dir: "/run/user/1000".into(),
            runtime_user_dir_exists: true,
            session_bus_socket: "/run/user/1000/bus".into(),
            session_bus_socket_exists: true,
            dbus_session_bus_address: Some("unix:path=/run/user/1000/bus".into()),
            at_spi_reachable: true,
            background_mpx_available: true,
        };
        assert_eq!(snap.display_server_kind(), "Xorg");
        assert!(!snap.is_virtual_pointer_host());
        let probes = doctor_host_probes(&snap);
        assert!(
            probes.iter().all(|p| !p.warn),
            "{:?}",
            probes
                .iter()
                .filter(|p| p.warn)
                .map(|p| p.label)
                .collect::<Vec<_>>()
        );
        let display = probes.iter().find(|p| p.label == "display server").unwrap();
        assert_eq!(display.message, "Xorg (DISPLAY=:0)");
    }

    #[test]
    fn compositor_comm_table_matches_xfce_and_picom() {
        assert_eq!(
            compositors_from_comms(&["bash", "xfwm4", "picom", "chromium"]),
            vec!["xfwm4".to_owned(), "picom".to_owned()]
        );
        assert!(compositors_from_comms(&["sshd", "bash"]).is_empty());
    }

    #[test]
    fn macos_skip_section_is_explicit() {
        let text = diagnose_macos_skipped_section();
        assert!(text.contains("skipped"));
        assert!(text.contains("not applicable on Linux"));
        assert!(text.contains("TCC"));
        assert!(text.contains("linux host section"));
    }

    #[test]
    fn uinput_status_labels_are_stable() {
        assert_eq!(UinputStatus::Accessible.label(), "accessible");
        assert_eq!(UinputStatus::Missing.label(), "missing");
        assert_eq!(UinputStatus::Inaccessible.label(), "inaccessible");
    }
}
