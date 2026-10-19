## Depend on a crate

Depend on them from git, pinned to a release tag or a commit:

```toml test="config" id="cargo-dependency"
[dependencies]
cua-sdk = { git = "https://github.com/trycua/cua", tag = "cua-sdk-v{{version}}" }
tokio = { version = "1", features = ["full"] }
```

Inside this repository, use a path dependency (`path = "libs/cua/crates/cua-sdk"`). The crates build with the toolchain in `libs/cua/rust-toolchain.toml`.

## Stability

- All crates release in lockstep with the SDK version under one `cua-sdk-v<version>` tag. Mix crates from one tag only.
- Stable: `cua-sdk` and the `cua-spacesd-client` client. Before 1.0 a minor release may break them, with a deprecated alias kept for at least one minor release where possible (for example `EnvClient`, now `SpacesdClient`). Patch releases never do.
- Unstable: `cua-fleet`, `cua-sandbox-core` and `cua-spaces` follow the Fleet and Spaces services and can change in any minor release. Items named `testing`, `conformance` or `coverage` are test support, not API.
- Not API: items hidden from these pages (`#[doc(hidden)]`), the UniFFI scaffolding (`uniffi_*`, `ffi_*`), and cargo features other than the defaults.

Other workspace crates (`cua-proto`, `cua-daemon`, `cua-vmm`, `cua-image`, the media crates, `cua-cli`) are implementation details. The Cua Spaces crates (`cua-spaces-ext`, `cua-spaces-ffi`, `cua-spaces-cli`, `cua-spaces-app-core`, `cua-teleport`, `cua-keyvault`, `cua-volume`) are source-available under FSL-1.1-MIT and not part of the SDK; the Swift API of the app export is on the [Cua Spaces app export pages](/cua-sdk/reference/spaces/app-core). the wire contract is the [protocol reference](/cua-sdk/reference/protocol). Each page shows item summaries and declarations; `cargo doc --open` in `libs/cua` has the full comments.
