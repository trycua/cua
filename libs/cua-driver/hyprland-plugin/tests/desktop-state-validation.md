# Desktop-state safety validation

On 2026-09-05, five matched fault-only controls and five Driver MCP background
drag episodes passed in a disposable Omarchy Cua Fleet VM. Moving, resizing,
locking, turning the display off, or destroying the target cancelled the
background action within the 750 ms test bound. Agent input state and authority
cleared, old grants were refused, and freshly approved recovery succeeded.

Each action produced no additional foreground effects compared with its
fault-only control. This does **not** mean every fault leaves the foreground
unchanged: the real session lock itself released the foreground grab and changed
focus. The action and control had the same measured effects.

This is focused evidence for [draft #3572](https://github.com/trycua/cua/pull/3572),
not a supported release or production certification. The normal plugin build
remains discovery-only. The [earlier real-app](realapp-validation.md) and
[held-input lifecycle](lifecycle-validation.md) results retain their own source
identities; this pass does not recertify them.

## Exact artifacts

All ten final episodes used the same native binaries and final executable test
helpers. Changes after the native source SHA were confined to tests and
documentation. The commits adding this report change documentation only.

| Component | Tested identity |
| --- | --- |
| Native Driver and plugin source | `fc225fef9bbaa07c9953e1d80cf6fd82d66bd4fd` |
| Final executable test-helper source | `9b1c71c67145173d902c6638805afb355b8eb305` |
| Omarchy | `4.0.1-1`, disposable KVM Fleet VM |
| Hyprland | `0.56.2-1`, commit `efb50993780079460b0cbed1363e2166a2de1d9f` |
| Portal | `xdg-desktop-portal-hyprland 1.4.1-1` |
| Driver source and installed version | `0.23.2`, custom task-local source build, not the stock release |
| Plugin | `0.1.0`, explicitly enabled input experiment |
| Toolchain | GCC `16.1.1`, CMake `4.4.3`, Rust `1.97.1` |
| Display | One 1920 × 1080 output, scale 1 |

The SHA-256 artifact digests are:

- Native source archive: `7b7643136e80e67422fa7f37104aeed15c022529b2fd9d24ba86c4f073ffc3b4`.
- Driver: `57b1de8296cc41e6e8121b181056bcb1c1f20cc9d531becbf0f3221c96992b1b`.
- Plugin: `7e105e0701fe0d1104f7389e654dff59c7d7503e4e380ced1638e8295ed20358`.
- Primary-pointer fixture: `32e52f55ac1be4261ba32b9328264c04e28a2f8da464f4d1f1c4a50550751cdf`.
- Session-lock fixture: `237b694749137a518f47eb73691719d2c183ebc7649a16935e071b6aba11b856`.
- Final test-helper archive: `d63a18294b40194cab5b7255eb51386a9636583666156d70f046d38d394750e1`.
- Retained evidence archive: `9e40e9136310b77798c23d91de628a196fedf15d72405f24391616cade2c44be`.

The installed Driver and both running service executables had matching digests.
One service performed actions; a separate service from the same binary recorded
video and observed state. The action service had recording disabled. Separate
connections to one service would not isolate synchronous recording overhead.
The ordinary Driver installation was preserved. No host installation or Fleet
image publication occurred.

## Geometry regression

The previous drag check compared against mutable cached target geometry. An
intervening target request or approval refresh could update that cache before
the timer compared revisions, masking movement during an active drag.

The experiment now pins each drag to the geometry revision accepted at its
start. The regression test refreshes the target cache before checking the active
drag and requires cancellation. Its assertions remain active in Release builds
with `NDEBUG` defined.

## Method and foreground comparison

[`desktop_state_live.py`](desktop_state_live.py) drives native GTK raw-event
fixtures through real Driver MCP `drag` and `press_key` calls. The Fleet SDK
provides VM transport and setup, not the background input path. An independent
virtual primary pointer holds a real Wayland grab in the foreground fixture.
It is not physical mouse input.

Each control/action pair binds the same fault, foreground identity and geometry,
native source, module and Driver digests, adversary binaries, test-helper bytes,
and observer-service arrangement. Background targets are verified from fresh
Driver snapshots. The operator signs short-lived target/connection/epoch-bound
test grants outside the guest.

The action episode waits for the application's received button press before
injecting a fault into a two-second drag. Both the typed refusal and observed
cleared state must arrive within 750 ms of fault dispatch, early enough that
natural completion cannot pass. Cleared state means no lease, active drag,
held button or keys, or agent pointer/keyboard focus. Surviving targets must also
journal a matching button release. Unexpected keys or scroll events fail the
drag episode.

The comparison uses foreground application deltas, Wayland wire events,
compositor input/focus traces, the ordered cursor path, and exact active-window,
workspace, and cursor readback. Matching endpoints alone cannot pass. For lock,
the control captures the compositor's own release and focus changes; the action
must not add effects beyond those.

Destruction requires independently observed target disappearance. Both lanes'
seat, pointer, and keyboard resource counts must return to a baseline measured
before launching the target. A destroyed surface cannot acknowledge release, so
this case does not claim an application-received release. Recovery uses a new
process identity even if Hyprland reuses the old window address.

Every action refuses the old grant, refuses unapproved recovery input, requires
a distinct fresh grant, and then delivers exactly one Escape press/release pair.
The recovery checks also require unchanged foreground application and compositor
state and no foreground wire-input leakage. See the
[runner instructions](README.md#desktop-state-fault-controls) for setup and
watchdog requirements.

## Recorded results

All five controls and all five action episodes passed with no cleanup errors.
The following times are individual observations measured from fault dispatch,
not a latency distribution or performance guarantee. Observed cleared-state
time includes the status-query delay; it is not the exact native cleanup instant.

| Fault | MCP refusal | Cleared state observed | App received release | Result |
| --- | ---: | ---: | ---: | --- |
| Move | 241.08 ms | 439.13 ms | 242.88 ms | `cancelled` |
| Resize | 189.02 ms | 354.70 ms | 188.64 ms | `cancelled` |
| Real session lock | 60.96 ms | 130.60 ms | 76.52 ms | `cancelled` |
| DPMS off | 64.40 ms | 263.39 ms | 66.47 ms | `cancelled` |
| Target destruction | 7.03 ms | 299.71 ms | Not receivable | `stale_target` |

All ten results were recomputed offline from the raw application journals,
action responses, status records, primary traces, and window-manager snapshots.
The ten original recordings fully decoded. Uniform video samples and selected
fault/recovery frames were visually inspected, including target movement,
resizing, disappearance, black lock/display-off intervals, and recovery.

Video is supporting evidence, not the timing oracle. The recordings are not
frame-exactly synchronized with the event journals. An attempted cross-clock
conversion produced invalid sample positions; visual inspection instead used
video-local timestamps without altering the original recordings. Event latency
comes from the monotonic test journals, not the video timeline.

The private evidence bundle retains original videos, per-file digests, passing
and failed episodes, and setup/build records. Operational logs, grants, and
machine-specific evidence are not committed to the public repository.

## Supporting checks and retained failures

- All 79 Python helper tests passed on the host and Fleet guest.
- All seven portable CTests passed on the native guest and in a host Release
  build, including the cached-geometry regression with `NDEBUG` enabled.
- Two initial move episodes exceeded the 750 ms MCP-response bound at roughly
  865 ms and 985 ms while synchronous action recording was enabled. Native
  release was prompt, but those episodes failed and remain retained. The final
  matrix uses an independent observer service and keeps the same 750 ms bound.
  It makes no passing recording-inclusive action-service latency claim.
- A lock fixture without surfaces encountered Hyprland's missing-surface grace
  period and exceeded its acknowledgment deadline. The final fixture supplies
  opaque black shared-memory surfaces for every advertised output and waits for
  real protocol lock/unlock acknowledgments. Recovery was graceful; the lock
  client was not killed to unlock the session.
- Newly launched targets could take primary focus during setup. The plugin
  correctly revoked grants for that primary-client conflict. Setup now restores
  and asserts the exact foreground actor before initial or recovery approval.
  Driver exact-window foreground activation explicitly refuses on Hyprland;
  the independent primary-seat fixture performed this authorized setup, bracketed
  by Driver observations. Setup is not background-isolation evidence.

Ordinary hosted CI supplements these focused native results. It does not certify
compositor behavior or replace the canonical desktop matrix.

## Remaining gates

This pass covers native GTK fixtures, not a new real-app or physical-host
certification. It does not establish Chromium/Ozone, Electron, GTK4, Qt6,
XWayland, subsurface, fractional-scaling, multi-monitor, or broad input-method
compatibility. Actions exercised one lane per episode; destruction inspected
both lanes' resource counts, not concurrent two-lane fault delivery.

Lock and DPMS are sampled states. An off/on transition entirely between samples
is not proven to revoke authority. Escape recovery uses an atomic key packet,
not interruption of a held-key stream. These remain separate test and design
gates, along with the complete canonical desktop matrix at the final candidate.

Production authorization UX, resource lifetime and hot-reload behavior,
capability-aware observation, and the protocol/RFC decision remain open. Keep
the PR draft. This evidence does not authorize merge, release, image publication,
or production rollout.
