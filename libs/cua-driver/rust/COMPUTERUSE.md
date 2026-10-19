# ComputerUse customization (branch computeruse/win-neutral-0.34.1)

Base: upstream tag `cua-driver-rs-v0.34.1` (commit 0c69d9a2c8abbb5051a2df4a23f0fa166d5385dc), the driver version
pinned by the ComputerUse SDK on macOS and Windows.

| Change | Where | Effect |
| --- | --- | --- |
| Neutral cursor (`neutral.1`, same patch as the macOS SDK build) | `crates/cursor-overlay/src/neutral_cursor.rs`, `render_state.rs`, `lib.rs` | With `COMPUTERUSE_CURSOR_STYLE=neutral` the agent cursor is a dark grey silhouette without action artwork or badges. Input, capture and coordinates unchanged. |
| Rego policy engine optional | `crates/cua-driver`, `crates/cua-driver-sdk`, `crates/platform-windows` Cargo.toml (`rego` feature, default on) | Default builds are unchanged; the product build drops regorus (and, on MSVC, the Spectre-mitigated library requirement). YAML policies remain. |
| `computeruse` product feature | `crates/cua-driver/src/computeruse_product.rs`, `cli.rs`, `main.rs`, `version_check.rs`, `telemetry.rs` | No GitHub release check, telemetry compiled off, no `check_for_update` / extension host tools, and the CLI refuses `update`, `check-update`, `channel`, `skills`, `extension`, `perception`, `autostart`, `mcp-config` and telemetry changes. |

Windows product build (Rust 1.97.1, MSVC x64):

```powershell
$env:CUA_DRIVER_GIT_SHA = '0c69d9a2c8abbb5051a2df4a23f0fa166d5385dc'
cargo build --release -p cua-driver --bin cua-driver --no-default-features --features computeruse
```

The SDK packages the resulting `cua-driver.exe` as `computeruse-driver-0.34.1-neutral.1-windows-x64.zip`
(see ComputerUse/windows/cua in the recoder repository).

## Further trimming candidates (analysis, not applied)

The ComputerUse SDK starts the driver only as "cua-driver mcp" over stdio and calls 19 of the 57 tools this build
lists (bring_to_front, click, drag, get_desktop_state, get_screen_size, get_window_state, hotkey, launch_app,
list_apps, list_windows, press_key, scroll, set_agent_cursor_enabled, set_value, type_text, zoom, invoke_menu,
verify_state, debug_window_info). The SDK raw pass-through (`cuh_call` with `raw:true`) can reach the others, so
any removal must first confirm that no consumer depends on it through raw calls.

Current product build: 31.5 MB, 386 crates in the dependency graph (`cargo tree -e normal`).

| Candidate | What goes | Evidence | Benefit | Risk / work |
| --- | --- | --- | --- | --- |
| Network stack | `ureq` + `rustls` + `ring` + `rustls-webpki` (+ `tar`, `flate2`, `semver` for archives and versions) | `cargo tree -i ureq`: only `cua-driver` uses it, for version_check, skills, extension downloads and telemetry upload, all disabled at runtime by `computeruse` | Driver can no longer open HTTPS at all (strongest no-network guarantee); fewer crates; size not measured | Medium: `cfg(feature = "computeruse")` out `version_check.rs`, `skills.rs`, the download half of `extension_manager.rs` (300 KB source) and the telemetry sender |
| Telemetry crate | `cua-telemetry` (linked, sending compiled off) | listed at depth 1 of the product tree | Small | Low: stub the few call sites |
| Daemon / HTTP transports | `serve.rs`, `driver_service_http.rs`, `mcp_http.rs`, `proxy.rs`, `autostart.rs` | SDK uses stdio `mcp` only | Removes listening sockets and Scheduled Task code | Medium: `call`/`status`/`stop` CLI diagnostics depend on the daemon; keep `mcp`, `list-tools`, `describe`, `doctor` |
| Browser (CDP) tools | 9 `browser_*` tools + `page`, `tokio-tungstenite` | not called by the SDK; Mobao drives browsers through its own native host | Fewer tools in `tools/list` for raw callers, smaller binary | Medium: `cua-driver-core::browser` / `cdp.rs` are shared modules, needs a feature in core |
| Recording / replay | `start_recording`, `stop_recording`, `get_recording_state`, `replay_trajectory`, `install_ffmpeg` | not called; Mobao has its own recorder; `install_ffmpeg` already refused by the SDK raw filter | Removes ffmpeg download path | Low-medium |
| Sessions / history | `start/end/escalate/get/list_session`, `get_session_state`, history tools; `keyring`, `chacha20poly1305`, `coset` | history encryption (`cua-driver-core/src/history.rs`, `platform-windows/src/history.rs`) | Drops Credential Manager use and crypto crates | High: cursor sessions and per-call state may use the session store; verify `set_agent_cursor_enabled` first |
| Rarely needed input / window tools | `kill_app`, `set_window_frame`, `double_click`, `right_click`, `clipboard_read/write`, `get_cursor_position`, `move_cursor`, `set_agent_cursor_theme/motion`, `get_agent_cursor_state`, `check_permissions`, `health_report`, `get/set_config`, `get_accessibility_tree`, `parse_visual_regions` | not called by the SDK (`click` already carries count/button) | An earlier exploratory build of the same tag without most of these was 26.97 MB (not re-measured on this branch) | Low per tool, but each removal changes the public tool list; `kill_app` and clipboard removal also shrink the blast radius of raw calls |
| PiP preview | `pip-preview` crate and `platform-windows/src/pip` | Windows implementation is a compile-clean stub | Small | Low |
| Duplicate `windows` crate | 0.58 (core, platform-windows) and 0.61 (cua-driver) | product tree | Smaller binary and build time | Medium: API differences between the two versions |
| Release profile | fat LTO, `codegen-units = 1`, `panic = "abort"`, `strip = true` | an earlier LTO build was 29.5 MB | About 2 MB, no code change | Low: longer builds; `panic = "abort"` changes crash behaviour (tokio task panics) |
| CLI help | help still lists the refused verbs | `cua-driver --help` | Clearer product surface | Trivial |

Already absent from the Windows binary: platform-macos / platform-linux, regorus (Rego), cua-perception (not linked), cua-driver-uia (not shipped by the SDK).

