#![cfg(target_os = "linux")]

use cua_driver_core::action_record::ActionEffect;
use cua_driver_core::cursor_events::{self, CursorEvent, CursorEventPhase};
use cua_driver_core::tool::ToolRegistry;
use cua_driver_testkit::keyboard_fixture::KeyboardFixture;
use serde_json::{json, Value};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

struct OverlayBarrier {
    released: Arc<(Mutex<bool>, Condvar)>,
    finished: Arc<Notify>,
    queued: Arc<Notify>,
}

impl OverlayBarrier {
    fn install() -> Self {
        let released = Arc::new((Mutex::new(false), Condvar::new()));
        let finished = Arc::new(Notify::new());
        let queued = Arc::new(Notify::new());
        let wait = released.clone();
        let first = finished.clone();
        let second = queued.clone();
        cursor_events::install_cursor_event_sink(Arc::new(move |event| {
            if let CursorEvent::Action { session, phase, .. } = event {
                if session.ends_with("keyboard-held") && phase == CursorEventPhase::End {
                    first.notify_one();
                    let (lock, wake) = &*wait;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = wake.wait(released).unwrap();
                    }
                }
                if session.ends_with("keyboard-queued") && phase == CursorEventPhase::Begin {
                    second.notify_one();
                }
            }
        }));
        Self {
            released,
            finished,
            queued,
        }
    }

    fn release(&self) {
        let (lock, wake) = &*self.released;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
}

impl Drop for OverlayBarrier {
    fn drop(&mut self) {
        self.release();
        cursor_events::clear_cursor_event_sink();
    }
}

fn args(fixture: &KeyboardFixture, session: &str, tool: &str) -> Value {
    let mut args = json!({"pid": fixture.pid(), "window_id": fixture.window_id,
        "session": session, "delivery_mode":"foreground"});
    if tool == "press_key" {
        args["key"] = json!("f5");
    } else {
        args["keys"] = json!(["ctrl", "h"]);
    }
    args
}

async fn registry() -> Arc<ToolRegistry> {
    let registry = platform_linux::tools::build_registry(false);
    for session in ["keyboard-held", "keyboard-queued"] {
        let result = registry
            .invoke("start_session", json!({"session":session}))
            .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
    }
    Arc::new(registry)
}

async fn queued_keyboard(tool: &'static str, cancel: bool) {
    let fixture = KeyboardFixture::spawn(false);
    let registry = registry().await;
    let barrier = OverlayBarrier::install();
    let first_args = args(&fixture, "keyboard-held", "press_key");
    let first_registry = registry.clone();
    let first = tokio::spawn(async move { first_registry.invoke("press_key", first_args).await });
    tokio::time::timeout(Duration::from_secs(10), barrier.finished.notified())
        .await
        .unwrap();
    fixture.key_event("down", 0xffc2);
    fixture.key_event("up", 0xffc2);
    let next_args = args(&fixture, "keyboard-queued", tool);
    let next_registry = registry.clone();
    let next = tokio::spawn(async move { next_registry.invoke(tool, next_args).await });
    tokio::time::timeout(Duration::from_secs(10), barrier.queued.notified())
        .await
        .unwrap();
    fixture.assert_quiet();
    if cancel {
        next.abort();
        assert!(next.await.unwrap_err().is_cancelled());
        barrier.release();
        assert_ne!(first.await.unwrap().is_error, Some(true));
        fixture.assert_quiet();
    } else {
        barrier.release();
        assert_ne!(first.await.unwrap().is_error, Some(true));
        assert_ne!(next.await.unwrap().is_error, Some(true));
        let key = if tool == "press_key" { 0xffc2 } else { 0x68 };
        fixture.key_event("down", key);
        fixture.key_event("up", key);
    }
    let final_args = json!({"pid":fixture.pid(),"window_id":fixture.window_id,"key":"f6","delivery_mode":"foreground"});
    let recovery = registry.invoke("press_key", final_args).await;
    assert_ne!(recovery.is_error, Some(true), "{recovery:?}");
    assert_eq!(fixture.key_event("down", 0xffc3)["flags"], 0);
    fixture.key_event("up", 0xffc3);
    fixture.assert_quiet();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires an isolated X11/Openbox desktop"]
async fn press_key_waits_for_the_admitted_action_to_finish() {
    queued_keyboard("press_key", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires an isolated X11/Openbox desktop"]
async fn hotkey_waits_for_the_admitted_action_to_finish() {
    queued_keyboard("hotkey", false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires an isolated X11/Openbox desktop"]
async fn cancelled_queued_press_key_never_delivers_and_releases_admission() {
    queued_keyboard("press_key", true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires an isolated X11/Openbox desktop"]
async fn cancelled_queued_hotkey_never_delivers_and_releases_admission() {
    queued_keyboard("hotkey", true).await;
}

#[tokio::test]
#[ignore = "requires an isolated X11/Openbox desktop"]
async fn key_down_followed_by_native_target_loss_is_not_a_public_clean_refusal() {
    let fixture = KeyboardFixture::spawn(true);
    let registry = platform_linux::tools::build_registry(false);
    let result = registry
        .invoke(
            "press_key",
            json!({"pid":fixture.pid(),"window_id":fixture.window_id,"key":"f5"}),
        )
        .await;
    fixture.key_event("down", 0xffc2);
    fixture.event("closed");
    let record = result
        .action_record
        .as_ref()
        .expect("possibly delivered input must have an execution record");
    assert_ne!(record.effect, ActionEffect::Refused, "{result:?}");
    assert_ne!(record.effect, ActionEffect::Confirmed, "{result:?}");
    assert!(record.actual_delivery.is_some(), "{result:?}");
}
