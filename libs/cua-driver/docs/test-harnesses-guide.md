# CUA Driver Test Harnesses Guide

This document explains how the CUA Driver tests are organized, what each layer
proves, and what still needs to be finished. It is written as a starting point
for contributors who are unfamiliar with the repository.

## The Short Version

There are two main test layers:

1. **Unit and protocol tests** exercise Rust code and the public MCP/CLI
   contract without launching a real application.
2. **Harness E2E tests** launch a small application built from this repository,
   drive it through the Rust driver, and verify an external application or
   desktop state.

The Rust tests are the source of truth. Python tests, old shell runners, and
historical recording scripts are not part of the canonical E2E path.

The canonical E2E command on every OS runs the complete matrix and takes no
suite selector:

```text
Linux:  scripts/ci/linux/run-rust-e2e.sh
Windows: .\scripts\ci\windows\run-rust-e2e.ps1 -RequireGui
macOS:  scripts/ci/macos/run-rust-e2e.sh
```

The OS workflow may fan the complete matrix out into independent jobs for
reporting and failure isolation. That is an execution detail; contributors
should think of it as one canonical suite.

## Repository Map

```text
cua/
|-- libs/cua-driver/
|   |-- rust/
|   |   |-- crates/
|   |   |   |-- cua-driver/          Rust driver and integration tests
|   |   |   |-- cua-driver-core/     Shared driver logic and unit tests
|   |   |   |-- cua-driver-testkit/  Shared Rust E2E helpers and evidence capture
|   |   |   |-- platform-linux/      Linux backend
|   |   |   |-- platform-macos/      macOS backend
|   |   |   |-- platform-windows/    Windows backend
|   |   |   `-- cursor-overlay/      Cursor evidence helper
|   |   |-- test-apps/               Ignored staged harness binaries
|   |   `-- Cargo.toml               Rust workspace
|   |-- tests/
|   |   |-- fixtures/
|   |   |   |-- shared/web/           Shared web page and external markers
|   |   |   |-- apps/                 Repo-local fixture sources
|   |   |   `-- build/                macOS/Linux/Windows fixture builders
|   |   `-- runners/                  Auxiliary VM and sandbox entrypoints
|   `-- docs/                         Test matrix, reporting, and contributor docs
`-- scripts/ci/
    |-- linux/run-rust-e2e.sh         Linux canonical runner
    |-- windows/run-rust-e2e.ps1      Windows canonical runner
    `-- macos/run-rust-e2e.sh         macOS canonical runner
```

The important separation is:

| Layer | Owns | Does not own |
| --- | --- | --- |
| Rust integration test | Scenarios, driver calls, assertions, action metadata | OS setup and fixture compilation |
| `cua-driver-testkit` | Session helpers, fixture launching, screenshots, recordings, trajectories | The scenario list |
| Fixture app | Visible controls and externally observable state markers | Driver correctness assertions |
| OS runner | Build environment, user session, test selection, artifact collection | Test behavior definitions |

There is deliberately no second Python E2E implementation that the Rust suite
has to mirror.

## How One E2E Test Works

Every canonical E2E cell follows this shape:

```text
OS runner
  -> builds the Rust driver and repo-local fixture
  -> starts or connects to a real desktop session
  -> Rust testkit starts one harness application
  -> Rust test discovers the target window
  -> get_window_state provides accessibility tree and screenshot
  -> action addresses a target by AX element index or PX coordinates
  -> delivery is foreground or background where the action supports it
  -> fixture state, focus, pixels, or protocol response is checked
  -> testkit writes video, screenshots, trajectory, logs, and result data
```

A tool returning `ok` is not enough to pass an E2E cell. The fixture must show
that the action happened, or the test must verify a documented structured
refusal and the absence of focus or input side effects.

## The Test Layers

### Unit and Protocol Tests

These run without a repo-local GUI application and normally run without
`--ignored`:

| Location or prefix | What it proves |
| --- | --- |
| `rust/crates/*/src/**` | Core driver, platform-independent logic, schemas, and helpers |
| `protocol_*_test.rs` | MCP handshake, tool calls, sessions, media, and errors |
| `schema_*_test.rs` | Shared schema and backend consistency |
| `transport_config_persistence_test.rs` | CLI/MCP configuration persistence |
| `protocol_element_token_test.rs` | Element-token protocol behavior |

These tests should be fast, deterministic, and safe to run on ordinary CI
workers. They do not prove that a real click, key, scroll, or background input
reached an application.

### Harness E2E Tests

These are Rust integration tests under:

```text
libs/cua-driver/rust/crates/cua-driver/tests/
```

Most are marked `#[ignore]` because they require a desktop, built fixtures, and
platform permissions. They are selected by the OS runner rather than the
ordinary unit command.

The major families are:

| Family | Purpose |
| --- | --- |
| Shared app | Same web behavior tested through Electron and Tauri |
| Native harness | Toolkit-specific controls and native window behavior |
| Web integration | WebView/CDP/page-tool plumbing |

Delivery is not a test family. It is a dimension on each action row: an action
is tested in foreground and background modes whenever the driver and OS support
both. Capture and desktop scope are separate environment checks, while focus
preservation is a cross-cutting oracle that can be attached to any action row.
Focus, z-order, cursor, and desktop-state checks are cross-cutting invariants,
not a separate family. These names describe responsibilities, not separate
sources of truth or commands to run instead of the selector-free canonical
invocation.

## What The Complete Run Includes

### Windows

Runner: `scripts/ci/windows/run-rust-e2e.ps1`

| Runner area | Rust test | Real harness or app |
| --- | --- | --- |
| Shared app matrix | `cross_platform_behavior_test.rs` | Electron and Tauri |
| Native controls | `harness_wpf_test.rs` | Repo-local WPF app |
| Native controls | `harness_winui3_test.rs` | Repo-local WinUI3 app |
| Web integration | `harness_web_test.rs` | WebView2 and Electron |
| Capture contract | `capture_contract_test.rs` | WPF plus driver tree/image output |
| Launch contract | `launch_windows_test.rs` | Repo-local Electron launch and focus behavior |
| Agent cursor | `agent_cursor_windows_test.rs` | Source-built cursor overlay and pixel evidence |
| Desktop scope | `desktop_scope_windows_test.rs` | Windowless desktop input and scope rejection |

Cross-cutting instrumentation used by these rows includes the testkit
`DesktopObserver`, capture validation, cursor evidence, and desktop-scope
checks. These invariants are attached to their owning action rows rather than
run as a separate test family.

Windows is currently the broadest native matrix. It covers UIA controls, web
integration, background focus checks, and Windows-specific input routes. The
desktop observer is attached to shared and native action rows wherever
background delivery is tested.

### macOS

Runner: `scripts/ci/macos/run-rust-e2e.sh`

| Runner area | Rust test | Real harness or app |
| --- | --- | --- |
| Shared app matrix | `cross_platform_behavior_test.rs` | Electron and Tauri |
| Native controls | `harness_appkit_test.rs` | Repo-local AppKit app |
| Native controls | `harness_swiftui_test.rs` | Repo-local SwiftUI app |
| Capture contract | `capture_contract_test.rs` | Installed driver and macOS capture APIs |
| Desktop scope | `desktop_scope_macos_test.rs` | macOS window and desktop scope |

The WKWebView fixture exists, but it does not yet have a dedicated Rust E2E
target in the canonical runner. `installed_app_launch_macos_test.rs` and
`installed_app_textedit_macos_test.rs` are optional real-app checks for
Calculator/TextEdit and are not part of the canonical run.

### Linux

Runner: `scripts/ci/linux/run-rust-e2e.sh`

| Runner area | Rust test | Real harness or app |
| --- | --- | --- |
| Shared app matrix | `cross_platform_behavior_test.rs` | Electron and Tauri |
| Native controls | `harness_gtk3_test.rs` | Repo-local GTK3 app |
| Capture contract | `capture_contract_test.rs` | Linux capture backend |
| Desktop scope | `desktop_scope_linux_test.rs` | X11/Wayland desktop scope |

Linux has separate X11 and Wayland concerns. Nix supplies the reproducible
build and desktop environment, but the E2E test still needs an actual X11 or
Wayland session. Linux does not need GIF output; MP4, screenshots, accessibility
trees, trajectories, and logs are the useful evidence.

## AX, PX, and Delivery

These terms describe different dimensions:

| Term | Meaning |
| --- | --- |
| AX | Address a target through its accessibility/UI automation element |
| PX | Address a target by screen coordinates or pointer geometry |
| Foreground | The target may be brought to the foreground for delivery |
| Background | The target should receive the action without being raised or stealing focus |
| Window scope | Capture or action is limited to one target window |
| Desktop scope | Capture or action covers the full desktop |

The shared and native action matrices should test left click, right click,
double click, typing, keys, hotkeys, scroll, child windows, and drag across
AX/PX and foreground/background combinations where the driver supports them.
Unsupported background routes require an explicit refusal contract with an
allowed structured code and desktop-side-effect oracles. A refusal fails a
cell that requires delivery. There should not be a separate "delivery" family
whose only purpose is to repeat those same actions in the background.

Native harness rows use the same typed case/result contract as the shared
matrix. Current native `set_value` rows declare background delivery because
their contract includes no-focus and no-raise observations; actions without a
delivery concept use `not_applicable` explicitly.

## Cross-Cutting Invariants

The desktop observer is cross-cutting test instrumentation. It answers the same
question for any action, harness, or test family:

> Did the driver perform or reject the operation without disturbing the user's
> foreground application or desktop?

`cua-driver-testkit::DesktopObserver` owns the shared interface. Native Windows,
macOS, and Linux backends snapshot foreground-window, target z-order, cursor,
and leaked-input state before and after an action. Background rows opt into the
observer directly; there is no special guard suite.

| Invariant or scenario | What it checks |
| --- | --- |
| Background click/type/key | The target action does not move focus away from the user's foreground window |
| Minimized app launch | `launch_app(start_minimized=true)` does not raise the new app |
| Background hotkey | A keyboard chord does not steal focus |
| Child-window click | A target-created window does not unexpectedly become foreground |
| Background screenshot | Reading the target does not change focus or z-order |
| Agent cursor visibility | The cursor appears in the captured pixels when enabled and moved |

A focus assertion can prove "no focus steal" while failing to prove that a
click changed the target application state. An action row must therefore check
both the target's external state and, when background delivery is under test,
the cross-cutting desktop observer.

These tests require a real interactive Windows user desktop. They reject
Session 0, locked desktops, and disconnected RDP sessions. Without
`CUA_REQUIRE_GUI=1`, an unusable desktop can self-skip for local development;
the canonical Windows runner enables the hard-failure behavior.

## Evidence

Canonical GUI runs are expected to produce evidence per test cell:

```text
artifacts/cua-driver/<os>/
|-- recordings/<cell-label>-pid<pid>-<sequence>/recording.mp4
|-- recordings/<cell-label>-pid<pid>-<sequence>/trajectory.json
|-- cases.jsonl
|-- environment.jsonl
|-- results.jsonl
|-- summary.md
`-- <rust-target>.log
```

The GitHub Actions summary contains one row per meaningful behavioral cell,
including its OS, harness, action, AX/PX targeting, delivery mode, driver route,
expected and observed behavior, oracles, and one evidence link. The link uses
the exact video path as its label and opens the owning lane archive. Unit tests
need normal test output and logs; they do not need desktop video.

## What Is Implemented Today

- Rust owns the canonical scenario definitions and external-state assertions.
- Electron and Tauri use the same shared web fixture across supported OSs.
- Native Windows, macOS, and Linux harnesses are repo-local applications built
  from source.
- The three OS runners use a selector-free command for the complete matrix.
- Shared and native harness owners emit the same typed v2 result records.
- Canonical GUI rows collect a trajectory and MP4, validated before reporting.
- Strict lane preflights fail on missing fixtures, desktop access, permissions,
  accessibility, capture, or recording support instead of silently skipping.
- GitHub summaries link every evidence-bearing row to its lane archive and
  display the exact recording path. The trajectory path remains in the typed
  evidence and archive.
- Unit/protocol tests remain separate from interactive E2E tests.

## What Still Needs Implementation

The remaining work is platform coverage and validation, not another test
hierarchy:

1. **Complete macOS web coverage.** Add a dedicated Rust WKWebView E2E target
   if WKWebView remains a supported harness.
2. **Fresh OS validation.** Run the complete matrix on Windows, Linux
   X11/Wayland, and macOS, then classify actual failures as driver bugs,
   fixture bugs, environment failures, or expected refusals.
3. **Wayland validation.** Exercise the declared Wayland routes in a real
   compositor session; Xvfb proves only the X11 dimension.
4. **Flake cleanup.** Replace remaining fixed native waits with external-state
   polling and add fixture reset tokens before reusing a harness process.

## File Convergence Plan

The goal is not to put every assertion into one enormous test file. The goal is
to give each behavior one clear owner and make cross-cutting evidence reusable.

### Target Ownership

```text
rust/crates/cua-driver-testkit/src/
`-- observer.rs                     Cross-OS desktop-side-effect interface

rust/crates/cua-driver/tests/
|-- cross_platform_behavior_test.rs Shared Electron/Tauri action matrix
|-- harness_wpf_test.rs             Windows WPF action rows
|-- harness_winui3_test.rs          Windows WinUI3 action rows
|-- harness_web_test.rs             WebView2/Electron page and CDP rows
|-- harness_appkit_test.rs          macOS AppKit action rows
|-- harness_swiftui_test.rs         macOS SwiftUI action rows
|-- harness_gtk3_test.rs            Linux GTK3 action rows
|-- capture_contract_test.rs        Tree and screenshot read contract
|-- desktop_scope_<os>_test.rs      Window/desktop scope invariants
`-- protocol_*_test.rs              Protocol and schema tests
```

The desktop observer is a helper, not a test family. An action row invokes it when
the row is testing background delivery. The row then records both outcomes:

1. Did the target application state change, or did the driver return the
   documented structured refusal?
2. Did focus, z-order, cursor, and desktop state remain within the contract?

The deleted guard and modality files remain owned by the typed shared/native
rows and the launch, cursor, capture, and desktop-scope contracts. The
canonical runner is the only user-facing command; lane selectors are
internal diagnostics.

## Contributor Workflow

When adding a new scenario:

1. Add or update the repo-local fixture and its external state marker.
2. Add the Rust scenario under `rust/crates/cua-driver/tests/`.
3. Declare AX/PX addressing, foreground/background delivery, scope, and oracle.
4. Add the scenario to `docs/test-matrix.md` and this guide when it changes the
   cross-OS structure.
5. Update only the OS runner selection when the test is platform-specific.
6. Run the smallest Rust test locally, then run the OS command before
   calling the matrix complete.

The goal is one understandable Rust E2E model across platforms, with
platform-specific harnesses where the OS genuinely differs.
