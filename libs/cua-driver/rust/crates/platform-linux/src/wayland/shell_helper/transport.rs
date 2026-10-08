//! Persistent session-bus transport for synchronous compositor helpers.
//! A separate runtime also serves callers already inside Tokio without nested block_on.
use std::sync::{mpsc, OnceLock};
use std::time::Duration;
use tokio::sync::Mutex;
use zbus::{Connection, Message};

static CONNECTION: Mutex<Option<Connection>> = Mutex::const_new(None);

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("compositor D-Bus runtime")
    })
}

pub(super) fn call(
    destination: &str,
    path: &str,
    method: &str,
    args: &[String],
    timeout: Duration,
) -> Option<String> {
    let destination = destination.to_owned();
    let path = path.to_owned();
    let method = method.to_owned();
    let args = args.to_vec();
    let (tx, rx) = mpsc::sync_channel(1);
    runtime().spawn(async move {
        let result = tokio::time::timeout(timeout, async {
            let connection = {
                let mut shared = CONNECTION.lock().await;
                if shared.is_none() {
                    *shared = Some(
                        Connection::session()
                            .await
                            .map_err(|_| Failure::Transport)?,
                    );
                }
                shared.as_ref().ok_or(Failure::Transport)?.clone()
            };
            invoke(&connection, &destination, &path, &method, &args).await
        })
        .await
        .unwrap_or(Err(Failure::Transport));
        // A timeout or transport error may mean the session bus went away.
        // Reconnect on the next call; never replay a possibly applied mutation.
        // An error reply (no helper on the bus, a refused capture) keeps the
        // connection: reconnecting would not change the answer.
        if matches!(result, Err(Failure::Transport)) {
            CONNECTION.lock().await.take();
        }
        let _ = tx.send(result.ok());
    });
    rx.recv_timeout(timeout + Duration::from_millis(100))
        .ok()
        .flatten()
}

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The peer answered with an error, or the call or reply had an
    /// unexpected shape. The connection is healthy.
    Reply,
    /// The connection failed or the call timed out.
    Transport,
}

fn classify(error: zbus::Error) -> Failure {
    match error {
        zbus::Error::MethodError(..) | zbus::Error::FDO(_) => Failure::Reply,
        _ => Failure::Transport,
    }
}

// Preserve the small textual reply contract consumed by shell_helper's existing
// parsers. Bodies on the wire are typed; strings are never shell or GVariant code.
async fn invoke(
    connection: &Connection,
    destination: &str,
    path: &str,
    method: &str,
    args: &[String],
) -> Result<String, Failure> {
    let (interface, member) = method.rsplit_once('.').ok_or(Failure::Reply)?;
    let parse_u32 = |value: &String| value.parse::<u32>().map_err(|_| Failure::Reply);
    let parse_i32 = |value: &String| value.parse::<i32>().map_err(|_| Failure::Reply);
    macro_rules! send {
        ($body:expr) => {
            connection
                .call_method(Some(destination), path, Some(interface), member, &$body)
                .await
                .map_err(classify)?
        };
    }
    let reply: Message = match (member, args) {
        (
            "GetNameOwner"
            | "GetConnectionUnixProcessID"
            | "GetConnectionUnixUser"
            | "SetCursorColor"
            | "SetSessionLabel",
            [value],
        ) => send!((value.as_str(),)),
        ("GetRects" | "GetVersion" | "Capture" | "HideCursor", []) => send!(()),
        ("Activate", [id]) => send!((parse_u32(id)?,)),
        ("MoveCursor" | "ClickPulse", [x, y]) => {
            send!((parse_i32(x)?, parse_i32(y)?))
        }
        ("SetCursorState", [action, delivery, target, active]) => send!((
            action.as_str(),
            delivery.as_str(),
            target.as_str(),
            active.parse::<bool>().map_err(|_| Failure::Reply)?
        )),
        _ => return Err(Failure::Reply),
    };
    let body = reply.body();
    let decoded = match member {
        "GetRects" => body.deserialize::<String>().ok(),
        "GetNameOwner" | "Capture" => body
            .deserialize::<String>()
            .ok()
            .map(|value| format!("('{value}',)")),
        "GetVersion" | "GetConnectionUnixProcessID" | "GetConnectionUnixUser" => body
            .deserialize::<u32>()
            .ok()
            .map(|value| format!("(uint32 {value},)")),
        "Activate" => body
            .deserialize::<bool>()
            .ok()
            .map(|value| format!("({value},)")),
        _ => body.deserialize::<()>().ok().map(|()| "()".into()),
    };
    decoded.ok_or(Failure::Reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::process::{Child, Command, Stdio};

    struct PrivateBus(Child);
    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    // Titles gdbus's GVariant text would escape: an apostrophe, quotes, a
    // backslash, a newline and non-ASCII. A typed reply carries them intact.
    const RECTS: &str = r#"[{"id":3,"pid":41,"title":"Tomorrow's \"plan\" C:\\ 1\n2 飞书","x":0,"y":0,"w":10,"h":10},{"id":7,"pid":42,"title":"Target","x":14,"y":12,"w":800,"h":600}]"#;

    struct Helper;
    #[zbus::interface(name = "org.cua.WinRects")]
    impl Helper {
        fn get_version(&self) -> u32 {
            8
        }
        fn get_rects(&self) -> String {
            RECTS.into()
        }
        fn capture(&self) -> String {
            "cG5n".into()
        }
        fn activate(&self, id: u32) -> bool {
            id == 7
        }
        fn set_cursor_state(
            &self,
            action: &str,
            delivery: &str,
            target: &str,
            active: bool,
        ) -> zbus::fdo::Result<()> {
            if (action, delivery, target, active) != ("observe", "", "window", false) {
                return Err(zbus::fdo::Error::InvalidArgs("wrong state".into()));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn typed_roundtrips_on_one_private_connection() {
        // Use an explicit config: a sandbox (Nix) has no session.conf, and
        // then the daemon exits without printing an address.
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
            Ok(child) => PrivateBus(child),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping: dbus-daemon is not installed");
                return;
            }
            Err(error) => panic!("private test bus: {error}"),
        };
        let mut address = String::new();
        std::io::BufReader::new(bus.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(!address.trim().is_empty(), "dbus-daemon printed no address");
        let service = zbus::connection::Builder::address(address.trim())
            .unwrap()
            .name("org.cua.WinRects")
            .unwrap()
            .serve_at("/org/cua/WinRects", Helper)
            .unwrap()
            .build()
            .await
            .unwrap();
        let client = zbus::connection::Builder::address(address.trim())
            .unwrap()
            .build()
            .await
            .unwrap();
        let call = |method: &'static str, args: Vec<String>| {
            let client = client.clone();
            async move {
                invoke(
                    &client,
                    "org.cua.WinRects",
                    "/org/cua/WinRects",
                    &format!("org.cua.WinRects.{method}"),
                    &args,
                )
                .await
            }
        };
        // One connection serves repeated calls.
        for _ in 0..3 {
            assert_eq!(
                call("GetVersion", vec![]).await.as_deref(),
                Ok("(uint32 8,)")
            );
            // GetRects is the raw JSON, so punctuation in one title no longer
            // hides every window from the parsers.
            let rects = call("GetRects", vec![]).await.unwrap();
            assert_eq!(rects, RECTS);
            let windows = super::super::parse_windows(&rects, None).unwrap();
            assert_eq!(windows.len(), 2);
            assert_eq!(windows[0].title, "Tomorrow's \"plan\" C:\\ 1\n2 飞书");
            assert_eq!(
                super::super::parse_window_origin(&rects, 42),
                Some((14, 12))
            );
            let raw = call("Capture", vec![]).await.unwrap();
            assert_eq!(raw, "('cG5n',)");
            assert_eq!(super::super::decode_capture(&raw), Some(b"png".to_vec()));
        }
        assert_eq!(
            call("Activate", vec!["7".into()]).await.as_deref(),
            Ok("(true,)")
        );
        assert_eq!(
            call("Activate", vec!["8".into()]).await.as_deref(),
            Ok("(false,)")
        );
        assert_eq!(
            call(
                "SetCursorState",
                vec!["observe".into(), "".into(), "window".into(), "false".into()]
            )
            .await
            .as_deref(),
            Ok("()")
        );
        // Error replies and malformed arguments leave the connection healthy:
        // they are not a reason to reconnect.
        assert_eq!(
            call(
                "SetCursorState",
                vec!["click".into(), "".into(), "window".into(), "true".into()]
            )
            .await,
            Err(Failure::Reply)
        );
        assert_eq!(
            call("Activate", vec!["-1".into()]).await,
            Err(Failure::Reply)
        );
        assert_eq!(call("NoSuchMethod", vec![]).await, Err(Failure::Reply));
        assert_eq!(
            call("GetVersion", vec![]).await.as_deref(),
            Ok("(uint32 8,)")
        );
        drop(service);
    }
}
