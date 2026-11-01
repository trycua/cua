//! Read-only static serving of a file or a folder.
//!
//! The rules, each with a test:
//!
//! - only `GET` and `HEAD`;
//! - the request path is percent-decoded once, then split; a `..` segment, a
//!   NUL or a backslash is refused outright (never resolved);
//! - hidden entries (a segment starting with `.`) do not exist: `.git`, `.env`
//!   and friends in a shared folder are neither served nor listed;
//! - the resolved path is canonicalised and must stay inside the canonical
//!   root, so a symlink that points out of the folder is a 403 (and is not
//!   listed), while one that points inside it works;
//! - only regular files are served (no sockets, devices or FIFOs);
//! - a folder with an `index.html` serves it, otherwise a listing.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bytes::Bytes;
use futures_util::StreamExt as _;
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode, header};
use http_body_util::{BodyExt as _, Full, StreamBody};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
use tokio_util::io::ReaderStream;

use crate::GwBody;

/// Entries a listing shows at most.
pub const MAX_LISTING: usize = 5_000;

/// What to serve: a single file, or a folder.
#[derive(Clone, Debug)]
pub enum Root {
    /// One file (canonical path).
    File(PathBuf),
    /// A folder (canonical path).
    Dir(PathBuf),
}

fn empty_body() -> GwBody {
    Full::new(Bytes::new()).map_err(|n| match n {}).boxed()
}

fn full(bytes: impl Into<Bytes>) -> GwBody {
    Full::new(bytes.into()).map_err(|n| match n {}).boxed()
}

/// A plain-text response.
pub fn text(status: StatusCode, msg: &str) -> Response<GwBody> {
    let mut r = Response::new(full(msg.to_string()));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn hardened(mut r: Response<GwBody>) -> Response<GwBody> {
    let h = r.headers_mut();
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}

/// Splits a request path into safe segments, or the status to refuse it
/// with.
fn segments(raw_path: &str) -> Result<Vec<String>, StatusCode> {
    let path = raw_path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut out = Vec::new();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(StatusCode::FORBIDDEN),
            s if s.starts_with('.') => return Err(StatusCode::NOT_FOUND),
            s => out.push(s.to_string()),
        }
    }
    Ok(out)
}

/// Serves `rest` (the request path below the share, with its query) from
/// `root`.
pub async fn serve(
    root: &Root,
    method: &Method,
    rest: &str,
    headers: &HeaderMap,
) -> Response<GwBody> {
    let mut r = serve_inner(root, method, rest, headers).await;
    r = hardened(r);
    r
}

async fn serve_inner(
    root: &Root,
    method: &Method,
    rest: &str,
    headers: &HeaderMap,
) -> Response<GwBody> {
    if method != Method::GET && method != Method::HEAD {
        let mut r = text(StatusCode::METHOD_NOT_ALLOWED, "this link is read-only");
        r.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return r;
    }
    let segs = match segments(rest) {
        Ok(s) => s,
        Err(code) => return text(code, "not available"),
    };
    let head = method == Method::HEAD;
    match root {
        Root::File(path) => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ok = match segs.as_slice() {
                [] => true,
                [one] => one == name,
                _ => false,
            };
            if !ok {
                return text(StatusCode::NOT_FOUND, "not found");
            }
            serve_file(path, headers, head).await
        }
        Root::Dir(dir) => {
            let mut joined = dir.clone();
            for s in &segs {
                joined.push(s);
            }
            let real = match tokio::fs::canonicalize(&joined).await {
                Ok(p) => p,
                Err(_) => return text(StatusCode::NOT_FOUND, "not found"),
            };
            if !real.starts_with(dir) {
                return text(StatusCode::FORBIDDEN, "not available");
            }
            let meta = match tokio::fs::metadata(&real).await {
                Ok(m) => m,
                Err(_) => return text(StatusCode::NOT_FOUND, "not found"),
            };
            if meta.is_dir() {
                let path_part = rest.split(['?', '#']).next().unwrap_or("");
                if !segs.is_empty() && !path_part.ends_with('/') {
                    // Relative, so it is right whatever prefix the client sees.
                    let last = segs.last().map(String::as_str).unwrap_or("");
                    let mut r = text(StatusCode::PERMANENT_REDIRECT, "");
                    if let Ok(v) = HeaderValue::from_str(&format!(
                        "{}/",
                        utf8_percent_encode(last, NON_ALPHANUMERIC)
                    )) {
                        r.headers_mut().insert(header::LOCATION, v);
                    }
                    return r;
                }
                let index = real.join("index.html");
                if let Ok(real_index) = tokio::fs::canonicalize(&index).await
                    && real_index.starts_with(dir)
                    && tokio::fs::metadata(&real_index)
                        .await
                        .is_ok_and(|m| m.is_file())
                {
                    return serve_file(&real_index, headers, head).await;
                }
                listing(dir, &real, &segs, head).await
            } else if meta.is_file() {
                serve_file(&real, headers, head).await
            } else {
                text(StatusCode::NOT_FOUND, "not found")
            }
        }
    }
}

fn mime_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "txt" | "md" | "markdown" | "log" | "yaml" | "yml" | "toml" | "ini" | "rs" | "py"
        | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "java" | "kt" | "swift" | "sh"
        | "rb" | "sql" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn etag(len: u64, modified: Option<SystemTime>) -> String {
    let secs = modified
        .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("W/\"{len:x}-{secs:x}\"")
}

/// A single `bytes=a-b` range of a `len`-byte body: `Ok(None)` when absent
/// or not a single range (the whole file is served), `Err(())` when
/// unsatisfiable.
fn parse_range(h: Option<&HeaderValue>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(v) = h.and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let Some(spec) = v.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Ok(None);
    };
    let (start, end) = match (a.trim(), b.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
            (len.saturating_sub(n), len.saturating_sub(1))
        }
        (s, "") => (s.parse().map_err(|_| ())?, len.saturating_sub(1)),
        (s, e) => (
            s.parse().map_err(|_| ())?,
            e.parse::<u64>().map_err(|_| ())?.min(len.saturating_sub(1)),
        ),
    };
    if len == 0 || start >= len || start > end {
        return Err(());
    }
    Ok(Some((start, end)))
}

async fn serve_file(path: &Path, req_headers: &HeaderMap, head: bool) -> Response<GwBody> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return text(StatusCode::NOT_FOUND, "not found"),
    };
    let meta = match file.metadata().await {
        Ok(m) if m.is_file() => m,
        _ => return text(StatusCode::NOT_FOUND, "not found"),
    };
    let len = meta.len();
    let modified = meta.modified().ok();
    let tag = etag(len, modified);
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, mime_of(path))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::ETAG, &tag);
    if let Some(m) = modified {
        builder = builder.header(header::LAST_MODIFIED, httpdate::fmt_http_date(m));
    }
    if req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == tag || t.trim() == "*"))
    {
        return builder
            .status(StatusCode::NOT_MODIFIED)
            .body(empty_body())
            .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "error"));
    }
    let range = match parse_range(req_headers.get(header::RANGE), len) {
        Ok(r) => r,
        Err(()) => {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                .body(empty_body())
                .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "error"));
        }
    };
    let (status, start, count) = match range {
        Some((s, e)) => {
            builder = builder.header(header::CONTENT_RANGE, format!("bytes {s}-{e}/{len}"));
            (StatusCode::PARTIAL_CONTENT, s, e - s + 1)
        }
        None => (StatusCode::OK, 0, len),
    };
    builder = builder.status(status).header(header::CONTENT_LENGTH, count);
    let body = if head || count == 0 {
        empty_body()
    } else {
        if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return text(StatusCode::INTERNAL_SERVER_ERROR, "could not read the file");
        }
        let stream = ReaderStream::new(file.take(count)).map(|r| {
            r.map(http_body::Frame::data)
                .map_err(|e| Box::new(e) as crate::BoxError)
        });
        http_body_util::BodyExt::boxed(StreamBody::new(stream))
    };
    builder
        .body(body)
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "error"))
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

fn human(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

async fn listing(root: &Path, dir: &Path, segs: &[String], head: bool) -> Response<GwBody> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(r) => r,
        Err(_) => return text(StatusCode::NOT_FOUND, "not found"),
    };
    let mut rows: Vec<(bool, String, u64, Option<SystemTime>)> = Vec::new();
    let mut truncated = false;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // An entry that resolves outside the folder (a symlink out) is not
        // listed, so the listing never advertises what a request would refuse.
        let Ok(real) = tokio::fs::canonicalize(entry.path()).await else {
            continue;
        };
        if !real.starts_with(root) {
            continue;
        }
        let Ok(meta) = tokio::fs::metadata(&real).await else {
            continue;
        };
        if !(meta.is_dir() || meta.is_file()) {
            continue;
        }
        if rows.len() >= MAX_LISTING {
            truncated = true;
            break;
        }
        rows.push((meta.is_dir(), name, meta.len(), meta.modified().ok()));
    }
    rows.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let title = if segs.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", segs.join("/"))
    };
    let mut html = String::new();
    html.push_str(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"robots\" content=\"noindex\"><title>",
    );
    html.push_str(&esc(&title));
    html.push_str(
        "</title><style>:root{color-scheme:light dark}body{font:15px/1.5 system-ui,sans-serif;\
margin:0 auto;max-width:48rem;padding:1rem 16px}h1{font-size:1rem;font-weight:600;\
word-break:break-all}table{width:100%;border-collapse:collapse}td{padding:.35rem .25rem;\
border-top:1px solid color-mix(in srgb,currentColor 15%,transparent)}\
td.n{width:100%;word-break:break-all}td.s,td.m{white-space:nowrap;opacity:.65;text-align:right;\
padding-left:.75rem}a{color:inherit}</style></head><body><h1>",
    );
    html.push_str(&esc(&title));
    html.push_str("</h1><table>");
    if !segs.is_empty() {
        html.push_str("<tr><td class=\"n\"><a href=\"../\">../</a></td><td class=\"s\"></td><td class=\"m\"></td></tr>");
    }
    for (is_dir, name, size, modified) in &rows {
        let href = utf8_percent_encode(name, NON_ALPHANUMERIC).to_string();
        let slash = if *is_dir { "/" } else { "" };
        let when = modified
            .map(httpdate::fmt_http_date)
            .map(|d| d.trim_end_matches(" GMT").to_string())
            .unwrap_or_default();
        html.push_str(&format!(
            "<tr><td class=\"n\"><a href=\"{href}{slash}\">{}{slash}</a></td><td class=\"s\">{}</td><td class=\"m\">{}</td></tr>",
            esc(name),
            if *is_dir { String::new() } else { human(*size) },
            esc(&when),
        ));
    }
    html.push_str("</table>");
    if rows.is_empty() {
        html.push_str("<p>This folder is empty.</p>");
    }
    if truncated {
        html.push_str(&format!("<p>Showing the first {MAX_LISTING} entries.</p>"));
    }
    html.push_str("</body></html>");
    let len = html.len();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_LENGTH, len)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'",
        )
        .body(if head { empty_body() } else { full(html) })
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "error"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(root: &Root, path: &str) -> (StatusCode, HeaderMap, String) {
        get_with(root, &Method::GET, path, &HeaderMap::new()).await
    }

    async fn get_with(
        root: &Root,
        method: &Method,
        path: &str,
        h: &HeaderMap,
    ) -> (StatusCode, HeaderMap, String) {
        let r = serve(root, method, path, h).await;
        let (parts, body) = r.into_parts();
        let bytes = body
            .collect()
            .await
            .map(|b| b.to_bytes())
            .unwrap_or_default();
        (
            parts.status,
            parts.headers,
            String::from_utf8_lossy(&bytes).to_string(),
        )
    }

    fn tree() -> (tempfile::TempDir, Root) {
        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap().join("site");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), "hello world").unwrap();
        std::fs::write(root.join("sub/b.html"), "<b>b</b>").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        std::fs::write(root.parent().unwrap().join("outside.txt"), "OUTSIDE").unwrap();
        (d, Root::Dir(root))
    }

    #[tokio::test]
    async fn serves_files_and_listings_read_only() {
        let (_d, root) = tree();
        let (s, h, b) = get(&root, "/a.txt").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "hello world"));
        assert_eq!(h[header::CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(h["x-content-type-options"], "nosniff");
        let (s, _, b) = get(&root, "/").await;
        assert_eq!(s, StatusCode::OK);
        assert!(b.contains("a.txt") && b.contains("sub/"), "{b}");
        assert!(!b.contains(".env") && !b.contains(".git"), "{b}");
        // A folder without the trailing slash redirects relatively.
        let (s, h, _) = get(&root, "/sub").await;
        assert_eq!(
            (s, h[header::LOCATION].to_str().unwrap()),
            (StatusCode::PERMANENT_REDIRECT, "sub/")
        );
        let (s, _, b) = get(&root, "/sub/").await;
        assert!(s == StatusCode::OK && b.contains("b.html"));
        for m in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
            let (s, h, _) = get_with(&root, &m, "/a.txt", &HeaderMap::new()).await;
            assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{m}");
            assert_eq!(h[header::ALLOW], "GET, HEAD");
        }
        let (s, _, b) = get_with(&root, &Method::HEAD, "/a.txt", &HeaderMap::new()).await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, ""));
    }

    #[tokio::test]
    async fn traversal_is_refused_in_every_spelling() {
        let (_d, root) = tree();
        for p in [
            "/../outside.txt",
            "/sub/../../outside.txt",
            "/%2e%2e/outside.txt",
            "/%2E%2E%2Foutside.txt",
            "/sub/%2e%2e/%2e%2e/outside.txt",
            "/..%2foutside.txt",
            "/sub\\..\\..\\outside.txt",
            "/%00",
            "//..//outside.txt",
        ] {
            let (s, _, b) = get(&root, p).await;
            assert!(
                matches!(
                    s,
                    StatusCode::FORBIDDEN | StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
                ),
                "{p} -> {s}"
            );
            assert!(!b.contains("OUTSIDE"), "{p} leaked");
        }
    }

    #[tokio::test]
    async fn hidden_entries_do_not_exist() {
        let (_d, root) = tree();
        for p in ["/.env", "/.git/config", "/sub/../.env", "/%2eenv"] {
            let (s, _, b) = get(&root, p).await;
            assert_ne!(s, StatusCode::OK, "{p}");
            assert!(!b.contains("SECRET"), "{p}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_may_not_leave_the_folder() {
        let (d, root) = tree();
        let Root::Dir(dir) = &root else {
            unreachable!()
        };
        std::os::unix::fs::symlink(d.path().join("outside.txt"), dir.join("link-out.txt")).unwrap();
        std::os::unix::fs::symlink(d.path(), dir.join("dir-out")).unwrap();
        std::os::unix::fs::symlink(dir.join("a.txt"), dir.join("link-in.txt")).unwrap();
        for p in ["/link-out.txt", "/dir-out/outside.txt", "/dir-out/"] {
            let (s, _, b) = get(&root, p).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{p}");
            assert!(!b.contains("OUTSIDE"));
        }
        let (s, _, b) = get(&root, "/link-in.txt").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "hello world"));
        let (_, _, listing) = get(&root, "/").await;
        assert!(listing.contains("link-in.txt"));
        assert!(
            !listing.contains("link-out") && !listing.contains("dir-out"),
            "{listing}"
        );
    }

    #[tokio::test]
    async fn a_single_file_serves_only_itself() {
        let (_d, root) = tree();
        let Root::Dir(dir) = &root else {
            unreachable!()
        };
        let one = Root::File(dir.join("a.txt"));
        for p in ["/", "/a.txt"] {
            let (s, _, b) = get(&one, p).await;
            assert_eq!((s, b.as_str()), (StatusCode::OK, "hello world"), "{p}");
        }
        for p in ["/b.txt", "/sub/b.html", "/../outside.txt", "/a.txt/x"] {
            assert_ne!(get(&one, p).await.0, StatusCode::OK, "{p}");
        }
    }

    #[tokio::test]
    async fn index_html_wins_over_a_listing() {
        let (_d, root) = tree();
        let Root::Dir(dir) = &root else {
            unreachable!()
        };
        std::fs::write(dir.join("index.html"), "<h1>home</h1>").unwrap();
        let (s, h, b) = get(&root, "/").await;
        assert_eq!((s, b.as_str()), (StatusCode::OK, "<h1>home</h1>"));
        assert!(
            h[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
    }

    #[tokio::test]
    async fn ranges_and_etags() {
        let (_d, root) = tree();
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=6-".parse().unwrap());
        let (s, hh, b) = get_with(&root, &Method::GET, "/a.txt", &h).await;
        assert_eq!((s, b.as_str()), (StatusCode::PARTIAL_CONTENT, "world"));
        assert_eq!(hh[header::CONTENT_RANGE], "bytes 6-10/11");
        h.insert(header::RANGE, "bytes=-5".parse().unwrap());
        assert_eq!(get_with(&root, &Method::GET, "/a.txt", &h).await.2, "world");
        h.insert(header::RANGE, "bytes=50-60".parse().unwrap());
        assert_eq!(
            get_with(&root, &Method::GET, "/a.txt", &h).await.0,
            StatusCode::RANGE_NOT_SATISFIABLE
        );
        let (_, hh, _) = get(&root, "/a.txt").await;
        let mut c = HeaderMap::new();
        c.insert(header::IF_NONE_MATCH, hh[header::ETAG].clone());
        assert_eq!(
            get_with(&root, &Method::GET, "/a.txt", &c).await.0,
            StatusCode::NOT_MODIFIED
        );
    }

    #[tokio::test]
    async fn listing_escapes_names() {
        let (_d, root) = tree();
        let Root::Dir(dir) = &root else {
            unreachable!()
        };
        std::fs::write(dir.join("<img src=x onerror=alert(1)>.txt"), "x").unwrap();
        let (_, h, b) = get(&root, "/").await;
        assert!(!b.contains("<img src=x"), "{b}");
        assert!(b.contains("&lt;img src=x"));
        assert!(
            h[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("default-src 'none'")
        );
    }
}
