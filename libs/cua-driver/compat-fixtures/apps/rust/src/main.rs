use cua_driver_sdk::CuaDriver;

fn main() {
    let driver = CuaDriver::connect(None).expect("create compatibility client");
    let endpoint = driver.socket_path();
    assert!(!endpoint.trim().is_empty(), "default endpoint must be selected");
    println!("{endpoint}");
}
