//! Pure scrubbing of events before they leave the machine.
//!
//! Hard rules: exception values and messages are cleared (they can carry
//! Space names, file names, emails); the user, request bodies and headers,
//! breadcrumbs, contexts and the host name are dropped; every string that
//! remains loses home folder prefixes (`~`) and emails (`[email]`); URLs
//! lose query strings and fragments.

use sentry::protocol::{DebugImage, Event, Frame, Stacktrace};
use serde_json::Value;

/// Marker directories whose next path segment is a user name.
const HOME_PARENTS: [&str; 3] = ["/Users/", "/home/", "\\Users\\"];

/// [`scrub_text_with_home`] with no extra home folder.
pub fn scrub_text(s: &str) -> String {
    scrub_text_with_home(s, None)
}

/// Replaces home folder prefixes (`/Users/<x>`, `/home/<x>`,
/// `C:\Users\<x>`, and `home` itself) with `~` and emails with `[email]`.
pub fn scrub_text_with_home(s: &str, home: Option<&str>) -> String {
    let mut out = s.to_string();
    if let Some(h) = home.map(|h| h.trim_end_matches(['/', '\\']))
        && h.len() > 1
        && out.contains(h)
    {
        out = out.replace(h, "~");
    }
    scrub_emails(&scrub_home_dirs(&out))
}

fn is_sep(c: char) -> bool {
    matches!(c, '/' | '\\') || c.is_whitespace() || matches!(c, '"' | '\'' | ':' | ',' | ')')
}

fn scrub_home_dirs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let hit = HOME_PARENTS
            .iter()
            .filter_map(|m| find_ci(rest, m).map(|i| (i, m.len())))
            .min_by_key(|(i, _)| *i);
        let Some((i, mlen)) = hit else {
            out.push_str(rest);
            return out;
        };
        let after = &rest[i + mlen..];
        let seg_len = after.find(is_sep).unwrap_or(after.len());
        if seg_len == 0 {
            out.push_str(&rest[..i + mlen]);
            rest = after;
            continue;
        }
        let mut head = &rest[..i];
        // A Windows drive prefix (`C:`) belongs to the home folder.
        let b = head.as_bytes();
        if mlen == 7
            && b.len() >= 2
            && b[b.len() - 1] == b':'
            && b[b.len() - 2].is_ascii_alphabetic()
        {
            head = &head[..head.len() - 2];
        }
        out.push_str(head);
        out.push('~');
        rest = &after[seg_len..];
    }
}

/// Case-insensitive (ASCII) find of an ASCII `needle`.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn scrub_emails(s: &str) -> String {
    if !s.contains('@') {
        return s.to_string();
    }
    let local_ok = |c: char| c.is_alphanumeric() || "._%+-".contains(c);
    let domain_ok = |c: char| c.is_alphanumeric() || ".-".contains(c);
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (at, _) in s.match_indices('@') {
        if at < last {
            continue;
        }
        let start = s[last..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| local_ok(*c))
            .last()
            .map_or(at, |(i, _)| last + i);
        let tail = &s[at + 1..];
        let dlen = tail.find(|c: char| !domain_ok(c)).unwrap_or(tail.len());
        let domain = tail[..dlen].trim_end_matches(['.', '-']);
        if start < at && domain.contains('.') && !domain.starts_with('.') {
            out.push_str(&s[last..start]);
            out.push_str("[email]");
            last = at + 1 + domain.len();
        }
    }
    out.push_str(&s[last..]);
    out
}

/// A URL without credentials, query string or fragment, and with its path
/// scrubbed. Anything that does not parse is returned as scrubbed text cut
/// at the first `?` or `#`.
pub fn scrub_url(url: &str, home: Option<&str>) -> String {
    let cut = url.split(['?', '#']).next().unwrap_or("");
    let no_creds = match cut.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority.rsplit('@').next().unwrap_or(authority);
            format!("{scheme}://{host}{path}")
        }
        None => cut.to_string(),
    };
    scrub_text_with_home(&no_creds, home)
}

fn scrub_opt(s: &mut Option<String>, home: Option<&str>) {
    if let Some(v) = s {
        *v = scrub_text_with_home(v, home);
    }
}

fn scrub_json(v: &mut Value, home: Option<&str>) {
    match v {
        Value::String(s) => *s = scrub_text_with_home(s, home),
        Value::Array(a) => a.iter_mut().for_each(|x| scrub_json(x, home)),
        Value::Object(m) => m.values_mut().for_each(|x| scrub_json(x, home)),
        _ => {}
    }
}

fn scrub_frame(f: &mut Frame, home: Option<&str>) {
    scrub_opt(&mut f.filename, home);
    scrub_opt(&mut f.abs_path, home);
    scrub_opt(&mut f.package, home);
    // Function and module names are code, not user data, but run the same
    // pass in case a closure name embeds a path.
    scrub_opt(&mut f.function, home);
    scrub_opt(&mut f.symbol, home);
    scrub_opt(&mut f.module, home);
    f.pre_context.clear();
    f.post_context.clear();
    f.context_line = None;
    f.vars.clear();
}

fn scrub_stacktrace(st: &mut Option<Stacktrace>, home: Option<&str>) {
    if let Some(st) = st {
        st.frames.iter_mut().for_each(|f| scrub_frame(f, home));
        st.registers.clear();
    }
}

/// Applies every privacy rule to `event`. `home` is the actual home folder
/// of this process, if known.
pub fn scrub_event(mut event: Event<'static>, home: Option<&str>) -> Event<'static> {
    event.message = None;
    event.logentry = None;
    event.user = None;
    event.server_name = None;
    event.breadcrumbs.values.clear();
    event.contexts.clear();
    event.modules.clear();
    event.template = None;
    event.logger = None;
    scrub_opt(&mut event.culprit, home);
    scrub_opt(&mut event.transaction, home);

    for ex in &mut event.exception.values {
        ex.value = None;
        scrub_opt(&mut ex.module, home);
        scrub_stacktrace(&mut ex.stacktrace, home);
        scrub_stacktrace(&mut ex.raw_stacktrace, home);
    }
    scrub_stacktrace(&mut event.stacktrace, home);
    for t in &mut event.threads.values {
        scrub_opt(&mut t.name, home);
        scrub_stacktrace(&mut t.stacktrace, home);
        scrub_stacktrace(&mut t.raw_stacktrace, home);
    }

    if let Some(req) = &mut event.request {
        if let Some(url) = &mut req.url {
            let clean = scrub_url(url.as_str(), home);
            if let Ok(u) = clean.parse() {
                *url = u;
            } else {
                req.url = None;
            }
        }
        req.query_string = None;
        req.cookies = None;
        req.data = None;
        req.headers.clear();
        req.env.clear();
    }

    for v in event.tags.values_mut() {
        *v = scrub_text_with_home(v, home);
    }
    event.extra.values_mut().for_each(|v| scrub_json(v, home));

    let meta = event.debug_meta.to_mut();
    meta.sdk_info = None;
    for img in &mut meta.images {
        match img {
            DebugImage::Symbolic(i) => {
                i.name = scrub_text_with_home(&i.name, home);
                scrub_opt(&mut i.debug_file, home);
            }
            DebugImage::Apple(i) => i.name = scrub_text_with_home(&i.name, home),
            DebugImage::Wasm(i) => {
                i.name = scrub_text_with_home(&i.name, home);
                i.code_file = scrub_text_with_home(&i.code_file, home);
                scrub_opt(&mut i.debug_file, home);
            }
            _ => {}
        }
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Exception, Request, User, Values};

    #[test]
    fn home_paths_become_tilde() {
        assert_eq!(scrub_text("/Users/alice/Library/x.rs"), "~/Library/x.rs");
        assert_eq!(
            scrub_text("at /home/bob/.cargo/lib.rs:3"),
            "at ~/.cargo/lib.rs:3"
        );
        assert_eq!(scrub_text(r"C:\Users\Carol\AppData\x"), r"~\AppData\x");
        assert_eq!(scrub_text("/users/dave/x"), "~/x");
        assert_eq!(
            scrub_text_with_home("/opt/me/work/a.rs", Some("/opt/me")),
            "~/work/a.rs"
        );
        assert_eq!(scrub_text("/Users/"), "/Users/");
        assert_eq!(scrub_text("/usr/lib/libc.dylib"), "/usr/lib/libc.dylib");
        assert_eq!(scrub_text("/Users/a/x and /Users/b/y"), "~/x and ~/y");
    }

    #[test]
    fn emails_are_replaced() {
        assert_eq!(
            scrub_text("mail f.bonacci+x@trycua.com now"),
            "mail [email] now"
        );
        assert_eq!(scrub_text("<a@b.io>,c@d.org"), "<[email]>,[email]");
        assert_eq!(scrub_text("user@host"), "user@host");
        assert_eq!(scrub_text("@handle and a @ b"), "@handle and a @ b");
    }

    #[test]
    fn urls_lose_query_fragment_and_credentials() {
        assert_eq!(
            scrub_url("https://u:p@spaces.cua.ai/a/b?token=1&e=a@b.co#frag", None),
            "https://spaces.cua.ai/a/b"
        );
        assert_eq!(scrub_url("/Users/x/f?y=1", None), "~/f");
    }

    fn dirty() -> Event<'static> {
        let mut ev = Event {
            message: Some("boom in Space acme-secrets".into()),
            server_name: Some("Francescos-MacBook.local".into()),
            user: Some(User {
                ip_address: Some("1.2.3.4".parse().unwrap()),
                email: Some("a@b.co".into()),
                ..Default::default()
            }),
            request: Some(Request {
                url: Some("https://x.test/p?q=secret#f".parse().unwrap()),
                query_string: Some("q=secret".into()),
                headers: [("Cookie".to_string(), "s=1".to_string())].into(),
                ..Default::default()
            }),
            exception: Values::from(vec![Exception {
                ty: "panic".into(),
                value: Some("failed to open /Users/alice/Secret Space.txt".into()),
                stacktrace: Some(Stacktrace {
                    frames: vec![Frame {
                        filename: Some("/Users/alice/src/lib.rs".into()),
                        abs_path: Some("/Users/alice/src/lib.rs".into()),
                        function: Some("run".into()),
                        context_line: Some("let k = 1;".into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        ev.tags.insert("path".into(), "/home/bob/x a@b.co".into());
        ev.extra
            .insert("n".into(), serde_json::json!({"p": ["/Users/zed/q"]}));
        ev
    }

    #[test]
    fn event_is_scrubbed() {
        let ev = scrub_event(dirty(), None);
        assert!(ev.message.is_none() && ev.logentry.is_none());
        assert!(ev.server_name.is_none());
        assert!(ev.user.is_none());
        assert_eq!(ev.exception.values[0].value, None);
        assert_eq!(ev.exception.values[0].ty, "panic");
        let f = &ev.exception.values[0].stacktrace.as_ref().unwrap().frames[0];
        assert_eq!(f.filename.as_deref(), Some("~/src/lib.rs"));
        assert_eq!(f.abs_path.as_deref(), Some("~/src/lib.rs"));
        assert_eq!(f.function.as_deref(), Some("run"));
        assert!(f.context_line.is_none());
        let r = ev.request.clone().unwrap();
        assert_eq!(r.url.clone().unwrap().as_str(), "https://x.test/p");
        assert!(r.query_string.is_none() && r.headers.is_empty());
        assert_eq!(ev.tags["path"], "~/x [email]");
        assert_eq!(ev.extra["n"]["p"][0], "~/q");
        let all = format!("{ev:?}");
        for needle in ["alice", "bob", "zed", "secret", "MacBook", "a@b.co"] {
            assert!(!all.contains(needle), "{needle} leaked: {all}");
        }
    }
}
