# Cua-driver CI runners

These scripts are thin entrypoints around the Rust integration tests. They
build the repo-local fixture applications, run one strict Rust environment
preflight, set testkit paths, execute Rust targets, invoke the Rust report
validator, and collect artifacts. They do not define behavioral rows, push
code, or alter branches. The shared lane runs both the cross-platform action
catalog and the embedded-browser exact-or-refused catalog.

Standalone Chrome/Edge adversarial coverage is a separate optional real-browser
suite because it requires an installed external browser. Run it explicitly:

```bash
# macOS and Linux
scripts/ci/run-rust-standalone-browser-e2e.sh

# Windows PowerShell in a console/RDP user session
scripts/ci/windows/run-rust-standalone-browser-e2e.ps1
```

The runners require an existing desktop user session and a fresh artifact
directory. They stage the repo-owned Electron foreground sentinel when the
fixture is absent and execute each scenario in an independent Cargo process.
In a pure native Wayland session, the Unix runner also enables the driver's
native Wayland backend and launches the sentinel through Wayland, so the result
cannot silently fall back to X11. To exercise the same matrix in the repo-owned
native Sway session, run:

```bash
CUA_E2E_WAYLAND_RUNNER="$PWD/scripts/ci/run-rust-standalone-browser-e2e.sh" \
  scripts/ci/linux/run-rust-e2e-wayland.sh
```

When a Nix-store Chromium is used on a non-NixOS VM, its SUID sandbox helper
cannot carry the required root ownership. Set `CUA_E2E_BROWSER_NO_SANDBOX=1`
only for that isolated test VM; ordinary installed-browser runs keep Chromium's
sandbox enabled.

Release validation sets `CUA_TEST_REQUIRE_EXTERNAL_BROWSERS=1` so a missing
browser fails instead of silently omitting the suite.

By default, Linux preserves the compatibility lane's historical product
selection: Chrome when installed, otherwise Chromium, plus Edge when present.
Maintainer certification runs can request an exact product set:

```bash
CUA_E2E_BROWSER_PRODUCTS=chrome,chromium,edge \
  scripts/ci/run-rust-standalone-browser-e2e.sh
```

Every named product is mandatory. An unknown name, duplicate name, or missing
executable fails before a behavioral row runs, so the matrix cannot silently
shrink. Use `CUA_E2E_BROWSER_BIN` with `CUA_E2E_BROWSER_NAME` only for a
single-product diagnostic run. Every launched product also appends its
CDP-reported product, version, protocol version, user agent, and exact source
SHA to `browser-provenance.jsonl` beside the matrix report.

For the test layout and the distinction between unit tests, shared harnesses,
and native harnesses, see
`libs/cua-driver/docs/test-harnesses-guide.md`.

| Runner | Session | Canonical command |
| --- | --- | --- |
| `linux/run-rust-e2e.sh` | Existing Linux X11 or Wayland desktop | no selector |
| `linux/run-rust-e2e-wayland.sh` | Headless native Sway session | no selector |
| `linux/run-rust-e2e-inject.sh` | Nested `cua-compositor` session | no selector |
| `linux/run-rust-e2e-desktop.sh` | Existing representative Linux desktop | no selector |
| `windows/run-rust-e2e.ps1` | Windows console/RDP user session | `-RequireGui` |
| `macos/run-rust-e2e.sh` | Logged-in macOS session already prepared by the maintainer wrapper | no selector |

Use the command without a selector for the canonical complete run. CI sets the
private `CUA_E2E_INTERNAL_LANE` partition to `shared`, `native`, or `capture`
when it fans the same matrix into independent jobs. Those values are not public
alternate suites.

The maintainer-facing macOS command is
`libs/cua-driver/tests/runners/macos-lume/run-all.sh`. It verifies the private
Lume seed, installs the exact committed source, and then delegates to the thin
`macos/run-rust-e2e.sh` matrix runner above. There is no GitHub-hosted macOS GUI
job. Pass `--standalone-browser` to run the optional installed Chrome/Edge
browser matrix after the canonical repo-local harness matrix.

Run the Wayland wrapper through `nix develop .#cua-driver-wayland-e2e`. It
creates a pure Wayland session with Xwayland disabled and delegates every
scenario to `run-rust-e2e.sh`.

Run the nested compositor wrapper through
`nix develop .#cua-driver-inject-e2e`. This environment is experimental and
proves only the private compositor-owned route. Use `run-rust-e2e-desktop.sh`
for maintainer checks on representative GNOME, KDE, or real-Xorg sessions.

The GitHub-hosted Windows workflow is canonical when its strict preflight proves
an interactive desktop. The workflow also accepts a runner label so maintainers
can replay the same command on an Azure VM with an active RDP session for
environment parity; that replay is not a separate test definition or source of
behavioral truth.
