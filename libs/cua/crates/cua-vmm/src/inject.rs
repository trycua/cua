//! A static cua-spacesd in any OCI image.
//!
//! An image that does not carry cua-spacesd (`alpine:3.20`, `node:20`) is
//! still a full Space for the container and microVM kinds: the backend puts
//! a static (musl) `cua-spacesd` into the guest at [`GUEST_PATH`] and makes
//! it the entrypoint, `cua-spacesd supervise -- <the image's command>`. The
//! supervisor starts the image's own ENTRYPOINT and CMD as a child with the
//! image's user and working directory, serves cua-spacesd beside it, reaps
//! zombies and forwards signals (see `cua-spacesd-supervise`).
//!
//! This module finds the host's copy of the binary and builds the command;
//! each backend gets the file into its guest its own way (a Docker upload,
//! a layer of the microVM rootfs, a read-only bind mount).

use std::path::{Path, PathBuf};

use crate::error::{Result, VmmError};
use crate::host;
use crate::runtime::OciRunConfig;
use crate::types::Arch;

/// The environment variable naming the host's static cua-spacesd (for the
/// host's architecture), the way `CUA_MICROVM_INIT` names the guest init.
pub const ENV_BINARY: &str = "CUA_SPACESD_STATIC";

/// Where the binary sits in the guest.
pub const GUEST_PATH: &str = "/.cua/bin/cua-spacesd";

/// The directory holding it in the guest.
pub const GUEST_DIR: &str = "/.cua/bin";

/// File name of the static binary for `arch`, next to `cua`.
pub fn file_name(arch: Arch) -> String {
    format!("cua-spacesd-linux-{}", arch.qemu())
}

/// `e_machine` of an ELF for `arch`.
fn elf_machine(arch: Arch) -> u16 {
    match arch {
        Arch::X86_64 => 62,
        Arch::Aarch64 => 183,
    }
}

/// Whether `elf` is a static Linux executable for `arch`.
pub fn check_static_elf(elf: &[u8], arch: Arch) -> std::result::Result<(), String> {
    if elf.len() < 64 || &elf[..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return Err("it is not a 64-bit little-endian Linux executable".into());
    }
    let machine = u16::from_le_bytes([elf[18], elf[19]]);
    if machine != elf_machine(arch) {
        return Err(format!(
            "it is built for another architecture (e_machine {machine}), not {}",
            arch.qemu()
        ));
    }
    let interp = crate::elf::interpreter(elf);
    match interp {
        Some(None) => Ok(()),
        Some(Some(i)) => Err(format!(
            "it is dynamically linked (it asks for {i}); it must be a static musl build"
        )),
        None => Err("it is not an ELF executable".into()),
    }
}

/// The host's static cua-spacesd for guests of `arch`: `$CUA_SPACESD_STATIC`
/// (host architecture only), else the first `cua-spacesd-linux-<arch>` of
/// [`search_paths`] that is a file, else the one on `PATH`.
pub fn find_binary(arch: Arch) -> Option<PathBuf> {
    if arch == Arch::host()
        && let Some(p) = std::env::var_os(ENV_BINARY)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let name = file_name(arch);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    search_paths(
        &name,
        exe_dir.as_deref(),
        module_dir().as_deref(),
        &host::cua_home(),
    )
    .into_iter()
    .find(|p| p.is_file())
    .or_else(|| host::which(&name))
}

/// Where [`find_binary`] looks for `name`, in order:
///
/// 1. next to the running executable (`cua`, the daemon, an app's sidecar),
///    and in an app bundle's `Contents/Resources` beside `Contents/MacOS`;
/// 2. next to the module this code is linked into (`module_dir`), and in
///    its `bin/`: when Node or Python loads the SDK library in-process the
///    executable is `node` or `python`, and the binary ships with the
///    library instead. The npm native package keeps it beside the library
///    and `cua`; the wheel keeps the library at `cua/` and the binary in
///    `cua/bin/` (the `cua` console command hands it to the CLI from the
///    `cua-cli` wheel as `$CUA_SPACESD_STATIC`);
/// 3. `$CUA_HOME/bin` (`~/.cua/bin`).
pub fn search_paths(
    name: &str,
    exe_dir: Option<&Path>,
    module_dir: Option<&Path>,
    cua_home: &Path,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = exe_dir {
        out.push(d.join(name));
        out.push(d.join("../Resources").join(name));
    }
    if let Some(d) = module_dir.filter(|d| Some(*d) != exe_dir) {
        out.push(d.join(name));
        out.push(d.join("bin").join(name));
    }
    out.push(cua_home.join("bin").join(name));
    out
}

/// The directory of the executable or shared library this code is linked
/// into: the SDK library (`libcua_sdk.so`, `libcua_sdk.dylib`) when a
/// language binding loaded it, else the executable itself.
#[cfg(unix)]
pub fn module_dir() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: dladdr only reads the loader's tables for an address in this
    // module; `dli_fname` is a NUL-terminated string the loader owns.
    let fname = unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        let addr = module_dir as *const () as *const libc::c_void;
        if libc::dladdr(addr, &mut info) == 0 || info.dli_fname.is_null() {
            return None;
        }
        CStr::from_ptr(info.dli_fname).to_bytes().to_vec()
    };
    let p = Path::new(std::ffi::OsStr::from_bytes(&fname));
    // glibc names the main program by its argv[0], which can be relative
    // or bare; current_exe covers that case.
    if !p.is_absolute() {
        return None;
    }
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    p.parent().map(Path::to_path_buf)
}

/// The directory of the executable or DLL this code is linked into.
#[cfg(windows)]
pub fn module_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: the address is a function of this module; the handle is not
    // reference-counted and the buffer is sized for the call.
    let path = unsafe {
        if GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            module_dir as *const () as *const u16,
            &mut module,
        ) == 0
        {
            return None;
        }
        let mut buf = vec![0u16; 32768];
        let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
        if n == 0 || n >= buf.len() {
            return None;
        }
        PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]))
    };
    path.parent().map(Path::to_path_buf)
}

/// [`find_binary`], checked: the file must be a static Linux executable
/// for `arch`. The error says where the binary ships and how to point at
/// one.
pub fn require_binary(arch: Arch) -> Result<PathBuf> {
    let Some(p) = find_binary(arch) else {
        return Err(VmmError::missing(
            "the static cua-spacesd",
            format!(
                "an image without cua-spacesd gets one injected, a static Linux build that \
                 ships next to `cua` as {}; set {ENV_BINARY} to its path, or create the \
                 Space with spacesd=false to run the image as it is",
                file_name(arch)
            ),
        ));
    };
    let head = {
        use std::io::Read;
        let mut buf = vec![0u8; 4096];
        let n = std::fs::File::open(&p)
            .and_then(|mut f| f.read(&mut buf))
            .map_err(|e| VmmError::other(format!("{}: {e}", p.display())))?;
        buf.truncate(n);
        buf
    };
    // Program headers can sit beyond 4 KiB in theory; the build's layout
    // puts them right after the ELF header, so the head decides.
    check_static_elf(&head, arch).map_err(|why| {
        VmmError::invalid(format!(
            "the static cua-spacesd {} is not usable: {why}",
            p.display()
        ))
    })?;
    Ok(p)
}

/// Whether `user` is root (nothing to drop to).
fn is_root(user: &str) -> bool {
    matches!(
        user.trim(),
        "" | "0" | "root" | "0:0" | "root:root" | "0:root" | "root:0"
    )
}

/// The container command that makes the injected cua-spacesd the guest's
/// PID 1: `command` (the Space's own, which replaces the image's) or the
/// image's `ENTRYPOINT` and `CMD`, run by `cua-spacesd supervise` with the
/// image's user and working directory.
///
/// A Space's own command ends the Space when it exits, with its status. The
/// image's default command does not: `/bin/sh` and `node` exit at once
/// without a terminal, and the Space is still the Space.
pub fn wrapper_command(run: &OciRunConfig, command: Option<&[String]>) -> Vec<String> {
    let own = command.filter(|c| !c.is_empty());
    let argv: Vec<String> = match own {
        Some(c) => c.to_vec(),
        None => run
            .entrypoint
            .iter()
            .chain(run.cmd.iter())
            .cloned()
            .collect(),
    };
    let mut out = vec![GUEST_PATH.to_string(), "supervise".to_string()];
    if let Some(u) = run.user.as_deref().filter(|u| !is_root(u)) {
        out.extend(["--user".to_string(), u.to_string()]);
    }
    if let Some(w) = run.working_dir.as_deref().filter(|w| !w.is_empty()) {
        out.extend(["--workdir".to_string(), w.to_string()]);
    }
    out.extend([
        "--on-exit".to_string(),
        if own.is_some() { "exit" } else { "serve" }.to_string(),
    ]);
    if !argv.is_empty() {
        out.push("--".to_string());
        out.extend(argv);
    }
    out
}

/// A tar (uncompressed) that adds the binary at [`GUEST_PATH`] (root, 0555)
/// with its parent directories, for an image layer or a Docker upload.
#[cfg(any(feature = "container", feature = "microvm"))]
pub fn layer_tar(binary: &Path) -> Result<Vec<u8>> {
    let data =
        std::fs::read(binary).map_err(|e| VmmError::other(format!("{}: {e}", binary.display())))?;
    let mtime = host::now_secs();
    let mut b = tar::Builder::new(Vec::new());
    for dir in [".cua/", ".cua/bin/"] {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_uid(0);
        h.set_gid(0);
        h.set_size(0);
        h.set_mtime(mtime);
        b.append_data(&mut h, dir, std::io::empty())?;
    }
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_mode(0o555);
    h.set_uid(0);
    h.set_gid(0);
    h.set_size(data.len() as u64);
    h.set_mtime(mtime);
    b.append_data(&mut h, GUEST_PATH.trim_start_matches('/'), data.as_slice())?;
    Ok(b.into_inner()?)
}

/// A short stable identifier of the binary's content (the hash prefix a
/// cached rootfs is named with).
pub fn content_id(binary: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(binary)
        .map_err(|e| VmmError::other(format!("{}: {e}", binary.display())))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize())[..16].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> OciRunConfig {
        OciRunConfig {
            entrypoint: vec!["docker-entrypoint.sh".into()],
            cmd: vec!["node".into()],
            env: vec![],
            working_dir: Some("/app".into()),
            user: Some("node".into()),
        }
    }

    #[test]
    fn the_images_command_runs_under_the_supervisor_and_keeps_the_space_up() {
        let c = wrapper_command(&run(), None);
        assert_eq!(
            c,
            [
                GUEST_PATH,
                "supervise",
                "--user",
                "node",
                "--workdir",
                "/app",
                "--on-exit",
                "serve",
                "--",
                "docker-entrypoint.sh",
                "node"
            ]
        );
    }

    #[test]
    fn a_spaces_own_command_replaces_the_images_and_ends_the_space() {
        let own = vec!["sleep".to_string(), "5".to_string()];
        let c = wrapper_command(&run(), Some(&own));
        assert_eq!(&c[c.len() - 3..], ["--", "sleep", "5"]);
        assert!(c.windows(2).any(|w| w == ["--on-exit", "exit"]));
        assert!(!c.contains(&"docker-entrypoint.sh".to_string()));
    }

    #[test]
    fn root_and_empty_users_are_not_dropped_to() {
        for u in [
            None,
            Some(""),
            Some("0"),
            Some("root"),
            Some("0:0"),
            Some("root:root"),
        ] {
            let r = OciRunConfig {
                user: u.map(String::from),
                ..run()
            };
            assert!(
                !wrapper_command(&r, None).contains(&"--user".to_string()),
                "{u:?}"
            );
        }
        let r = OciRunConfig {
            user: Some("1000:1000".into()),
            ..run()
        };
        assert!(
            wrapper_command(&r, None)
                .windows(2)
                .any(|w| w == ["--user", "1000:1000"])
        );
    }

    #[test]
    fn an_image_with_no_command_serves_alone() {
        let c = wrapper_command(&OciRunConfig::default(), None);
        assert_eq!(c, [GUEST_PATH, "supervise", "--on-exit", "serve"]);
    }

    fn elf(machine: u16, interp: bool) -> Vec<u8> {
        let mut e = vec![0u8; 64 + 56 + 32];
        e[..4].copy_from_slice(b"\x7fELF");
        e[4] = 2;
        e[5] = 1;
        e[18..20].copy_from_slice(&machine.to_le_bytes());
        e[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        e[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        e[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        let ph = 64;
        let kind: u32 = if interp { 3 } else { 1 };
        e[ph..ph + 4].copy_from_slice(&kind.to_le_bytes());
        if interp {
            e[ph + 8..ph + 16].copy_from_slice(&120u64.to_le_bytes());
            e[ph + 32..ph + 40].copy_from_slice(&8u64.to_le_bytes());
            e[120..127].copy_from_slice(b"/ld.so\0");
        }
        e
    }

    #[test]
    fn only_a_static_executable_of_the_guests_architecture_is_accepted() {
        assert!(check_static_elf(&elf(183, false), Arch::Aarch64).is_ok());
        assert!(check_static_elf(&elf(62, false), Arch::X86_64).is_ok());
        assert!(
            check_static_elf(&elf(62, false), Arch::Aarch64)
                .unwrap_err()
                .contains("another architecture")
        );
        assert!(
            check_static_elf(&elf(183, true), Arch::Aarch64)
                .unwrap_err()
                .contains("dynamically linked")
        );
        assert!(check_static_elf(b"MZ", Arch::Aarch64).is_err());
    }

    #[test]
    fn the_binary_is_found_by_env_then_by_name() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("custom");
        std::fs::write(
            &p,
            elf(
                if cfg!(target_arch = "aarch64") {
                    183
                } else {
                    62
                },
                false,
            ),
        )
        .unwrap();
        // SAFETY: tests in this module that touch the variable run
        // sequentially in one test.
        unsafe { std::env::set_var(ENV_BINARY, &p) };
        assert_eq!(find_binary(Arch::host()), Some(p.clone()));
        assert_eq!(require_binary(Arch::host()).unwrap(), p);
        unsafe { std::env::remove_var(ENV_BINARY) };
        assert_eq!(file_name(Arch::Aarch64), "cua-spacesd-linux-aarch64");
        assert_eq!(file_name(Arch::X86_64), "cua-spacesd-linux-x86_64");
    }

    #[test]
    fn the_search_goes_exe_then_library_then_cua_home() {
        let n = "cua-spacesd-linux-x86_64";
        let p = search_paths(
            n,
            Some(Path::new("/app/MacOS")),
            Some(Path::new("/py/cua")),
            Path::new("/home/u/.cua"),
        );
        assert_eq!(
            p,
            [
                PathBuf::from("/app/MacOS").join(n),
                PathBuf::from("/app/MacOS/../Resources").join(n),
                PathBuf::from("/py/cua").join(n),
                PathBuf::from("/py/cua/bin").join(n),
                PathBuf::from("/home/u/.cua/bin").join(n),
            ]
        );
        // `cua` itself: the module is the executable, looked in once.
        let same = search_paths(
            n,
            Some(Path::new("/x")),
            Some(Path::new("/x")),
            Path::new("/h"),
        );
        assert_eq!(same.len(), 3);
    }

    fn first(name: &str, module: &Path, home: &Path) -> Option<PathBuf> {
        search_paths(
            name,
            Some(Path::new("/nonexistent-exe-dir")),
            Some(module),
            home,
        )
        .into_iter()
        .find(|p| p.is_file())
    }

    #[test]
    fn the_npm_and_wheel_layouts_resolve_from_the_library() {
        let n = file_name(Arch::X86_64);
        let home = tempfile::tempdir().unwrap();
        // npm: @trycua/cua-linux-x64-gnu/{libcua_sdk.so,cua,cua-spacesd-linux-*}
        let npm = tempfile::tempdir().unwrap();
        std::fs::write(npm.path().join("libcua_sdk.so"), b"").unwrap();
        std::fs::write(npm.path().join("cua"), b"").unwrap();
        std::fs::write(npm.path().join(&n), b"").unwrap();
        assert_eq!(
            first(&n, npm.path(), home.path()),
            Some(npm.path().join(&n))
        );
        // wheel: cua/libcua_sdk.so and cua/bin/{cua,cua-spacesd-linux-*}
        let whl = tempfile::tempdir().unwrap();
        std::fs::create_dir(whl.path().join("bin")).unwrap();
        std::fs::write(whl.path().join("libcua_sdk.so"), b"").unwrap();
        std::fs::write(whl.path().join("bin/cua"), b"").unwrap();
        std::fs::write(whl.path().join("bin").join(&n), b"").unwrap();
        assert_eq!(
            first(&n, whl.path(), home.path()),
            Some(whl.path().join("bin").join(&n))
        );
        // Neither: ~/.cua/bin.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(first(&n, empty.path(), home.path()), None);
        std::fs::create_dir(home.path().join("bin")).unwrap();
        std::fs::write(home.path().join("bin").join(&n), b"").unwrap();
        assert_eq!(
            first(&n, empty.path(), home.path()),
            Some(home.path().join("bin").join(&n))
        );
    }

    #[test]
    fn the_module_dir_is_the_file_this_code_is_linked_into() {
        // In a test binary the module is the test executable; in the SDK
        // it is libcua_sdk, which this cannot load, so the check is that
        // the lookup names a real module and not the process's cwd.
        let got = module_dir().expect("dladdr names this module");
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(got, exe.parent().unwrap());
    }

    #[cfg(any(feature = "container", feature = "microvm"))]
    #[test]
    fn the_layer_adds_the_binary_with_its_directories() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sd");
        std::fs::write(&p, b"binary").unwrap();
        let tar_bytes = layer_tar(&p).unwrap();
        let mut a = tar::Archive::new(tar_bytes.as_slice());
        let names: Vec<(String, u32)> = a
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.path().unwrap().to_string_lossy().into_owned(),
                    e.header().mode().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                (".cua/".to_string(), 0o755),
                (".cua/bin/".to_string(), 0o755),
                (".cua/bin/cua-spacesd".to_string(), 0o555)
            ]
        );
    }
}
