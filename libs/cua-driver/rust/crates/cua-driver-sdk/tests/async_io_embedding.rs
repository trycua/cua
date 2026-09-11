#![cfg(target_os = "linux")]

#[test]
fn sdk_and_async_io_keyring_link_together() {
    println!(
        "sdk driver handle: {} bytes",
        std::mem::size_of::<cua_driver_sdk::CuaDriver>()
    );
    println!("oo7 keyring: {} bytes", std::mem::size_of::<oo7::Keyring>());
}
