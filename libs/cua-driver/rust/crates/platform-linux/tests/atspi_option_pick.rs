// SPDX-License-Identifier: MIT

//! Picking a combo box option through AT-SPI against a fake accessible on a
//! private D-Bus bus. Its own test binary: the AT-SPI connection is process
//! wide and is pointed at that bus through `DBUS_SESSION_BUS_ADDRESS`.

#![cfg(target_os = "linux")]

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use platform_linux::atspi::native::{
    activate_option_ref, select_option_ref, ObjectRef, OptionPick, OptionPickSent,
};
use zbus::zvariant::OwnedObjectPath;

const SERVICE: &str = "org.cua.FakeCombo";
const COMBO: &str = "/org/a11y/atspi/accessible/combo";
/// A combo box with no options.
const EMPTY_COMBO: &str = "/org/a11y/atspi/accessible/empty";
/// A combo box whose one child cannot be read (nothing serves it).
const UNREADABLE_COMBO: &str = "/org/a11y/atspi/accessible/unreadable";
/// Each option answers its pick as its name says.
const OPTIONS: [&str; 5] = ["Acts", "Stalls", "Declines", "Raises", "Vanishes"];

struct PrivateBus {
    daemon: Child,
    /// Holds the bus socket.
    _dir: tempfile::TempDir,
}
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// Answer a pick the way option `index` says. A stall outlasts the
/// driver's per-element budget.
async fn answer(index: i32) -> zbus::fdo::Result<bool> {
    match OPTIONS.get(index as usize).copied() {
        Some("Acts") => Ok(true),
        Some("Stalls") => {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(true)
        }
        Some("Declines") => Ok(false),
        Some("Vanishes") => Err(zbus::fdo::Error::UnknownObject("no such object".into())),
        _ => Err(zbus::fdo::Error::Failed("raised after acting".into())),
    }
}

fn option_path(index: usize) -> String {
    format!("{COMBO}/{index}")
}

/// The accessibility bus launcher: the a11y bus is this same private bus.
struct A11yBus(String);
#[zbus::interface(name = "org.a11y.Bus")]
impl A11yBus {
    fn get_address(&self) -> String {
        self.0.clone()
    }
}

struct Combo;
#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Combo {
    fn get_role_name(&self) -> String {
        "combo box".into()
    }
    fn get_interfaces(&self) -> Vec<String> {
        vec![
            "org.a11y.atspi.Accessible".into(),
            "org.a11y.atspi.Selection".into(),
        ]
    }
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        (0..OPTIONS.len())
            .map(|index| {
                (
                    SERVICE.to_owned(),
                    OwnedObjectPath::try_from(option_path(index)).unwrap(),
                )
            })
            .collect()
    }
    #[zbus(property)]
    fn name(&self) -> String {
        "Size".into()
    }
}

/// A combo box whose children are `children` (object paths on this service).
struct BareCombo(&'static [&'static str]);
#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl BareCombo {
    fn get_role_name(&self) -> String {
        "combo box".into()
    }
    fn get_interfaces(&self) -> Vec<String> {
        vec!["org.a11y.atspi.Accessible".into()]
    }
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.0
            .iter()
            .map(|path| {
                (
                    SERVICE.to_owned(),
                    OwnedObjectPath::try_from(*path).unwrap(),
                )
            })
            .collect()
    }
    #[zbus(property)]
    fn name(&self) -> String {
        "Bare".into()
    }
}

struct ComboSelection;
#[zbus::interface(name = "org.a11y.atspi.Selection")]
impl ComboSelection {
    async fn select_child(&self, child_index: i32) -> zbus::fdo::Result<bool> {
        answer(child_index).await
    }
}

struct Item(usize);
#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Item {
    fn get_role_name(&self) -> String {
        "menu item".into()
    }
    fn get_interfaces(&self) -> Vec<String> {
        vec![
            "org.a11y.atspi.Accessible".into(),
            "org.a11y.atspi.Action".into(),
        ]
    }
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        Vec::new()
    }
    #[zbus(property)]
    fn name(&self) -> String {
        OPTIONS[self.0].into()
    }
}

struct ItemAction(usize);
#[zbus::interface(name = "org.a11y.atspi.Action")]
impl ItemAction {
    #[zbus(property, name = "NActions")]
    fn n_actions(&self) -> i32 {
        1
    }
    fn get_name(&self, _index: i32) -> String {
        "click".into()
    }
    async fn do_action(&self, _index: i32) -> zbus::fdo::Result<bool> {
        answer(self.0 as i32).await
    }
}

/// Start a private bus serving the fake combo box and point the driver's
/// AT-SPI connection at it. `None` when dbus-daemon is missing.
fn fake_combo() -> Option<(PrivateBus, tokio::runtime::Runtime, zbus::Connection)> {
    // An explicit config: a sandbox (Nix) has no session.conf.
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bus.conf");
    std::fs::write(
        &config,
        format!(
            "<busconfig><type>session</type>\
             <listen>unix:dir={}</listen><auth>EXTERNAL</auth>\
             <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
             <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
            dir.path().display()
        ),
    )
    .unwrap();
    let spawned = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn();
    let mut bus = match spawned {
        Ok(daemon) => PrivateBus { daemon, _dir: dir },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: dbus-daemon is not installed");
            return None;
        }
        Err(error) => panic!("private test bus: {error}"),
    };
    let mut address = String::new();
    std::io::BufReader::new(bus.daemon.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_owned();
    assert!(!address.is_empty(), "dbus-daemon printed no address");
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let service = runtime.block_on(async {
        let mut builder = zbus::connection::Builder::address(address.as_str())
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .name("org.a11y.Bus")
            .unwrap()
            .serve_at("/org/a11y/bus", A11yBus(address.clone()))
            .unwrap()
            .serve_at(COMBO, Combo)
            .unwrap()
            .serve_at(COMBO, ComboSelection)
            .unwrap()
            .serve_at(EMPTY_COMBO, BareCombo(&[]))
            .unwrap()
            .serve_at(
                UNREADABLE_COMBO,
                BareCombo(&["/org/a11y/atspi/accessible/unreadable/gone"]),
            )
            .unwrap();
        for index in 0..OPTIONS.len() {
            builder = builder
                .serve_at(option_path(index), Item(index))
                .unwrap()
                .serve_at(option_path(index), ItemAction(index))
                .unwrap();
        }
        builder.build().await.unwrap()
    });
    Some((bus, runtime, service))
}

fn picked(name: &str) -> OptionPick {
    OptionPick::Picked { name: name.into() }
}

/// One test: the driver's AT-SPI connection is set up once per process.
#[test]
fn a_pick_that_was_sent_but_not_answered_is_never_reported_as_failed() {
    let Some((_bus, _runtime, _service)) = fake_combo() else {
        return;
    };
    let combo = ObjectRef {
        bus: SERVICE.into(),
        path: COMBO.into(),
    };
    for (route, pick) in [
        (
            "activate",
            activate_option_ref as fn(&ObjectRef, &str) -> anyhow::Result<OptionPick>,
        ),
        ("select", select_option_ref),
    ] {
        assert_eq!(pick(&combo, "Acts").unwrap(), picked("Acts"), "{route}");
        // The toolkit declined, or the option was gone: nothing changed.
        for name in ["Declines", "Vanishes"] {
            let error = pick(&combo, name).unwrap_err();
            assert!(!error.is::<OptionPickSent>(), "{route} {name}: {error:#}");
        }
        assert!(
            matches!(pick(&combo, "Huge").unwrap(), OptionPick::NoMatch { .. }),
            "{route}"
        );
        // Sent, then no answer or an error reply: the option may be set.
        for name in ["Stalls", "Raises"] {
            let error = pick(&combo, name).unwrap_err();
            assert!(
                error.is::<OptionPickSent>(),
                "{route} {name}: a sent pick came back as a plain error: {error:#}"
            );
        }
    }
    // An open popup that answers with no options lists none; one whose
    // child cannot be read proves nothing (it shares this process's one
    // AT-SPI connection, so it lives in this test).
    let bare = |path: &str| ObjectRef {
        bus: SERVICE.into(),
        path: path.into(),
    };
    assert_eq!(
        activate_option_ref(&bare(EMPTY_COMBO), "Blue").unwrap(),
        OptionPick::NoOptions
    );
    assert_eq!(
        activate_option_ref(&bare(UNREADABLE_COMBO), "Blue").unwrap(),
        OptionPick::Unavailable
    );
}
