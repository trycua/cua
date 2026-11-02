/// A function an iOS app could link: proves the Rust iOS targets build.
#[no_mangle]
pub extern "C" fn hellosim_add(a: i32, b: i32) -> i32 {
    a + b
}
