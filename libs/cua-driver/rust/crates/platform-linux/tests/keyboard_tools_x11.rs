#![cfg(target_os = "linux")]

use anyhow::{bail, Result};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::ToolRegistry;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

struct Desktop {
    conn: RustConnection,
    target: Window,
    sentinel: Window,
    registry: ToolRegistry,
}

impl Desktop {
    fn new() -> Result<Self> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let target = Self::window(&conn, root, 0)?;
        let sentinel = Self::window(&conn, root, 400)?;
        conn.set_input_focus(InputFocus::PARENT, sentinel, x11rb::CURRENT_TIME)?;
        assert_eq!(conn.get_input_focus()?.reply()?.focus, sentinel);
        let desktop = Self {
            conn,
            target,
            sentinel,
            registry: platform_linux::tools::register_all(),
        };
        desktop.drain()?;
        Ok(desktop)
    }

    fn window(conn: &RustConnection, root: Window, x: i16) -> Result<Window> {
        let window = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            x,
            0,
            200,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
        )?
        .check()?;
        let pid_atom = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            pid_atom,
            AtomEnum::CARDINAL,
            &[std::process::id()],
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"Cua keyboard test",
        )?
        .check()?;
        conn.map_window(window)?.check()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while conn.get_window_attributes(window)?.reply()?.map_state != MapState::VIEWABLE {
            if Instant::now() >= deadline {
                bail!("keyboard fixture window did not map");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(window)
    }

    fn args(&self, fields: Value) -> Value {
        let mut args = json!({"pid": std::process::id(), "window_id": self.target});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        args
    }

    async fn call(&self, tool: &str, fields: Value) -> ToolResult {
        self.registry.invoke(tool, self.args(fields)).await
    }

    fn drain(&self) -> Result<Vec<KeyPressEvent>> {
        self.conn.get_input_focus()?.reply()?;
        let mut events = Vec::new();
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::KeyPress(event) | Event::KeyRelease(event) => events.push(event),
                Event::Error(error) => bail!("X11 observer error: {error:?}"),
                _ => {}
            }
        }
        Ok(events)
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        let _ = self.conn.destroy_window(self.target);
        let _ = self.conn.destroy_window(self.sentinel);
        let _ = self.conn.flush();
    }
}

#[tokio::test]
#[ignore = "requires an isolated X11 display"]
async fn authorization_process_target_is_refused_without_input() -> Result<()> {
    let desktop = Desktop::new()?;
    for (tool, fields) in [
        (
            "press_key",
            json!({"key":"f5","delivery_mode":"background"}),
        ),
        (
            "hotkey",
            json!({"keys":["ctrl","shift","k"],"delivery_mode":"foreground"}),
        ),
    ] {
        let result = desktop.call(tool, fields).await;
        assert_eq!(result.is_error, Some(true), "{result:?}");
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["refusal"]["code"], "permission_denied");
        assert!(structured["delivery"].is_null());
        assert!(structured["evidence"].is_null());
        assert!(desktop.drain()?.is_empty());
        assert_eq!(
            desktop.conn.get_input_focus()?.reply()?.focus,
            desktop.sentinel
        );
    }
    Ok(())
}
