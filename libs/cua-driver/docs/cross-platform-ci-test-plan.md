# Cross-Platform CI and E2E Test Plan

Status: implementation plan, with the source-built unit and manual runner
foundation now landed on the working branch

## Goal

Make the Rust harnesses the source of truth for user behavior on Linux and
Windows. Keep unit tests cheap and OS-scoped. Run desktop end-to-end tests as a
maintainer-controlled gate with the real session each platform requires.

The same Rust scenario should run on both platforms whenever the behavior is
supported. The runner may use different platform APIs, but the scenario ID,
application oracle, and outcome rules should stay the same.

## Test Tiers

### Tier 1: Rust unit and compile checks

Run automatically on pull requests when an affected platform or shared Rust
path changes.

- No desktop session, TCC, RDP, AT-SPI, or Azure credentials.
- Run on `ubuntu-latest` and `windows-latest` in separate jobs.
- Use `cargo test --workspace --all-targets --locked`.
- Run Clippy in the same OS-scoped workflow where its cost remains acceptable.

Shared code changes run both OS jobs. A platform-only change runs one job.

### Tier 2: Fast Linux integration checks

Keep the small Xvfb and AT-SPI checks that do not require a real user desktop.
These checks can remain automatic if their runtime stays short and stable.

They cover protocol setup, capture modes, accessibility reachability, and
desktop-scope contract errors. They do not replace the behavioral harness.

### Tier 3: Maintainer e2e gate

Run the same Rust behavioral matrix against real harness applications.

- Linux: the manual Rust runner for the canonical matrix; Nix X11 and Nix
  Wayland remain supporting compositor checks until the source-built app pilot
  is complete.
- Windows: Azure Windows with an active RDP user session.
- Trigger: `workflow_dispatch` with a commit ref, PR number, and suite.
- Optional nightly and post-merge runs can catch drift without slowing every PR.
- The VM or runner never pushes code.

### Tier 4: Supporting compatibility checks

Keep toolkit and distro checks that cover behavior outside the canonical harness
matrix. These checks should be labeled as supporting coverage and should not
claim that a user workflow passed when they only prove window discovery or a
non-error response.

## Canonical Behavioral Matrix

The Rust test suite owns the scenario list. CI workflows should select a suite,
not duplicate the scenario definitions in YAML or Python.

### Applications

- Electron
- Tauri

### Surfaces

- Accessibility action path
- Pixel-coordinate path

### Delivery modes

- Background
- Foreground

### Scenarios

- Left, right, and double click action oracles
- AX/PX action addressing across supported controls
- Editor type, save, and saved-state oracle
- Enter key and hotkey state oracles
- Nested scroll and scroll-offset oracle
- Child-window creation oracle
- Drag and drop-state oracle
- Focus sentinel and window posture checks

Each result has one of these outcomes:

```text
pass
expected_refusal
known_gap
not_applicable
unexpected_failure
```

An expected refusal must include the structured driver error and capability
reason. A driver response without an external application-state change is not a
pass.

The runner should emit a common result record:

```json
{
  "scenario": "right_click",
  "os": "windows",
  "app": "electron",
  "surface": "ax",
  "delivery": "background",
  "outcome": "pass",
  "oracle": "editor_status=saved:exact"
}
```

Platform-specific applications such as WPF and WinUI 3 remain focused Windows
coverage. GTK and Qt remain focused Linux coverage. They supplement the shared
matrix and do not define cross-platform acceptance.

## Linux Structure

Nix remains the Linux desktop test environment. It should build the driver and
repo-local harness applications from source and run the Rust tests against those
outputs. The test should not download a prebuilt application and Linux e2e runs
do not need GIF recording.

Proposed layout:

```text
nix/cua-driver/
  package.nix
  module.nix
  README.md
  lib/
    app-catalog.nix
    artifacts.nix
    atspi-session.nix
    mcp-client.nix
    wayland-session.nix
    x11-session.nix
  checks/
    x11/
      harness-electron.nix
      harness-tauri.nix
      input-cursor-click.nix
      input-parallel-drag.nix
      service-config-persistence.nix
      service-integration.nix
    wayland/
      app-background-read.nix
      input-background-terminal.nix
      input-cursor-click.nix
      input-parallel-drag.nix
      service-integration.nix
```

The current generic `linux-background-gui.nix` should be split. Its read-only
app entries should become supporting checks or be removed after equivalent
canonical harness coverage exists. Its Chromium and Tk write paths should be
retained until the Rust harness covers the same contract.

Use explicit flake check names such as:

```text
cua-driver-linux-x11-harness-electron
cua-driver-linux-x11-harness-tauri
cua-driver-linux-x11-service-config-persistence
cua-driver-linux-wayland-gnome-input-background-terminal
```

Do not put `gif` in a test name. Screenshots, AX trees, structured results, and
driver logs are enough for routine artifacts. Capture a PNG on failure or when
diagnosing a pixel scenario.

The testkit needs test-only path overrides so Nix can use immutable build
outputs:

```text
CUA_TEST_DRIVER_BIN
CUA_TEST_APPS_ROOT
CUA_TEST_WORKSPACE_ROOT
```

Local runs keep their current defaults.

## Windows Structure

Windows uses the same Rust scenarios and repo-local harness source. The Azure VM
builds the applications on demand inside the active RDP session.

Workflow and helper names:

```text
.github/workflows/ci-rust-windows.yml
.github/workflows/e2e-rust-windows.yml
scripts/ci/windows/
  verify-user-session.ps1
  build-harnesses.ps1
  run-rust-e2e.ps1
  collect-artifacts.ps1
```

The Windows e2e workflow now:

1. Check out or sync the requested commit.
2. Confirm that the target process runs in the active RDP user session, not
   Session 0.
3. Build Electron, Tauri, WPF, and WinUI 3 harnesses from source.
4. Build the Rust driver and integration tests.
5. Run the shared behavioral matrix, then Windows-native tests.
6. Collect structured results, UIA trees, screenshots, and driver logs.
7. Leave the VM unable to push or modify the source branch.

The Windows unit workflow runs on `windows-latest` and does not attempt these
desktop steps.

## Workflow Names

Use the distinction between `ci` and `e2e` in the filename and displayed job
name:

```text
CI: Rust Linux unit
CI: Rust Windows unit
CI: Linux fast integration
E2E: Linux Nix X11
E2E: Linux Nix Wayland
E2E: Linux Azure desktop
E2E: Windows Azure RDP
```

The Nix X11 and Wayland workflows should stop running automatically for every
PR update once the maintainer gate is available. Keep manual dispatch, nightly
runs, and post-merge regression runs.

## Legacy Test Migration

Keep an inventory before deleting tests. For each test record its scenario,
oracle, OS, window system, app, and current CI role.

Classify each test as:

- `canonical`: a trusted user-behavior acceptance test.
- `supporting`: useful backend or toolkit coverage.
- `diagnostic`: useful during investigation but not a product gate.
- `retire`: redundant or weaker than a canonical replacement.

Migration order:

1. Add the testkit path overrides and strict fixture preflight.
2. Land the OS-scoped unit checks and the manual desktop runners.
3. Pilot source-built harness packaging inside Nix before moving app builds
   into the Nix sandbox.
4. Add missing scenario coverage before removing an old test.
5. Move weaker tests out of required CI.
6. Delete tests only after the replacement has passed repeatedly on the target
   platform.
7. Update the Nix and workflow READMEs to describe the new ownership.

Do not delete specialized Wayland, MPX, WPF, or WinUI coverage merely because
the shared Electron/Tauri matrix exists. Retain those tests until the shared
matrix covers the behavior they uniquely exercise.

## Acceptance Criteria

- Shared scenario IDs and external oracles are identical on Linux and Windows.
- Shared Rust changes run both unit-test jobs.
- Platform-only changes run only the affected OS unit job.
- Linux e2e builds harness applications from source in the Linux desktop runner;
  a fully sandboxed Nix app-build pilot remains a separate follow-up.
- Windows e2e builds harness applications from source in the Azure VM.
- No Linux e2e test requires GIF recording.
- No canonical test passes on driver success alone.
- Unsupported background behavior produces a structured refusal or remains a
  visible known gap.
- Maintainer e2e runs publish a result table and failure artifacts linked to the
  PR.
- Legacy tests are either classified, upgraded, moved out of required CI, or
  removed with an equivalent canonical replacement.

## Non-Goals

- Do not make Windows use Nix.
- Do not create a second Python behavioral framework.
- Do not download opaque prebuilt desktop fixtures.
- Do not weaken external-state assertions to make a matrix green.
- Do not make Azure or RDP credentials available to untrusted PR code.
