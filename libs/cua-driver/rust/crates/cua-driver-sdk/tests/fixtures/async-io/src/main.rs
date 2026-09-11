#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    println!(
        "sdk driver handle: {} bytes",
        std::mem::size_of::<cua_driver_sdk::CuaDriver>()
    );
    println!("oo7 keyring: {} bytes", std::mem::size_of::<oo7::Keyring>());
}
