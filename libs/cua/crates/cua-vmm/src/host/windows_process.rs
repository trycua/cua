//! Windows process identity and QMP listener ownership. No privilege escalation.

use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, FILETIME, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::AF_INET;
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};

use crate::error::{Result, VmmError};
use crate::host::ProcessState;

/// Recorded at spawn, before any QMP action. A PID alone is not an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub creation_time: u64,
    pub executable: PathBuf,
}

/// Holds the original kernel process object across inspection and exit polling.
pub struct Process {
    pid: u32,
    handle: OwnedHandle,
}

fn open_error(pid: u32, source: std::io::Error) -> Result<Option<Process>> {
    // Windows returns INVALID_PARAMETER for a PID which no longer exists. PID 0
    // is rejected by open(), so it cannot be confused with that documented case.
    if source.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
        Ok(None)
    } else {
        Err(VmmError::ProcessCheck { pid, source })
    }
}

impl Process {
    /// Clone the kernel object returned by spawn, never reopen its numeric PID.
    pub(crate) fn from_child(child: &std::process::Child) -> Result<Self> {
        // SAFETY: the child owns this live process handle for the whole borrow.
        // Cloning duplicates that same object and gives it independent ownership.
        let borrowed =
            unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(child.as_raw_handle()) };
        let handle = borrowed
            .try_clone_to_owned()
            .map_err(|source| VmmError::ProcessCheck {
                pid: child.id(),
                source,
            })?;
        Ok(Self {
            pid: child.id(),
            handle,
        })
    }

    pub fn open(pid: u32) -> Result<Option<Self>> {
        if pid == 0 {
            return Err(VmmError::invalid("process ID must be nonzero"));
        }
        // SAFETY: OpenProcess takes a numeric PID and returns an owned handle or
        // null. Only query and synchronization rights are requested.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return open_error(pid, std::io::Error::last_os_error());
        }
        // SAFETY: the successful OpenProcess handle is owned exactly once; the
        // OwnedHandle destructor closes it on every return/error path.
        Ok(Some(Self {
            pid,
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        }))
    }

    pub fn state(&self) -> Result<ProcessState> {
        // SAFETY: OwnedHandle keeps this valid process handle open. Zero timeout
        // is nonblocking and a signaled process object establishes actual exit.
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(ProcessState::Exited),
            WAIT_TIMEOUT => Ok(ProcessState::Running),
            WAIT_FAILED => Err(VmmError::ProcessCheck {
                pid: self.pid,
                source: std::io::Error::last_os_error(),
            }),
            value => Err(VmmError::ProcessCheck {
                pid: self.pid,
                source: std::io::Error::other(format!("unexpected process wait result {value}")),
            }),
        }
    }

    pub fn identity(&self) -> Result<Identity> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the handle is valid and all four writable FILETIMEs are live.
        if unsafe {
            GetProcessTimes(
                self.handle.as_raw_handle(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(VmmError::ProcessCheck {
                pid: self.pid,
                source: std::io::Error::last_os_error(),
            });
        }
        let mut path = vec![0u16; 32_768];
        let mut len = path.len() as u32;
        // SAFETY: the API receives a valid handle, a writable UTF-16 buffer, and
        // its capacity; flags 0 request a Win32 executable path.
        if unsafe {
            QueryFullProcessImageNameW(self.handle.as_raw_handle(), 0, path.as_mut_ptr(), &mut len)
        } == 0
        {
            return Err(VmmError::ProcessCheck {
                pid: self.pid,
                source: std::io::Error::last_os_error(),
            });
        }
        path.truncate(len as usize);
        let executable = std::fs::canonicalize(PathBuf::from(std::ffi::OsString::from_wide(&path)))
            .map_err(|source| VmmError::ProcessCheck {
                pid: self.pid,
                source,
            })?;
        Ok(Identity {
            creation_time: (u64::from(creation.dwHighDateTime) << 32)
                | u64::from(creation.dwLowDateTime),
            executable,
        })
    }

    pub fn verify(&self, expected: &Identity) -> Result<()> {
        let actual = self.identity()?;
        if actual.creation_time != expected.creation_time
            || !same_path(&actual.executable, &expected.executable)
        {
            return Err(VmmError::ProcessIdentity {
                pid: self.pid,
                detail: "creation time or executable does not match recorded QEMU".into(),
            });
        }
        Ok(())
    }
}

pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.to_str(), b.to_str()), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b))
}

/// The PID listening on this exact loopback IPv4 endpoint, not merely its port.
pub(crate) fn listener_pid(port: u16) -> Result<Option<u32>> {
    let mut bytes = 0u32;
    // SAFETY: null asks for the required buffer size; the size pointer is valid.
    let mut result = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut bytes,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    for _ in 0..3 {
        if result == 0 && bytes == 0 {
            return Ok(None);
        }
        if result != ERROR_INSUFFICIENT_BUFFER {
            return Err(VmmError::Io(std::io::Error::from_raw_os_error(
                result as i32,
            )));
        }
        if bytes as usize > 16 * 1024 * 1024 || (bytes as usize) < std::mem::size_of::<u32>() {
            return Err(VmmError::Qmp("invalid TCP owner table size".into()));
        }
        // usize storage provides alignment for the Windows DWORD table/rows.
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: the aligned allocation has at least the requested byte count;
        // the API writes only within that capacity or reports a new size.
        result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if result != 0 {
            return Err(VmmError::Io(std::io::Error::from_raw_os_error(
                result as i32,
            )));
        }
        let offset = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table);
        // SAFETY: successful API output includes the initial DWORD. Bounds of
        // the variable row array are checked before any row is read.
        let count = unsafe { buffer.as_ptr().cast::<u32>().read() } as usize;
        let end = count
            .checked_mul(std::mem::size_of::<MIB_TCPROW_OWNER_PID>())
            .and_then(|n| n.checked_add(offset))
            .ok_or_else(|| VmmError::Qmp("invalid TCP owner table length".into()))?;
        if end > bytes as usize || end > buffer.len() * std::mem::size_of::<usize>() {
            return Err(VmmError::Qmp("truncated TCP owner table".into()));
        }
        let mut owner = None;
        for index in 0..count {
            // SAFETY: the offset/row count were bounded above; read_unaligned
            // also handles the documented possible table padding safely.
            let row = unsafe {
                buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset + index * std::mem::size_of::<MIB_TCPROW_OWNER_PID>())
                    .cast::<MIB_TCPROW_OWNER_PID>()
                    .read_unaligned()
            };
            if row.dwLocalAddr.to_ne_bytes() == [127, 0, 0, 1]
                && u16::from_be(row.dwLocalPort as u16) == port
            {
                if owner.is_some_and(|pid| pid != row.dwOwningPid) {
                    return Err(VmmError::Qmp(
                        "ambiguous loopback QMP listener owner".into(),
                    ));
                }
                owner = Some(row.dwOwningPid);
            }
        }
        return Ok(owner);
    }
    Err(VmmError::Qmp(
        "TCP owner table changed during inspection".into(),
    ))
}

pub(crate) fn require_listener_owner(pid: u32, port: u16) -> Result<()> {
    match listener_pid(port)? {
        Some(owner) if owner == pid => Ok(()),
        Some(_) => Err(VmmError::ProcessIdentity {
            pid,
            detail: format!("loopback QMP listener {port} belongs to a different process"),
        }),
        None => Err(VmmError::Qmp(format!(
            "loopback QMP listener {port} is not ready"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Child, Command, Stdio};

    struct Fixture(Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the child this test spawned, never a PID/name lookup.
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn fixture() -> Fixture {
        Fixture(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "host::windows_process::tests::process_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    #[test]
    #[ignore = "spawned only by Windows process unit tests"]
    fn process_fixture() {
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes).unwrap();
    }

    #[test]
    fn detects_live_then_exited_same_process_object() {
        let mut child = fixture();
        let process = Process::from_child(&child.0).unwrap();
        assert_eq!(process.state().unwrap(), ProcessState::Running);
        let identity = process.identity().unwrap();
        process.verify(&identity).unwrap();
        drop(child.0.stdin.take());
        assert!(child.0.wait().unwrap().success());
        assert_eq!(process.state().unwrap(), ProcessState::Exited);
    }

    #[test]
    fn refuses_reused_or_mismatched_identity() {
        let child = fixture();
        let process = Process::open(child.0.id()).unwrap().unwrap();
        let mut identity = process.identity().unwrap();
        identity.creation_time ^= 1;
        assert!(matches!(
            process.verify(&identity),
            Err(VmmError::ProcessIdentity { .. })
        ));
        identity = process.identity().unwrap();
        identity.executable = PathBuf::from("C:\\not-this-process.exe");
        assert!(matches!(
            process.verify(&identity),
            Err(VmmError::ProcessIdentity { .. })
        ));
    }

    #[test]
    fn only_absent_pid_error_is_exit_and_permission_errors_remain_unknown() {
        assert!(
            open_error(
                123,
                std::io::Error::from_raw_os_error(ERROR_INVALID_PARAMETER as i32)
            )
            .unwrap()
            .is_none()
        );
        for code in [5, 6, 8] {
            match open_error(123, std::io::Error::from_raw_os_error(code)) {
                Err(VmmError::ProcessCheck { pid, source }) => {
                    assert_eq!(pid, 123);
                    assert_eq!(source.raw_os_error(), Some(code));
                }
                _ => panic!("inspection error was treated as process exit"),
            }
        }
        assert!(matches!(Process::open(0), Err(VmmError::Invalid(_))));
    }

    #[test]
    fn listener_ownership_is_exact() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        require_listener_owner(std::process::id(), port).unwrap();
        assert!(matches!(
            require_listener_owner(0, port),
            Err(VmmError::ProcessIdentity { .. })
        ));
    }
}
