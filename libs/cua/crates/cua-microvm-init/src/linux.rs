//! The Linux half of the init (see the crate docs).

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use cua_microvm_init::{
    BootConfig, Exec, ExecFrame, Identity, PORT, Reply, Request, TOKEN_FILE, group_lookup,
    passwd_lookup, split_user, table_listens,
};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// Exit statuses collected by the reaper, by pid.
struct Reaped {
    statuses: Mutex<HashMap<i32, i32>>,
    changed: Condvar,
}

static REAPED: OnceLock<Reaped> = OnceLock::new();
/// The workload's pid (0 until booted).
static WORKLOAD: AtomicI32 = AtomicI32::new(0);
/// Set by SIGINT (Ctrl-Alt-Del) and SIGTERM: shut down.
static STOP: AtomicBool = AtomicBool::new(false);
/// The workload's environment, also the base of `exec` requests.
static ENV: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();

fn reaped() -> &'static Reaped {
    REAPED.get_or_init(|| Reaped {
        statuses: Mutex::new(HashMap::new()),
        changed: Condvar::new(),
    })
}

fn env_store() -> &'static Mutex<Vec<(String, String)>> {
    ENV.get_or_init(|| Mutex::new(Vec::new()))
}

fn log(msg: impl AsRef<str>) {
    eprintln!("cua-init: {}", msg.as_ref());
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn mount(
    src: &str,
    target: &str,
    fstype: &str,
    flags: libc::c_ulong,
    data: &str,
) -> std::io::Result<()> {
    let (s, t, f, d) = (cstr(src), cstr(target), cstr(fstype), cstr(data));
    let rc = unsafe {
        libc::mount(
            s.as_ptr(),
            t.as_ptr(),
            if fstype.is_empty() {
                std::ptr::null()
            } else {
                f.as_ptr()
            },
            flags,
            if data.is_empty() {
                std::ptr::null()
            } else {
                d.as_ptr().cast()
            },
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        let e = std::io::Error::last_os_error();
        Err(std::io::Error::new(
            e.kind(),
            format!("mount {target} ({fstype}): {e}"),
        ))
    }
}

fn mkdir(p: &str) {
    let _ = std::fs::create_dir_all(p);
}

/// Mounts the image under an overlay and switches root into it.
fn switch_root() -> std::io::Result<()> {
    for d in ["/dev", "/proc", "/sys", "/base", "/rw", "/newroot"] {
        mkdir(d);
    }
    // The kernel mounts nothing for an initramfs.
    let _ = mount("devtmpfs", "/dev", "devtmpfs", libc::MS_NOSUID, "mode=0755");
    mount(
        "proc",
        "/proc",
        "proc",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        "",
    )?;
    mount(
        "sysfs",
        "/sys",
        "sysfs",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        "",
    )?;
    // The image is an ext4 on virtio-pmem mapped with DAX (every VM reads
    // the host's page cache) and the scratch disk the only block device;
    // or a squashfs on the first block device and the scratch disk second.
    let scratch = if Path::new("/dev/pmem0").exists() {
        mount(
            "/dev/pmem0",
            "/base",
            "ext4",
            libc::MS_RDONLY,
            "dax=always,noload",
        )
        .or_else(|_| mount("/dev/pmem0", "/base", "ext4", libc::MS_RDONLY, "noload"))?;
        "/dev/vda"
    } else {
        if mount("/dev/vda", "/base", "squashfs", libc::MS_RDONLY, "").is_err() {
            mount("/dev/vda", "/base", "ext4", libc::MS_RDONLY, "")?;
        }
        "/dev/vdb"
    };
    mount(scratch, "/rw", "ext4", libc::MS_NOATIME, "")?;
    grow_scratch(scratch, "/rw");
    mkdir("/rw/upper");
    mkdir("/rw/work");
    mount(
        "overlay",
        "/newroot",
        "overlay",
        0,
        "lowerdir=/base,upperdir=/rw/upper,workdir=/rw/work",
    )?;
    for (old, new) in [
        ("/dev", "/newroot/dev"),
        ("/proc", "/newroot/proc"),
        ("/sys", "/newroot/sys"),
    ] {
        mkdir(new);
        mount(old, new, "", libc::MS_MOVE, "")?;
    }
    apply_xattrs("/newroot");
    std::env::set_current_dir("/newroot")?;
    mount(".", "/", "", libc::MS_MOVE, "")?;
    let dot = cstr(".");
    if unsafe { libc::chroot(dot.as_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    std::env::set_current_dir("/")?;
    Ok(())
}

/// Sets the extended attributes [`cua_microvm_init::XATTRS_FILE`] lists
/// (in the merged root at `root`, so the files are copied up into the
/// scratch disk once), then removes the list.
fn apply_xattrs(root: &str) {
    use base64::Engine as _;
    let list = format!("{root}/{}", cua_microvm_init::XATTRS_FILE);
    let Ok(raw) = std::fs::read(&list) else {
        return;
    };
    let entries: Vec<cua_microvm_init::Xattr> = match serde_json::from_slice(&raw) {
        Ok(e) => e,
        Err(e) => return log(format!("{list}: {e}")),
    };
    for x in &entries {
        let (Ok(value), Ok(path), Ok(name)) = (
            B64.decode(&x.value),
            CString::new(format!("{root}/{}", x.path)),
            CString::new(x.name.as_str()),
        ) else {
            continue;
        };
        let rc = unsafe {
            libc::lsetxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        if rc != 0 {
            log(format!(
                "xattr {} on /{}: {}",
                x.name,
                x.path,
                std::io::Error::last_os_error()
            ));
        }
    }
    let _ = std::fs::remove_file(&list);
}

/// Grows the scratch filesystem mounted at `mnt` to fill its device
/// `dev` (online, `EXT4_IOC_RESIZE_FS`): a host that cannot make an ext4
/// of the asked size (macOS) attaches a small formatted one in a larger,
/// sparse file. Nothing to do when it already fills the device. The
/// filesystem grows in the background.
fn grow_scratch(dev: &str, mnt: &str) {
    use std::os::fd::AsRawFd;
    let size = || -> std::io::Result<(u64, u64, u64)> {
        let mut f = File::open(dev)?;
        let mut bytes: u64 = 0;
        // BLKGETSIZE64 = _IOR(0x12, 114, size_t)
        const BLKGETSIZE64: libc::c_ulong = 0x8008_1272;
        if unsafe { libc::ioctl(f.as_raw_fd(), BLKGETSIZE64 as _, &mut bytes) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut sb = vec![0u8; 2048];
        f.read_exact(&mut sb)?;
        let (blocks, block) = cua_microvm_init::ext4_size(&sb[1024..])
            .ok_or_else(|| std::io::Error::other("no ext4 superblock"))?;
        Ok((blocks, block, bytes))
    };
    let (blocks, block, bytes) = match size() {
        Ok(s) => s,
        Err(e) => return log(format!("scratch size: {e}")),
    };
    let Some(mut to) = cua_microvm_init::ext4_grow_to(blocks, block, bytes) else {
        return;
    };
    let Ok(dir) = File::open(mnt) else { return };
    // In the background (it takes a few hundred ms): the directory stays
    // open across the switch of root, and the filesystem is usable while
    // it grows.
    std::thread::spawn(move || {
        // EXT4_IOC_RESIZE_FS = _IOW('f', 16, __u64)
        const EXT4_IOC_RESIZE_FS: libc::c_ulong = 0x4008_6610;
        if unsafe { libc::ioctl(dir.as_raw_fd(), EXT4_IOC_RESIZE_FS as _, &mut to) } != 0 {
            log(format!(
                "growing the scratch disk to {} GiB: {}",
                bytes >> 30,
                std::io::Error::last_os_error()
            ));
        }
    });
}

/// The mounts a Linux userland expects, and `/dev`'s symlinks.
fn mount_basics() {
    mkdir("/dev/pts");
    mkdir("/dev/shm");
    mkdir("/run");
    let _ = mount(
        "devpts",
        "/dev/pts",
        "devpts",
        libc::MS_NOSUID | libc::MS_NOEXEC,
        "newinstance,ptmxmode=0666,mode=0620,gid=5",
    );
    let _ = mount(
        "shm",
        "/dev/shm",
        "tmpfs",
        libc::MS_NOSUID | libc::MS_NODEV,
        "mode=1777",
    );
    let _ = mount(
        "run",
        "/run",
        "tmpfs",
        libc::MS_NOSUID | libc::MS_NODEV,
        "mode=0755",
    );
    mkdir("/dev/mqueue");
    let _ = mount(
        "mqueue",
        "/dev/mqueue",
        "mqueue",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        "",
    );
    let _ = mount(
        "cgroup2",
        "/sys/fs/cgroup",
        "cgroup2",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
        "",
    );
    let _ = std::fs::remove_file("/dev/ptmx");
    let _ = std::os::unix::fs::symlink("pts/ptmx", "/dev/ptmx");
    for (link, target) in [
        ("/dev/fd", "/proc/self/fd"),
        ("/dev/stdin", "/proc/self/fd/0"),
        ("/dev/stdout", "/proc/self/fd/1"),
        ("/dev/stderr", "/proc/self/fd/2"),
    ] {
        let _ = std::os::unix::fs::symlink(target, link);
    }
    if !Path::new("/tmp").exists() {
        mkdir("/tmp");
    }
    let _ = std::fs::set_permissions("/tmp", std::os::unix::fs::PermissionsExt::from_mode(0o1777));
}

/// `lo` up (the kernel's `ip=` parameter brings up `eth0`).
fn loopback_up() {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return;
        }
        let mut req: libc::ifreq = std::mem::zeroed();
        for (i, b) in b"lo\0".iter().enumerate() {
            req.ifr_name[i] = *b as libc::c_char;
        }
        if libc::ioctl(fd, libc::SIOCGIFFLAGS as _, &mut req) == 0 {
            req.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            libc::ioctl(fd, libc::SIOCSIFFLAGS as _, &req);
        }
        libc::close(fd);
    }
}

fn write_etc(hostname: &str) {
    let hosts = format!(
        "127.0.0.1\tlocalhost\n::1\tlocalhost ip6-localhost ip6-loopback\n127.0.1.1\t{hostname}\n"
    );
    replace_file("/etc/hosts", hosts.as_bytes(), 0o644);
    replace_file("/etc/hostname", format!("{hostname}\n").as_bytes(), 0o644);
}

/// Writes `path` (replacing a symlink, which images often have for
/// `/etc/resolv.conf`).
fn replace_file(path: &str, data: &[u8], mode: u32) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = std::fs::remove_file(path);
    }
    if let Some(dir) = Path::new(path).parent() {
        mkdir(&dir.to_string_lossy());
    }
    if std::fs::write(path, data).is_ok() {
        let _ = std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(mode));
    }
}

fn set_hostname(name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    unsafe {
        libc::sethostname(name.as_ptr().cast(), name.len());
    }
    write_etc(name);
}

fn apply_identity(id: &Identity) -> Result<(), String> {
    if let Some(n) = &id.unix_nanos {
        let tv = libc::timeval {
            tv_sec: (*n / 1_000_000_000) as _,
            tv_usec: ((*n % 1_000_000_000) / 1000) as _,
        };
        unsafe {
            libc::settimeofday(&tv, std::ptr::null());
        }
    }
    if let Some(hex) = &id.entropy {
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
            .collect();
        // The machine id is world-readable: a host that sends none (an
        // older one) has it taken from its own 16 bytes, and only the rest
        // is credited to the pool as secret.
        let (pool, id_bytes) = if id.machine_id.is_some() {
            (&bytes[..], &[][..])
        } else if bytes.len() >= 32 {
            bytes.split_at(bytes.len() - 16)
        } else {
            (&bytes[..], &bytes[..bytes.len().min(16)])
        };
        add_entropy(pool);
        if id_bytes.len() == 16 {
            let derived: String = id_bytes.iter().map(|b| format!("{b:02x}")).collect();
            set_machine_id(&derived);
        }
    }
    // A copy of a VM must not keep its machine id.
    if let Some(m) = &id.machine_id {
        set_machine_id(m);
    }
    if let Some(h) = &id.hostname {
        set_hostname(h);
    }
    if let Some(token) = &id.token {
        write_token(token, id.token_group.as_deref().unwrap_or("cua"))?;
    }
    if !id.env.is_empty() {
        let mut env = env_store().lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in &id.env {
            env.retain(|(key, _)| key != k);
            env.push((k.clone(), v.clone()));
        }
    }
    Ok(())
}

/// Writes the machine id (`/etc/machine-id`, created when the image has
/// none, and D-Bus's own copy when the image keeps one). Programs already
/// running keep the id they read at start.
fn set_machine_id(id: &str) {
    let id = id.trim();
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return;
    }
    let line = format!("{}\n", id.to_ascii_lowercase());
    replace_file("/etc/machine-id", line.as_bytes(), 0o444);
    let dbus = "/var/lib/dbus/machine-id";
    if std::fs::symlink_metadata(dbus).is_ok_and(|m| m.file_type().is_file()) {
        replace_file(dbus, line.as_bytes(), 0o444);
    }
}

/// Mixes `bytes` into the kernel's pool and credits them
/// (`RNDADDENTROPY`; they come from the host's CSPRNG), then reseeds the
/// CRNG at once (`RNDRESEEDCRNG`): crediting alone leaves a restored
/// guest's CRNG on its snapshot's state until the next scheduled reseed
/// (up to a minute), unless the kernel saw a VMGenID change.
fn add_entropy(bytes: &[u8]) {
    #[repr(C)]
    struct RandPoolInfo {
        entropy_count: libc::c_int,
        buf_size: libc::c_int,
        buf: [u8; 64],
    }
    let n = bytes.len().min(64);
    let mut info = RandPoolInfo {
        entropy_count: (n * 8) as libc::c_int,
        buf_size: n as libc::c_int,
        buf: [0; 64],
    };
    info.buf[..n].copy_from_slice(&bytes[..n]);
    // _IOW('R', 0x03, int[2])
    const RNDADDENTROPY: libc::c_ulong = 0x4008_5203;
    // _IO('R', 0x07)
    const RNDRESEEDCRNG: libc::c_ulong = 0x5207;
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open("/dev/random") {
        use std::os::fd::AsRawFd;
        unsafe {
            if libc::ioctl(f.as_raw_fd(), RNDADDENTROPY as _, &info) != 0 {
                log(format!(
                    "RNDADDENTROPY: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if libc::ioctl(f.as_raw_fd(), RNDRESEEDCRNG as _) != 0 {
                log(format!(
                    "RNDRESEEDCRNG: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }
}

/// The token file, as `ensure-env-token.sh` writes it: `/run/cua` 0750 and
/// the file 0640, both root:`group`.
fn write_token(token: &str, group: &str) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let gid = std::fs::read_to_string("/etc/group")
        .ok()
        .and_then(|g| group_lookup(&g, group))
        .unwrap_or(0);
    let dir = Path::new(TOKEN_FILE)
        .parent()
        .unwrap_or(Path::new("/run/cua"));
    // cua-spacesd watches this directory and wakes on any event in it: the
    // only one a rotation makes is the final rename (the file is written
    // next door, and the directory is fixed only when it is not right
    // already), so the new token is read the moment it is in place.
    let meta = std::fs::metadata(dir).ok();
    if meta.is_none() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if meta.as_ref().is_none_or(|m| m.uid() != 0 || m.gid() != gid) {
        chown(dir, 0, gid);
    }
    if meta
        .as_ref()
        .is_none_or(|m| m.permissions().mode() & 0o7777 != 0o750)
    {
        let _ = std::fs::set_permissions(dir, PermissionsExt::from_mode(0o750));
    }
    // A private directory on the same filesystem (/run), so the rename is
    // atomic.
    let stage = dir.parent().unwrap_or(Path::new("/run")).join(".cua-init");
    std::fs::create_dir_all(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    let _ = std::fs::set_permissions(&stage, PermissionsExt::from_mode(0o700));
    let tmp = stage.join("env-token");
    let body = if token.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", token.trim())
    };
    std::fs::write(&tmp, body).map_err(|e| format!("token: {e}"))?;
    chown(&tmp, 0, gid);
    let _ = std::fs::set_permissions(&tmp, PermissionsExt::from_mode(0o640));
    std::fs::rename(&tmp, TOKEN_FILE).map_err(|e| format!("token: {e}"))?;
    Ok(())
}

fn chown(p: &Path, uid: u32, gid: u32) {
    if let Ok(c) = CString::new(p.as_os_str().as_encoded_bytes()) {
        unsafe {
            libc::chown(c.as_ptr(), uid, gid);
        }
    }
}

/// Applies `user[:group]` (names from the image's `/etc/passwd`) to `cmd`.
fn apply_user(cmd: &mut Command, spec: Option<&str>) -> Result<Option<String>, String> {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let (user, group) = split_user(spec);
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let (uid, mut gid, home) =
        passwd_lookup(&passwd, user).ok_or_else(|| format!("unknown user {user:?}"))?;
    if let Some(g) = group {
        let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
        gid = group_lookup(&groups, g).ok_or_else(|| format!("unknown group {g:?}"))?;
    }
    cmd.gid(gid).uid(uid);
    Ok(Some(home))
}

fn boot(cfg: &BootConfig) -> Result<(), String> {
    if WORKLOAD.load(Ordering::SeqCst) != 0 {
        return Err("the workload is already running".into());
    }
    if let Some(id) = &cfg.identity {
        apply_identity(id)?;
    }
    let (prog, args) = cfg
        .argv
        .split_first()
        .ok_or_else(|| "the image has no command (ENTRYPOINT/CMD)".to_string())?;
    {
        let mut env = env_store().lock().unwrap_or_else(|e| e.into_inner());
        *env = cfg.env.clone();
    }
    // Await-token-file mode: an empty token file from the start, so
    // cua-spacesd follows this file (not the token-sync copy) and sees the
    // token the moment the host sends it.
    let awaits = cfg
        .env
        .iter()
        .any(|(k, v)| k == "CUA_ENV_AWAIT_TOKEN_FILE" && v == "1");
    if awaits && !Path::new(TOKEN_FILE).exists() {
        write_token("", "cua")?;
    }
    let mut cmd = Command::new(prog);
    cmd.args(args).env_clear().envs(cfg.env.iter().cloned());
    if !cfg.env.iter().any(|(k, _)| k == "PATH") {
        cmd.env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        );
    }
    let home = apply_user(&mut cmd, cfg.user.as_deref())?;
    if !cfg.env.iter().any(|(k, _)| k == "HOME") {
        cmd.env("HOME", home.as_deref().unwrap_or("/root"));
    }
    if let Some(h) = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
    {
        cmd.env("HOSTNAME", h);
    }
    cmd.current_dir(
        cfg.workdir
            .as_deref()
            .filter(|w| !w.is_empty())
            .unwrap_or("/"),
    );
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            // PID 1 ignores nothing for its children: restore defaults.
            for s in [libc::SIGINT, libc::SIGTERM, libc::SIGCHLD, libc::SIGPIPE] {
                libc::signal(s, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let child = cmd
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("start {prog}: {e}"))?;
    WORKLOAD.store(child.id() as i32, Ordering::SeqCst);
    log(format!(
        "started {} (pid {})",
        cfg.argv.join(" "),
        child.id()
    ));
    // The reaper owns the exit status; never wait on `child`.
    std::mem::forget(child);
    Ok(())
}

/// Waits for `pid`'s exit status from the reaper, until `deadline`.
fn wait_pid(pid: i32, deadline: Option<Instant>) -> Option<i32> {
    let r = reaped();
    let mut st = r.statuses.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(s) = st.remove(&pid) {
            return Some(s);
        }
        match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return None;
                }
                st = r
                    .changed
                    .wait_timeout(st, d - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            None => st = r.changed.wait(st).unwrap_or_else(|e| e.into_inner()),
        }
    }
}

fn exit_code(status: i32) -> i64 {
    if libc::WIFEXITED(status) {
        i64::from(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        128 + i64::from(libc::WTERMSIG(status))
    } else {
        -1
    }
}

fn send(w: &Mutex<File>, line: &impl serde::Serialize) {
    let mut s = serde_json::to_vec(line).unwrap_or_default();
    s.push(b'\n');
    let mut f = w.lock().unwrap_or_else(|e| e.into_inner());
    let _ = f.write_all(&s);
}

fn exec(req: &Exec, w: &Arc<Mutex<File>>) {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(&req.script).env_clear();
    {
        let env = env_store().lock().unwrap_or_else(|e| e.into_inner());
        cmd.envs(env.iter().cloned());
    }
    cmd.envs(req.env.iter().cloned());
    match apply_user(&mut cmd, req.user.as_deref()) {
        Ok(Some(home)) => {
            cmd.env("HOME", home);
        }
        Ok(None) => {}
        Err(e) => return send(w, &ExecFrame::Error(e)),
    }
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            for s in [libc::SIGINT, libc::SIGTERM, libc::SIGCHLD, libc::SIGPIPE] {
                libc::signal(s, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let stdin = match &req.stdin {
        Some(b) => match B64.decode(b) {
            Ok(v) => Some(v),
            Err(e) => return send(w, &ExecFrame::Error(format!("stdin: {e}"))),
        },
        None => None,
    };
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .current_dir("/");
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return send(w, &ExecFrame::Error(format!("/bin/sh: {e}"))),
    };
    let pid = child.id() as i32;
    if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    let pump = |mut r: Box<dyn Read + Send>, err: bool, w: Arc<Mutex<File>>| {
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match r.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = B64.encode(&buf[..n]);
                        send(
                            &w,
                            &if err {
                                ExecFrame::Stderr(data)
                            } else {
                                ExecFrame::Stdout(data)
                            },
                        );
                    }
                }
            }
        })
    };
    let out = child
        .stdout
        .take()
        .map(|o| pump(Box::new(o), false, w.clone()));
    let err = child
        .stderr
        .take()
        .map(|e| pump(Box::new(e), true, w.clone()));
    std::mem::forget(child);
    let deadline = req
        .timeout_ms
        .map(|ms| Instant::now() + Duration::from_millis(ms));
    let status = match wait_pid(pid, deadline) {
        Some(s) => exit_code(s),
        None => {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
                libc::kill(pid, libc::SIGKILL);
            }
            wait_pid(pid, Some(Instant::now() + Duration::from_secs(5)));
            124
        }
    };
    for t in [out, err].into_iter().flatten() {
        let _ = t.join();
    }
    send(w, &ExecFrame::Exit(status));
}

fn listening(port: u16) -> bool {
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .any(|t| table_listens(&t, port))
}

/// Stops everything and powers off (Firecracker exits on a guest reboot).
fn power_off(grace: Duration) -> ! {
    let pid = WORKLOAD.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
            libc::kill(pid, libc::SIGTERM);
        }
        let _ = wait_pid(pid, Some(Instant::now() + grace));
    }
    unsafe {
        libc::kill(-1, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(200));
    unsafe {
        libc::kill(-1, libc::SIGKILL);
        libc::sync();
        libc::reboot(libc::RB_AUTOBOOT);
    }
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn handle(conn: OwnedFd) {
    let file = File::from(conn);
    let Ok(reader) = file.try_clone() else { return };
    let w = Arc::new(Mutex::new(file));
    let mut line = String::new();
    let mut reader = BufReader::new(reader);
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => return send(&w, &Reply::err(format!("bad request: {e}"))),
    };
    match req {
        Request::Ping => send(
            &w,
            &Reply {
                version: Some(cua_microvm_init::VERSION.into()),
                booted: Some(WORKLOAD.load(Ordering::SeqCst) != 0),
                ..Reply::ok()
            },
        ),
        Request::Boot(cfg) => send(
            &w,
            &match boot(&cfg) {
                Ok(()) => Reply::ok(),
                Err(e) => Reply::err(e),
            },
        ),
        Request::Identity(id) => send(
            &w,
            &match apply_identity(&id) {
                Ok(()) => Reply::ok(),
                Err(e) => Reply::err(e),
            },
        ),
        Request::Listening { port } => send(
            &w,
            &Reply {
                listening: Some(listening(port)),
                ..Reply::ok()
            },
        ),
        Request::Exec(e) => exec(&e, &w),
        Request::Sync => {
            unsafe { libc::sync() };
            send(&w, &Reply::ok());
        }
        Request::Shutdown { grace_secs } => {
            send(&w, &Reply::ok());
            power_off(Duration::from_secs(grace_secs));
        }
        Request::Forward { port } => forward(port, reader, &w),
    }
}

/// [`Request::Forward`]: connects to the guest's `127.0.0.1:port`, answers,
/// then copies bytes both ways until either side closes. Bytes the client
/// sent after its request line (already in `reader`'s buffer) go first.
fn forward(port: u16, mut reader: BufReader<File>, w: &Arc<Mutex<File>>) {
    use std::net::{Shutdown, TcpStream};
    let tcp = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(t) => t,
        Err(e) => return send(w, &Reply::err(format!("127.0.0.1:{port}: {e}"))),
    };
    let _ = tcp.set_nodelay(true);
    send(w, &Reply::ok());
    let (Ok(mut to_guest), Ok(mut from_guest)) = (tcp.try_clone(), tcp.try_clone()) else {
        return;
    };
    let pending = reader.buffer().to_vec();
    let up = std::thread::spawn(move || {
        if to_guest.write_all(&pending).is_ok() {
            let _ = std::io::copy(reader.get_mut(), &mut to_guest);
        }
        let _ = to_guest.shutdown(Shutdown::Write);
    });
    let out = {
        let f = w.lock().unwrap_or_else(|e| e.into_inner());
        f.try_clone()
    };
    if let Ok(mut out) = out {
        let _ = std::io::copy(&mut from_guest, &mut out);
        // Half-close toward the host: vsock stream sockets support it.
        unsafe {
            use std::os::fd::AsRawFd;
            libc::shutdown(out.as_raw_fd(), libc::SHUT_WR);
        }
    }
    let _ = up.join();
    let _ = tcp.shutdown(Shutdown::Both);
}

fn vsock_listener() -> std::io::Result<OwnedFd> {
    unsafe {
        let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_vm = std::mem::zeroed();
        addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
        addr.svm_port = PORT;
        addr.svm_cid = libc::VMADDR_CID_ANY;
        use std::os::fd::AsRawFd;
        if libc::bind(
            fd.as_raw_fd(),
            (&addr as *const libc::sockaddr_vm).cast(),
            std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        ) != 0
            || libc::listen(fd.as_raw_fd(), 64) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd)
    }
}

fn serve(listener: OwnedFd) {
    use std::os::fd::AsRawFd;
    loop {
        let c = unsafe {
            libc::accept4(
                listener.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        };
        if c < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                log(format!("vsock accept: {e}"));
                std::thread::sleep(Duration::from_millis(50));
            }
            continue;
        }
        let conn = unsafe { OwnedFd::from_raw_fd(c) };
        std::thread::spawn(move || handle(conn));
    }
}

extern "C" fn on_stop(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn install_signals() {
    unsafe {
        // Ctrl-Alt-Del (Firecracker's SendCtrlAltDel) signals PID 1
        // instead of rebooting at once.
        libc::reboot(libc::LINUX_REBOOT_CMD_CAD_OFF);
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_stop as *const () as usize;
        // No SA_RESTART: waitpid returns EINTR so the reaper sees STOP.
        sa.sa_flags = 0;
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Reaps every child (PID 1 inherits orphans), hands statuses to waiters,
/// and powers off when the workload exits or on Ctrl-Alt-Del.
fn reap_forever() -> ! {
    loop {
        if STOP.load(Ordering::SeqCst) {
            log("shutdown requested");
            power_off(Duration::from_secs(10));
        }
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid < 0 {
            // No children yet (before the workload), or EINTR.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let r = reaped();
        let workload = WORKLOAD.load(Ordering::SeqCst);
        if pid == workload {
            log(format!("workload exited ({})", exit_code(status)));
            WORKLOAD.store(-1, Ordering::SeqCst);
            power_off(Duration::from_secs(5));
        }
        let mut st = r.statuses.lock().unwrap_or_else(|e| e.into_inner());
        st.insert(pid, status);
        // Orphans nobody waits for would pile up: keep the map bounded.
        if st.len() > 4096 {
            st.clear();
        }
        drop(st);
        r.changed.notify_all();
    }
}

pub fn main() {
    if std::process::id() != 1 {
        eprintln!("cua-microvm-init must run as PID 1 of a microVM");
        std::process::exit(2);
    }
    install_signals();
    if let Err(e) = switch_root() {
        log(format!("cannot mount the image: {e}"));
        std::thread::sleep(Duration::from_secs(1));
        unsafe { libc::reboot(libc::RB_AUTOBOOT) };
    }
    mount_basics();
    loopback_up();
    unsafe {
        let lim = libc::rlimit {
            rlim_cur: 1 << 20,
            rlim_max: 1 << 20,
        };
        libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
    }
    replace_file(
        "/etc/resolv.conf",
        format!("nameserver {}\n", cua_microvm_init::DNS_IP).as_bytes(),
        0o644,
    );
    set_hostname("localhost");
    match vsock_listener() {
        Ok(l) => {
            std::thread::spawn(move || serve(l));
        }
        Err(e) => log(format!("vsock: {e}")),
    }
    log("ready");
    reap_forever();
}
