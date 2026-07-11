# Rust E2E Harness Convergence Plan

## Re-review Verdict

The original direction remains sound:

- Rust owns scenarios, assertions, and result records.
- Repo-local harness applications are the canonical E2E targets.
- AX/PX targeting and foreground/background delivery are dimensions on an
  action. They are not test families.
- Focus, z-order, cursor, and desktop checks are cross-cutting observations.
- `all` is the contributor-facing command on every OS. CI may fan it into
  internal jobs for failure isolation.

This review changes five parts of the earlier plan:

1. Do not make a historical cell count a target. The shared matrix must cover
   supported route combinations, while every omission needs a route
   equivalence or unsupported-capability reason.
2. Separate test status from driver behavior. A test may pass because a
   required action was delivered or because a declared unsupported route was
   refused correctly. Those outcomes must remain distinct in reports.
3. Treat environment readiness as a lane preflight. A missing desktop, TCC
   grant, fixture, AT-SPI bus, recorder, or interactive Windows session must
   fail once before behavioral cells run.
4. Use a typed Rust case catalog as the matrix source. Do not add a second
   `matrix.yaml`, Python collector, or shell-owned scenario list.
5. Preserve one evidence bundle per cell, but reuse the driver and harness
   process where explicit state reset proves isolation.

The target is a smaller set of tests with named coverage reasons and strong
external oracles. Test count is not a success metric.

## Non-negotiable Rules

1. A successful tool response never proves delivery. A delivered action must
   change fixture or desktop state that the test reads independently.
2. A background action must also prove that it did not steal focus, raise the
   target, move the real cursor, or leak partial input when those invariants
   apply.
3. A refusal is valid only when the cell contract expects refusal, the driver
   returns an allowed structured refusal code, and the desktop observer sees no
   side effect.
4. A cell that requires delivery fails when the driver refuses, even if the
   refusal is honest.
5. A required canonical fixture or desktop capability cannot become a passing
   early return. Optional tests must be declared optional before execution.
6. Known gaps do not use `#[should_panic]` and do not count as green coverage.
   They run in a named optional lane with a linked issue until fixed.
7. Shell and PowerShell runners build the environment and collect artifacts.
   They do not infer behavioral results from Cargo output.
8. The Rust source build under test must be recorded in every run. macOS may
   proxy through the installed app bundle for TCC, but that bundle must come
   from the same source revision.

## Current Branch State

The branch is partway through the migration. The plan must start from this
state rather than the older 60-cell proposal.

### Already present

- `cross_platform_behavior_test.rs` has one typed 32-cell catalog per shared
  host. It covers AX and PX with foreground and background delivery for click,
  text, keyboard, scroll, and child-window actions; PX drag and AX editor-save
  each cover both delivery modes.
- Declaration and result schema v2 record action, `targeting`, delivery, scope,
  backend route, expected and observed behavior, independent test status,
  required oracles, duration, and evidence.
- Background refusal matching uses the explicit structured set:
  `background_unavailable`, `background_occluded`, and
  `background_uipi_blocked`.
- Every catalog route is explicit for Win32, Quartz, X11, and Wayland; the
  Windows Chromium PX background route remains required delivery.
- Every background shared cell attaches independent focus, z-order, real
  cursor, leaked-input, and fixture-state observations. Windows, macOS, and X11
  use direct testkit observers; an occluding Electron sentinel supplies the
  focus and leaked-input journal. Unsupported Wayland observations fail closed.
- Electron and Tauri use repo-local builds of the same shared web fixture.
- Windows, Linux, and macOS runners have strict source, fixture, AX, capture,
  and video preflights. Shell runners no longer synthesize behavioral rows.
- Rust validates duplicate, missing, contradictory, and evidence-less results
  before rendering the GitHub summary.
- Per-cell source-driver recording support exists in `cua-driver-testkit`.

### Still incomplete

- Three older shared tests remain in source pending deletion, although the
  canonical runners now select only the typed catalog.
- `guard_ux_test.rs`, `modality_input_e2e_test.rs`, and
  `modality_background_test.rs` overlap and still allow many runtime skips.
- Windows `all` still exposes `guard` and `modality` as test families and does
  not run the Windows desktop-scope target.
- Linux `all` still reports schema-only dispatch checks as E2E behavior.
- The macOS preflight is implemented, but this host currently reports an
  ad-hoc-signed daemon without a reusable Screen Recording grant.
- `harness_appkit_scroll_expected_fail` still uses `#[should_panic]`.
- Native harness tests do not emit the same typed result records as the shared
  matrix.
- Fixed coordinates, fixed CDP ports, sleeps, and early-return skips remain in
  canonical targets.

## Target Test Model

### Typed case catalog

Every behavioral cell is declared in Rust with one structure shared by web and
native harness tests:

```rust
struct CaseSpec {
    id: &'static str,
    platform: Platform,
    display_server: DisplayServer,
    harness: Harness,
    action: Action,
    targeting: Targeting,
    delivery: Delivery,
    scope: Scope,
    expectation: ContractExpectation,
    oracles: &'static [OracleKind],
    route: DriverRoute,
}

enum ContractExpectation {
    Deliver,
    Refuse { allowed_codes: &'static [RefusalCode] },
}
```

`RefusalCode` is an enum, not a string-prefix check. Its initial Windows set is
`BackgroundUnavailable`, `BackgroundOccluded`, and `BackgroundUipiBlocked`;
Linux currently declares only `BackgroundUnavailable`. A cell lists the exact
codes allowed by its controlled setup.

`Targeting` uses `Ax`, `Px`, `Page`, or `NotApplicable`. Use `targeting` in the
schema instead of `capture_mode`; capture is a separate read contract.

`DriverRoute` names the implementation path that justifies coverage, such as
UIA Invoke, PostMessage, coordinate injection, CGEvent, AT-SPI action, libei,
or CDP. It is test metadata, not a request parameter.

The catalog is the machine-readable inventory. Contributor documentation and
the coverage table are generated from it or checked against it. There is no
second matrix file to keep in sync.

### Result record

Use one schema version and these independent fields:

| Field | Meaning |
| --- | --- |
| `cell_id` | Stable case id |
| `platform`, `display_server` | OS and Win32/Quartz/X11/Wayland environment |
| `harness`, `toolkit` | Electron, Tauri, WPF, WinUI3, WebView2, AppKit, SwiftUI, GTK3 |
| `action`, `targeting`, `delivery`, `scope` | Contract dimensions |
| `driver_route` | Backend path covered by the cell |
| `expected_behavior` | `DELIVER` or `REFUSE` |
| `test_status` | `PASS`, `FAIL`, `SKIP`, or `ENVIRONMENT_ERROR` |
| `observed_behavior` | `DELIVERED`, `REFUSED`, `NO_EFFECT`, `ERROR`, or `NOT_RUN` |
| `refusal_code` | Structured code when observed behavior is `REFUSED` |
| `oracles` | App state and attached desktop observations |
| `known_issue` | Optional issue id; it never changes a failure to a pass |
| `evidence` | Video, trajectory, screenshots, structured state, and log paths |

This removes the ambiguous `EXPECTED_REFUSAL` status. A refusal contract passes
only when `expected_behavior=REFUSE`, `observed_behavior=REFUSED`, the code is
allowed, and all no-side-effect oracles pass. Reports count delivered and
refused passes separately.

### Coverage selection

Do not restore the full Cartesian product automatically. Select cells by
driver route:

1. Every supported action has foreground and background coverage when the tool
   exposes both delivery modes.
2. Every distinct targeting path used by that action has at least one cell.
3. Every distinct OS backend route has at least one cell.
4. Renderer or toolkit duplication is kept only when it changes the route or
   has produced a real compatibility defect.
5. Every omitted combination names an `equivalent_to` cell or an unsupported
   contract reason.

The shared catalog currently declares 32 cells per host. Add a missing
combination when it reaches a different driver route. Remove a combination
only when another cell proves the same route with an equal or stronger oracle.

## Cross-cutting Desktop Observer

Add one testkit interface that snapshots desktop state before and after an
operation:

```text
DesktopObservation
  foreground window
  target z-order and minimized state
  focus-change journal
  real cursor position
  optional leaked-input journal
```

Implement Windows first with `focus-monitor-win`. Add macOS and Linux adapters
behind the same interface after the Windows contract is stable.

Attach the observer to:

- every background delivery cell;
- refusal cells;
- launch/minimize cells;
- screenshot and capture cells that promise no focus change;
- cursor evidence cells.

For successful background delivery, the cell requires both target-state change
and unchanged desktop invariants. For refusal, the cell requires no target
change and unchanged desktop invariants. A focus-only pass never proves input
delivery.

## Process And Evidence Lifecycle

Use these boundaries:

- one Rust source build per lane;
- one driver daemon or MCP process per lane;
- one harness process per harness group when the fixture has a verified reset
  operation;
- one recording session and result record per cell;
- a harness restart after crash, reset failure, or window identity change.

Each fixture reset must return a generation token. The next cell verifies the
new token and clean marker state before acting. Until a harness has this reset
contract, keep process-per-cell isolation.

Video remains required for every canonical E2E cell. Test video capture once in
the lane preflight. A recorder failure aborts the lane before the case catalog
runs, rather than generating the same permission failure for every cell.

## Environment Preflight

Each OS runner performs one preflight and emits one environment record.

Common checks:

- source revision and driver version match;
- required fixture binaries exist;
- the display/user session is interactive;
- the driver can list and inspect a preflight fixture;
- accessibility and capture permissions work;
- a short video starts, stops, and passes `ffprobe`;
- artifact directories are writable.

Platform checks:

| Platform | Required preflight |
| --- | --- |
| Windows | Non-Session-0 interactive desktop, input desktop, foreground sentinel, FFmpeg, UIA visibility |
| macOS | App-bundle daemon identity, live socket, Accessibility, Screen Recording, fixture window visibility |
| Linux X11 | X server, DBus, AT-SPI, window manager, capture, input backend |
| Linux Wayland | Compositor, DBus, AT-SPI, portal/capture path, libei or declared refusal path |

Canonical invocations set strict mode. Missing required capabilities produce
`ENVIRONMENT_ERROR`; they never return from a test as a pass.

## File Ownership And Disposition

| Current file | Final owner or action |
| --- | --- |
| `cross_platform_behavior_test.rs` | Shared Electron/Tauri case catalog and external fixture-state oracles; remove the three legacy tests after parity |
| `harness_wpf_test.rs` | WPF-specific rows using the common case/result runner |
| `harness_winui3_test.rs` | WinUI3-specific rows; keep only toolkit-distinct behavior |
| `harness_web_test.rs` | WebView2 and Page/CDP behavior; do not mix Page targeting with AX/PX labels |
| `harness_appkit_test.rs` | AppKit rows; move the scroll gap to an optional issue-linked lane and remove `#[should_panic]` |
| `harness_swiftui_test.rs` | SwiftUI controls and popover behavior |
| `harness_gtk3_test.rs` | Minimal GTK3/AT-SPI rows for X11 and Wayland |
| `guard_ux_test.rs` | Migrate observations into the desktop observer, then delete |
| `modality_input_e2e_test.rs` | Map each action and current failure into shared/native cells, then delete |
| `modality_background_test.rs` | Move WPF actions into WPF cells and capture checks into capture ownership, then delete |
| `modality_capture_mode_test.rs` | Rename to `capture_contract_test.rs`; one owner for tree/image inclusion behavior |
| `modality_desktop_scope_*.rs` | Rename to `desktop_scope_*_test.rs`; keep platform-specific scope contracts |
| `modality_focus_test.rs` | Deleted; shared click/type cells own focus preservation and launch focus has a separate optional owner |
| `modality_launch_focus_macos_test.rs` | Optional real-app lane with issue ownership; never part of canonical harness counts |
| `modality_dispatch_test.rs` | Move schema assertions to protocol tests and real GUI assertions to platform cells, then delete |
| `modality_dispatch_linux_test.rs` | Keep only real X11/Wayland delivery behavior; move schema checks to protocol tests |
| `harness_libreoffice_test.rs` | Optional installed-app lane; exclude from `all` and canonical counts |
| `protocol_*`, schema, transport tests | Unit/protocol gate; no desktop video and no behavioral matrix rows |
| `tests/fixtures/shared/scenarios.json` | Prune only after selector and marker-reference audit |

## CI Shape

The contributor command stays `all`. Suite selectors remain internal and
diagnostic.

### Pull requests

- Run unit/protocol jobs by affected OS paths.
- Shared core or schema changes trigger Linux and Windows unit jobs.
- Platform-only changes trigger that platform's unit job.
- E2E is maintainer-dispatched and may become an optional pre-merge gate.

### Maintainer E2E

- Windows GitHub-hosted runners run the matrix when the preflight proves an
  interactive desktop. Background/focus-sensitive cells also run on the
  registered Azure RDP self-hosted runner, which is authoritative for that
  contract.
- Linux GitHub-hosted runners run the Nix-defined X11 lane. Wayland is a
  separate Nix-defined maintainer lane. Linux produces no GIF requirement.
- macOS runs on a logged-in, TCC-authorized host through the canonical macOS
  runner. A future self-hosted runner must use the same preflight.

CI may fan `all` into shared, native, capture/scope, and platform jobs. Those
are execution partitions, not alternate public test suites.

## Reporting And Evidence

Rust emits one record for every declared cell. A shared Rust reporter then:

1. rejects duplicate or missing cell ids;
2. verifies the result against the case contract;
3. verifies every required evidence file exists and is readable;
4. renders the behavioral table and the declared coverage table;
5. renders unit/protocol suites in a separate section;
6. fails when a declared cell produced no result.

Do not parse `test ... ok` lines to create behavioral rows. Cargo/JUnit output
may still provide failure annotations for unit tests.

Upload one artifact bundle per behavioral cell with a stable artifact name.
Each GitHub summary row links to that cell artifact and lists the exact video,
trajectory, screenshot, and log paths. GitHub cannot deep-link to a file inside
a multi-cell artifact archive, so a single lane archive cannot satisfy the
per-row evidence-link requirement by itself.

## Implementation Slices

### Slice 1: Contract and preflight

- Finalize `CaseSpec`, result enums, refusal-code enums, and one schema version.
- Add reporter validation for duplicate, missing, and contradictory records.
- Add strict environment preflight to Windows, Linux, and macOS runners.
- Stop shell runners from inventing behavioral rows.

Exit: a missing session, fixture, permission, or recorder fails once as an
environment error, and a synthetic CaseSpec set renders a valid report.

### Slice 2: Shared matrix integrity

- Map every shared cell to an explicit driver route.
- Add any missing route cells; document every omitted equivalent.
- Add the desktop observer to background and refusal cells.
- Preserve editor-save and all existing external markers.
- Run old and new shared tests side by side, then remove the three legacy tests.

Exit: no fake drag pass, no arbitrary error accepted as refusal, no shared
legacy test, and one result/evidence bundle per shared cell.

### Slice 3: Windows convergence

- Move guard, modality-input, and modality-background assertions into shared,
  WPF, WinUI3, capture, launch, or desktop-scope owners.
- Preserve every current failing action as a failing required-delivery cell or
  an issue-linked optional cell. Do not convert it to a green refusal.
- Add Windows desktop-scope to `all`.
- Delete the three transitional files only after cell-by-cell parity.

Exit: Windows has no guard/modality family, and every background cell has both
target-state and desktop-side-effect evidence.

### Slice 4: macOS and Linux convergence

- Adopt the common case/result runner in AppKit, SwiftUI, and GTK3 tests.
- Replace canonical early-return skips with preflight failures.
- Split Linux schema checks from real desktop behavior.
- Rename capture and desktop-scope ownership files.
- Move AppKit scroll and real-app checks to explicit optional issue lanes.

Exit: native cells emit the same records as shared cells, X11 and Wayland are
separate dimensions, and macOS failures distinguish TCC from driver behavior.

### Slice 5: Flake and fixture cleanup

- Replace fixed coordinates with discovered geometry.
- Allocate CDP ports per process.
- Replace sleeps with deadline polling on external markers.
- Add fixture generation-token reset and reuse harness processes only after it
  proves clean state.
- Prune fixture controls and markers with no live selector or oracle reference.

Exit: no canonical cell depends on VM-specific coordinates, fixed shared ports,
or an unexplained sleep.

### Slice 6: CI validation and deletion

- Run macOS `all` locally after install-local and TCC preflight.
- Run Windows `all` on GitHub-hosted and Azure RDP runners.
- Run Linux X11 `all`, then the Nix Wayland lane.
- Compare old/new cells before each transitional file deletion.
- Update contributor docs and the PR description from the generated catalog.

Exit: every declared cell is delivered, refused according to contract, or
fails with a linked unresolved bug. Environment failures are separate.

## Deletion Gates

A test or fixture path may be deleted only when:

1. every assertion maps to a CaseSpec or unit/optional owner;
2. the replacement oracle is equal or stronger;
3. old and replacement outcomes were compared on each affected OS;
4. a required-delivery failure remains visible;
5. refusal cells use an explicit allowed code and desktop-side-effect proof;
6. runners, docs, and artifact labels stop referencing the old target in the
   same change;
7. the reporter finds no missing declared cells.

## Definition Of Done

- Rust has one typed behavioral catalog and one result schema.
- Required GUI prerequisites cannot pass through early returns.
- Test status and observed driver behavior are separate fields.
- Every supported action has foreground/background coverage by distinct driver
  route, with reasons for omitted combinations.
- Every delivered action has an external target-state oracle.
- Every background or refusal cell has desktop-side-effect evidence.
- No permanent guard, modality, or delivery test family remains.
- No `#[should_panic]` known-gap E2E test remains.
- No orphaned target is counted as coverage.
- Shared and native harnesses emit the same result/evidence shape.
- Unit/protocol tests stay desktop-independent and video-free.
- Windows, Linux X11/Wayland, and macOS `all` runs produce classified outcomes
  and per-cell evidence links.
