// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The `volume` group against an in-process cua-spacesd core: the doctor
//! serves a throwaway volume and the core mounts it with this machine's
//! backend (NFS on macOS and Windows, FUSE on Linux). Then a Space's view at
//! the default mount point (`V:` on Windows; `~/Cua Volume` under a
//! temporary HOME elsewhere): public/ read-only, its own folder writable,
//! nothing else visible, a large file both ways, and no mount after
//! detaching. Opt-in (it makes real mounts, of temporary directories only,
//! and unmounts them): CUA_SPACESD_VOLUME_TEST=1. The Windows lane is
//! `volume-windows` in ci-cua-spacesd.yml.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use guest_tungstenite::tungstenite::Message;

use cua_spacesd_client::diagnose::Status;
use cua_spacesd_client::{pb, SpacesdClient};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};

const TOKEN: &str = "doctor-volume-token";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_guest_mount_round_trips_and_detaches() {
    if std::env::var("CUA_SPACESD_VOLUME_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_SPACESD_VOLUME_TEST=1 (it mounts a temporary directory)");
        return;
    }
    std::env::set_var("CUA_ENV_TEST_SANDBOX", "1");
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("CUA_VOLUME_TEST_LOG")
                .unwrap_or_else(|_| "cua_spacesd_server::services=info".into()),
        )
        .with_test_writer()
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().join("data"),
        downloads_dir: Some(dir.path().join("downloads")),
        teleport_home: Some(dir.path().join("home")),
        media_quic_port: 0,
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some(TOKEN.into()));
    let addr = cua_spacesd_server::spawn_local(ServerBuilder::new(ctx).build())
        .await
        .unwrap();
    let client = SpacesdClient::connect_url(&format!("http://{addr}"), Some(TOKEN.into()))
        .await
        .unwrap();
    let caps = client.capabilities().await.unwrap();
    let f = caps
        .features
        .iter()
        .find(|f| f.name == "volume.mount")
        .expect("volume.mount is reported");
    assert!(f.supported, "{}", f.limitation);
    let c = cua_spacesd_doctor::checks::volume::mount_round_trip(&client, "t1").await;
    assert_eq!(c.status, Status::Pass, "{}", c.message);
    eprintln!("{}", c.message);
    let s = client
        .volume()
        .get_volume_status(pb::GetVolumeStatusRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        pb::VolumeState::try_from(s.state).unwrap(),
        pb::VolumeState::Detached
    );
    space_view_at_the_default_mount_point(&client, dir.path()).await;
}

/// Serves `vfs` to the core's `/volume` socket until the returned task is
/// aborted.
async fn serve(
    client: &SpacesdClient,
    ws_path: &str,
    servers: &Arc<cua_volume::guest::GuestServers>,
) -> tokio::task::JoinHandle<()> {
    let ws = client.open_websocket(ws_path).await.unwrap();
    let (sink, stream) = ws.split();
    let sink = sink.with(|bytes: Vec<u8>| async move {
        Ok::<_, guest_tungstenite::tungstenite::Error>(Message::Binary(bytes.into()))
    });
    let source = stream.filter_map(|m| async move {
        match m {
            Ok(Message::Binary(b)) => Some(b.to_vec()),
            _ => None,
        }
    });
    tokio::spawn(cua_hotspot::peer::serve_egress(
        Box::pin(source),
        Box::pin(sink),
        servers.dialer(),
    ))
}

async fn space_view_at_the_default_mount_point(client: &SpacesdClient, dir: &std::path::Path) {
    use cua_spacesd_server::services::volume::is_mounted_async;
    use cua_volume::{Condition, Context, Drive};
    // The default mount point is under HOME off Windows: keep it temporary.
    if !cfg!(windows) {
        std::env::set_var("HOME", dir);
    }
    let drive = Drive::open_local(&dir.join("space-home"));
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let user = drive.session(Context::user());
    user.write("public/rules.md", b"be kind".to_vec(), Condition::None)
        .await
        .unwrap();
    user.write("public/big.bin", big.clone(), Condition::None)
        .await
        .unwrap();
    user.write("agents/ada/notes.md", b"private".to_vec(), Condition::None)
        .await
        .unwrap();
    let vfs = cua_volume::vfs::Vfs::new(
        &drive,
        Context::space("local:lab"),
        None,
        None,
        &dir.join("space-state"),
    )
    .unwrap();
    let servers = Arc::new(cua_volume::guest::GuestServers::start(vfs).await.unwrap());
    let attached = client
        .volume()
        .attach_volume(pb::AttachVolumeRequest {
            mount_path: String::new(),
            ticket_ttl: None,
        })
        .await
        .unwrap()
        .into_inner();
    let peer = serve(client, &attached.ws_path, &servers).await;
    let mut status = None;
    for _ in 0..150 {
        let s = client
            .volume()
            .get_volume_status(pb::GetVolumeStatusRequest {})
            .await
            .unwrap()
            .into_inner();
        match pb::VolumeState::try_from(s.state).unwrap() {
            pb::VolumeState::Mounted => {
                status = Some(s);
                break;
            }
            pb::VolumeState::Error => panic!("the mount failed: {}", s.detail),
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    let status = status.expect("mounted in 30 s");
    let root = std::path::PathBuf::from(&status.mount_path);
    eprintln!("mounted {} over {}", root.display(), status.backend);
    if cfg!(windows) {
        assert_eq!(status.mount_path, "V:\\", "the default is the V: drive");
    } else {
        assert!(
            root == dir.join("Cua Volume") || root == std::path::Path::new("/volume"),
            "{}",
            root.display()
        );
    }
    let (r, want) = (root.clone(), big.clone());
    let outcome = tokio::task::spawn_blocking(move || {
        let mut names: Vec<String> = std::fs::read_dir(&r)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["public", "spaces"], "only the Space's view");
        assert_eq!(
            std::fs::read_to_string(r.join("public").join("rules.md")).unwrap(),
            "be kind"
        );
        assert!(std::fs::read(r.join("public").join("big.bin")).unwrap() == want);
        if cfg!(windows) {
            // Client for NFS looks names up without regard to case.
            eprintln!(
                "case-insensitive lookup: {:?}",
                std::fs::read_to_string(r.join("PUBLIC").join("Rules.MD"))
            );
        }
        let refused = std::fs::write(r.join("public").join("x.md"), b"no");
        assert_eq!(
            refused.map_err(|e| e.kind()),
            Err(std::io::ErrorKind::PermissionDenied),
            "public/ is read-only for a Space"
        );
        let mine = r.join("spaces").join("local-lab");
        std::fs::write(mine.join("big.bin"), &want).unwrap();
        std::fs::write(mine.join("out.txt"), b"from the guest").unwrap();
        assert_eq!(
            std::fs::read(mine.join("out.txt")).unwrap(),
            b"from the guest"
        );
        rename(&mine.join("out.txt"), &mine.join("done.txt"));
        std::fs::create_dir(mine.join("sub")).unwrap();
        std::fs::remove_dir(mine.join("sub")).unwrap();
    })
    .await;
    if let Err(e) = outcome {
        let _ = client
            .volume()
            .detach_volume(pb::DetachVolumeRequest::default())
            .await;
        std::panic::resume_unwind(e.into_panic());
    }
    // The guest's writes land in the volume; the refused one does not.
    let mut landed = false;
    for _ in 0..50 {
        let done = user.read("spaces/local-lab/done.txt", None).await;
        let b = user.read("spaces/local-lab/big.bin", None).await;
        if matches!(&done, Ok((d, _)) if d == b"from the guest")
            && matches!(&b, Ok((d, _)) if *d == big)
        {
            landed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(landed, "the guest's writes reached the volume");
    assert!(user.read("public/x.md", None).await.is_err());
    client
        .volume()
        .detach_volume(pb::DetachVolumeRequest {
            volume_id: attached.volume_id,
        })
        .await
        .unwrap();
    peer.abort();
    for _ in 0..50 {
        if !is_mounted_async(&root).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        !is_mounted_async(&root).await,
        "{} is unmounted",
        root.display()
    );
}

/// Renames within one directory of the mount, the way most programs do.
/// Off Windows that is rename(2) (`std::fs::rename`). On Windows it is
/// `MoveFileExW` without flags (Explorer, cmd's `ren`), and the test first
/// records how every rename path fares on the Windows NFS client; see
/// [`windows_renames`].
fn rename(from: &std::path::Path, to: &std::path::Path) {
    #[cfg(windows)]
    {
        windows_renames(from.parent().unwrap());
        win::move_file_ex(from, to, 0).unwrap_or_else(|e| panic!("MoveFileExW: os error {e}"));
    }
    #[cfg(not(windows))]
    std::fs::rename(from, to).unwrap();
}

/// Every way a Windows program renames, on the Cua Volume mount: printed,
/// and asserted for the paths Windows' own tools use. `std::fs::rename`
/// (MoveFileExW with MOVEFILE_REPLACE_EXISTING) fails with
/// ERROR_NOT_SAME_DEVICE (17) in the Windows NFS client before any call
/// reaches the server, while cmd's `ren` works; the rest of the matrix
/// shows which other paths share that limitation (the guide and the
/// cua-volume skill say which).
#[cfg(windows)]
fn windows_renames(dir: &std::path::Path) {
    use std::io::ErrorKind;
    let fresh = |name: &str| {
        let p = dir.join(name);
        std::fs::write(&p, name.as_bytes()).unwrap();
        p
    };
    let ps = |cmd: String| {
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
            .output()
            .unwrap();
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    let mut report = Vec::new();
    let (a, b) = (fresh("r1.txt"), dir.join("r1-renamed.txt"));
    let move_plain = win::move_file_ex(&a, &b, 0);
    report.push(format!("MoveFileExW(0), new name: {move_plain:?}"));
    let a = fresh("r2.txt");
    let rename_item = ps(format!(
        "Rename-Item -LiteralPath '{}' -NewName 'r2-renamed.txt'",
        a.display()
    ));
    report.push(format!("Rename-Item: {rename_item:?}"));
    let (a, b) = (fresh("r3.txt"), dir.join("r3-moved.txt"));
    let move_item = ps(format!(
        "Move-Item -LiteralPath '{}' -Destination '{}'",
        a.display(),
        b.display()
    ));
    report.push(format!("Move-Item, new name: {move_item:?}"));
    let (a, b) = (fresh("r4.txt"), dir.join("r4-renamed.txt"));
    let std_rename = std::fs::rename(&a, &b);
    report.push(format!("std::fs::rename, new name: {std_rename:?}"));
    let (a, b) = (fresh("r5.txt"), dir.join("r5-renamed.txt"));
    report.push(format!(
        "MoveFileExW(REPLACE_EXISTING), new name: {:?}",
        win::move_file_ex(&a, &b, win::MOVEFILE_REPLACE_EXISTING)
    ));
    let (a, b) = (fresh("r6.txt"), fresh("r6-existing.txt"));
    report.push(format!(
        "MoveFileExW(REPLACE_EXISTING), onto an existing file: {:?}",
        win::move_file_ex(&a, &b, win::MOVEFILE_REPLACE_EXISTING)
    ));
    let (a, b) = (fresh("r7.txt"), dir.join("r7-renamed.txt"));
    report.push(format!(
        "FileRenameInfo, ReplaceIfExists=false: {:?}",
        win::rename_info(&a, &b, false, 0)
    ));
    let (a, b) = (fresh("r8.txt"), dir.join("r8-renamed.txt"));
    report.push(format!(
        "FileRenameInfo, ReplaceIfExists=true: {:?}",
        win::rename_info(&a, &b, false, 1)
    ));
    let (a, b) = (fresh("r9.txt"), dir.join("r9-renamed.txt"));
    report.push(format!(
        "FileRenameInfoEx, POSIX|REPLACE_IF_EXISTS: {:?}",
        win::rename_info(&a, &b, true, 3)
    ));
    let (a, b) = (fresh("r10.txt"), fresh("r10-existing.txt"));
    report.push(format!(
        "Move-Item -Force, onto an existing file: {:?}",
        ps(format!(
            "Move-Item -Force -LiteralPath '{}' -Destination '{}'",
            a.display(),
            b.display()
        ))
    ));
    // The same renames by the drive's UNC name, and relative to an open
    // directory handle.
    let unc = |p: &std::path::Path| -> Option<std::path::PathBuf> {
        let s = p.to_str()?;
        let remote = win::remote_name(s.get(..2)?)?;
        Some(std::path::PathBuf::from(format!("{remote}{}", s.get(2..)?)))
    };
    let (a, b) = (fresh("r14.txt"), dir.join("r14-renamed.txt"));
    report.push(format!("drive V: is {:?}", win::remote_name("V:")));
    match (unc(&a), unc(&b)) {
        (Some(ua), Some(ub)) => {
            report.push(format!(
                "MoveFileExW(0), both by UNC ({}): {:?}",
                ua.display(),
                win::move_file_ex(&ua, &ub, 0)
            ));
            let (a, b) = (fresh("r15.txt"), dir.join("r15-renamed.txt"));
            report.push(format!(
                "MoveFileExW(0), target by UNC: {:?}",
                win::move_file_ex(&a, &unc(&b).unwrap(), 0)
            ));
        }
        _ => report.push("no UNC name for V:".into()),
    }
    let a = fresh("r16.txt");
    report.push(format!(
        "FileRenameInfo relative to the directory handle: {:?}",
        win::rename_relative(&a, dir, "r16-renamed.txt")
    ));
    let py = |code: &str, a: &std::path::Path, b: &std::path::Path| {
        std::process::Command::new("python")
            .args(["-c", code])
            .arg(a)
            .arg(b)
            .output()
            .map(|o| {
                if o.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&o.stderr)
                        .trim()
                        .lines()
                        .last()
                        .unwrap_or("")
                        .to_string())
                }
            })
    };
    let (a, b) = (fresh("r11.txt"), dir.join("r11-renamed.txt"));
    report.push(format!(
        "Python os.rename, new name: {:?}",
        py("import os,sys; os.rename(sys.argv[1], sys.argv[2])", &a, &b)
    ));
    let (a, b) = (fresh("r12.txt"), dir.join("r12-renamed.txt"));
    report.push(format!(
        "Python os.replace, new name: {:?}",
        py(
            "import os,sys; os.replace(sys.argv[1], sys.argv[2])",
            &a,
            &b
        )
    ));
    let (a, b) = (fresh("r13.txt"), fresh("r13-existing.txt"));
    report.push(format!(
        "Python os.replace, onto an existing file: {:?}",
        py(
            "import os,sys; os.replace(sys.argv[1], sys.argv[2])",
            &a,
            &b
        )
    ));
    eprintln!(
        "renames on the Windows NFS client:\n  {}",
        report.join("\n  ")
    );
    // Windows' own tools rename.
    assert_eq!(
        move_plain,
        Ok(()),
        "MoveFileExW without flags (Explorer, cmd ren)"
    );
    assert_eq!(rename_item, Ok(()), "PowerShell Rename-Item");
    assert_eq!(move_item, Ok(()), "PowerShell Move-Item");
    // std::fs::rename asks to replace the target: the documented limitation.
    assert!(
        match &std_rename {
            Ok(()) => true,
            Err(e) => e.kind() == ErrorKind::CrossesDevices,
        },
        "std::fs::rename: {std_rename:?}"
    );
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
    pub use windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileRenameInfo, FileRenameInfoEx, MoveFileExW, SetFileInformationByHandle,
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain([0]).collect()
    }

    /// The network name of `drive` (`V:`).
    pub fn remote_name(drive: &str) -> Option<String> {
        use windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW;
        let local: Vec<u16> = drive.encode_utf16().chain([0]).collect();
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: NUL-terminated input, a buffer of `len` wide chars.
        if unsafe { WNetGetConnectionW(local.as_ptr(), buf.as_mut_ptr(), &mut len) } != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    fn open(p: &Path, access: u32) -> Result<windows_sys::Win32::Foundation::HANDLE, u32> {
        let w = wide(p);
        // SAFETY: a NUL-terminated path; no security attributes or template.
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            // SAFETY: no arguments.
            Err(unsafe { GetLastError() })
        } else {
            Ok(h)
        }
    }

    /// `FileRenameInfo` with `RootDirectory` set to `dir` and a bare name.
    pub fn rename_relative(from: &Path, dir: &Path, name: &str) -> Result<(), u32> {
        let d = open(
            dir,
            0x0001 /* FILE_LIST_DIRECTORY */ | 0x0002, /* FILE_ADD_FILE */
        )?;
        let r = set_rename(from, &name.encode_utf16().collect::<Vec<_>>(), false, 0, d);
        // SAFETY: our handle.
        unsafe { CloseHandle(d) };
        r
    }

    /// `MoveFileExW`; the error is the Win32 code.
    pub fn move_file_ex(from: &Path, to: &Path, flags: u32) -> Result<(), u32> {
        let (f, t) = (wide(from), wide(to));
        // SAFETY: NUL-terminated wide strings.
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), flags) } != 0 {
            Ok(())
        } else {
            // SAFETY: no arguments.
            Err(unsafe { GetLastError() })
        }
    }

    /// `SetFileInformationByHandle` with `FileRenameInfo` (or `…Ex` with
    /// `flags`; for the plain class `flags` != 0 is ReplaceIfExists).
    pub fn rename_info(from: &Path, to: &Path, ex: bool, flags: u32) -> Result<(), u32> {
        let name: Vec<u16> = to.as_os_str().encode_wide().collect();
        set_rename(from, &name, ex, flags, std::ptr::null_mut())
    }

    fn set_rename(
        from: &Path,
        name: &[u16],
        ex: bool,
        flags: u32,
        root: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<(), u32> {
        let h = open(from, DELETE)?;
        let head = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let size = head + (name.len() + 1) * 2;
        // u64 words: aligned for FILE_RENAME_INFO.
        let mut buf = vec![0u64; size.div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: `buf` holds the struct and the name with its NUL.
        let ok = unsafe {
            if ex {
                (*info).Anonymous.Flags = flags;
            } else {
                (*info).Anonymous.ReplaceIfExists = flags != 0;
            }
            (*info).RootDirectory = root;
            (*info).FileNameLength = (name.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
            SetFileInformationByHandle(
                h,
                if ex { FileRenameInfoEx } else { FileRenameInfo },
                info.cast(),
                size as u32,
            )
        };
        // SAFETY: no arguments.
        let err = unsafe { GetLastError() };
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        if ok != 0 {
            Ok(())
        } else {
            Err(err)
        }
    }
}
