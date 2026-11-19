// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A WebSocket as a byte stream (`futures::io::AsyncRead + AsyncWrite`) for
//! yamux. Message boundaries carry no meaning: every write becomes one
//! binary message and reads concatenate binary messages.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::{Sink, Stream};

/// A WebSocket message type the adapter can carry.
pub trait WsMessage: Sized {
    /// A binary message.
    fn binary(data: Vec<u8>) -> Self;
    /// What a received message means for the byte stream.
    fn classify(self) -> Frame;
}

/// Classification of a received message.
pub enum Frame {
    /// Payload bytes.
    Data(Bytes),
    /// End of stream.
    Close,
    /// Control or text message; ignored.
    Skip,
}

impl WsMessage for tokio_tungstenite::tungstenite::Message {
    fn binary(data: Vec<u8>) -> Self {
        Self::Binary(data)
    }

    fn classify(self) -> Frame {
        match self {
            Self::Binary(b) => Frame::Data(Bytes::from(b)),
            Self::Close(_) => Frame::Close,
            _ => Frame::Skip,
        }
    }
}

impl WsMessage for axum::extract::ws::Message {
    fn binary(data: Vec<u8>) -> Self {
        Self::Binary(data.into())
    }

    fn classify(self) -> Frame {
        match self {
            Self::Binary(b) => Frame::Data(b),
            Self::Close(_) => Frame::Close,
            _ => Frame::Skip,
        }
    }
}

/// The adapter. `M` is the WebSocket library's message type.
pub struct WsIo<S, M> {
    inner: S,
    buffer: Bytes,
    eof: bool,
    _message: std::marker::PhantomData<fn() -> M>,
}

impl<S, M> WsIo<S, M> {
    /// Wraps a WebSocket.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buffer: Bytes::new(),
            eof: false,
            _message: std::marker::PhantomData,
        }
    }
}

fn other<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, e.to_string())
}

impl<S, M, E> futures_util::io::AsyncRead for WsIo<S, M>
where
    S: Stream<Item = Result<M, E>> + Unpin,
    M: WsMessage,
    E: std::fmt::Display,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if !self.buffer.is_empty() {
                let n = self.buffer.len().min(out.len());
                out[..n].copy_from_slice(&self.buffer[..n]);
                let rest = self.buffer.split_off(n);
                self.buffer = rest;
                return Poll::Ready(Ok(n));
            }
            if self.eof {
                return Poll::Ready(Ok(0));
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    self.eof = true;
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(other(e))),
                Poll::Ready(Some(Ok(message))) => match message.classify() {
                    Frame::Data(data) => self.buffer = data,
                    Frame::Close => self.eof = true,
                    Frame::Skip => {}
                },
            }
        }
    }
}

impl<S, M> futures_util::io::AsyncWrite for WsIo<S, M>
where
    S: Sink<M> + Unpin,
    S::Error: std::fmt::Display,
    M: WsMessage,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_ready(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(other(e))),
            Poll::Ready(Ok(())) => {}
        }
        Pin::new(&mut self.inner)
            .start_send(M::binary(data.to_vec()))
            .map_err(other)?;
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx).map_err(other)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx).map_err(other)
    }
}
