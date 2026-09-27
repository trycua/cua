use std::path::{Path, PathBuf};

pub fn local_path_from_file_url(raw: &str) -> Option<PathBuf> {
    let rest = raw.strip_prefix("file://")?;
    let rest = match rest.split_once('/') {
        Some(("", _)) => rest,
        Some(("localhost", _)) => rest.strip_prefix("localhost").unwrap_or(rest),
        _ => return None,
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let decoded = percent_decode(rest);
    let path = Path::new(&decoded);
    path.is_absolute().then(|| path.to_path_buf())
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                decoded.push((high << 4) | low);
                i += 3;
                continue;
            }
        }

        decoded.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_escapes_become_the_real_filesystem_path() {
        assert_eq!(
            local_path_from_file_url("file:///Users/x/My%20Notes.txt"),
            Some(PathBuf::from("/Users/x/My Notes.txt"))
        );
        assert_eq!(
            local_path_from_file_url("file://localhost/tmp/%E2%9C%93.txt"),
            Some(PathBuf::from("/tmp/✓.txt"))
        );
        assert_eq!(
            local_path_from_file_url("file:///tmp/100%25.txt"),
            Some(PathBuf::from("/tmp/100%.txt"))
        );
    }

    #[test]
    fn a_literal_percent_that_is_not_an_escape_survives() {
        assert_eq!(
            local_path_from_file_url("file:///tmp/50%off.txt"),
            Some(PathBuf::from("/tmp/50%off.txt"))
        );
    }

    #[test]
    fn only_absolute_local_file_urls_resolve_to_a_path() {
        for raw in [
            "https://example.com/doc.txt",
            "file://remote-host/share/doc.txt",
            "file://",
            "/Users/x/plain-path.txt",
            "",
        ] {
            assert_eq!(
                local_path_from_file_url(raw),
                None,
                "{raw} must not resolve"
            );
        }
    }

    #[test]
    fn query_and_fragment_suffixes_are_dropped_before_decoding() {
        assert_eq!(
            local_path_from_file_url("file:///tmp/a.txt?v=2#top"),
            Some(PathBuf::from("/tmp/a.txt"))
        );
        assert_eq!(
            local_path_from_file_url("file:///tmp/a%23b.txt"),
            Some(PathBuf::from("/tmp/a#b.txt"))
        );
    }
}
