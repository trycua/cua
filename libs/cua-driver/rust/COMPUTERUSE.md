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
