# Embedded default cursor theme

`cua.default.lottie` is the canonical source archive for Cua Driver's built-in
cursor. `cua.default.cua-theme` is the bounded runtime artifact compiled from
that archive.

The privileged overlay embeds and decodes only the `.cua-theme` artifact. It
never parses dotLottie ZIP files or Lottie JSON at runtime. The artifact
contains bounded vector geometry, paints, transforms, and sampled animation
frames rather than a fixed-resolution pixel atlas. Skia rasterizes those
commands at the live display backing scale.

Regenerate both files from the Rust workspace:

```bash
python3 crates/cursor-overlay/assets/build_default_theme.py
cargo run -p cursor-theme-cli -- build \
  crates/cursor-overlay/assets/cua.default.lottie \
  --output crates/cursor-overlay/assets/cua.default.cua-theme
cargo run -p cursor-theme-cli -- inspect \
  crates/cursor-overlay/assets/cua.default.cua-theme
```

The Python generator uses only the standard library and writes deterministic
ZIP metadata and entry ordering. A Rust test verifies that the source hash
inside the compiled artifact matches the checked-in `.lottie` bytes.

The source uses Cua blue as a palette key and white for outlines. At runtime,
only the embedded default is recolored to the stable session fill. Installed
custom themes retain their authored colors. The shared floating motion is
applied to the selected action animation. Delivery and target context is
painted by the host-owned session badge and is not part of theme artifacts.
