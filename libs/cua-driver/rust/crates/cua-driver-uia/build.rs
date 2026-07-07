fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    embed_windows_version_info(
        "cua-driver-uia",
        "cua-driver",
        Some("cua-driver-uia.manifest"),
    );
}

fn embed_windows_version_info(
    file_description: &str,
    product_name: &str,
    manifest_file: Option<&str>,
) {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let version_u64 = windows_version_u64(&version);

    let mut resource = winresource::WindowsResource::new();
    resource.set("FileDescription", file_description);
    resource.set("ProductName", product_name);
    resource.set("CompanyName", "trycua");
    resource.set("FileVersion", &version);
    resource.set("ProductVersion", &version);
    if let Some(manifest_file) = manifest_file {
        resource.set_manifest_file(manifest_file);
        println!("cargo:rerun-if-changed={manifest_file}");
    }
    resource.set_version_info(winresource::VersionInfo::FILEVERSION, version_u64);
    resource.set_version_info(winresource::VersionInfo::PRODUCTVERSION, version_u64);
    resource
        .compile()
        .expect("failed to embed Windows VERSIONINFO");
}

fn windows_version_u64(version: &str) -> u64 {
    let mut parts = [0_u64; 4];
    for (idx, part) in version.split(['.', '-']).take(4).enumerate() {
        parts[idx] = part.parse::<u64>().unwrap_or(0).min(0xffff);
    }
    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}
