# cua-spacesd-client

Typed async Rust client for **cua-spacesd**, the daemon inside every Cua Space
(`cua.env.v1`, gRPC on port 3211). Use it to drive a Space from your own
harness: run commands, read and write files, take screenshots, and send
pointer and keyboard input. No agent layer, no hand-written protobuf.

```sh
cargo add cua-spacesd-client
cargo add tokio --features full
```

```rust,no_run
use cua_spacesd_client::{SpacesdClient, ScreenshotOptions};

#[tokio::main]
async fn main() -> cua_spacesd_client::Result<()> {
    // The Space's spacesd endpoint and token (`cua spaces show <name>`).
    let space = SpacesdClient::connect_url("127.0.0.1:3211", Some("TOKEN".into())).await?;

    let out = space.run("uname -a").await?;
    println!("{}", out.stdout_str());

    let shot = space.screenshot(ScreenshotOptions::default()).await?;
    std::fs::write("screen.png", &shot.image).unwrap();

    space.click(100.0, 200.0).await?;
    space.type_text("hello").await?;
    Ok(())
}
```

- Endpoints: `http(s)://host:port`, bare `host:port` (port 3211 by default),
  or a relay URL. Native gRPC (HTTP/2) with a gRPC-Web fallback.
- Auth: the Space's token, sent as `authorization: Bearer <token>`.
- The generated types are re-exported as `cua_spacesd_client::pb`
  (from [`cua-proto`](https://crates.io/crates/cua-proto)).

Not using Rust? The `.proto` files are attached to every `cua-sdk-v*`
release as `cua-proto-<version>.tar.gz`, and spacesd serves gRPC server
reflection on :3211.

Docs: <https://cua.ai/docs/cua-sdk>. MIT licensed.
