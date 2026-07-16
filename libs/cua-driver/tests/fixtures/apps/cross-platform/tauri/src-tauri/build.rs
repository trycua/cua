fn main() {
    println!("cargo:rerun-if-changed=../web");
    tauri_build::build()
}
