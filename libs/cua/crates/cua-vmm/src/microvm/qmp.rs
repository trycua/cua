//! QEMU's machine protocol (QMP) on a microVM's Unix-domain socket, from
//! Windows: QEMU listens on `unix:<path>` (Windows 10 and later support
//! `AF_UNIX`), in the VM's own run directory under the cua home, so only this
//! user can open it. One short connection per command: a greeting,
//! `qmp_capabilities`, the command, its answer.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::error::{Result, VmmError};

/// An `AF_UNIX` stream connection to `path`, as a tokio stream (std has none
/// for Windows yet: the socket is made with Winsock and wrapped as a TCP
/// stream, which only ever sends and receives).
#[cfg(windows)]
fn connect_unix(path: &Path) -> std::io::Result<tokio::net::TcpStream> {
    use std::os::windows::io::{FromRawSocket, RawSocket};
    use windows_sys::Win32::Networking::WinSock::{
        AF_UNIX, INVALID_SOCKET, SOCK_STREAM, SOCKADDR, SOCKADDR_UN, WSADATA, WSAStartup,
        closesocket, connect, socket,
    };
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    unsafe {
        let mut data: WSADATA = std::mem::zeroed();
        WSAStartup(0x0202, &mut data);
        let mut addr: SOCKADDR_UN = std::mem::zeroed();
        addr.sun_family = AF_UNIX;
        if bytes.len() >= addr.sun_path.len() {
            return Err(std::io::Error::other(format!(
                "the socket path {} is longer than a Unix socket path may be",
                path.display()
            )));
        }
        for (i, b) in bytes.iter().enumerate() {
            addr.sun_path[i] = *b as i8;
        }
        let s = socket(AF_UNIX as i32, SOCK_STREAM, 0);
        if s == INVALID_SOCKET {
            return Err(std::io::Error::last_os_error());
        }
        if connect(
            s,
            (&addr as *const SOCKADDR_UN).cast::<SOCKADDR>(),
            std::mem::size_of::<SOCKADDR_UN>() as i32,
        ) != 0
        {
            let e = std::io::Error::last_os_error();
            closesocket(s);
            return Err(e);
        }
        let std_stream = std::net::TcpStream::from_raw_socket(s as RawSocket);
        std_stream.set_nonblocking(true)?;
        tokio::net::TcpStream::from_std(std_stream)
    }
}

/// One QMP command on the socket `sock`; the answer's `return` value.
pub async fn command(
    sock: &Path,
    execute: &str,
    arguments: Value,
    timeout: Duration,
) -> Result<Value> {
    #[cfg(windows)]
    {
        use serde_json::json;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let fut = async {
            let stream = connect_unix(sock)
                .map_err(|e| VmmError::other(format!("qmp {}: {e}", sock.display())))?;
            let mut r = BufReader::new(stream);
            let mut line = String::new();
            r.read_line(&mut line).await?; // the greeting
            let send = |c: Value| {
                let mut b = serde_json::to_vec(&c).expect("json");
                b.push(b'\n');
                b
            };
            r.get_mut()
                .write_all(&send(json!({"execute": "qmp_capabilities"})))
                .await?;
            loop {
                line.clear();
                if r.read_line(&mut line).await? == 0 {
                    return Err(VmmError::other("qmp: closed"));
                }
                if line.contains("\"return\"") {
                    break;
                }
            }
            r.get_mut()
                .write_all(&send(json!({"execute": execute, "arguments": arguments})))
                .await?;
            loop {
                line.clear();
                if r.read_line(&mut line).await? == 0 {
                    return Err(VmmError::other("qmp: closed before the answer"));
                }
                let v: Value = serde_json::from_str(line.trim())?;
                // Events are interleaved with answers.
                if let Some(e) = v.get("error") {
                    return Err(VmmError::other(format!(
                        "qemu {execute}: {}",
                        e.get("desc").and_then(Value::as_str).unwrap_or("failed")
                    )));
                }
                if let Some(ret) = v.get("return") {
                    return Ok(ret.clone());
                }
            }
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| VmmError::other(format!("qemu {execute}: no answer within {timeout:?}")))?
    }
    #[cfg(not(windows))]
    {
        let _ = (sock, execute, arguments, timeout);
        Err(VmmError::other(
            "QMP on a Unix socket from here is a Windows transport",
        ))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn a_too_long_socket_path_is_refused_not_truncated() {
        let long = std::path::PathBuf::from("C:\\".to_string() + &"a".repeat(200));
        let e = super::connect_unix(&long).unwrap_err();
        assert!(e.to_string().contains("longer"), "{e}");
    }
}
