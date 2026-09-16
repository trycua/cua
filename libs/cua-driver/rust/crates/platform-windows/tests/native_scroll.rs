#![allow(non_snake_case, non_upper_case_globals, dead_code, unused_imports)]

extern crate self as windows;

use std::{cell::RefCell, sync::mpsc};

#[derive(Default)]
struct Provider {
    releases: usize,
    scrolls: usize,
    late_scrolls: usize,
    pattern_error: bool,
    scroll_error_at: Option<usize>,
    pause_pattern: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    pause_scroll: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
}

thread_local! { static PROVIDER: RefCell<Provider> = RefCell::default(); }

fn pause(boundary: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>) {
    if let Some((entered, resume)) = boundary {
        entered.send(()).unwrap();
        resume
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
}

pub mod core {
    pub trait Interface {}
}

pub mod Win32 {
    pub mod System {
        pub mod Com {
            use crate::Win32::UI::Accessibility::{AutomationClass, IUIAutomation};
            pub const COINIT_MULTITHREADED: u32 = 0;
            pub const CLSCTX_INPROC_SERVER: u32 = 1;
            pub fn CoInitializeEx(_: Option<()>, _: u32) -> anyhow::Result<()> {
                Ok(())
            }
            pub fn CoCreateInstance(
                _: &AutomationClass,
                _: Option<()>,
                _: u32,
            ) -> anyhow::Result<IUIAutomation> {
                panic!("unexpected automation creation")
            }
        }
    }
    pub mod UI {
        pub mod Accessibility {
            use crate::{pause, PROVIDER};
            #[derive(Clone)]
            pub struct IUIAutomationElement;
            impl IUIAutomationElement {
                pub unsafe fn from_raw(_: *mut std::ffi::c_void) -> Self {
                    Self
                }
                pub fn GetCurrentPattern(&self, _: i32) -> anyhow::Result<Pattern> {
                    pause(PROVIDER.with(|provider| provider.borrow_mut().pause_pattern.take()));
                    if PROVIDER.with(|provider| provider.borrow().pattern_error) {
                        anyhow::bail!("provider lookup failed");
                    }
                    Ok(Pattern)
                }
                pub fn CurrentIsOffscreen(&self) -> anyhow::Result<Bool> {
                    panic!("unexpected visibility read")
                }
                pub fn CurrentBoundingRectangle(&self) -> anyhow::Result<Rect> {
                    panic!("unexpected geometry read")
                }
            }
            impl Drop for IUIAutomationElement {
                fn drop(&mut self) {
                    PROVIDER.with(|provider| provider.borrow_mut().releases += 1);
                }
            }
            pub struct Pattern;
            impl Pattern {
                pub fn cast<T: Default>(&self) -> anyhow::Result<T> {
                    Ok(T::default())
                }
            }
            #[derive(Default)]
            pub struct IUIAutomationScrollPattern;
            impl IUIAutomationScrollPattern {
                pub fn Scroll(&self, _: i32, _: i32) -> anyhow::Result<()> {
                    let error = PROVIDER.with(|provider| {
                        let mut provider = provider.borrow_mut();
                        provider.scrolls += 1;
                        provider.late_scrolls +=
                            usize::from(!cua_driver_core::tool::native_dispatch_allowed());
                        provider.scroll_error_at == Some(provider.scrolls)
                    });
                    pause(PROVIDER.with(|provider| provider.borrow_mut().pause_scroll.take()));
                    if error {
                        anyhow::bail!("provider failed after receiving scroll");
                    }
                    Ok(())
                }
                pub fn CurrentVerticallyScrollable(&self) -> anyhow::Result<Bool> {
                    panic!("unexpected capability read")
                }
            }
            #[derive(Default)]
            pub struct IUIAutomationScrollItemPattern;
            impl IUIAutomationScrollItemPattern {
                pub fn ScrollIntoView(&self) -> anyhow::Result<()> {
                    panic!("unexpected alternate actuator")
                }
            }
            pub struct AutomationClass;
            pub const CUIAutomation: AutomationClass = AutomationClass;
            pub struct IUIAutomation;
            impl IUIAutomation {
                pub fn ControlViewWalker(&self) -> anyhow::Result<Walker> {
                    panic!("unexpected ancestor walk")
                }
            }
            pub struct Walker;
            impl Walker {
                pub fn GetParentElement(
                    &self,
                    _: &IUIAutomationElement,
                ) -> anyhow::Result<IUIAutomationElement> {
                    panic!("unexpected ancestor lookup")
                }
            }
            pub struct Bool;
            impl Bool {
                pub fn as_bool(&self) -> bool {
                    panic!("unexpected property")
                }
            }
            pub struct Rect {
                pub left: i32,
                pub top: i32,
                pub right: i32,
                pub bottom: i32,
            }
            pub const UIA_ScrollPatternId: i32 = 1;
            pub const UIA_ScrollItemPatternId: i32 = 2;
            pub const ScrollAmount_LargeDecrement: i32 = -2;
            pub const ScrollAmount_LargeIncrement: i32 = 2;
            pub const ScrollAmount_SmallDecrement: i32 = -1;
            pub const ScrollAmount_SmallIncrement: i32 = 1;
            pub const ScrollAmount_NoAmount: i32 = 0;
        }
    }
}

mod input {
    pub fn point_in_window_bounds(_: u64, _: i32, _: i32) -> bool {
        panic!("unexpected pointer targeting")
    }
}
mod uia {
    pub mod fg_bypass {
        pub fn run_with_uwp_bypass<T>(
            _: isize,
            _: impl FnOnce() -> anyhow::Result<T>,
        ) -> anyhow::Result<T> {
            panic!("unexpected alternate scroll path")
        }
    }
}

#[path = "../src/uia/scroll.rs"]
mod scroll;

fn reset(provider: Provider) {
    PROVIDER.with(|state| *state.borrow_mut() = provider);
}

fn assert_borrow_preserved() {
    assert_eq!(PROVIDER.with(|provider| provider.borrow().releases), 0);
}

#[test]
fn acknowledged_scroll_preserves_the_retained_reference() {
    reset(Provider::default());
    unsafe { scroll::scroll_element(1, "down", 2) }.unwrap();
    assert_eq!(PROVIDER.with(|provider| provider.borrow().scrolls), 2);
    assert_borrow_preserved();
}

#[test]
fn provider_lookup_failure_preserves_the_reference_without_claiming_delivery() {
    reset(Provider {
        pattern_error: true,
        ..Provider::default()
    });
    let error = unsafe { scroll::scroll_element(1, "down", 1) }.unwrap_err();
    let result = cua_driver_core::protocol::ToolResult::from_native_error(
        error,
        cua_driver_core::action_record::RequestedDelivery::Background,
    );
    assert_eq!(result.is_error, Some(true));
    assert!(result.action_record.is_none());
    assert_eq!(PROVIDER.with(|provider| provider.borrow().scrolls), 0);
    assert_borrow_preserved();
}

#[test]
fn attempted_failure_is_unknown_and_does_not_repeat_or_release_the_target() {
    for failed_call in [1, 2] {
        reset(Provider {
            scroll_error_at: Some(failed_call),
            ..Provider::default()
        });
        let error = unsafe { scroll::scroll_element(1, "down", 3) }.unwrap_err();
        let result = cua_driver_core::protocol::ToolResult::from_native_error(
            error,
            cua_driver_core::action_record::RequestedDelivery::Background,
        );
        let record = result
            .action_record
            .expect("attempted delivery must remain explicit");
        assert_eq!(
            record.actual_delivery,
            Some(cua_driver_core::action_record::ActualDelivery::Unknown)
        );
        assert_eq!(
            record.transport,
            cua_driver_core::action_record::ActionTransport::WindowsUiaScroll
        );
        assert_eq!(
            PROVIDER.with(|provider| provider.borrow().scrolls),
            failed_call
        );
        assert_borrow_preserved();
    }
}

async fn cancel_at_provider_boundary(after_first_scroll: bool) {
    use cua_driver_core::tool::{scope_native_dispatch, spawn_native};
    let (entered, reading) = mpsc::channel();
    let (resume, blocked) = mpsc::channel();
    let (send_worker, worker) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(scope_native_dispatch(None, async move {
        let worker = spawn_native(move || {
            let mut provider = Provider::default();
            if after_first_scroll {
                provider.pause_scroll = Some((entered, blocked));
            } else {
                provider.pause_pattern = Some((entered, blocked));
            }
            reset(provider);
            let error = unsafe { scroll::scroll_element(1, "down", 3) }.unwrap_err();
            assert_borrow_preserved();
            let result = cua_driver_core::protocol::ToolResult::from_native_error(
                error,
                cua_driver_core::action_record::RequestedDelivery::Background,
            );
            assert_eq!(result.action_record.is_some(), after_first_scroll);
            PROVIDER.with(|provider| (provider.borrow().scrolls, provider.borrow().late_scrolls))
        });
        send_worker.send(worker).unwrap();
        std::future::pending::<()>().await;
    }));
    let worker = worker.await.unwrap();
    reading
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    resume.send(()).unwrap();
    assert_eq!(worker.await.unwrap(), (usize::from(after_first_scroll), 0));
}

#[tokio::test]
async fn cancellation_during_lookup_prevents_the_first_mutation() {
    cancel_at_provider_boundary(false).await;
}

#[tokio::test]
async fn cancellation_after_one_scroll_stops_the_batch_without_inventing_a_refusal() {
    cancel_at_provider_boundary(true).await;
}
