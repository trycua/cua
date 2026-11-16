//! Gives the `cua` binary's main thread an 8 MiB stack on Windows, the size
//! Linux and macOS already give it. Windows defaults to 1 MiB, and the main
//! thread parses the command tree and polls the top-level command future,
//! which overflowed it (STATUS_STACK_OVERFLOW) in unoptimized builds.
//!
//! Built on Windows, `cua.exe` also gets its version resource (company,
//! description, version), which the Windows Firewall prompt shows as the
//! app's name and publisher ([`version_resource`]). A cross build from
//! another OS skips it: it needs the Windows resource compiler.

fn main() {
    const STACK: u32 = 8 * 1024 * 1024;
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" {
        if env == "msvc" {
            println!("cargo:rustc-link-arg-bins=/STACK:{STACK}");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK}");
        }
    }
    if os == "windows" && cfg!(windows) && env == "msvc" {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let rc = out.join("cua-resources.rc");
        std::fs::write(&rc, version_resource("Cua", "cua", "Cua", &version))
            .expect("write the resource script");
        embed_resource::compile_for(&rc, ["cua"], embed_resource::NONE);
    }
    println!("cargo:rerun-if-changed=build.rs");
}

/// The Windows version resource (`VS_VERSION_INFO`): what Explorer's
/// Details tab and the Windows Firewall prompt show. The prompt reads the
/// name from `FileDescription` and the publisher from `CompanyName`, not
/// from the Authenticode signature, so without it a signed binary showed
/// Publisher "Unknown" (CUA-3515). Numeric constants only, so no Windows
/// SDK header is needed: 1 = VS_VERSION_INFO, 0x40004 = VOS_NT_WINDOWS32,
/// 1 = VFT_APP.
fn version_resource(description: &str, internal: &str, product: &str, version: &str) -> String {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u16>().unwrap_or(0))
        .chain(std::iter::repeat(0));
    let numeric: Vec<String> = (0..4)
        .map(|_| parts.next().unwrap_or(0).to_string())
        .collect();
    let numeric = numeric.join(",");
    format!(
        r#"1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Cua AI, Inc."
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{internal}"
      VALUE "LegalCopyright", "Copyright (c) 2026 Cua AI, Inc."
      VALUE "OriginalFilename", "{internal}.exe"
      VALUE "ProductName", "{product}"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
