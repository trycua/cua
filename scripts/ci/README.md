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

### Canonical and supporting runners

This table lists every desktop runner and its evidence authority. Only a
canonical runner, run in full at the exact source SHA, certifies desktop
behavior. A scoped gate certifies only its own scope and does not replace
the three OS rows. Supporting runners help with diagnosis or convenience and
never certify.

| Runner | Workflow or environment | Authority |
| --- | --- | --- |
| `scripts/ci/linux/run-rust-e2e.sh` | `e2e-rust-linux.yml`, GitHub-hosted X11 | Canonical: Linux |
| `scripts/ci/windows/run-rust-e2e.ps1 -RequireGui` | `e2e-rust-windows.yml`, GitHub-hosted, when its strict desktop preflight passes | Canonical: Windows |
| `libs/cua-driver/tests/runners/macos-lume/run-all.sh` | Logged-in, TCC-authorized Lume worker; every complete run includes the standalone-browser matrix | Canonical: macOS |
| `scripts/ci/macos/run-rust-e2e.sh` | Called by the Lume wrapper and the hosted macOS script | Matrix implementation behind the macOS rows; refuses to run without the wrapper's unrestricted daemon |
| `scripts/ci/run-rust-standalone-browser-e2e.sh`, `scripts/ci/windows/run-rust-standalone-browser-e2e.ps1` | `e2e-rust-standalone-browsers.yml`; the macOS Lume wrapper runs it as well | Scoped gate: installed Chrome and Edge, required for browser-facing changes |
| `scripts/ci/linux/run-rust-e2e-wayland.sh`, `run-rust-e2e-inject.sh` | `e2e-rust-linux-wayland.yml` (`sway`, `sway-xwayland`, `cua-compositor`) | Scoped gate: native Wayland compositor lanes, kept separate from X11 |
| `run-rust-e2e.sh` in a native Hyprland desktop, plus the allowlisted `hyprland_*` rows | Maintainer's prepared native Hyprland desktop | Scoped gate: native Hyprland; no automated lane |
| `scripts/ci/macos/run-hosted-rust-e2e.sh` | `e2e-rust-macos.yml` with `mode=hosted` | Supporting: supplemental hosted macOS matrix; does not replace Lume |
| `scripts/ci/linux/run-rust-e2e-desktop.sh` | Representative GNOME or KDE maintainer desktop | Supporting: environment coverage |
| `libs/cua-driver/tests/runners/windows/run-all.ps1` | Local RDP or console session | Supporting: convenience wrapper that calls the canonical Windows runner; certify through the canonical row |
| `libs/cua-driver/tests/runners/windows-sandbox/` | Windows Sandbox | Supporting: legacy local smoke; never certifies |
| Azure RDP replay of the Windows runner | Maintainer Azure VM | Supporting: optional environment-parity replay, or a fallback when the hosted preflight cannot prove a capability |
| `scripts/ci/linux/run-valgrind-e2e.py` | `ci-cua-driver-valgrind.yml` | Supporting: memory-safety diagnostic |
| `scripts/ci/linux/preflight-rust-e2e.sh`, `scripts/ci/windows/preflight-rust-e2e.ps1` | `ci-cua-driver-preflight.yml` or a local desktop | Supporting: lightweight host readiness only |
| One-off app smokes and manual recordings | Any | Supporting: diagnostics only |

Nix source checks (`ci-nix-linux.yml`) are a separate build and unit gate,
not a desktop runner.

### Choose the evidence tier

Use the narrowest useful tier during implementation, then complete the required
platform evidence before delivery. The tiers describe evidence authority, not
new workflow gates or permission to omit the final matrix.

| Tier | Examples | What it establishes |
| --- | --- | --- |
| Fast | `CI: Cua Driver quick feedback (non-certifying)`, focused local tests | Formatting and shared contracts; no native desktop behavior. |
| Platform | `E2E: Rust Linux interactive`, `E2E: Rust Windows interactive`, the logged-in macOS Lume runner | Complete canonical desktop behavior at an exact source SHA when all required lanes and strict preflight pass. |
| Live | Protected Jev evidence workflows | Bounded provider behavior with exact signed candidate and platform prerequisites; not a replacement for the canonical matrix. |
| Release-evidence | Signed candidate, certification records, private evidence validation | Artifact provenance and final review material; a recording or upload alone is not a passing desktop test. |

For a change after a certified candidate, compare the tested commit with the
proposed commit. The advisory classifier returns potentially affected desktop
platforms and reasons as JSON:

```bash
python3 .github/scripts/cua_driver_e2e_impact.py TESTED_SHA CANDIDATE_SHA
```

Pass two full committed SHAs from this checkout; the command does not read a
dirty worktree or start a test. It includes both sides of a rename. Explicit
documentation and non-certifying diagnostic tooling can return an empty
`affected_platforms` list. Platform implementation, harness, and workflow
changes name the affected OS; shared, unknown, or ambiguous files return all
three. Check the actual diff and the repository's certification timing rule
before reusing any evidence. An empty list is advice about *new changes since
the tested SHA*, not a certification result or a waiver of the complete stable
candidate matrix. Keep the earlier exact-SHA result and account for each
subsequent change in the PR review record.

### Automatic release gate

Stable Cua Driver releases need no manual E2E or publish dispatch. The
`cua-driver-rs-v*` tag that Release Please creates starts
`.github/workflows/cd-rust-cua-driver.yml`, which calls the Linux, Windows,
hosted macOS, and standalone-browser E2E workflows through `workflow_call`
against the exact tag SHA. The draft release is published only when those
suites, the builds, and the release artifact checks pass. Nightly builds skip
this gate. The logged-in macOS Lume matrix is not part of the automatic gate;
it remains pre-merge evidence. Manual dispatch of the same workflows stays the
way to certify a pull request candidate.

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
when it fans the same matrix into independent jobs. The hosted macOS wrapper
also accepts `browser`, which it routes to the standalone browser suite instead
of the repo-local matrix. Those values are not public alternate suites.

The complete harness result at the exact source SHA is the behavioral gate.
Manual app smokes, standalone videos, legacy runners, and environment-parity
replays are useful diagnostics but cannot replace it. When installed-browser
behavior is in scope, run the standalone browser suite in addition to the
complete repo-local matrix.

The maintainer-facing macOS command is
`libs/cua-driver/tests/runners/macos-lume/run-all.sh`. It verifies the private
Lume seed, installs the exact committed source, and then delegates to the thin
`macos/run-rust-e2e.sh` matrix runner above. It then always runs the installed
Chrome/Edge browser matrix after the canonical repo-local harness matrix.

Run the canonical logged-in Lume gate directly from Terminal in the disposable
guest; do not install or register a GitHub Actions runner in that guest. After a
successful `run-all.sh` invocation, bundle the private
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

After that prerequisite passes, four fresh hosted runners execute the shared,
native, capture, and browser lanes through `macos/run-hosted-rust-e2e.sh`. Each
runner refuses unexpected hosts or pre-existing app state, creates a temporary
certificate-backed identity and Keychain, installs the exact source as
`CuaDriverLocal.app`, seeds only its Accessibility and Screen Capture TCC rows,
records the app's separate `replayd` approval before its first direct capture,
verifies the daemon-attributed permission result, and delegates to
`macos/run-rust-e2e.sh`. The browser lane instead runs
`run-rust-standalone-browser-e2e.sh` against the same unrestricted installed
daemon for the image's Google Chrome and Microsoft Edge, writing evidence to
`artifacts/cua-driver/macos-standalone-browser/` like the Lume gate. It fails
before bootstrap when either browser is missing or fails the driver's vendor
code-signing requirement, and it never shrinks the product set. The image's Edge
carries a stray `com.apple.FinderInfo` attribute that strict verification
rejects; the lane removes only that unsigned attribute and records each path. The `certify` job requires every lane. GitHub's image-level approval covers the hosted runner
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

## Cua Driver release safety

The canonical installers (`https://cua.ai/driver/install.sh` and `install.ps1`) install the version baked into them on `main`. If that version is broken, every fresh install breaks. This happened with 0.28.3: its macOS app was published unsigned, and the installer correctly refused it (#4109).

A stable release has exactly one path to users, and nobody dispatches anything along it:

1. Merging the Release Please pull request creates the `cua-driver-rs-v*` tag and a draft release.
2. The tag push runs `.github/workflows/cd-rust-cua-driver.yml`. It builds every artifact, runs the Linux, Windows, hosted macOS, and standalone-browser E2E gates against the tag SHA, and verifies the candidate signatures.
3. `release` publishes the draft only after all of those pass.
4. The published assets are verified again. Only then does `advance-installer-version` bake the new version into the installers on `main`.

These gates stop a broken release from being published or baked:

| Gate | Where | Blocks |
| --- | --- | --- |
| Notarization is required to publish | the first preflight step, the macOS build, and the `release` job | any tag build that has macOS notarization disabled |
| Candidate signatures | `verify-macos-release-signatures` and `verify-windows-release-signatures`, both required by `release` alongside the E2E gates | uploading archives whose `CuaDriver.app` is not Developer ID signed by `YCK386LBJ7`, not accepted as `Notarized Developer ID`, or not stapled, or whose Windows binaries lack a valid, timestamped Authenticode signature from Cua AI, Inc. |
| Published-release verification | `verify-published-signatures` and `verify-published-installers`, both required by `advance-installer-version` | baking a version whose public assets fail the same signature checks, or that the canonical installers cannot install on macOS, Linux, or Windows |
| Withdrawn versions | `.github/release-state/cua-driver-rs-withdrawn-versions` | baking, certifying, or installing a listed version |
| Installer canary | `.github/workflows/monitor-branded-installers.yml` | nothing, but it opens or updates a `bug` issue within six hours when a default install fails |

Every gate is a required `needs:` with no `always()` bypass. A failed gate therefore leaves the draft unpublished, or the installers on the previous version. To recover a flaky gate, re-run the failed jobs of the tag run. That re-runs the push event; it is not a publish dispatch. Don't publish, upload, or edit release assets by hand to work around a failed gate. Fix the cause and ship a new release.

### Withdrawn versions

The withdrawn list has one `x.y.z # reason` entry per line. The installers can't read repository files when they run through `curl | bash` or `irm | iex`, so each one carries a copy of the list:
- `CUA_DRIVER_RS_WITHDRAWN_VERSIONS` in `libs/cua-driver/scripts/_install-rust.sh`
- `$Script:CuaDriverRsWithdrawnVersions` in `libs/cua-driver/scripts/install.ps1`

`validate_release_versions.py` fails if either copy differs from the file, or if the baked version is withdrawn.

The installers treat a withdrawn version this way:
- **Pin:** an explicit pin is refused.
- **API resolution:** withdrawn versions are skipped.
- **Stale baked value:** an installer copy that still bakes a withdrawn version prints a warning and resolves the newest eligible release instead.

A macOS signature failure always fails closed. The installer never downgrades to another release on its own.

To check a published release on a Mac:

```bash
gh release download cua-driver-rs-v0.28.2 --repo trycua/cua --dir /tmp/cua-release \
  --pattern 'cua-driver-rs-0.28.2-darwin-*.tar.gz'
python3 .github/scripts/verify_cua_driver_release_signatures.py macos \
  --artifacts /tmp/cua-release --version 0.28.2
```

To check Windows, run the `windows` subcommand on a Windows machine.

### Runbook: roll the installers back to the last good release

Use this runbook when the canary issue opens, or when users report that default installs fail. #4150 is the worked example.

1. **Confirm the failure.** Read the canary run. Then run the verifier above against the baked version (`.github/release-state/cua-driver-rs-published-version`).
2. **Choose the rollback version.** Pick the newest earlier release that passes the verifier on macOS and Windows and whose installer-compatibility run passes. List assets with `gh release view cua-driver-rs-v<version> --json assets`.
3. **Open a `fix(cua-driver): ...` pull request** that makes these changes:
   - Add the bad version with its reason and issue link to `.github/release-state/cua-driver-rs-withdrawn-versions`, and to both installer copies of the list.
   - Set `CUA_DRIVER_RS_BAKED_VERSION` in `_install-rust.sh`, `$Script:CuaDriverRsBakedVersion` in `install.ps1`, and `.github/release-state/cua-driver-rs-published-version` to the rollback version. The three must agree.
   - Run `python3 .github/scripts/validate_release_versions.py --product driver` and `python3 -m pytest libs/cua-driver/scripts/tests/test_install_version_fallback.py`.
4. **Merge it.** The branded endpoints serve `main`. The canary runs on the push, so compare its installed version with the rollback version. To recheck the public one-liners after the endpoints refresh, dispatch `Monitor branded installer endpoints`.
5. **Ship the fix as a new Release Please release.** Its tag run bakes the installers forward automatically once the published assets pass verification. Leave the withdrawn release and its assets in place for audit.
