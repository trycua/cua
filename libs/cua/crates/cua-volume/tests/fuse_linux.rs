// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in (needs `/dev/fuse` and `fusermount3`; set CUA_DRIVE_FUSE_TEST=1):
//! mounts a drive with FUSE and uses it through the kernel like any
//! program would. `tests/run-fuse-linux.sh` runs it in a Linux container.
#![cfg(all(feature = "fuse", target_os = "linux"))]

use std::sync::Arc;
use std::time::Duration;

use cua_volume::vfs::Vfs;
use cua_volume::{Condition, Context, Drive};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drive_works_through_the_kernel() {
    if std::env::var("CUA_DRIVE_FUSE_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_DRIVE_FUSE_TEST=1 (tests/run-fuse-linux.sh)");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let mnt = dir.path().join("mnt");
    std::fs::create_dir_all(&mnt).unwrap();
    let drive = Drive::open_local(&home);
    let user = drive.session(Context::user());
    let big: Vec<u8> = (0..(24u32 << 20)).map(|i| (i % 251) as u8).collect();
    user.write("public/big.bin", big.clone(), Condition::None)
        .await
        .unwrap();
    let vfs = Vfs::new(
        &drive,
        Context::user(),
        None,
        None,
        &home.join("drive/mount"),
    )
    .unwrap();
    let m = cua_volume::fuse::FuseMount::mount(vfs.clone(), &mnt).unwrap();
    let p = mnt.clone();
    let big2 = big.clone();
    let t0 = std::time::Instant::now();
    // Kernel-facing file operations run off the async workers.
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut names: Vec<String> = std::fs::read_dir(&p)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["agents", "public", "spaces"]);
        // A ranged read in the middle of a large file.
        let mut f = std::fs::File::open(p.join("public/big.bin")).unwrap();
        assert_eq!(f.metadata().unwrap().len(), big2.len() as u64);
        f.seek(SeekFrom::Start(12 << 20)).unwrap();
        let mut buf = vec![0u8; 65536];
        f.read_exact(&mut buf).unwrap();
        assert_eq!(buf, big2[(12 << 20)..(12 << 20) + 65536]);
        // Write, close (uploads), read back; folders; rename; delete.
        std::fs::create_dir(p.join("public/notes")).unwrap();
        let mut w = std::fs::File::create(p.join("public/notes/a.md")).unwrap();
        w.write_all(b"hello from fuse").unwrap();
        drop(w);
        std::fs::rename(p.join("public/notes/a.md"), p.join("public/notes/b.md")).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.join("public/notes/b.md")).unwrap(),
            "hello from fuse"
        );
        assert!(std::fs::metadata(p.join("public/notes/a.md")).is_err());
        std::fs::write(p.join("public/gone.txt"), b"x").unwrap();
        std::fs::remove_file(p.join("public/gone.txt")).unwrap();
        assert!(std::fs::create_dir(p.join("public/.cua-x")).is_err());
        eprintln!("kernel file operations took {:?}", t0.elapsed());
    })
    .await
    .unwrap();
    // The drive (not just the mount) holds the result.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        user.read("public/notes/b.md", None).await.unwrap().0,
        b"hello from fuse"
    );
    assert!(user.read("public/notes/a.md", None).await.is_err());
    assert!(user.read("public/gone.txt", None).await.is_err());
    // A change made outside the mount shows up in it.
    user.write(
        "public/notes/c.md",
        b"from the api".to_vec(),
        Condition::None,
    )
    .await
    .unwrap();
    let p = mnt.clone();
    tokio::task::spawn_blocking(move || {
        for _ in 0..40 {
            if std::fs::read(p.join("public/notes/c.md")).ok().as_deref()
                == Some(&b"from the api"[..])
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the API write never showed up in the mount");
    })
    .await
    .unwrap();
    m.unmount().await.unwrap();
    assert!(
        std::fs::read_dir(&mnt).unwrap().next().is_none(),
        "unmounted"
    );
    let _ = Arc::strong_count(&vfs);
}

/// The guest path: a Space mounts its view through a stream to the host
/// (here a loopback TCP connection standing in for the spacesd tunnel), as
/// an unprivileged user when run that way (`fusermount3`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_space_mounts_its_view_through_the_host() {
    use cua_volume::remote::{RemoteFs, serve};
    use cua_volume::vfs::FsOps;
    if std::env::var("CUA_DRIVE_FUSE_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_DRIVE_FUSE_TEST=1 (tests/run-fuse-linux.sh)");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let mnt = dir.path().join("volume");
    std::fs::create_dir_all(&mnt).unwrap();
    let drive = Drive::open_local(&home);
    let user = drive.session(Context::user());
    user.write("public/rules.md", b"be kind".to_vec(), Condition::None)
        .await
        .unwrap();
    // Host: the Space's own view, served on loopback.
    let vfs = Vfs::new(
        &drive,
        Context::space("local:lab"),
        None,
        None,
        &home.join("drive/m"),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let host_fs: Arc<dyn FsOps> = vfs.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(serve(s, host_fs.clone()));
        }
    });
    // Guest: FUSE over the stream.
    let remote = RemoteFs::new(tokio::net::TcpStream::connect(addr).await.unwrap());
    // As root (spacesd's mount helper) the mount is shared with the desktop
    // user; unprivileged it is the caller's own.
    // SAFETY: geteuid never fails.
    let root = unsafe { libc::geteuid() } == 0;
    let m =
        cua_volume::fuse::FuseMount::mount_with(remote, &mnt, root, "cua-volume", None).unwrap();
    let p = mnt.clone();
    let t0 = std::time::Instant::now();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        assert_eq!(
            std::fs::read_to_string(p.join("public/rules.md")).unwrap(),
            "be kind"
        );
        // public/ is read-only for a Space.
        assert!(std::fs::write(p.join("public/x.md"), b"no").is_err());
        let mine = p.join("spaces/local-lab");
        let mut f = std::fs::File::create(mine.join("report.md")).unwrap();
        f.write_all(b"written in the Space").unwrap();
        drop(f);
        assert_eq!(
            std::fs::read_to_string(mine.join("report.md")).unwrap(),
            "written in the Space"
        );
        // Another agent's home is not visible to the Space.
        assert!(std::fs::read_dir(p.join("agents")).is_err());
        eprintln!("guest file operations took {:?}", t0.elapsed());
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        user.read("spaces/local-lab/report.md", None)
            .await
            .unwrap()
            .0,
        b"written in the Space"
    );
    m.unmount().await.unwrap();
}
