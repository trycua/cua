//! Hotspot (reverse-SOCKS egress) tunnel protocol.
//!
//! cua-spacesd runs a SOCKS5 listener on guest loopback and relays each
//! outbound connection over one message channel (a WebSocket in practice) to
//! a *peer* that performs the real egress from its own network. This crate
//! holds the parts both sides share: the frame codec ([`frame`]) and the
//! peer ([`peer::serve_egress`], used by the SDK). The listener side (the
//! SOCKS server and its hub) stays in the driver's `cua-spacesd-socks`.
//!
//! One tunnel frame per binary message:
//! ```text
//! [op:u8][stream:u32 BE][payload…]
//!   OPEN(1)     proxy→peer  payload = [port:u16 BE][host:utf8]
//!   OPEN_OK(2)  peer→proxy  payload = —
//!   OPEN_ERR(3) peer→proxy  payload = reason:utf8
//!   DATA(4)     both        payload = bytes
//!   CLOSE(5)    both        payload = —
//! ```

pub mod frame;
pub use frame::Frame;

/// Frames buffered per direction before backpressure.
pub const CHANNEL_DEPTH: usize = 64;
/// Read size for relayed TCP streams.
pub const READ_CHUNK: usize = 32 * 1024;

/// The egress peer: the side that dials targets from its own network.
pub mod peer {
    use super::{CHANNEL_DEPTH, Frame, READ_CHUNK};
    use futures_util::{Sink, SinkExt, Stream, StreamExt};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::sync::mpsc;

    /// Serves the peer end of a hotspot channel: dials every `OPEN` with
    /// `dial` and relays bytes, until `source` ends.
    pub async fn serve_egress<S, K, D, F>(mut source: S, mut sink: K, dial: D)
    where
        S: Stream<Item = Vec<u8>> + Unpin + Send,
        K: Sink<Vec<u8>> + Unpin + Send + 'static,
        D: Fn(String, u16) -> F + Send + Sync + 'static,
        F: std::future::Future<Output = std::io::Result<TcpStream>> + Send + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::channel::<Frame>(CHANNEL_DEPTH);
        let writer = tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if sink.send(frame.encode()).await.is_err() {
                    break;
                }
            }
        });
        let dial = Arc::new(dial);
        let mut streams: HashMap<u32, mpsc::Sender<Vec<u8>>> = HashMap::new();
        while let Some(message) = source.next().await {
            let Ok(frame) = Frame::decode(&message) else {
                continue;
            };
            match frame {
                Frame::Open { stream, host, port } => {
                    let out = out_tx.clone();
                    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(CHANNEL_DEPTH);
                    streams.insert(stream, tx);
                    let connect = dial(host, port);
                    tokio::spawn(async move {
                        let target = match connect.await {
                            Ok(target) => target,
                            Err(error) => {
                                let _ = out
                                    .send(Frame::OpenErr {
                                        stream,
                                        reason: error.to_string(),
                                    })
                                    .await;
                                return;
                            }
                        };
                        let _ = out.send(Frame::OpenOk { stream }).await;
                        let (mut r, mut w) = target.into_split();
                        let mut b = vec![0u8; READ_CHUNK];
                        loop {
                            tokio::select! {
                                n = r.read(&mut b) => match n {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => if out.send(Frame::Data { stream, bytes: b[..n].to_vec() }).await.is_err() { break },
                                },
                                m = rx.recv() => match m {
                                    Some(bytes) => if w.write_all(&bytes).await.is_err() { break },
                                    None => break,
                                },
                            }
                        }
                        let _ = out.send(Frame::Close { stream }).await;
                    });
                }
                Frame::Data { stream, bytes } => {
                    if let Some(tx) = streams.get(&stream) {
                        let _ = tx.send(bytes).await;
                    }
                }
                Frame::Close { stream } => {
                    streams.remove(&stream);
                }
                _ => {}
            }
        }
        writer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    /// The next frame from the peer (bounded wait).
    async fn next(rx: &mut mpsc::Receiver<Vec<u8>>) -> Frame {
        let m = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("frame within 5 s")
            .expect("peer open");
        Frame::decode(&m).unwrap()
    }

    #[tokio::test]
    async fn peer_dials_relays_and_reports_failures() {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = echo.accept().await.unwrap();
            let mut b = [0u8; 64];
            let n = s.read(&mut b).await.unwrap();
            s.write_all(&b[..n]).await.unwrap();
        });

        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(16);
        let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(16);
        let source = futures_util::stream::unfold(in_rx, |mut rx| async move {
            rx.recv().await.map(|m| (m, rx))
        });
        let sink = futures_util::sink::unfold(out_tx, |tx, m: Vec<u8>| async move {
            tx.send(m).await.map_err(|_| ())?;
            Ok::<_, ()>(tx)
        });
        tokio::spawn(peer::serve_egress(
            Box::pin(source),
            Box::pin(sink),
            |host: String, port| async move {
                if host == "refused.invalid" {
                    return Err(std::io::Error::other("refused"));
                }
                TcpStream::connect((host.as_str(), port)).await
            },
        ));
        let send = |f: Frame| {
            let tx = in_tx.clone();
            async move { tx.send(f.encode()).await.unwrap() }
        };
        send(Frame::Open {
            stream: 9,
            host: "refused.invalid".into(),
            port: 1,
        })
        .await;
        assert!(matches!(
            next(&mut out_rx).await,
            Frame::OpenErr { stream: 9, .. }
        ));

        send(Frame::Open {
            stream: 1,
            host: "127.0.0.1".into(),
            port: echo_addr.port(),
        })
        .await;
        assert_eq!(next(&mut out_rx).await, Frame::OpenOk { stream: 1 });
        send(Frame::Data {
            stream: 1,
            bytes: b"ping".to_vec(),
        })
        .await;
        assert_eq!(
            next(&mut out_rx).await,
            Frame::Data {
                stream: 1,
                bytes: b"ping".to_vec()
            }
        );
        assert_eq!(next(&mut out_rx).await, Frame::Close { stream: 1 });
    }
}
