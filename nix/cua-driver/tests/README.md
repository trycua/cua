# Linux Nix test layout

Nix expressions are grouped by what they prove:

| Path | Role |
| --- | --- |
| `rust-unit.nix` | Source-built Rust workspace checks without a desktop session |
| `integration.nix` | Driver service starts and answers protocol requests |
| `set-config.nix` | Configuration persistence contract |
| `screenshot.nix` | Capture contract in a managed display |
| `wayland/` | Compositor-specific supporting and red/green checks |
| `linux-background-gui.nix` | Legacy toolkit matrix, supporting coverage only |
| `linux-*-gif.nix` | Legacy visual diagnostics, never the canonical Rust matrix |

New user-behavior coverage belongs in the Rust test harness first. A Nix check
may provide the session and package environment, but it should invoke the
shared Rust scenario or clearly identify itself as supporting coverage.
