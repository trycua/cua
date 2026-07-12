# Linux support completion plan

**Status:** Proposed

**Scope:** Linux driver behavior, canonical Rust E2E evidence, and release validation

**Principle:** Standard Wayland and the optional compositor-owned injection environment are separate products with separate claims.

## Goal

Close the remaining Linux gaps without restoring the deleted Python, shell, GIF,
or real-app fixture suites. Reuse the repo-local Rust harness catalogs and their
external state, focus, z-order, cursor, input-leak, screenshot, trajectory, and
video oracles.

The work is complete when each supported action either:

- delivers and changes fixture-owned state under every required side-effect oracle;
- returns the declared structured refusal before dispatch; or
- is recorded as an environment gap with a named owner and representative test lane.

A successful driver response is never delivery evidence.

## Current baseline

| Environment | Current evidence | Main gaps |
| --- | --- | --- |
| X11/Openbox on Xvfb | 108/108 declared outcomes: 71 deliveries and 37 exact refusals | Real-Xorg MPX/uinput and parallel pointer behavior are not proven by Xvfb |
| Sway/wlroots, native Wayland | GTK3 31/31 plus capture/scope; Electron AX background/foreground click and foreground typing | Electron hotkey, renderer PX geometry, and WebKitGTK without DRM/EGL |
| GNOME 46/Mutter | GTK3 31/31 using WinRects, AT-SPI, and portal/libei | Shared Electron/Tauri matrix and portal MP4 evidence |
| KDE/KWin | AT-SPI discovery and generic toplevel support | Target-addressable activation adapter and a representative Plasma 6 lane |
| Nested `cua-compositor` | Driver and compositor code remain, but canonical evidence was deleted | Packaging, startup, route metadata, truthful refusals, independent observer, and typed matrix |

Standard Wayland cannot generally route raw pointer or keyboard events to an
arbitrary occluded, unfocused surface. The nested `cua-compositor` is different:
it owns the compositor and can route directly to a client surface through its
private control socket. Evidence from that environment must never be presented
as a stock Sway, GNOME, KDE, or general Wayland capability.

## Target environment matrix

Keep the matrix small. An environment earns a lane only when it proves a
contract that another lane cannot.

| ID | Environment | Purpose | Ownership |
| --- | --- | --- | --- |
| E1 | X11/Openbox on Xvfb | XSendEvent, XTest, AT-SPI, capture, scope, complete shared/native catalogs | Hosted GitHub runner; canonical |
| E2 | Sway on native Wayland | wlroots virtual pointer, screencopy, foreign toplevel, and standard-Wayland refusals | Hosted GitHub runner; canonical |
| E3 | Nested `cua-compositor` | Raw focus-free per-surface input and multi-pointer behavior | Hosted GitHub runner; experimental until proven |
| E4 | GNOME/Mutter real session | WinRects, portal/libei, shared renderer apps, portal recording | Maintainer VM; representative |
| E5 | Real Xorg session | MPX/uinput behavior unavailable under Xvfb | Maintainer VM; optional |
| E6 | KDE Plasma 6/KWin | KWin activation and portal behavior | Maintainer VM; experimental |
| E7 | Real DRM/EGL renderer session | WebKitGTK/Tauri accessibility tree and native renderer geometry | Prefer E4; use a separate VM only if needed |

Labwc, Hyprland, and other wlroots compositors remain expected-compatible but
unproven unless a user report demonstrates a meaningful divergence. XWayland is
not a separate lane.

## Phase 0: Reconcile the truth

Update stale internal references before changing behavior.

**Work**

- Mark stale sections of `libs/cua-driver/docs/test-harness-convergence-plan.md`
  as superseded by current evidence.
- Rewrite `nix/cua-driver/tests/README.md`; it currently names deleted fixtures.
- Remove the empty `nix/cua-driver/tests/wayland/` directory if it is still empty.
- Add the experimental nested-compositor row to
  `libs/cua-driver/docs/action-support.md` and link it from the public platform
  support page.
- Replace misleading "EIS compositor" wording. The current route is a private
  Unix socket protocol, not libei/EIS.

**Acceptance**

- No documentation points to a deleted fixture or workflow.
- Every public capability has current evidence or an explicit experimental gap.

## Phase 1: Make results identify the real route

The result schema currently says only `Wayland` and can make nested injection
look like a standard virtual-pointer or AT-SPI route.

**Work**

- Add `LinuxCuaCompositorInject` to `DriverRoute` in
  `cua-driver-testkit/src/e2e.rs`.
- Add environment fields for `compositor` and available `input_backends`.
- Detect values such as `openbox-x11`, `sway`, `gnome-mutter`, `kwin`, and
  `cua-compositor-nested` in the environment preflight.
- Update `cua-e2e-report` and the Markdown summary to display the compositor and
  input route without changing test-family or action naming.
- Keep backward compatibility with existing result artifacts.

**Acceptance**

- E1 and E2 retain identical cell outcomes.
- Every delivered cell identifies both its display server and actual route.
- E2 and E3 artifacts cannot be mistaken for each other.

## Phase 2: Audit and harden nested injection

This is a proof gate, not an assumption. The existing path has two correctness
risks:

1. `app_id_for_window` appears to compare a toplevel object ID obtained from one
   Wayland connection with objects from a new connection. Protocol IDs are
   connection-scoped, so target resolution must be tested and likely replaced
   with stable identity resolution.
2. The compositor silently ignores unsupported Unicode and keys and has no
   scroll or hotkey verb. The driver must never return success for those cases.

**Work**

- Add protocol handshake/version and acknowledgements.
- Resolve targets through stable app/window identity, including duplicate app IDs.
- Pre-validate all text and key shapes before sending.
- Return exact `background_unavailable` refusals for unsupported text, key,
  hotkey, scroll, or drag shapes until their protocol verb exists.
- Add focused unit tests with a mock Unix socket for encoding, acknowledgements,
  validation, target ambiguity, and error mapping.
- Verify or correct the stale `LIBEI_SOCKET` comment in `wayland/libei.rs`.

**Acceptance**

- No unsupported command can be silently dropped.
- No route depends on connection-local Wayland object IDs as stable identities.
- A compositor protocol mismatch fails preflight once, before the matrix starts.

## Phase 3: Restore the compositor as a typed environment

Do not restore the old Nix/Python fixture suite. Package only the environment,
then run the existing Rust catalogs inside it.

**Work**

- Export `cua-compositor` as a flake package and build check.
- Add a `cua-driver-inject-e2e` development shell containing the compositor and
  existing Rust harness dependencies.
- Add `scripts/ci/linux/run-rust-e2e-inject.sh` to start the nested session,
  perform capability preflight, and delegate to the canonical
  `scripts/ci/linux/run-rust-e2e.sh` command.
- Add an independent `CuaCompositor` observer in the testkit. Read-only control
  queries must report focus and z-order directly; the driver response cannot be
  the observer.
- Reuse the GTK3 and shared `CaseSpec` catalogs. Key expectations by detected
  environment capability, not by weakening an oracle.
- Add a repo-local parallel-drag journal action because the old test asserted
  only command completion and therefore supplied insufficient evidence.
- Record every cell with before/after screenshots, trajectory data, and video.

**Minimum Step A matrix**

| Background PX action | Initial contract |
| --- | --- |
| Left, right, and double click | Deliver |
| ASCII type text | Deliver |
| Supported named key | Deliver |
| Single drag | Deliver after socket routing exists; otherwise exact refusal |
| Two simultaneous drags | Deliver with fixture state for both paths |
| Scroll | Exact refusal until an axis verb exists |
| Hotkey | Exact refusal until a chord verb exists |
| Unsupported Unicode or key | Exact refusal before socket dispatch |

All delivery rows require fixture state, unchanged sentinel focus and z-order,
and no leaked input. Cursor preservation is not an oracle when the compositor
route intentionally has no physical cursor.

**Acceptance**

- `nix build .#cua-compositor` is a CI check.
- The hosted experimental lane completes the Step A matrix three consecutive
  times on one source SHA with owned videos and no orphan evidence.
- The two useful historical claims, focus-free typing and simultaneous drags,
  are superseded by stronger typed fixture evidence.

**Deletion gate**

If the compositor cannot build, target stable identities, or support truthful
focus/z-order observation after two focused implementation cycles, remove the
entire dormant subsystem and its public capability claim in one PR. Do not leave
advertised but untested production branches indefinitely.

## Phase 4: Complete the injection protocol

After Step A is green, add protocol verbs for axis/scroll, modifier chords,
Unicode text, broader named keys, and single drag. Flip a test from refusal to
delivery only when the unchanged fixture and side-effect oracles pass.

**Acceptance**

- Background left/right/double click, Unicode typing, named keys, hotkeys,
  scroll, single drag, and parallel drag all have typed delivery evidence.
- Remaining refusals describe genuinely unsupported public shapes.

## Phase 5: Close stock Sway gaps

Work independently of the custom-compositor lane.

**Work**

- Repair Electron foreground AX/PX hotkey delivery in the driver.
- Repair renderer-to-compositor PX origin reconstruction.
- Keep Tauri/WebKitGTK environment-limited on hosted pixman Sway; do not use
  fixture changes or renderer-disabling flags as representative proof.

**Acceptance**

- Existing failing cells become delivered through external fixture state, or
  remain named gaps with exact diagnostics and a linked issue.

## Phase 6: Representative desktop coverage

### GNOME and WebKitGTK

- Run the unchanged shared Electron catalog on the existing GNOME VM.
- Add portal MP4 capture with the same ownership and nonblank checks as Sway.
- Check `/dev/dri` first and run the Tauri/WebKitGTK catalog there when a real
  render node is available.
- If it is unavailable, provision a dedicated DRM/EGL VM rather than adapting
  the fixture to software-renderer behavior.

### KDE

- Implement a target-addressable KWin activation adapter using supported KWin
  scripting or D-Bus interfaces.
- Validate activation through an independent observer before enabling portal
  input.
- Use Plasma 6; do not accept the broken Plasma 5.27 cloud image as evidence.

### Real Xorg

- Add an optional maintainer lane for MPX/uinput and parallel pointer behavior.
- Do not describe Xvfb refusals as proof that real-Xorg delivery is impossible.

## Phase 7: Release validation policy

- Keep E1 X11 and E2 Sway dispatch-only or maintainer-triggered, but require
  green run links on the exact release SHA in the release checklist.
- Keep E3-E7 optional until each environment has an owner and three stable runs.
- Store results, summary, screenshots, trajectories, logs, and videos per lane.
- Fail preflight once for a missing session capability; do not turn all cells
  into skips.

## Suggested PR sequence

1. Documentation reconciliation.
2. Result route and environment metadata.
3. Nested-injection refusal hardening and mock-socket tests.
4. Sway hotkey repair and PX geometry repair as independent changes.
5. Compositor package, build check, protocol query support, and stable identity.
6. Testkit observer, typed Step A matrix, runner, and experimental workflow.
7. Protocol completion and expectation flips.
8. GNOME shared/video and representative Tauri renderer evidence.
9. KWin adapter and Plasma 6 evidence.
10. Release checklist gating.

Each PR must leave existing canonical lanes green. A new environment may fail
only in a clearly labeled experimental job and may not change the public
contract of E1 or E2.

## Anti-overfitting rules

- No Python behavioral runner, second matrix file, terminal/GIF fixture, or old
  Nix test restoration.
- No command-return-only oracle.
- No AT-SPI delivery described as raw PX injection.
- No raw-background claim without physical occlusion and focus/z-order guards.
- No fixture expectation change made solely to accommodate driver behavior.
- No compositor proliferation without a unique contract and named owner.
- No software-renderer workaround accepted as representative WebKitGTK evidence.

## Definition of done

- E1 remains 108/108 on the final source SHA.
- E2 has no silent no-effect outcome; hotkey and PX geometry are delivered or
  recorded as precise gaps.
- E3 has the complete typed raw-injection matrix and truthful route metadata, or
  the dormant subsystem and its claim are removed.
- GNOME has shared Electron, portal video, and representative WebKitGTK evidence.
- KDE has verified target-addressable activation on Plasma 6 or remains clearly
  experimental with that adapter as the named blocker.
- Every supported or refused action is represented by a typed Rust row and
  external evidence across the environments where its contract differs.
- Public and contributor documentation link current evidence and never conflate
  standard Wayland with compositor-owned raw injection.
