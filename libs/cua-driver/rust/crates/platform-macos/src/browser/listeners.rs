//! The TCP listening sockets of one process, read from the kernel through
//! libproc (in libSystem: no subprocess, no entitlement, no extra dependency).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use libc::{c_int, c_void};

/// `PROC_PIDFDSOCKETINFO`, `SOCKINFO_TCP`, `TSI_S_LISTEN`, `INI_IPV4` and
/// `INI_IPV6` from `<sys/proc_info.h>`.
const PROC_PIDFDSOCKETINFO: c_int = 3;
const SOCKINFO_TCP: c_int = 2;
const TSI_S_LISTEN: c_int = 1;
const INI_IPV4: u8 = 0x1;
const INI_IPV6: u8 = 0x2;

/// `struct in_sockinfo`, up to the local address.
#[repr(C)]
#[derive(Clone, Copy)]
struct InSockInfo {
    _insi_fport: c_int,
    insi_lport: c_int,
    _insi_gencnt: u64,
    _insi_flags: u32,
    _insi_flow: u32,
    insi_vflag: u8,
    _insi_ip_ttl: u8,
    _rfu_1: u32,
    _insi_faddr: [u8; 16],
    /// `union in4in6_addr`: an IPv4 address sits in the last 4 bytes.
    insi_laddr: [u8; 16],
    _insi_v4_v6: [u8; 16],
}

/// `struct tcp_sockinfo`.
#[repr(C)]
#[derive(Clone, Copy)]
struct TcpSockInfo {
    tcpsi_ini: InSockInfo,
    tcpsi_state: c_int,
    _tcpsi_timer: [c_int; 4],
    _tcpsi_mss: c_int,
    _tcpsi_flags: u32,
    _rfu_1: u32,
    _tcpsi_tp: u64,
}

/// `struct socket_info`; every field before `soi_kind` is opaque here.
#[repr(C)]
#[derive(Clone, Copy)]
struct SocketInfo {
    _head: [u64; 29],
    soi_kind: c_int,
    _rfu_1: u32,
    soi_proto: SocketProto,
}

/// The `soi_proto` union, sized by its largest member.
#[repr(C)]
#[derive(Clone, Copy)]
union SocketProto {
    pri_tcp: TcpSockInfo,
    _size: [u64; 66],
}

/// `struct socket_fdinfo`: a 24-byte `struct proc_fileinfo`, then the socket.
#[repr(C)]
#[derive(Clone, Copy)]
struct SocketFdInfo {
    _pfi: [u64; 3],
    psi: SocketInfo,
}

const _: () = assert!(std::mem::size_of::<InSockInfo>() == 80);
const _: () = assert!(std::mem::size_of::<TcpSockInfo>() == 120);
const _: () = assert!(std::mem::offset_of!(TcpSockInfo, tcpsi_state) == 80);
const _: () = assert!(std::mem::offset_of!(InSockInfo, insi_vflag) == 24);
const _: () = assert!(std::mem::offset_of!(InSockInfo, insi_laddr) == 48);
const _: () = assert!(std::mem::offset_of!(SocketInfo, soi_kind) == 232);
const _: () = assert!(std::mem::offset_of!(SocketInfo, soi_proto) == 240);
const _: () = assert!(std::mem::size_of::<SocketInfo>() == 768);
const _: () = assert!(std::mem::size_of::<SocketFdInfo>() == 792);

unsafe extern "C" {
    fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn proc_pidfdinfo(
        pid: c_int,
        fd: c_int,
        flavor: c_int,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
}

/// The local address of every TCP socket `pid` holds in LISTEN state, as
/// `lsof -a -n -p PID -iTCP -sTCP:LISTEN` reports it. A process that has
/// exited or belongs to another user has none.
pub(crate) fn tcp_listeners(pid: i64) -> std::io::Result<Vec<SocketAddr>> {
    let Ok(pid) = c_int::try_from(pid) else {
        return Ok(Vec::new());
    };
    let fds = open_fds(pid)?;
    let mut listeners = fds
        .iter()
        .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
        .filter_map(|fd| tcp_listener(pid, fd.proc_fd))
        .collect::<Vec<_>>();
    listeners.sort_unstable();
    listeners.dedup();
    Ok(listeners)
}

fn open_fds(pid: c_int) -> std::io::Result<Vec<libc::proc_fdinfo>> {
    let entry = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: a null buffer only asks for the size of the table.
    let probed = unsafe { proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if probed <= 0 {
        return gone_or_error();
    }
    // Headroom for descriptors opened between the two calls.
    let capacity = probed as usize / entry + 64;
    let mut fds = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        capacity
    ];
    // SAFETY: the kernel writes at most `capacity * entry` bytes into `fds`.
    let written = unsafe {
        proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast(),
            (capacity * entry) as c_int,
        )
    };
    if written <= 0 {
        return gone_or_error();
    }
    fds.truncate(written as usize / entry);
    Ok(fds)
}

fn gone_or_error<T>() -> std::io::Result<Vec<T>> {
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH | libc::EPERM) => Ok(Vec::new()),
        _ => Err(error),
    }
}

fn tcp_listener(pid: c_int, fd: i32) -> Option<SocketAddr> {
    let mut info = std::mem::MaybeUninit::<SocketFdInfo>::zeroed();
    let size = std::mem::size_of::<SocketFdInfo>() as c_int;
    // SAFETY: the kernel writes at most `size` bytes; a descriptor closed in
    // the meantime returns 0 and is skipped.
    let written = unsafe {
        proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDSOCKETINFO,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: fully written by the kernel, and every field is plain data.
    let info = unsafe { info.assume_init() };
    if info.psi.soi_kind != SOCKINFO_TCP {
        return None;
    }
    // SAFETY: `soi_kind == SOCKINFO_TCP` selects the `pri_tcp` member.
    let tcp = unsafe { info.psi.soi_proto.pri_tcp };
    if tcp.tcpsi_state != TSI_S_LISTEN {
        return None;
    }
    let local = tcp.tcpsi_ini;
    let ip = if local.insi_vflag & INI_IPV4 != 0 {
        let [.., a, b, c, d] = local.insi_laddr;
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    } else if local.insi_vflag & INI_IPV6 != 0 {
        IpAddr::V6(Ipv6Addr::from(local.insi_laddr))
    } else {
        return None;
    };
    Some(SocketAddr::new(ip, u16::from_be(local.insi_lport as u16)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_listeners_of_this_process_by_address() {
        let v4 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let v6 = std::net::TcpListener::bind("[::1]:0").unwrap();
        let any = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let connected = std::net::TcpStream::connect(v4.local_addr().unwrap()).unwrap();
        let found = tcp_listeners(i64::from(std::process::id())).unwrap();
        for listener in [&v4, &v6, &any] {
            assert!(found.contains(&listener.local_addr().unwrap()), "{found:?}");
        }
        assert!(
            !found.contains(&connected.local_addr().unwrap()),
            "{found:?}"
        );
    }

    #[test]
    fn a_process_that_does_not_exist_has_no_listeners() {
        assert_eq!(tcp_listeners(i64::from(i32::MAX)).unwrap(), Vec::new());
        assert_eq!(tcp_listeners(i64::MAX).unwrap(), Vec::new());
    }
}
