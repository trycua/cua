//! Prints the names of a Cua Spaces channel as `key=value` lines, for the
//! scripts that build and check an app bundle
//! (`apps/cua-spaces-macos/scripts/channel-names.sh`), so none of them
//! hard-codes a name: [`CuaChannel`] owns every one (CUA-1776).
//!
//! ```text
//! cargo run -q -p cua-home --example channel_names -- local
//! ```

use cua_home::channel::CuaChannel;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let Some(channel) = CuaChannel::parse(&arg) else {
        eprintln!("usage: channel_names <stable|local>");
        std::process::exit(2);
    };
    // The cua home as the bundle stamps it (`~/.cua-local`), not the path on
    // this machine.
    let home = channel.home_dir();
    let home = format!(
        "~/{}",
        home.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let lines = [
        ("channel", channel.name().to_string()),
        ("app_name", channel.app_name().to_string()),
        ("bundle_id", channel.bundle_id().to_string()),
        ("helper_label", channel.helper_label().to_string()),
        ("url_scheme", channel.url_scheme().to_string()),
        ("home", home),
        ("env", channel.default_env().name().to_string()),
    ];
    for (key, value) in lines {
        println!("{key}={value}");
    }
}
