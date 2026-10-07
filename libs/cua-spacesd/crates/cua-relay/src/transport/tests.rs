use super::*;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn selection(url: &str, vars: &[(&str, &str)]) -> Result<Option<Uri>, &'static str> {
    route(&url.parse().unwrap(), |key| {
        vars.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| OsString::from(v))
    })
}

#[test]
fn proxy_precedence_and_bypass() {
    let url = "https://relay.cua.ai:443/";
    for (vars, expected) in [
        (vec![], None),
        (
            vec![("https_proxy", "http://lower:80")],
            Some("http://lower:80/"),
        ),
        (
            vec![
                ("HTTPS_PROXY", "http://upper:80"),
                ("https_proxy", "http://lower:80"),
            ],
            Some("http://upper:80/"),
        ),
        (
            vec![("HTTPS_PROXY", ""), ("https_proxy", "http://lower:80")],
            None,
        ),
        (
            vec![("HTTPS_PROXY", ""), ("ALL_PROXY", "http://all:80")],
            Some("http://all:80/"),
        ),
        (vec![("HTTP_PROXY", "http://wrong:80")], None),
        (
            vec![("HTTPS_PROXY", "socks5://bad"), ("NO_PROXY", ".cua.ai")],
            None,
        ),
        (
            vec![("HTTPS_PROXY", "http://proxy"), ("NO_PROXY", "other, *")],
            None,
        ),
    ] {
        assert_eq!(
            selection(url, &vars).unwrap().as_ref().map(Uri::to_string),
            expected.map(String::from),
            "{vars:?}"
        );
    }
    assert!(selection(
        "http://127.0.0.1/",
        &[("HTTP_PROXY", "http://proxy"), ("NO_PROXY", "127.0.0.0/8")]
    )
    .unwrap()
    .is_none());
    assert!(
        selection("http://127.0.0.1/", &[("HTTP_PROXY", "http://proxy")])
            .unwrap()
            .is_some()
    );
}

#[test]
fn unsupported_configuration_is_not_direct() {
    for proxy in [
        "https://proxy",
        "socks5://proxy",
        "http://user:secret@proxy",
        "http://proxy/path",
        "http://proxy/?secret",
        "http://proxy:0",
        "http://proxy:99999",
        "http://proxy:bad",
        "bad proxy",
    ] {
        assert!(
            selection("https://relay.cua.ai/", &[("HTTPS_PROXY", proxy)]).is_err(),
            "{proxy}"
        );
    }
    assert!(selection(
        "https://relay.cua.ai/",
        &[("HTTPS_PROXY", "http://proxy"), ("REQUEST_METHOD", "GET")]
    )
    .is_err());
    assert!(selection(
        "https://relay.cua.ai/",
        &[
            ("HTTPS_PROXY", "http://proxy"),
            ("REQUEST_METHOD", "GET"),
            ("NO_PROXY", "relay.cua.ai")
        ]
    )
    .unwrap()
    .is_none());
}

#[cfg(unix)]
#[test]
fn non_unicode_winner_is_rejected_but_bypass_still_applies() {
    use std::os::unix::ffi::OsStringExt;
    let uri = "https://relay.cua.ai/".parse().unwrap();
    assert!(route(&uri, |key| (key == "HTTPS_PROXY")
        .then(|| OsString::from_vec(vec![255])))
    .is_err());
    assert!(route(&uri, |key| match key {
        "HTTPS_PROXY" => Some(OsString::from_vec(vec![255])),
        "NO_PROXY" => Some("relay.cua.ai".into()),
        _ => None,
    })
    .unwrap()
    .is_none());
}

#[test]
fn relay_destinations() {
    for (url, expected) in [
        (
            "wss://relay.cua.ai/path?secret",
            "https://relay.cua.ai:443/",
        ),
        ("ws://[::1]:123/a", "http://[::1]:123/"),
    ] {
        assert_eq!(
            destination(&url.parse().unwrap()).unwrap().to_string(),
            expected
        );
    }
    assert!(destination(&"ws://user:secret@relay.cua.ai/".parse().unwrap()).is_err());
}

async fn fake_proxy(response: Vec<u8>) -> (Uri, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
        }
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "CONNECT relay.invalid:443 HTTP/1.1\r\nhost: relay.invalid:443\r\n\r\n"
        );
        socket.write_all(&response).await.unwrap();
        let mut rest = Vec::new();
        let result = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut rest))
            .await
            .expect("orphan proxy socket");
        assert!(
            result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::ConnectionReset
        );
    });
    (uri, task)
}

#[tokio::test]
async fn connect_retains_read_ahead_and_accepts_informational_response() {
    let (proxy, task) =
        fake_proxy(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\nread-ahead".to_vec())
            .await;
    let mut stream = proxy_stream(&proxy, &"https://relay.invalid:443/".parse().unwrap())
        .await
        .unwrap();
    let mut bytes = [0; 10];
    stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"read-ahead");
    drop(stream);
    task.await.unwrap();
}

#[tokio::test]
async fn refused_connect_and_bounded_headers_close_without_fallback_or_secrets() {
    let mut responses: Vec<(Vec<u8>, String)> = [201, 204, 302, 403, 407, 500].iter().map(|status| {
        let expected = if *status == 407 { "proxy authentication required: HTTP 407".to_owned() } else { format!("proxy CONNECT refused: HTTP {}", StatusCode::from_u16(*status).unwrap()) };
        let body = if *status == 204 { "\r\n" } else { "Content-Length: 99999\r\n\r\nSECRET-CANARY" };
        (format!("HTTP/1.1 {status} SECRET-CANARY\r\nProxy-Authenticate: SECRET-CANARY\r\n{body}").into_bytes(), expected)
    }).collect();
    responses.push((
        format!("HTTP/1.1 200 OK\r\n{}\r\n", "X: a\r\n".repeat(65)).into_bytes(),
        "proxy CONNECT failed".into(),
    ));
    responses.push((
        format!("HTTP/1.1 200 OK\r\nX: {}", "a".repeat(17000)).into_bytes(),
        "proxy CONNECT failed".into(),
    ));
    responses.push((
        b"not HTTP SECRET-CANARY\r\n\r\n".to_vec(),
        "proxy CONNECT failed".into(),
    ));
    for (response, expected) in responses {
        let (proxy, task) = fake_proxy(response).await;
        let error = match tokio::time::timeout(
            Duration::from_secs(1),
            proxy_stream(&proxy, &"https://relay.invalid:443/".parse().unwrap()),
        )
        .await
        .unwrap()
        {
            Ok(_) => panic!("unexpected tunnel"),
            Err(e) => e,
        };
        assert_eq!(format!("{error:?}"), format!("Lost({expected:?})"));
        assert!(!format!("{error:?}").contains("SECRET-CANARY"));
        task.await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_drops_partial_connect_and_driver() {
    let (proxy, task) = fake_proxy(b"HTTP/1.1 200".to_vec()).await;
    assert!(tokio::time::timeout(
        Duration::from_millis(100),
        proxy_stream(&proxy, &"https://relay.invalid:443/".parse().unwrap())
    )
    .await
    .is_err());
    task.await.unwrap();
}

#[test]
fn proxy_fragments_are_rejected_unless_bypassed() {
    for raw in ["http://proxy/#secret", "http://proxy#secret"] {
        assert!(selection("https://relay.cua.ai/", &[("HTTPS_PROXY", raw)]).is_err());
        assert!(selection(
            "https://relay.cua.ai/",
            &[("HTTPS_PROXY", raw), ("NO_PROXY", "relay.cua.ai")]
        )
        .unwrap()
        .is_none());
    }
}

#[tokio::test]
async fn proxy_authentication_required_is_distinct_and_redacted() {
    let (proxy, task) = fake_proxy(
        b"HTTP/1.1 407 SECRET-CANARY\r\nProxy-Authenticate: SECRET-CANARY\r\n\r\n".to_vec(),
    )
    .await;
    let error = match proxy_stream(&proxy, &"https://relay.invalid:443/".parse().unwrap()).await {
        Ok(_) => panic!("unexpected tunnel"),
        Err(error) => error,
    };
    task.await.unwrap();
    assert!(
        matches!(error, SessionEnd::Lost(reason) if reason == "proxy authentication required: HTTP 407")
    );
}

#[tokio::test]
async fn repeated_informational_responses_remain_cancellable() {
    let (proxy, task) = fake_proxy(b"HTTP/1.1 100 Continue\r\n\r\n".repeat(1024)).await;
    assert!(tokio::time::timeout(
        Duration::from_millis(100),
        proxy_stream(&proxy, &"https://relay.invalid:443/".parse().unwrap())
    )
    .await
    .is_err());
    task.await.unwrap();
}
