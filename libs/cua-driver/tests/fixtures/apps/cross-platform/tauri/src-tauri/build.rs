fn main() {
    println!("cargo:rerun-if-changed=../web");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
