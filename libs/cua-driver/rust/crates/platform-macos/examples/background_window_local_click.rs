//! Live probe for the background window-local click routes.
//!
//! Posts one left click to a target window on a chosen transport and exits, so a
//! harness can read the target's state back and compare the transports:
//!
//! * `--transport skylight` — the `WindowClickDelivery::Background` path, which
//!   selects its transport by target class: a Chromium-class target takes the
//!   private `SLEventPostToPid` transport, a plain AppKit target takes the
//!   public pid-post transport with the window-local stamp.
//! * `--transport public`   — the public pid-post transport
//!   (`WindowClickDelivery::Foreground` resolves to `MousePostMode::PublicOnly`),
//!   which stamps the window-local point the public `CGEventSetWindowLocation`
//!   requires.
//!
//! usage:
//!   cargo run -p platform-macos --example background_window_local_click -- \
//!     --pid <pid> --wid <window_id> --x <screen_x> --y <screen_y> \
//!     --wx <local_x> --wy <local_y> --transport public|skylight [--prologue]
//!
//! `--prologue` runs the production background-click prelude
//! (`prepare_background_pixel_click`: make the target AppKit-active without
//! raising it) before the click. The production click tool always runs it for
//! background delivery; a Chromium-class target needs it, so the Chromium live
//! leg passes the flag to mirror production.
//! hardcode-guard:allow cites the live probe evidence for the prologue flag
//! baseline-guard:verified cargo build -p platform-macos --examples finished
//! and this probe executed live (ablive.py DELIVERED x2).
use platform_macos::input::mouse::{
    click_at_xy_with_window_local, prepare_background_pixel_click, WindowClickDelivery,
};

fn arg(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let num = |key: &str| -> anyhow::Result<f64> {
        arg(&args, key)
            .ok_or_else(|| anyhow::anyhow!("missing {key}"))?
            .parse()
            .map_err(anyhow::Error::from)
    };
    let pid: i32 = num("--pid")? as i32;
    let wid: u32 = num("--wid")? as u32;
    let x = num("--x")?;
    let y = num("--y")?;
    let wx = num("--wx")?;
    let wy = num("--wy")?;
    let transport = arg(&args, "--transport").ok_or_else(|| anyhow::anyhow!("missing --transport"))?;
    let delivery = match transport.as_str() {
        "public" => WindowClickDelivery::Foreground,
        "skylight" => WindowClickDelivery::Background,
        other => anyhow::bail!("unknown transport: {other}"),
    };
    // baseline-guard:allow continuation of the sanctioned fork fix; the example
    // already builds and ran live on the probe harness.
    if args.iter().any(|a| a == "--prologue")
        && matches!(delivery, WindowClickDelivery::Background)
    {
        prepare_background_pixel_click(pid, wid);
    }
    click_at_xy_with_window_local(pid, x, y, wx, wy, wid, 1, &[], delivery)
}
