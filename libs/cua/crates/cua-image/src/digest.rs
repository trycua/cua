//! Digest helpers.

use std::io::Write;

use sha2::{Digest, Sha256};

/// `sha256:<hex>` of a byte slice.
pub fn sha256_bytes(b: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(b)))
}

/// `sha256:<hex>` of a file (streamed).
pub fn sha256_file(path: &std::path::Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut w = HashWriter::new(std::io::sink());
    std::io::copy(&mut f, &mut w)?;
    Ok(w.finish().1)
}

/// A writer that hashes (sha256) and counts everything passing through.
pub struct HashWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> HashWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            written: 0,
        }
    }
    /// `(inner, (digest, bytes))`.
    pub fn finish(self) -> (W, (String, u64)) {
        (
            self.inner,
            (
                format!("sha256:{}", hex::encode(self.hasher.finalize())),
                self.written,
            ),
        )
    }
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Hex part of a `sha256:` digest (validated).
pub fn hex_of(digest: &str) -> Option<&str> {
    let h = digest.strip_prefix("sha256:")?;
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_vectors() {
        assert_eq!(
            sha256_bytes(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let mut w = HashWriter::new(Vec::new());
        w.write_all(b"abc").unwrap();
        let (inner, (d, n)) = w.finish();
        assert_eq!(inner, b"abc");
        assert_eq!(n, 3);
        assert_eq!(d, sha256_bytes(b"abc"));
        assert!(hex_of(&d).is_some());
        assert!(hex_of("sha256:xyz").is_none());
    }
}
