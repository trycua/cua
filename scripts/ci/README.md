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
In a Wayland session, the Unix runner also enables the driver's native Wayland
backend and launches the sentinel and test browsers through Wayland, so GNOME
and KDE cannot silently fall back to X11 merely because they retain `DISPLAY`
for XWayland. To exercise the same matrix in the repo-owned native Sway session,
run:

```bash
CUA_E2E_WAYLAND_RUNNER="$PWD/scripts/ci/run-rust-standalone-browser-e2e.sh" \
  scripts/ci/linux/run-rust-e2e-wayland.sh
```

To prove the fail-closed boundary without a compositor-specific identity
adapter, run the same browser runner in a native Wayland session with
`CUA_E2E_WAYLAND_SESSION=generic` on the Sway worker. That environment selects
only the real-browser generic-Wayland refusal row. The harness retains
`SWAYSOCK` as an out-of-band focus and z-order oracle, while the spawned Cua
Driver daemon receives an intentionally unusable socket path and cannot use
Sway identity.
Ordinary attachment rows are not applicable because the lane deliberately
withholds exact compositor identity from the product under test.

When a Nix-store Chromium is used on a non-NixOS VM, its SUID sandbox helper
cannot carry the required root ownership. Set `CUA_E2E_BROWSER_NO_SANDBOX=1`
only for that isolated test VM; ordinary installed-browser runs keep Chromium's
sandbox enabled.

Snap-confined Chromium is not a portable Xvfb certification target. Its launch
can depend on a real login-session cgroup in addition to X11 authentication,
so an SSH-created Xvfb session may reject the browser before its CDP endpoint
starts. Use a distribution-native browser package for X11 certification, or
run the snap in its real logged-in desktop session. Treat this as a runner
preflight failure, not a Cua Driver behavioral result.

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
On Linux the runner compiles `cua-driver` with `portal-input`, matching the
published artifact so representative GNOME/KDE runs exercise the libei
fallback instead of a default-feature `wtype` refusal.

For the test layout and the distinction between unit tests, shared harnesses,
and native harnesses, see
`libs/cua-driver/docs/test-harnesses-guide.md`.

## Native pacman update test

The `Arch native pacman updates` job in `ci-rust-linux.yml` checks package
ownership with real pacman in a disposable Arch container. It packages the
candidate under `/usr/lib/cua-driver-pacman-test`, verifies CLI and MCP update
guidance through the installed binary and a symlink, and checks that an
unmanaged copy retains vendor updates and channel selection. A fake `pacman`
on `PATH` must not change either result. The job removes only its fixture
package and retains source, binary, toolchain, and test evidence.

To reproduce it, build `cua-driver` and `release_channel_cli_test` with
`--locked --features portal-input` in a disposable Arch guest/container. Run
as root with Xvfb and a session bus:

```bash
CUA_E2E_SOURCE_SHA=FULL_COMMIT_SHA CUA_E2E_UNRESTRICTED_GUI=1 \
  xvfb-run -a dbus-run-session -- env CUA_PACMAN_TEST_DISPOSABLE=1 \
  bash scripts/ci/linux/test-pacman-updates.sh \
    CANDIDATE_BINARY INTEGRATION_TEST_BINARY NEW_EVIDENCE_DIRECTORY
```

The runner rejects existing fixture packages and payload paths. Do not run it
on a user's host. This is package-update validation, not a Hyprland or Omarchy
desktop certification, and it does not replace the desktop matrix.

## Desktop runners

### Quick development feedback

`CI: Cua Driver quick feedback (non-certifying)` runs on relevant pull requests
or by manual dispatch. It checks Rust formatting, the shared web fixture journal,
the visual contract and perception protocol, and platform-independent core
unit tests. Use it while iterating on those areas. It does not create a
desktop session, exercise native input or capture, install a release, or issue
a certification artifact. Its goal is a warm-cache run under 10 minutes;
check the workflow's measured duration rather than treating the timeout as a
performance guarantee.

`CI: Cua Driver desktop readiness (non-certifying)` runs the lightweight
Linux and Windows checks in temporary GUI sessions when the preflight scripts
change. It does not invoke the strict preflight or any behavior matrix.

For a local focused iteration, run the matching commands from the repository
root (a first compilation may be much slower than a warm run):

```bash
cargo fmt --manifest-path libs/cua-driver/rust/Cargo.toml --all -- --check
node --test libs/cua-driver/tests/fixtures/shared/web/journal.test.cjs
cd libs/cua-driver/rust
cargo test --locked -p cua-driver-contract --test visual_contract
cargo test --locked -p cua-perception --test protocol
cargo test --locked -p cua-driver-core --lib
```

Before spending time on a desktop matrix, run the lightweight host readiness
check in the same logged-in desktop session that will run the test:

```bash
# Linux, from the repository root
scripts/ci/linux/preflight-rust-e2e.sh
```

```powershell
# Windows, from the repository root
.\scripts\ci\windows\preflight-rust-e2e.ps1
```

These commands report the architecture, display or input desktop, session bus
where applicable, recording tools, checkout SHA, and an already-built driver
version (not its source identity). They do not build fixtures, request
permissions, start the Driver, or write or clear E2E artifacts. A missing
binary or optional browser does not fail the lightweight check. The commands
cannot establish AX/UIA, capture, permission, video, fixture, or browser behavior.
The canonical runner's strict environment preflight still proves those before
any behavioral rows.
If `CUA_E2E_SOURCE_SHA` is set, the lightweight check fails when it differs
from the checked-out SHA. For a hosted Linux X11 lane, enter its `xvfb-run` and
`dbus-run-session` environment before running this check.

For desktop behavior, use a diagnostic lane only to narrow a failure. The
complete Linux, Windows, and macOS runs at the stable exact candidate SHA
remain the certification gate described below and in the test harnesses guide.
The Linux and Windows E2E workflows cache Rust dependencies/build products
per OS and lane, and cache npm downloads for the Electron fixture on exact-ref
dispatches. A dispatch against a different reviewed SHA may restore a Rust
cache but cannot save it. Exact-ref branch dispatches save only in that branch's
GitHub cache scope; Windows RDP parity replays cannot save a cache. No fixture `node_modules`,
recording, result, or certification artifact is cached.

To measure the change, compare the wall time of each job and its build/test
step in successive cold- and warm-cache full-matrix runs at a stable candidate.
Record queue, setup, execution, and upload separately. As a pre-cache baseline,
the exact-merge-SHA September 22, 2026 runs (`35789003557` Linux,
`35789006039` Windows) took 25m31s for Linux shared and 40m52s for Windows
shared (job start to completion). The corresponding behavior-matrix steps took
23m20s and 39m41s. The other lanes ran in parallel; these times are not a
median or evidence of a 30% improvement. Compare multiple warm runs before
claiming a sustained reduction, and retain the source SHA and cache-hit state
with each measurement.

| Runner                          | Session                                                            | Canonical command |
| ------------------------------- | ------------------------------------------------------------------ | ----------------- |
| `linux/run-rust-e2e.sh`         | Existing Linux X11 or Wayland desktop                              | no selector       |
| `linux/run-rust-e2e-wayland.sh` | Headless native Sway session                                       | no selector       |
| `linux/run-rust-e2e-inject.sh`  | Nested `cua-compositor` session                                    | no selector       |
| `linux/run-rust-e2e-desktop.sh` | Existing representative Linux desktop                              | no selector       |
| `windows/run-rust-e2e.ps1`      | Windows console/RDP user session                                   | `-RequireGui`     |
| `macos/run-rust-e2e.sh`         | Logged-in macOS session already prepared by the maintainer wrapper | no selector       |

Use the command without a selector for the canonical complete run. CI sets the
private `CUA_E2E_INTERNAL_LANE` partition to `shared`, `native`, or `capture`
when it fans the same matrix into independent jobs. Those values are not public
alternate suites.

The complete harness result at the exact source SHA is the behavioral gate.
Manual app smokes, standalone videos, legacy runners, and environment-parity
replays are useful diagnostics but cannot replace it. When installed-browser
behavior is in scope, run the standalone browser suite in addition to the
complete repo-local matrix.

The maintainer-facing macOS command is
`libs/cua-driver/tests/runners/macos-lume/run-all.sh`. It verifies the private
Lume seed, installs the exact committed source, and then delegates to the thin
`macos/run-rust-e2e.sh` matrix runner above. Pass `--standalone-browser` to run
the optional installed Chrome/Edge browser matrix after the canonical repo-local
harness matrix.

Run the canonical logged-in Lume gate directly from Terminal in the disposable
guest; do not install or register a GitHub Actions runner in that guest. After a
successful `run-all.sh --standalone-browser` invocation, bundle the private
artifact directories, calculate their SHA-256 digest, and dispatch
`.github/workflows/e2e-rust-macos.yml` in `lume` mode at the exact candidate SHA
with the harness run ID and digest. That protected `ubuntu-latest` job only
registers the direct result as a machine-readable certification artifact. It
does not execute macOS code or receive the live Jev credential. The separate
protected evidence workflow verifies that exact-SHA registration before exposing
its bounded credential.

The manual `.github/workflows/e2e-rust-macos.yml` workflow first probes a fresh
GitHub-hosted macOS 26 runner. It records the image, SIP state, desktop session,
display geometry, and OCR-verified TextEdit window and display captures for one
exact source SHA. Probe permission checks describe only the temporary probe
process. Dispatch only a reviewed commit SHA; the selected source is executable
test code and the bootstrap uses the hosted runner's passwordless sudo policy.

After that prerequisite passes, three fresh hosted runners execute the shared,
native, and capture partitions through `macos/run-hosted-rust-e2e.sh`. Each
runner refuses unexpected hosts or pre-existing app state, creates a temporary
certificate-backed identity and Keychain, installs the exact source as
`CuaDriverLocal.app`, seeds only its Accessibility and Screen Capture TCC rows,
records the app's separate `replayd` approval before its first direct capture,
verifies the daemon-attributed permission result, and delegates to
`macos/run-rust-e2e.sh`. GitHub's image-level approval covers the hosted runner
agent, while the bundled driver is its own responsible ScreenCaptureKit client
and would otherwise show the private-window-picker reminder over the headed
test. The lane uploads bootstrap, structured result, log, and video evidence
even on failure. Signing and trust operations have hard
deadlines, and a sacrificial binary proves the temporary identity works before
the release build begins. The certificate is trusted only on that ephemeral
runner and its removal is attempted with a bounded cleanup. The lane is
supplemental while the hosted image and temporary identity differ from the
release-parity Lume seed; it does not replace the Lume gate. Hosted jobs omit the
Lume-only encrypted history gate and `--experimental-history`; the desktop
behavior matrix does not depend on them.

Run the Wayland wrapper through `nix develop .#cua-driver-wayland-e2e`. It
creates a pure Wayland session with Xwayland disabled and delegates every
scenario to `run-rust-e2e.sh`.

The manually dispatched `sway-xwayland` capture lane uses the same wrapper with
XWayland forced on. Its capture contract places a real repo-owned GTK3 X11
fixture on inactive workspace 98, keeps an unrelated Wayland sentinel on the
active output, and requires `get_window_state` to return
`surface_identity_unproven` without screenshot bytes or a file. Sway IPC and
the fixture accessibility state are external focus, workspace, and mutation
oracles for that targeted regression.

Run the nested compositor wrapper through
`nix develop .#cua-driver-inject-e2e`. This environment is experimental and
proves only the private compositor-owned route. Use `run-rust-e2e-desktop.sh`
for maintainer checks on representative GNOME, KDE, or real-Xorg sessions.

The GitHub-hosted Windows workflow is canonical when its strict preflight proves
an interactive desktop. The workflow also accepts a runner label so maintainers
can replay the same command on an Azure VM with an active RDP session for
environment parity; that replay is not a separate test definition or source of
behavioral truth.
