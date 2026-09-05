# Isolated-input VM validation

On 2026-09-05, the nonshipping input experiment passed a focused native GTK3
test in an existing Omarchy Fleet VM. Real Driver MCP calls delivered click,
key, hotkey, scroll, and drag events to a background application while a
different application retained its primary-seat pointer grab.

This is evidence for one experimental configuration, not a supported release,
a complete desktop E2E result, or acceptance of [RFC #3550](https://github.com/trycua/cua/issues/3550).
The normal plugin build remains discovery-only. The implementation is in
[draft #3572](https://github.com/trycua/cua/pull/3572), with the discovery
foundation [#3547](https://github.com/trycua/cua/pull/3547) and native observation
[#3557](https://github.com/trycua/cua/pull/3557) as dependencies.

## Tested artifacts

The executable and module were built from an archive of the same committed
source, not from an uncommitted working tree.

| Component | Tested identity |
| --- | --- |
| Source commit | `e61413a053a6f44c09cac2b263fd9e76410b43d8` |
| Omarchy package | `4.0.1-1` |
| Hyprland package | `0.56.2-1` |
| Hyprland commit | `efb50993780079460b0cbed1363e2166a2de1d9f` |
| Portal package | `xdg-desktop-portal-hyprland 1.4.1-1` |
| Plugin version | `0.1.0`, experimental input build |
| Driver source and installed version | `0.23.2`, source build, not the published binary |
| Plugin compiler | GCC `16.1.1`, matching the compositor |
| Rust compiler | `1.97.1` |

The full Hyprland ABI fingerprint was
`efb50993780079460b0cbed1363e2166a2de1d9f_aq_0.14_hu_0.14_hg_0.5_hc_0.1_hlg_0.6`.
The build linked the compositor's shared C++ runtime, not a static copy.
The source archive SHA-256 was
`64d9f26f32c4c51ecaf1d81d49f249cf5bca2e13a20b5b886672d3997c1b0e03`.

Artifact SHA-256 values were:

- Plugin: `c011f16d1ea6f121bd9088a7b5031e56124f3f633913306b30bdf8d8759cdca3`.
- Installed Driver: `ffe064d7b175df3293fd13b488ab63c6056cb833b1adbe7a9b837bd11ae18274`.

The repository's `install-local.sh` installed the Driver into a task-local
guest directory. Its source SHA, resolved executable, version, digest, and
running service executable were checked. The pre-existing local and published
Driver binaries were preserved. No Fleet image was published.

Commit `5d4c2f7432cd6a1f7c3d8de0f6f6c0937bad3687` changes only Rust formatting
and module declaration order after this tested source. Subsequent validation
notes and the supplemental lease probe do not change either executable.

## Application and isolation evidence

[`driver_input_live.py`](driver_input_live.py) connected through a real Driver
MCP process to an unrestricted test service. Both
[GTK3 drawing-area fixtures](../../tests/fixtures/apps/linux/isolated-input/main.py)
had empty actionable accessibility trees; AT-SPI actions could not substitute
for the raw input under test. PyGObject `3.58.0` and pycairo `1.29.1` ran the
fixtures.

Each action had a fresh Cua window snapshot before and after. Independent
application event journals and Wayland wire logs supplied the behavior oracle.
The primary-seat adversary was [`primary_grab.c`](primary_grab.c), which held
the foreground left button through a virtual pointer. It was not a physical
mouse test. Background delivery used only the Driver MCP input route.

| Observation | Result |
| --- | --- |
| Background click | Raw button press arrived at `(200, 300)`, matching the window screenshot coordinates |
| Background key and hotkey | Application received `a` and `Shift+B` with the independent keymap |
| Background scroll | Application scroll counter increased from zero to one |
| Background drag | Motion events and a complete press/release arrived; no button remained held |
| Background aggregate | Click/release counter increased from zero to two; motion counter from zero to 14 |
| Foreground grab | Remained held during all five actions; the harness released it afterward |
| Primary pointer wire | No enter, leave, or button events during background delivery |
| Primary keyboard wire | No enter, leave, or key events during background delivery |
| Compositor state | Foreground identity, cursor position, and workspace stayed unchanged |
| Stop | Revoked the lease; a subsequent Driver click required new operator approval |

Driver correctly reported `route: synthetic_events` and `effect: unverifiable`.
The test's application evidence established delivery; the transport response
did not claim application success. The host signed a target/connection-bound
test grant, and only the public grant entered the guest.

## Other checks

- Six native CTests passed: protocol, status, Linux transport, two mocked API
  variants, and the CMake API probe. These are not six input application tests.
- Focused Rust tests passed: six `hyprland_input`, three `experimental_`, two
  `primary_seat`, and 23 shared `action_record` tests.
- All 24 Python operator, transport, and nested-lifecycle helper tests passed
  on the host with the optional cryptography dependency available.
- [`input_transport_test.py`](input_transport_test.py) passed against the final
  module: missing, malformed, and forged grants; replayed grants before and
  after Stop; replayed action sequences; NaN, infinity, negative, and
  out-of-bounds coordinates. Exactly one valid fixture click was dispatched.
- [`input_lease_live.py`](input_lease_live.py) separately proved that expiry
  refused further input and reconnect produced a fresh challenge, rejected
  the old connection's grant, and required fresh approval. These probes did
  not exercise cancellation while input was held.
- The exact module passed the two-owned-nested-compositor lifecycle runner:
  old sockets/connections closed, restart produced a fresh epoch and socket,
  the sibling remained responsive, and both owned compositors exited cleanly.
- A disposable foot client and both GTK fixtures survived unloading and
  reloading the final module in the parent session. Fresh Cua snapshots and
  unchanged process/window identities verified survival.
- After reload, Driver's first call reported its broken connection without
  replaying input. Following a fresh snapshot, a separately requested call
  reconnected and required approval in the new epoch. Fixture counters did
  not change on either refused call.

The supplemental lease probe's tested SHA-256 is
`42c623def921c4f359e7b8d2c2660951901372fc36a934ff4a5c53e6749d574c`.
Use a new evidence directory for each run. For `--case expiry`, sign its
`request.json` with click capability `1` and a short lifetime such as 15 seconds,
then promptly transfer only the public `grant.json`. For `--case disconnect`,
use a lifetime of at most 60 seconds. It must run alone: cleanup sends Stop.

## Failures corrected during the experiment

- Destroying seat resources immediately on unload disconnected a live foot
  client. The final experiment retains inert handlers and marks its ELF module
  `NODELETE`; the disposable-client regression then passed. This is a bounded
  test workaround, not safe production hot-unload support.
- Hidden symbol visibility created private copies of compositor globals.
  The experimental build uses the compositor-compatible visibility settings.
- Advertising seat version 8 prevented the nested compositor from binding;
  the experiment now advertises version 9.
- The Driver initially delivered input but rejected its missing execution
  record as `action_outcome_mismatch`. The isolated transport now records the
  shared synthetic-event action outcome.
- Decorated window bounds shifted pointer coordinates by four pixels. Using
  client-surface bounds fixed the final coordinate assertion.
- The fixture's button-event serialization initially raised an exception.
  Its corrected raw journal recorded both final button presses at `(200, 300)`.

## Remaining acceptance gates

This result does not cover GTK4, Qt6, LibreOffice, Chromium/Ozone, Electron,
XWayland, subsurfaces, Unicode/IME, raw text, modified pointer gestures,
fractional scaling, multiple monitors, multi-agent concurrency, or physical
Omarchy hardware. Earlier prototype results do not certify this plugin build.

Further native tests must exercise target loss, geometry changes during
delivery, lock/DPMS transitions, primary input entering a target or sibling
window, and held-state cleanup during expiry/disconnect/unload. The experiment
allows one short-lived lease and conservatively refuses a target when primary
input is in the same Wayland client. Normal permission UX, a stable protocol,
production resource lifetime, and the RFC decision remain open.

The complete canonical desktop matrix has not run for this candidate. The
Hyprland foreground-activation adapter and capability-aware observation hints
also remain separate Driver work: a successful isolated-input fixture does
not establish foreground parity or remove the stock Wayland fallback hints.
Keep this PR draft until its review and broader acceptance scope are decided.
