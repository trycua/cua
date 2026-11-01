//! Run only in the separately patched scratch workspace, never production.
use std::io::{self, Read};
use std::sync::atomic::Ordering;
use cua_driver_contract::{PressKeyInput, ActionTarget, InputDeliveryMode};
use cua_driver_sdk::{CuaDriver, DriverHostOptions};
use platform_linux::wayland::prototype::{self, Expected};

fn input(e: &Expected) -> PressKeyInput {
    PressKeyInput { key:e.key.clone(), target:Some(ActionTarget::Window {pid:e.pid,window_id:e.token}),
        scope:None, session:Some("rfc3506-one-shot".into()), modifiers:None,
        delivery_mode:Some(InputDeliveryMode::Foreground) }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "--self-check".into());
    if mode == "--self-check" {
        let e: Expected = serde_json::from_value(serde_json::json!({"owner":":1.8","pid":123,"token":7,
            "generation":"{00000000-0000-0000-0000-000000000001}",
            "internal_id":"{00000000-0000-0000-0000-000000000002}","deadline_ns":1,"op_seq":1,"key":"a"}))?;
        let wire = serde_json::to_value(input(&e))?;
        assert_eq!(wire["delivery_mode"], "foreground");
        assert_eq!(wire["target"]["window_id"],7);
        let op = prototype::arm(e)?;
        assert!(op.live().is_err());
        assert!(prototype::arm(op.expected.clone()).is_err());
        assert!(!op.may_start.load(Ordering::SeqCst));
        println!("SDK harness self-check PASS; no runtime, portal or input opened");
        return Ok(());
    }
    if mode == "--identity" {
        let (owner,snapshot)=prototype::identity_snapshot().ok_or("trusted identity snapshot unavailable")?;
        println!("{}",serde_json::json!({"owner":owner,"snapshot":snapshot,"now_ns":prototype::mono_ns()}));
        return Ok(());
    }
    if mode != "--execute" && mode != "--execute-closed" && mode != "--background-refusal" { return Err("use --self-check, --identity, --execute, --execute-closed or --background-refusal".into()); }
    let mut text=String::new(); io::stdin().read_to_string(&mut text)?;
    let e:Expected=serde_json::from_str(&text)?;
    let mut args=input(&e);
    if mode == "--background-refusal" { args.delivery_mode = Some(InputDeliveryMode::Background); }
    let op=prototype::arm(e)?;
    struct Close(std::sync::Arc<prototype::Operation>);
    impl Drop for Close { fn drop(&mut self) {self.0.close();} }
    let _close=Close(op.clone());
    if mode=="--execute-closed" {op.close();}
    let driver=CuaDriver::try_create_for_host(DriverHostOptions {
        cursor:cursor_overlay::CursorConfig { enabled:false,..Default::default() },
        host_owns_permission_ux:true, host_bundle_id:None, claude_code_compatibility:false,
        prepare_desktop_environment:false, register_host_tools:None,
        authorization_host:None, activity_observer:None,
    })?;
    let sdk_submission_ns=prototype::mono_ns();
    // Exactly one normal typed SDK call. No alternate tool/transport or retry.
    let result=driver.press_key(args).await;
    let caller_ack_ns=prototype::mono_ns();
    op.close();
    let result=match result {Ok(v)=>serde_json::from_str::<serde_json::Value>(&v.raw_json)?,Err(e)=>serde_json::json!({"sdk_error":e.to_string()})};
    println!("{}",serde_json::json!({"sdk_submission_ns":sdk_submission_ns,"caller_ack_ns":caller_ack_ns,
        "operation":op.report(),"result":result,"no_retry":true}));
    Ok(())
}
