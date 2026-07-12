# Linux driver checks

Nix has two jobs in the cua-driver test stack:

| Attribute | Purpose | CI |
| --- | --- | --- |
| `cua-driver-build` | Build the shipped Linux package from the locked Rust source | `ci-nix-linux.yml` |
| `cua-driver-linux-rust-unit` | Compile and run the source-owned headless Rust tests | `ci-nix-linux.yml` |
| `cua-driver-wayland-e2e` | Provide the Sway, GTK, WebKit, Electron, capture, and Rust toolchain used by native Wayland E2E | `e2e-rust-linux-wayland.yml` |
| `cua-driver-inject-e2e` | Provide the same typed harness toolchain plus the nested `cua-compositor` package | Experimental nested-injection workflow |
| `cua-compositor-build` | Build the optional compositor-owned injection backend against pinned wlroots | Flake check |

The Rust tests own all protocol and desktop behavior. The old NixOS Python
clients, GIF scenarios, real-app smoke rows, and compositor matrix were removed
after equivalent or stronger checks moved into the repo-owned Rust harnesses.
This avoids a second scenario catalog with different assertions.

Run the native Wayland matrix from the repository root:

```bash
nix develop .#cua-driver-wayland-e2e -c \
  scripts/ci/linux/run-rust-e2e-wayland.sh
```

The wrapper starts a pure Wayland Sway session with Xwayland disabled, then
calls the same canonical Rust runner used by X11. Results use the common typed
JSONL schema and retain MP4 trajectories under `artifacts/cua-driver/linux/`.
