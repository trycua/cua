//! The fake relay directory as a process (docker host-setup test, app
//! captures). `cua-fake-relay <account-token> <account-id> [email]` prints
//! its URL and serves until killed. Also reads `CUA_FAKE_RELAY_PORT`,
//! `CUA_FAKE_RELAY_FRESH_SIGN_IN=1` (the account's tokens count as a fresh
//! sign-in), `CUA_FAKE_RELAY_REQUIRE_DEVICES=1` (the machine API needs an
//! enrolled device) and `CUA_FAKE_RELAY_ENFORCE_AFTER=<unix secs>` (the end
//! of the grace period the device list reports).

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (token, id) = match (args.first(), args.get(1)) {
        (Some(t), Some(i)) => (t.clone(), i.clone()),
        _ => {
            eprintln!("usage: cua-fake-relay <account-token> <account-id> [email]");
            std::process::exit(2);
        }
    };
    let env = |name: &str| std::env::var(name).ok();
    let relay = cua_host::testing::FakeRelay::start_on(
        env("CUA_FAKE_RELAY_PORT")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
    )
    .await;
    relay.add_account(&token, &id, args.get(2).map(String::as_str));
    if env("CUA_FAKE_RELAY_FRESH_SIGN_IN").as_deref() == Some("1") {
        relay.fresh_sign_in(&id);
    }
    if env("CUA_FAKE_RELAY_REQUIRE_DEVICES").as_deref() == Some("1") {
        relay.require_devices(true);
    }
    if let Some(at) = env("CUA_FAKE_RELAY_ENFORCE_AFTER").and_then(|v| v.parse().ok()) {
        relay.set_enforce_after(at);
    }
    println!("{}", relay.url);
    let _ = tokio::signal::ctrl_c().await;
}
