//! Agentless screenshots over a VMM's VNC endpoint (Lume's
//! Virtualization.framework display, QEMU `-vnc`).
//!
//! The RFB client is the `vnc-rs` crate (MIT OR Apache-2.0); this module
//! only paints the rectangles of one full framebuffer update into a buffer
//! and encodes it as PNG.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use vnc::{PixelFormat, VncConnector, VncEncoding, VncError, VncEvent};

use crate::error::{Result, VmmError};
use crate::types::VncEndpoint;

/// One captured framebuffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framebuffer {
    /// PNG bytes (8-bit RGB).
    pub png: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Connects to `ep`, reads one full framebuffer and returns it as PNG.
/// Fails with [`VmmError::Timeout`] when no complete frame arrives within
/// `timeout`.
pub async fn capture_png(ep: &VncEndpoint, timeout: Duration) -> Result<Framebuffer> {
    let addr = format!("{}:{}", ep.host, ep.port);
    let tcp = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr))
        .await
        .map_err(|_| VmmError::Timeout {
            name: addr.clone(),
            secs: timeout.as_secs(),
            detail: "VNC connect".into(),
        })?
        .map_err(|e| VmmError::Other(format!("VNC connect to {addr}: {e}")))?;
    capture_png_from(tcp, ep.password.clone(), timeout)
        .await
        .map_err(|e| match e {
            VmmError::Timeout { secs, detail, .. } => VmmError::Timeout {
                name: addr.clone(),
                secs,
                detail,
            },
            other => other,
        })
}

/// [`capture_png`] over an already connected stream.
pub async fn capture_png_from<S>(
    stream: S,
    password: Option<String>,
    timeout: Duration,
) -> Result<Framebuffer>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    match tokio::time::timeout(timeout, capture(stream, password)).await {
        Ok(r) => r,
        Err(_) => Err(VmmError::Timeout {
            name: "vnc".into(),
            secs: timeout.as_secs(),
            detail: "no complete VNC frame arrived".into(),
        }),
    }
}

fn vnc_err(e: VncError) -> VmmError {
    VmmError::Other(format!("VNC: {e}"))
}

/// A framebuffer being painted, with a per-pixel coverage mask so the
/// capture ends once the first full update has painted every pixel.
struct Canvas {
    width: usize,
    height: usize,
    /// 32-bit pixels in the server's pixel format.
    pixels: Vec<u8>,
    covered: Vec<bool>,
    remaining: usize,
}

impl Canvas {
    fn new(width: u16, height: u16) -> Self {
        let (w, h) = (usize::from(width), usize::from(height));
        Self {
            width: w,
            height: h,
            pixels: vec![0; w * h * 4],
            covered: vec![false; w * h],
            remaining: w * h,
        }
    }

    fn done(&self) -> bool {
        self.width > 0 && self.height > 0 && self.remaining == 0
    }

    fn cover(&mut self, x: usize, y: usize) {
        let i = y * self.width + x;
        if !self.covered[i] {
            self.covered[i] = true;
            self.remaining -= 1;
        }
    }

    fn paint(&mut self, r: vnc::Rect, data: &[u8]) -> Result<()> {
        let (rx, ry, rw, rh) = (
            usize::from(r.x),
            usize::from(r.y),
            usize::from(r.width),
            usize::from(r.height),
        );
        if rx + rw > self.width || ry + rh > self.height || data.len() < rw * rh * 4 {
            return Err(VmmError::Other(format!(
                "VNC: rectangle {rw}x{rh}+{rx}+{ry} does not fit the {}x{} framebuffer",
                self.width, self.height
            )));
        }
        for row in 0..rh {
            let src = &data[row * rw * 4..(row + 1) * rw * 4];
            let dst = ((ry + row) * self.width + rx) * 4;
            self.pixels[dst..dst + rw * 4].copy_from_slice(src);
            for col in 0..rw {
                self.cover(rx + col, ry + row);
            }
        }
        Ok(())
    }

    fn copy(&mut self, dst: vnc::Rect, src: vnc::Rect) -> Result<()> {
        let (w, h) = (usize::from(src.width), usize::from(src.height));
        let mut tmp = Vec::with_capacity(w * h * 4);
        for row in 0..h {
            let (sx, sy) = (usize::from(src.x), usize::from(src.y) + row);
            if sx + w > self.width || sy >= self.height {
                return Err(VmmError::Other("VNC: CopyRect source out of bounds".into()));
            }
            let s = (sy * self.width + sx) * 4;
            tmp.extend_from_slice(&self.pixels[s..s + w * 4]);
        }
        self.paint(
            vnc::Rect {
                x: dst.x,
                y: dst.y,
                width: src.width,
                height: src.height,
            },
            &tmp,
        )
    }

    fn png(&self, pf: &PixelFormat) -> Result<Framebuffer> {
        let channel = |v: u32, shift: u8, max: u16| -> u8 {
            let max = u32::from(max.max(1));
            (((v >> shift) & max) * 255 / max) as u8
        };
        let mut rgb = Vec::with_capacity(self.width * self.height * 3);
        for &b in self.pixels.as_chunks::<4>().0 {
            let v = if pf.big_endian_flag != 0 {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            };
            rgb.extend_from_slice(&[
                channel(v, pf.red_shift, pf.red_max),
                channel(v, pf.green_shift, pf.green_max),
                channel(v, pf.blue_shift, pf.blue_max),
            ]);
        }
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, self.width as u32, self.height as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc
                .write_header()
                .map_err(|e| VmmError::Other(format!("PNG: {e}")))?;
            w.write_image_data(&rgb)
                .map_err(|e| VmmError::Other(format!("PNG: {e}")))?;
        }
        Ok(Framebuffer {
            png,
            width: self.width as u32,
            height: self.height as u32,
        })
    }
}

async fn capture<S>(stream: S, password: Option<String>) -> Result<Framebuffer>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    let client = VncConnector::new(stream)
        .set_auth_method(async move { password.ok_or(VncError::NoPassword) })
        // Raw (lossless, supported by every server) plus the DesktopSize
        // and LastRect pseudo-encodings, in the server's own pixel format:
        // Virtualization.framework's VNC server (Lume) traps, taking down
        // the host process that runs the VM, after a client's
        // SetPixelFormat (seen with a macOS 26 host).
        .add_encoding(VncEncoding::Raw)
        .add_encoding(VncEncoding::DesktopSizePseudo)
        .add_encoding(VncEncoding::LastRectPseudo)
        .allow_shared(true)
        .build()
        .map_err(vnc_err)?
        .try_start()
        .await
        .map_err(vnc_err)?
        .finish()
        .map_err(vnc_err)?;
    // The client asks for a full (non-incremental) frame on connect.
    let mut canvas: Option<Canvas> = None;
    let mut format: Option<PixelFormat> = None;
    let result = loop {
        let event = match client.recv_event().await {
            Ok(e) => e,
            Err(e) => break Err(vnc_err(e)),
        };
        let step = match event {
            VncEvent::SetResolution(s) => {
                canvas = Some(Canvas::new(s.width, s.height));
                Ok(())
            }
            VncEvent::SetPixelFormat(pf) => {
                if pf.bits_per_pixel != 32 || pf.true_color_flag == 0 {
                    Err(VmmError::Other(format!(
                        "VNC: unsupported server pixel format ({} bpp, true colour {})",
                        pf.bits_per_pixel, pf.true_color_flag
                    )))
                } else {
                    format = Some(pf);
                    Ok(())
                }
            }
            VncEvent::RawImage(r, data) => match canvas.as_mut() {
                Some(c) => c.paint(r, &data),
                None => Err(VmmError::Other("VNC: image before the resolution".into())),
            },
            VncEvent::Copy(dst, src) => match canvas.as_mut() {
                Some(c) => c.copy(dst, src),
                None => Err(VmmError::Other("VNC: copy before the resolution".into())),
            },
            VncEvent::Error(e) => Err(VmmError::Other(format!("VNC: {e}"))),
            _ => Ok(()),
        };
        if let Err(e) = step {
            break Err(e);
        }
        if let Some(c) = canvas.as_ref().filter(|c| c.done()) {
            break match &format {
                Some(pf) => c.png(pf),
                None => Err(VmmError::Other("VNC: no pixel format".into())),
            };
        }
    };
    let _ = client.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A fake RFB 3.8 server: `auth` picks VNC authentication (any response
    /// accepted) over None; the frame arrives as two Raw rectangles in the
    /// server's pixel format (`rgba`: red in the low byte, else `bgra`).
    async fn fake_server(mut s: tokio::io::DuplexStream, w: u16, h: u16, auth: bool, rgba: bool) {
        s.write_all(b"RFB 003.008\n").await.unwrap();
        let mut ver = [0u8; 12];
        s.read_exact(&mut ver).await.unwrap();
        s.write_all(&[1, if auth { 2 } else { 1 }]).await.unwrap();
        let chosen = s.read_u8().await.unwrap();
        assert_eq!(chosen, if auth { 2 } else { 1 });
        if auth {
            s.write_all(&[7u8; 16]).await.unwrap();
            let mut resp = [0u8; 16];
            s.read_exact(&mut resp).await.unwrap();
        }
        s.write_u32(0).await.unwrap(); // SecurityResult OK
        let _shared = s.read_u8().await.unwrap();
        s.write_u16(w).await.unwrap();
        s.write_u16(h).await.unwrap();
        let format = if rgba {
            PixelFormat::rgba()
        } else {
            PixelFormat::bgra()
        };
        let pf: Vec<u8> = format.into();
        s.write_all(&pf).await.unwrap();
        s.write_u32(4).await.unwrap();
        s.write_all(b"fake").await.unwrap();
        // SetEncodings (4 + 4n), then the FramebufferUpdateRequest (10);
        // never a SetPixelFormat (message type 0).
        let mut head = [0u8; 4];
        s.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 2, "no SetPixelFormat before SetEncodings");
        let n = u16::from_be_bytes([head[2], head[3]]) as usize;
        let mut encs = vec![0u8; 4 * n];
        s.read_exact(&mut encs).await.unwrap();
        // Raw plus the DesktopSize and LastRect pseudo-encodings, the set
        // known to work against Lume's VNC server.
        assert_eq!(
            encs,
            [0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21, 0xff, 0xff, 0xff, 0x20],
            "SetEncodings"
        );
        let mut req = [0u8; 10];
        s.read_exact(&mut req).await.unwrap();
        assert_eq!(req[0], 3);
        // FramebufferUpdate: top half red, bottom half blue.
        let (red, blue) = if rgba {
            ([255, 0, 0, 0], [0, 0, 255, 0])
        } else {
            ([0, 0, 255, 0], [255, 0, 0, 0])
        };
        let top = h / 2;
        s.write_all(&[0, 0]).await.unwrap();
        s.write_u16(2).await.unwrap();
        for (y, rh, px) in [(0, top, red), (top, h - top, blue)] {
            s.write_u16(0).await.unwrap();
            s.write_u16(y).await.unwrap();
            s.write_u16(w).await.unwrap();
            s.write_u16(rh).await.unwrap();
            s.write_i32(0).await.unwrap(); // Raw
            let px: Vec<u8> = (0..usize::from(w) * usize::from(rh))
                .flat_map(|_| px)
                .collect();
            s.write_all(&px).await.unwrap();
        }
        s.flush().await.unwrap();
        // Keep the connection open until the client hangs up.
        let mut sink = [0u8; 256];
        for _ in 0..64 {
            match s.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    #[tokio::test]
    async fn captures_a_full_frame_as_png() {
        for (auth, rgba) in [(false, false), (true, false), (false, true)] {
            let (client, server) = tokio::io::duplex(1 << 16);
            let srv = tokio::spawn(fake_server(server, 8, 6, auth, rgba));
            let pw = auth.then(|| "secret".to_string());
            let fb = capture_png_from(client, pw, Duration::from_secs(5))
                .await
                .unwrap();
            assert_eq!((fb.width, fb.height), (8, 6));
            let (w, h, rgb) = decode(&fb.png);
            assert_eq!((w, h), (8, 6));
            assert_eq!(&rgb[..3], &[255, 0, 0], "top-left is red");
            let last = rgb.len() - 3;
            assert_eq!(&rgb[last..], &[0, 0, 255], "bottom-right is blue");
            srv.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_password_protected_server_without_a_password_fails() {
        let (client, server) = tokio::io::duplex(1 << 16);
        let srv = tokio::spawn(async move {
            let mut s = server;
            s.write_all(b"RFB 003.008\n").await.unwrap();
            let mut ver = [0u8; 12];
            let _ = s.read_exact(&mut ver).await;
            let _ = s.write_all(&[1, 2]).await;
            let mut sink = [0u8; 64];
            let _ = s.read(&mut sink).await;
        });
        let err = capture_png_from(client, None, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("password"), "{err}");
        srv.abort();
    }

    #[tokio::test]
    async fn a_silent_server_times_out() {
        let (client, _server) = tokio::io::duplex(1 << 10);
        let err = capture_png_from(client, None, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(matches!(err, VmmError::Timeout { .. }), "{err}");
    }
}
