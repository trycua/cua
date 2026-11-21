// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A small RFB (VNC) client: screenshots, pointer and key events.
//!
//! It exists for one job: approving Cua Driver's macOS permission prompts
//! through the Mac's own Screen Sharing server while Cua Driver itself has no
//! Accessibility or Screen Recording grant. Input that arrives over Screen
//! Sharing does not depend on Cua Driver's TCC grants.
//!
//! Security types: None (1), VNC password (2), and Apple Remote Desktop
//! Diffie-Hellman (30), which macOS Screen Sharing offers for a macOS account.
//! Only Raw encoding is requested, with the DesktopSize pseudo-encoding:
//! Apple's Virtualization framework VNC server (Lume VMs) stops the VM when
//! a client leaves DesktopSize out.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use aes::cipher::{BlockEncrypt, KeyInit};
use image::ImageEncoder;
use num_bigint::BigUint;
use zeroize::Zeroizing;

pub const DEFAULT_PORT: u16 = 5900;
const ENCODING_RAW: i32 = 0;
const ENCODING_DESKTOP_SIZE: i32 = -223;
const KEYSYM_RETURN: u32 = 0xff0d;
const KEYSYM_TAB: u32 = 0xff09;

/// Credentials for the server. Values are zeroized on drop and never printed.
#[derive(Default)]
pub struct Credentials {
    pub username: Option<Zeroizing<String>>,
    pub password: Option<Zeroizing<String>>,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username.as_ref().map(|_| "<set>"))
            .field("password", &self.password.as_ref().map(|_| "<set>"))
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RfbError {
    Io(String),
    Protocol(String),
    /// The server offered no security type this client can use with the
    /// credentials it has.
    UnsupportedSecurity {
        offered: Vec<u8>,
        hint: &'static str,
    },
    /// The server refused the connection or the credentials, with its reason.
    Refused(String),
}

impl fmt::Display for RfbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "{message}"),
            Self::Protocol(message) => write!(f, "VNC protocol error: {message}"),
            Self::UnsupportedSecurity { offered, hint } => {
                write!(
                    f,
                    "the VNC server offers security types {offered:?}; {hint}"
                )
            }
            Self::Refused(reason) => write!(f, "the VNC server refused the connection: {reason}"),
        }
    }
}

impl std::error::Error for RfbError {}

impl From<std::io::Error> for RfbError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(format!("VNC connection failed: {error}"))
    }
}

pub type Result<T> = std::result::Result<T, RfbError>;

/// Parse `host`, `host:port`, `host::port` (vncdotool's spelling) or
/// `[v6addr]:port`. The port defaults to 5900.
pub fn parse_server(server: &str) -> Result<(String, u16)> {
    let server = server.trim();
    let bad = || {
        RfbError::Protocol(format!(
            "cannot read VNC server '{server}': use host or host:port"
        ))
    };
    if server.is_empty() {
        return Err(bad());
    }
    let port = |text: &str| text.parse::<u16>().map_err(|_| bad());
    if let Some(rest) = server.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or_else(bad)?;
        return match tail.strip_prefix(':') {
            None if tail.is_empty() => Ok((host.to_owned(), DEFAULT_PORT)),
            Some(text) => Ok((host.to_owned(), port(text.trim_start_matches(':'))?)),
            None => Err(bad()),
        };
    }
    if let Some((host, text)) = server
        .split_once("::")
        .filter(|(host, _)| !host.is_empty() && !host.contains(':'))
    {
        return Ok((host.to_owned(), port(text)?));
    }
    match server.rsplit_once(':') {
        Some((host, _)) if host.contains(':') => Ok((server.to_owned(), DEFAULT_PORT)),
        Some((host, text)) => Ok((host.to_owned(), port(text)?)),
        None => Ok((server.to_owned(), DEFAULT_PORT)),
    }
}

/// A screenshot in RGBA order.
pub struct Frame {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(
                &self.rgba,
                u32::from(self.width),
                u32::from(self.height),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|error| RfbError::Io(format!("cannot encode the screenshot: {error}")))?;
        Ok(out)
    }
}

pub struct RfbClient<S> {
    stream: S,
    width: u16,
    height: u16,
    timeout: Duration,
}

impl RfbClient<TcpStream> {
    pub fn connect(server: &str, credentials: &Credentials, timeout: Duration) -> Result<Self> {
        let (host, port) = parse_server(server)?;
        let address = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| RfbError::Io(format!("cannot resolve VNC server '{host}'")))?;
        let stream = TcpStream::connect_timeout(&address, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        Self::handshake(stream, credentials, timeout)
    }
}

impl<S: Read + Write> RfbClient<S> {
    /// Run the RFB handshake on an open stream, through ServerInit.
    pub fn handshake(mut stream: S, credentials: &Credentials, timeout: Duration) -> Result<Self> {
        let mut version = [0u8; 12];
        stream.read_exact(&mut version)?;
        let minor = server_version(&version)?;
        let client_minor = if minor >= 8 {
            8
        } else if minor >= 7 {
            7
        } else {
            3
        };
        stream.write_all(format!("RFB 003.{client_minor:03}\n").as_bytes())?;

        let security =
            if client_minor >= 7 {
                let count = read_u8(&mut stream)?;
                if count == 0 {
                    return Err(RfbError::Refused(read_reason(&mut stream)?));
                }
                let mut offered = vec![0u8; usize::from(count)];
                stream.read_exact(&mut offered)?;
                let chosen = choose_security(&offered, credentials)?;
                stream.write_all(&[chosen])?;
                chosen
            } else {
                match read_u32(&mut stream)? {
                    0 => return Err(RfbError::Refused(read_reason(&mut stream)?)),
                    kind @ (1 | 2) => {
                        let kind = kind as u8;
                        choose_security(&[kind], credentials)?;
                        kind
                    }
                    other => return Err(RfbError::UnsupportedSecurity {
                        offered: vec![other.min(255) as u8],
                        hint: "this client speaks None, VNC password and Apple Remote Desktop (30)",
                    }),
                }
            };

        match security {
            2 => {
                let mut challenge = [0u8; 16];
                stream.read_exact(&mut challenge)?;
                let password = credentials.password.as_deref().map_or("", String::as_str);
                stream.write_all(&vnc_password_response(password, &challenge))?;
            }
            30 => {
                let generator = read_u16(&mut stream)?;
                let key_len = usize::from(read_u16(&mut stream)?);
                if key_len == 0 || key_len > 1024 {
                    return Err(RfbError::Protocol(format!(
                        "Diffie-Hellman key length {key_len}"
                    )));
                }
                let mut prime = vec![0u8; key_len];
                stream.read_exact(&mut prime)?;
                let mut server_key = vec![0u8; key_len];
                stream.read_exact(&mut server_key)?;
                let mut private = Zeroizing::new(vec![0u8; key_len]);
                ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut private)
                    .map_err(|_| {
                        RfbError::Io("no system randomness for the key exchange".into())
                    })?;
                let response = ard_response(
                    generator,
                    &prime,
                    &server_key,
                    &BigUint::from_bytes_be(&private),
                    credentials.username.as_deref().map_or("", String::as_str),
                    credentials.password.as_deref().map_or("", String::as_str),
                )?;
                stream.write_all(&response)?;
            }
            _ => {}
        }

        // RFB 3.8 sends a result for every type; earlier versions skip it for None.
        if security != 1 || client_minor >= 8 {
            match read_u32(&mut stream)? {
                0 => {}
                _ if client_minor >= 8 => return Err(RfbError::Refused(read_reason(&mut stream)?)),
                _ => return Err(RfbError::Refused("authentication failed".into())),
            }
        }

        // ClientInit: shared, so a person already watching stays connected.
        stream.write_all(&[1])?;
        let width = read_u16(&mut stream)?;
        let height = read_u16(&mut stream)?;
        let mut ignored = [0u8; 16];
        stream.read_exact(&mut ignored)?;
        let name_len = read_u32(&mut stream)?;
        skip(&mut stream, u64::from(name_len))?;

        // 32 bits per pixel, little-endian, red at bit 16, green 8, blue 0.
        let mut set_pixel_format = vec![0u8, 0, 0, 0];
        set_pixel_format
            .extend_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        stream.write_all(&set_pixel_format)?;
        // SetEncodings: Raw, DesktopSize.
        let mut set_encodings = vec![2u8, 0, 0, 2];
        for encoding in [ENCODING_RAW, ENCODING_DESKTOP_SIZE] {
            set_encodings.extend_from_slice(&encoding.to_be_bytes());
        }
        stream.write_all(&set_encodings)?;
        Ok(Self {
            stream,
            width,
            height,
            timeout,
        })
    }

    #[cfg(test)]
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Request the whole screen and wait until every pixel has arrived. A
    /// resize starts the screenshot over at the new size.
    pub fn screenshot(&mut self) -> Result<Frame> {
        let deadline = Instant::now() + self.timeout;
        let mut canvas = Canvas::new(self.width, self.height);
        self.request_full_update()?;
        while canvas.remaining > 0 {
            if Instant::now() > deadline {
                return Err(RfbError::Io("timed out waiting for the screenshot".into()));
            }
            match read_u8(&mut self.stream)? {
                0 => {
                    skip(&mut self.stream, 1)?;
                    for _ in 0..read_u16(&mut self.stream)? {
                        let x = usize::from(read_u16(&mut self.stream)?);
                        let y = usize::from(read_u16(&mut self.stream)?);
                        let w = read_u16(&mut self.stream)?;
                        let h = read_u16(&mut self.stream)?;
                        match read_u32(&mut self.stream)? as i32 {
                            ENCODING_RAW => canvas.read_raw(
                                &mut self.stream,
                                x,
                                y,
                                usize::from(w),
                                usize::from(h),
                            )?,
                            ENCODING_DESKTOP_SIZE => {
                                (self.width, self.height) = (w, h);
                                canvas = Canvas::new(w, h);
                                self.request_full_update()?;
                            }
                            other => {
                                return Err(RfbError::Protocol(format!(
                                    "unrequested encoding {other}"
                                )))
                            }
                        }
                    }
                }
                1 => {
                    skip(&mut self.stream, 3)?;
                    let colours = read_u16(&mut self.stream)?;
                    skip(&mut self.stream, u64::from(colours) * 6)?;
                }
                2 => {}
                3 => {
                    skip(&mut self.stream, 3)?;
                    let len = read_u32(&mut self.stream)?;
                    skip(&mut self.stream, u64::from(len))?;
                }
                other => {
                    return Err(RfbError::Protocol(format!(
                        "unknown server message {other}"
                    )))
                }
            }
        }
        Ok(Frame {
            width: self.width,
            height: self.height,
            rgba: canvas.rgba,
        })
    }

    fn request_full_update(&mut self) -> Result<()> {
        let mut request = vec![3u8, 0];
        for value in [0, 0, self.width, self.height] {
            request.extend_from_slice(&value.to_be_bytes());
        }
        self.stream.write_all(&request)?;
        Ok(())
    }

    /// Move to (x, y) in framebuffer pixels, then press and release the left button.
    pub fn click(&mut self, x: u16, y: u16) -> Result<()> {
        if x >= self.width || y >= self.height {
            return Err(RfbError::Protocol(format!(
                "({x}, {y}) is outside the {}x{} screen",
                self.width, self.height
            )));
        }
        self.pointer(x, y, 0)?;
        std::thread::sleep(Duration::from_millis(100));
        self.pointer(x, y, 1)?;
        self.pointer(x, y, 0)
    }

    /// Type `text` key by key, then press Return.
    pub fn type_line(&mut self, text: &str) -> Result<()> {
        let keysyms = text.chars().map(keysym).collect::<Result<Vec<_>>>()?;
        for keysym in keysyms.into_iter().chain([KEYSYM_RETURN]) {
            self.key(keysym, true)?;
            self.key(keysym, false)?;
        }
        self.stream.flush()?;
        Ok(())
    }

    fn pointer(&mut self, x: u16, y: u16, buttons: u8) -> Result<()> {
        let mut message = vec![5u8, buttons];
        message.extend_from_slice(&x.to_be_bytes());
        message.extend_from_slice(&y.to_be_bytes());
        self.stream.write_all(&message)?;
        Ok(())
    }

    fn key(&mut self, keysym: u32, down: bool) -> Result<()> {
        let mut message = vec![4u8, u8::from(down), 0, 0];
        message.extend_from_slice(&keysym.to_be_bytes());
        self.stream.write_all(&message)?;
        Ok(())
    }
}

/// A framebuffer being filled, and how many of its pixels are still missing.
struct Canvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    covered: Vec<bool>,
    remaining: usize,
}

impl Canvas {
    fn new(width: u16, height: u16) -> Self {
        let (width, height) = (usize::from(width), usize::from(height));
        Self {
            width,
            height,
            rgba: vec![0; width * height * 4],
            covered: vec![false; width * height],
            remaining: width * height,
        }
    }

    /// Copy a Raw rectangle of B, G, R, X pixels into the canvas.
    fn read_raw(
        &mut self,
        stream: &mut impl Read,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) -> Result<()> {
        if x + w > self.width || y + h > self.height {
            return Err(RfbError::Protocol("rectangle outside the screen".into()));
        }
        let mut row = vec![0u8; w * 4];
        for line in y..y + h {
            stream.read_exact(&mut row)?;
            for (column, pixel) in row.chunks_exact(4).enumerate() {
                let index = line * self.width + x + column;
                self.rgba[index * 4..index * 4 + 4]
                    .copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
                if !self.covered[index] {
                    self.covered[index] = true;
                    self.remaining -= 1;
                }
            }
        }
        Ok(())
    }
}

fn keysym(c: char) -> Result<u32> {
    match c {
        '\t' => Ok(KEYSYM_TAB),
        ' '..='~' | '\u{a0}'..='\u{ff}' => Ok(c as u32),
        c if !c.is_control() => Ok(0x0100_0000 | c as u32),
        _ => Err(RfbError::Protocol(
            "the text has a control character".into(),
        )),
    }
}

fn server_version(version: &[u8; 12]) -> Result<u32> {
    let text = std::str::from_utf8(version).unwrap_or("");
    let parsed = text
        .strip_prefix("RFB ")
        .and_then(|rest| rest.strip_suffix('\n'))
        .and_then(|rest| rest.split_once('.'))
        .and_then(|(major, minor)| Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?)));
    match parsed {
        // RealVNC 4/5 and Intel AMT speak 3.8 to a 3.8 client.
        Some((major, _)) if major > 3 => Ok(8),
        Some((3, minor)) if minor >= 3 => Ok(minor),
        _ => Err(RfbError::Protocol(format!(
            "not a VNC server (it sent {:?})",
            text.trim_end()
        ))),
    }
}

fn choose_security(offered: &[u8], credentials: &Credentials) -> Result<u8> {
    let has_user = credentials.username.as_ref().is_some_and(|u| !u.is_empty());
    let has_password = credentials.password.is_some();
    if offered.contains(&30) && has_user && has_password {
        return Ok(30);
    }
    if offered.contains(&2) && has_password {
        return Ok(2);
    }
    if offered.contains(&1) {
        return Ok(1);
    }
    let hint = if offered.contains(&30) {
        "set CUA_VNC_USER and CUA_VNC_PASSWORD to a macOS account on that Mac"
    } else if offered.contains(&2) {
        "set CUA_VNC_PASSWORD to the server's VNC password"
    } else {
        "this client speaks None (1), VNC password (2) and Apple Remote Desktop (30)"
    };
    Err(RfbError::UnsupportedSecurity {
        offered: offered.to_vec(),
        hint,
    })
}

/// RFB 6143 section 7.2.2: DES-encrypt the challenge with the first eight
/// password bytes, each bit-reversed, as the key.
pub fn vnc_password_response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = Zeroizing::new([0u8; 8]);
    for (slot, byte) in key.iter_mut().zip(password.bytes()) {
        *slot = byte.reverse_bits();
    }
    let cipher = des::Des::new_from_slice(&key[..]).expect("an 8-byte DES key");
    let mut out = *challenge;
    for block in out.chunks_exact_mut(8) {
        cipher.encrypt_block(block.into());
    }
    out
}

/// Apple Remote Desktop authentication (security type 30): AES-128-ECB, keyed
/// by MD5 of the Diffie-Hellman secret, over a 64-byte user name and a 64-byte
/// password, each NUL-padded; then the client's public key.
pub fn ard_response(
    generator: u16,
    prime: &[u8],
    server_key: &[u8],
    private: &BigUint,
    username: &str,
    password: &str,
) -> Result<Vec<u8>> {
    if username.len() > 63 || password.len() > 63 {
        return Err(RfbError::Protocol(
            "Apple Remote Desktop takes a user name and password of at most 63 bytes".into(),
        ));
    }
    let key_len = prime.len();
    let prime = BigUint::from_bytes_be(prime);
    let public = BigUint::from(generator).modpow(private, &prime);
    let shared = Zeroizing::new(pad(
        &BigUint::from_bytes_be(server_key)
            .modpow(private, &prime)
            .to_bytes_be(),
        key_len,
    ));
    let digest = <md5::Md5 as md5::Digest>::digest(&shared[..]);
    let cipher = aes::Aes128::new_from_slice(&digest).expect("a 16-byte AES key");
    let mut block = Zeroizing::new([0u8; 128]);
    block[..username.len()].copy_from_slice(username.as_bytes());
    block[64..64 + password.len()].copy_from_slice(password.as_bytes());
    let mut out = block.to_vec();
    for chunk in out.chunks_exact_mut(16) {
        cipher.encrypt_block(chunk.into());
    }
    out.extend_from_slice(&pad(&public.to_bytes_be(), key_len));
    Ok(out)
}

fn pad(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len.saturating_sub(bytes.len())];
    out.extend_from_slice(bytes);
    out
}

fn read_u8(stream: &mut impl Read) -> Result<u8> {
    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16(stream: &mut impl Read) -> Result<u16> {
    let mut buf = [0u8; 2];
    stream.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}

fn read_u32(stream: &mut impl Read) -> Result<u32> {
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn read_reason(stream: &mut impl Read) -> Result<String> {
    let len = read_u32(stream)?;
    let mut reason = Vec::new();
    stream
        .take(u64::from(len.min(4096)))
        .read_to_end(&mut reason)?;
    Ok(String::from_utf8_lossy(&reason).into_owned())
}

fn skip(stream: &mut impl Read, len: u64) -> Result<()> {
    let copied = std::io::copy(&mut stream.take(len), &mut std::io::sink())?;
    if copied != len {
        return Err(RfbError::Io("the VNC server closed the connection".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn credentials(user: Option<&str>, password: Option<&str>) -> Credentials {
        Credentials {
            username: user.map(|u| Zeroizing::new(u.to_owned())),
            password: password.map(|p| Zeroizing::new(p.to_owned())),
        }
    }

    // Expected values below come from vncdotool's algorithm run in Python
    // with the `cryptography` package, not from this module.
    #[test]
    fn vnc_password_response_matches_reference() {
        let challenge: [u8; 16] = std::array::from_fn(|i| i as u8);
        assert_eq!(
            hex(&vnc_password_response("secret", &challenge)),
            "ee22539f33a5983ec12f9c2edbc995dd"
        );
        // Only the first eight bytes count.
        assert_eq!(
            hex(&vnc_password_response("longpassword", &challenge)),
            "5931256585fd62106d317e09fc963baf"
        );
    }

    const OAKLEY_GROUP_2: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF";

    #[test]
    fn ard_response_matches_reference() {
        let prime = unhex(OAKLEY_GROUP_2);
        let server_private = BigUint::parse_bytes(b"1234567890abcdef1234567890abcdef", 16).unwrap();
        let server_key = pad(
            &BigUint::from(2u8)
                .modpow(&server_private, &BigUint::from_bytes_be(&prime))
                .to_bytes_be(),
            128,
        );
        let client_private = BigUint::parse_bytes(b"fedcba0987654321fedcba0987654321", 16).unwrap();
        let response =
            ard_response(2, &prime, &server_key, &client_private, "admin", "hunter2").unwrap();
        assert_eq!(
            hex(&response),
            "d0a01da7270c19ac89c4d7efe3c552ba1a08f15feaa1063075c7ba5380ec06291a08f15feaa1063075c7ba5380ec06291a08f15feaa1063075c7ba5380ec0629cb32b037baea6e91eb00e9bcbcd6f56c1a08f15feaa1063075c7ba5380ec06291a08f15feaa1063075c7ba5380ec06291a08f15feaa1063075c7ba5380ec0629a49e9861bc70128ad73c00744f1a57162275939d9b10479c65510b2fd00e423dda598d341906f21c98535be518dd6f589f3a613aeaada094c7be7464fce38f26ac1acf4058ac2e7ae09c3593af1ffee8e085a1fa26f49f83afd529048e6d418abd12155718227767bfac45c71de1b09d88b2fe2e057f4bccdaa65d676a097cdf"
        );
    }

    #[test]
    fn parse_server_forms() {
        assert_eq!(
            parse_server("mac.local").unwrap(),
            ("mac.local".into(), 5900)
        );
        assert_eq!(
            parse_server("10.0.0.2:5901").unwrap(),
            ("10.0.0.2".into(), 5901)
        );
        assert_eq!(
            parse_server("127.0.0.1::51234").unwrap(),
            ("127.0.0.1".into(), 51234)
        );
        assert_eq!(parse_server("[::1]:5902").unwrap(), ("::1".into(), 5902));
        assert_eq!(parse_server("::1").unwrap(), ("::1".into(), 5900));
        assert!(parse_server("mac:notaport").is_err());
        assert!(parse_server("").is_err());
    }

    /// A scripted server on loopback. `script` runs against the accepted
    /// connection; the client runs on the test thread.
    fn serve(
        script: impl FnOnce(&mut TcpStream) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            script(&mut stream);
        });
        (address, handle)
    }

    fn read_n(stream: &mut TcpStream, n: usize) -> Vec<u8> {
        let mut buf = vec![0u8; n];
        stream.read_exact(&mut buf).unwrap();
        buf
    }

    /// ServerInit for a `width`x`height` screen, then consume SetPixelFormat
    /// and SetEncodings and check they ask for 32-bit Raw and DesktopSize.
    fn server_init(stream: &mut TcpStream, width: u16, height: u16) {
        assert_eq!(
            read_n(stream, 1),
            [1],
            "ClientInit asks for a shared session"
        );
        let mut init = Vec::new();
        init.extend_from_slice(&width.to_be_bytes());
        init.extend_from_slice(&height.to_be_bytes());
        init.extend_from_slice(&[0u8; 16]);
        init.extend_from_slice(&4u32.to_be_bytes());
        init.extend_from_slice(b"test");
        stream.write_all(&init).unwrap();
        let pixel_format = read_n(stream, 20);
        assert_eq!(pixel_format[0], 0);
        assert_eq!(&pixel_format[4..8], &[32, 24, 0, 1]);
        assert_eq!(
            read_n(stream, 12),
            [2, 0, 0, 2, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21]
        );
    }

    #[test]
    fn apple_server_uses_ard_and_the_server_can_decrypt_it() {
        let (address, server) = serve(|stream| {
            stream.write_all(b"RFB 003.889\n").unwrap();
            assert_eq!(read_n(stream, 12), b"RFB 003.008\n");
            stream.write_all(&[4, 30, 33, 36, 35]).unwrap();
            assert_eq!(read_n(stream, 1), [30]);
            let prime = unhex(OAKLEY_GROUP_2);
            let p = BigUint::from_bytes_be(&prime);
            let server_private = BigUint::from(0xc0ffee_u32);
            let mut message = vec![0, 2, 0, 128];
            message.extend_from_slice(&prime);
            message.extend_from_slice(&pad(
                &BigUint::from(2u8).modpow(&server_private, &p).to_bytes_be(),
                128,
            ));
            stream.write_all(&message).unwrap();
            let reply = read_n(stream, 256);
            let client_key = BigUint::from_bytes_be(&reply[128..]);
            let shared = pad(&client_key.modpow(&server_private, &p).to_bytes_be(), 128);
            let digest = <md5::Md5 as md5::Digest>::digest(&shared);
            let cipher = aes::Aes128::new_from_slice(&digest).unwrap();
            let mut plain = reply[..128].to_vec();
            for chunk in plain.chunks_exact_mut(16) {
                aes::cipher::BlockDecrypt::decrypt_block(&cipher, chunk.into());
            }
            assert_eq!(&plain[..6], b"admin\0");
            assert_eq!(&plain[64..72], b"hunter2\0");
            stream.write_all(&0u32.to_be_bytes()).unwrap();
            server_init(stream, 2, 1);
        });
        let client = RfbClient::connect(
            &address,
            &credentials(Some("admin"), Some("hunter2")),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(client.size(), (2, 1));
        server.join().unwrap();
    }

    #[test]
    fn apple_server_without_a_user_name_says_what_to_set() {
        let (address, server) = serve(|stream| {
            stream.write_all(b"RFB 003.889\n").unwrap();
            read_n(stream, 12);
            stream.write_all(&[4, 30, 33, 36, 35]).unwrap();
        });
        let error = RfbClient::connect(
            &address,
            &credentials(None, Some("pw")),
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        assert_eq!(
            error,
            RfbError::UnsupportedSecurity {
                offered: vec![30, 33, 36, 35],
                hint: "set CUA_VNC_USER and CUA_VNC_PASSWORD to a macOS account on that Mac",
            }
        );
        server.join().unwrap();
    }

    #[test]
    fn wrong_password_reports_the_servers_reason() {
        let (address, server) = serve(|stream| {
            stream.write_all(b"RFB 003.008\n").unwrap();
            read_n(stream, 12);
            stream.write_all(&[1, 2]).unwrap();
            assert_eq!(read_n(stream, 1), [2]);
            let challenge: [u8; 16] = std::array::from_fn(|i| i as u8);
            stream.write_all(&challenge).unwrap();
            assert_eq!(
                read_n(stream, 16),
                vnc_password_response("wrong", &challenge)
            );
            stream.write_all(&1u32.to_be_bytes()).unwrap();
            stream.write_all(&14u32.to_be_bytes()).unwrap();
            stream.write_all(b"Authentication").unwrap();
        });
        let error = RfbClient::connect(
            &address,
            &credentials(None, Some("wrong")),
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
        assert_eq!(error, RfbError::Refused("Authentication".into()));
        server.join().unwrap();
    }

    #[test]
    fn screenshot_click_and_typing_send_the_expected_bytes() {
        let (address, server) = serve(|stream| {
            stream.write_all(b"RFB 003.008\n").unwrap();
            read_n(stream, 12);
            stream.write_all(&[1, 1]).unwrap();
            assert_eq!(read_n(stream, 1), [1]);
            stream.write_all(&0u32.to_be_bytes()).unwrap();
            server_init(stream, 1, 1);

            // The screen grows to 2x2 before the first update; the client
            // asks again at the new size.
            assert_eq!(read_n(stream, 10), [3, 0, 0, 0, 0, 0, 0, 1, 0, 1]);
            stream
                .write_all(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 2, 0xff, 0xff, 0xff, 0x21])
                .unwrap();
            assert_eq!(read_n(stream, 10), [3, 0, 0, 0, 0, 0, 0, 2, 0, 2]);
            // A bell, then two updates: top row, then bottom row.
            stream.write_all(&[2]).unwrap();
            let mut update = vec![0u8, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0];
            update.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0]); // red, green (B G R X)
            stream.write_all(&update).unwrap();
            let mut update = vec![0u8, 0, 0, 1, 0, 0, 0, 1, 0, 2, 0, 1, 0, 0, 0, 0];
            update.extend_from_slice(&[255, 0, 0, 0, 255, 255, 255, 0]); // blue, white
            stream.write_all(&update).unwrap();

            // Click at (1, 0): move, press, release.
            assert_eq!(read_n(stream, 6), [5, 0, 0, 1, 0, 0]);
            assert_eq!(read_n(stream, 6), [5, 1, 0, 1, 0, 0]);
            assert_eq!(read_n(stream, 6), [5, 0, 0, 1, 0, 0]);

            // "a" then Return, each down and up.
            assert_eq!(read_n(stream, 8), [4, 1, 0, 0, 0, 0, 0, 0x61]);
            assert_eq!(read_n(stream, 8), [4, 0, 0, 0, 0, 0, 0, 0x61]);
            assert_eq!(read_n(stream, 8), [4, 1, 0, 0, 0, 0, 0xff, 0x0d]);
            assert_eq!(read_n(stream, 8), [4, 0, 0, 0, 0, 0, 0xff, 0x0d]);
        });
        let mut client =
            RfbClient::connect(&address, &credentials(None, None), Duration::from_secs(5)).unwrap();
        let frame = client.screenshot().unwrap();
        assert_eq!(
            frame.rgba,
            [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]
        );
        assert!(frame.to_png().unwrap().starts_with(b"\x89PNG"));
        assert_eq!(
            client.click(2, 0).err().unwrap(),
            RfbError::Protocol("(2, 0) is outside the 2x2 screen".into())
        );
        client.click(1, 0).unwrap();
        assert_eq!(
            client.type_line("a\u{7}").err().unwrap(),
            RfbError::Protocol("the text has a control character".into())
        );
        client.type_line("a").unwrap();
        server.join().unwrap();
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }
}
