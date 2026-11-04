// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A streaming reader for the first file part of a `multipart/form-data`
//! body (RFC 7578), for the `/files` route's `POST` upload.
//!
//! The body is fed in arbitrary chunks; the bytes of the first part that
//! carries a `filename` come out as they arrive, without buffering the file.
//! Parts without a filename are skipped, and anything after the file part is
//! ignored. Header blocks are limited to [`MAX_HEADER_BYTES`].

/// Largest header block of one part.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// What the parser needs next.
#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    /// Feed more bytes.
    More,
    /// The file part ended; everything of it has been returned.
    Done,
}

/// Why a body could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a well-formed multipart body.
    Malformed(&'static str),
    /// The closing boundary came without any file part.
    NoFilePart,
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    /// Looking for the first `--boundary`.
    Preamble,
    /// After a boundary line: `--` closes, CRLF opens a part.
    AfterBoundary,
    /// Reading a part's header block.
    Headers,
    /// Inside the body of a part; `file` says whether to return its bytes.
    Body { file: bool },
    /// The file part is complete.
    Done,
}

/// Incremental reader of the first file part.
pub struct FirstFile {
    /// `--boundary`
    open: Vec<u8>,
    /// `\r\n--boundary`
    delimiter: Vec<u8>,
    buf: Vec<u8>,
    state: State,
    filename: Option<String>,
    started: bool,
}

/// The boundary parameter of a `multipart/form-data` `Content-Type`.
pub fn boundary(content_type: &str) -> Option<String> {
    let mut parts = content_type.split(';');
    let media = parts.next()?.trim();
    if !media.eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    parts.find_map(|param| {
        let (key, value) = param.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("boundary") {
            return None;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        (!value.is_empty() && value.len() <= 200).then(|| value.to_owned())
    })
}

/// The `filename` parameter of a `Content-Disposition` header value.
fn filename_of(disposition: &str) -> Option<String> {
    // Parameters are `; key=value` with quoted or token values; a quoted
    // value may contain `;` and `\"`.
    let bytes = disposition.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip to the next parameter.
        while i < bytes.len() && (bytes[i] == b';' || bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b';' {
            i += 1;
        }
        let key = disposition[key_start..i].trim().to_ascii_lowercase();
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        let mut value = Vec::new();
        if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                value.push(bytes[i]);
                i += 1;
            }
            i += 1;
        } else {
            while i < bytes.len() && bytes[i] != b';' {
                value.push(bytes[i]);
                i += 1;
            }
        }
        if key == "filename" {
            return Some(String::from_utf8_lossy(&value).trim().to_owned());
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

impl FirstFile {
    /// A reader for a body delimited by `boundary`.
    pub fn new(boundary: &str) -> Self {
        let open = format!("--{boundary}").into_bytes();
        let mut delimiter = b"\r\n".to_vec();
        delimiter.extend_from_slice(&open);
        Self {
            open,
            delimiter,
            buf: Vec::new(),
            state: State::Preamble,
            filename: None,
            started: false,
        }
    }

    /// The file part's `filename`, once its headers have been read.
    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    /// Whether the file part has begun (its headers were read).
    pub fn started(&self) -> bool {
        self.started
    }

    /// Feeds the next bytes of the body; bytes of the file part are
    /// appended to `out`.
    pub fn feed(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<Progress, Error> {
        self.buf.extend_from_slice(data);
        loop {
            match self.state {
                State::Done => return Ok(Progress::Done),
                State::Preamble => {
                    // The first boundary may start the body or follow a CRLF
                    // of the preamble.
                    let at = if self.buf.starts_with(&self.open) {
                        Some(0)
                    } else {
                        find(&self.buf, &self.delimiter).map(|i| i + 2)
                    };
                    match at {
                        Some(i) => {
                            self.buf.drain(..i + self.open.len());
                            self.state = State::AfterBoundary;
                        }
                        None => {
                            // Keep only what could still begin a boundary.
                            let keep = self.delimiter.len().saturating_sub(1);
                            if self.buf.len() > keep {
                                let cut = self.buf.len() - keep;
                                self.buf.drain(..cut);
                            }
                            return Ok(Progress::More);
                        }
                    }
                }
                State::AfterBoundary => {
                    if self.buf.len() < 2 {
                        return Ok(Progress::More);
                    }
                    if self.buf.starts_with(b"--") {
                        return Err(Error::NoFilePart);
                    }
                    // Transport padding (spaces, tabs) may precede the CRLF.
                    let Some(end) = find(&self.buf, b"\r\n") else {
                        if self.buf.len() > 1024 {
                            return Err(Error::Malformed("boundary line too long"));
                        }
                        return Ok(Progress::More);
                    };
                    if !self.buf[..end].iter().all(|b| *b == b' ' || *b == b'\t') {
                        return Err(Error::Malformed("text after the boundary"));
                    }
                    self.buf.drain(..end + 2);
                    self.state = State::Headers;
                }
                State::Headers => {
                    // A part may have no headers at all: its block is then
                    // just the blank line.
                    let end = if self.buf.starts_with(b"\r\n") {
                        Some((0, 2))
                    } else {
                        find(&self.buf, b"\r\n\r\n").map(|i| (i, i + 4))
                    };
                    let Some((block_end, consumed)) = end else {
                        if self.buf.len() > MAX_HEADER_BYTES {
                            return Err(Error::Malformed("part headers too large"));
                        }
                        return Ok(Progress::More);
                    };
                    if block_end > MAX_HEADER_BYTES {
                        return Err(Error::Malformed("part headers too large"));
                    }
                    let block = String::from_utf8_lossy(&self.buf[..block_end]).into_owned();
                    self.buf.drain(..consumed);
                    let mut filename = None;
                    for line in block.split("\r\n") {
                        if let Some((name, value)) = line.split_once(':') {
                            if name.trim().eq_ignore_ascii_case("content-disposition") {
                                filename = filename_of(value);
                            }
                        }
                    }
                    let file = filename.is_some();
                    if file {
                        self.filename = filename;
                        self.started = true;
                    }
                    self.state = State::Body { file };
                }
                State::Body { file } => {
                    if let Some(i) = find(&self.buf, &self.delimiter) {
                        if file {
                            out.extend_from_slice(&self.buf[..i]);
                        }
                        self.buf.drain(..i + self.delimiter.len());
                        if file {
                            self.state = State::Done;
                            self.buf.clear();
                            return Ok(Progress::Done);
                        }
                        self.state = State::AfterBoundary;
                    } else {
                        // Everything but a possible partial delimiter is body.
                        let keep = self.delimiter.len() - 1;
                        if self.buf.len() > keep {
                            let cut = self.buf.len() - keep;
                            if file {
                                out.extend_from_slice(&self.buf[..cut]);
                            }
                            self.buf.drain(..cut);
                        }
                        return Ok(Progress::More);
                    }
                }
            }
        }
    }

    /// The body ended: ok only if the file part was complete.
    pub fn finish(&self) -> Result<(), Error> {
        match self.state {
            State::Done => Ok(()),
            _ if self.started => Err(Error::Malformed("the file part is cut short")),
            _ => Err(Error::NoFilePart),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(boundary: &str, parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = b"preamble\r\n".to_vec();
        for (headers, data) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n{headers}\r\n\r\n").as_bytes());
            out.extend_from_slice(data);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\nepilogue").as_bytes());
        out
    }

    fn read(boundary: &str, data: &[u8], step: usize) -> Result<(Vec<u8>, Option<String>), Error> {
        let mut reader = FirstFile::new(boundary);
        let mut out = Vec::new();
        for chunk in data.chunks(step) {
            if reader.feed(chunk, &mut out)? == Progress::Done {
                return Ok((out, reader.filename().map(str::to_owned)));
            }
        }
        reader.finish()?;
        Ok((out, reader.filename().map(str::to_owned)))
    }

    #[test]
    fn boundary_parameter() {
        assert_eq!(
            boundary("multipart/form-data; boundary=xyz").as_deref(),
            Some("xyz")
        );
        assert_eq!(
            boundary("Multipart/Form-Data;charset=u; Boundary=\"a b\"").as_deref(),
            Some("a b")
        );
        assert_eq!(boundary("application/json"), None);
        assert_eq!(boundary("multipart/form-data"), None);
    }

    #[test]
    fn filenames() {
        assert_eq!(
            filename_of("form-data; name=\"file\"; filename=\"a b.txt\"").as_deref(),
            Some("a b.txt")
        );
        assert_eq!(
            filename_of("form-data; name=file; filename=x;y").as_deref(),
            Some("x")
        );
        assert_eq!(
            filename_of("form-data; filename=\"q\\\"r;s\"").as_deref(),
            Some("q\"r;s")
        );
        assert_eq!(filename_of("form-data; name=\"field\""), None);
        assert_eq!(
            filename_of("form-data; name=\"filename\"; filename=\"\"").as_deref(),
            Some("")
        );
    }

    #[test]
    fn reads_the_file_at_every_chunk_size() {
        // Binary content that contains CRLFs, dashes and a near-boundary.
        let mut content = b"line\r\n--boun\r\n-- not it\r\n--bound".to_vec();
        content.pop();
        content.extend((0..=255u8).cycle().take(5000));
        let data = body(
            "bound",
            &[
                ("Content-Disposition: form-data; name=\"note\"", b"skip me"),
                (
                    "Content-Disposition: form-data; name=\"file\"; filename=\"f.bin\"\r\nContent-Type: application/octet-stream",
                    &content,
                ),
                ("Content-Disposition: form-data; name=\"x\"", b"after"),
            ],
        );
        for step in [1, 2, 3, 7, 13, 64, 1000, data.len()] {
            let (got, name) = read("bound", &data, step).unwrap();
            assert!(got == content, "step {step}");
            assert_eq!(name.as_deref(), Some("f.bin"));
        }
    }

    #[test]
    fn empty_file_and_no_preamble() {
        let data = b"--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"e\"\r\n\r\n\r\n--b--\r\n".to_vec();
        for step in [1, 5, data.len()] {
            let (got, name) = read("b", &data, step).unwrap();
            assert!(got.is_empty(), "step {step}");
            assert_eq!(name.as_deref(), Some("e"));
        }
    }

    #[test]
    fn errors() {
        let only_fields = body("b", &[("Content-Disposition: form-data; name=\"a\"", b"1")]);
        assert_eq!(read("b", &only_fields, 4).unwrap_err(), Error::NoFilePart);
        assert_eq!(read("b", b"garbage", 3).unwrap_err(), Error::NoFilePart);
        let cut = b"--b\r\nContent-Disposition: form-data; filename=\"f\"\r\n\r\nabc".to_vec();
        assert_eq!(
            read("b", &cut, 4).unwrap_err(),
            Error::Malformed("the file part is cut short")
        );
        let junk = b"--b junk\r\n".to_vec();
        assert!(matches!(
            read("b", &junk, 4).unwrap_err(),
            Error::Malformed(_)
        ));
        let mut huge = b"--b\r\nX: ".to_vec();
        huge.extend(std::iter::repeat_n(b'a', MAX_HEADER_BYTES + 10));
        assert!(matches!(
            read("b", &huge, 1024).unwrap_err(),
            Error::Malformed("part headers too large")
        ));
    }
}
