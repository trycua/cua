//! Regression: env futures must be `Send` so callers can `tokio::spawn`
//! them and export them through async FFI (cua-sdk, cua-daemon).

fn assert_send<T: Send>(_: T) {}

#[test]
fn connect_future_is_send() {
    let o = cua_spacesd_client::ConnectOptions::parse("127.0.0.1:1").unwrap();
    assert_send(cua_spacesd_client::SpacesdClient::connect(o));
}

#[allow(dead_code)]
fn call_futures_are_send(c: cua_spacesd_client::SpacesdClient) {
    let c2 = c.clone();
    assert_send(async move { c.health().await });
    assert_send(async move {
        c2.system()
            .get_capabilities(cua_proto::env::v1::GetCapabilitiesRequest {})
            .await
    });
}
