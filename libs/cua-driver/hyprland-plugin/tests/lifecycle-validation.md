# Held-input lifecycle validation

On 2026-09-05, the experimental Hyprland input path released a held background
button after operator Stop, agent disconnect, lease expiry, and plugin unload.
The recorded runs observed release in 18–62 ms, with the expiry interval measured
from compositor resume. The foreground fixture retained its grab and received
no pointer or keyboard events during each measured fault window.

This is focused reliability evidence for [draft
#3572](https://github.com/trycua/cua/pull/3572), not production certification.
The normal plugin build remains discovery-only. The earlier
[Calc and Inkscape results](realapp-validation.md) retain their own source SHA;
this pass does not recertify those applications.

## Exact artifacts

The native build uses committed source. Later commits change test orchestration
only; the report and test instructions add no native-code changes.

| Component | Tested identity |
| --- | --- |
| Native Driver and plugin source | `8882780b79514e35e5f5601800a740c17c95505c` |
| Final lifecycle helper source | `b8c7ad0c578d260bc743783824a6c45cef9106fa` |
| Baseline Driver source | `e71ed83f43e3d8cd2963c040b8d7d287bcbc9ba8` |
| Omarchy | `4.0.1-1`, disposable Cua Fleet VM |
| Hyprland | `0.56.2-1`, commit `efb50993780079460b0cbed1363e2166a2de1d9f` |
| Portal | `xdg-desktop-portal-hyprland 1.4.1-1` |
| Driver source and installed version | `0.23.2`, task-local source build |
| Plugin | `0.1.0`, explicitly enabled input experiment |
| Compiler | GCC `16.1.1`, matching the compositor |
| Recorder | `wf-recorder 0.6.0-2`, task-local installation |
| Display | One 1920 × 1080 output, scale 1 |

The SHA-256 values are:

- Native source archive: `533fa705e187655b6d4eb06657963571b6be43ed190f06c7b600820490261b5a`.
- Driver: `2680d5f3e013b845b5f9c8bad625a551a5049f62f92f36dd6ae21c6a9b5ae55a`.
- Plugin: `d7e9f050da45819304ca69b11dae1cf887ddb931efe1fd3747340fa710ddcd14`.
- `input_lifecycle_live.py`: `801996a702cfa0800e7790f2c0b87bd223c53c427cfeb0ac0665275ba9bb29ed`.

The installed Driver and the running service executable had identical digests.
Driver `get_config` reported the native source SHA above. The release installation
was preserved; the candidate and recorder were installed only in the guest's
task directory. No host installation or Fleet image publication occurred.

## Regression and fix

The baseline disconnect test released the button **1962.39 ms** after the agent
MCP proxy disconnected: the two-second drag completed instead of cancelling
promptly. It failed the 750 ms release bound even though foreground isolation
held and eventual release was balanced.

The common lifecycle registry now exposes a read-only pending-termination
signal. The experimental Linux adapter checks its private lifecycle identity
before and after socket readiness, polling at most every 25 ms while waiting.
On cancellation, it drops only that lifecycle's owned connection, allowing the
plugin to release input before the normal dispatch-cleanup barrier finishes.
Other platform adapters and the common final-cleanup barrier are unchanged.
The plugin's timer-expiry refusal now reports `lease_expired` explicitly.

## Test method

[`input_lifecycle_live.py`](input_lifecycle_live.py) drives two native GTK
DrawingArea fixtures through independent agent and observer MCP connections.
Every background action uses a reviewed target and fresh Driver snapshots.
The host operator signs a short-lived connection/target/epoch-bound grant;
the private key never enters the guest.

An independent virtual primary pointer holds a real foreground Wayland grab.
Application journals prove receipt of the synthetic press and matching release.
The foreground client's Wayland wire log detects input events, including
transient motion, enter/leave, buttons, axes, keys, and modifiers. Foreground
state and compositor cursor/focus/workspace readback supplement that log.
Matching cursor endpoints alone cannot pass the test.

Stop, disconnect, and unload occur within 900 ms of the application's actual
press in a two-second drag. Natural completion cannot satisfy the 750 ms release
bound. RPC entry is not the press timestamp: recording and snapshots introduced
859–1000 ms of pre-delivery work in these recorded runs.

Expiry uses a 500 ms drag and deliberately freezes the disposable compositor
past the grant deadline. A separate PID-fd watchdog resumes the exact process
if the caller fails. The measured release interval starts immediately before
SIGCONT, and the action must refuse with `lease_expired`. This proves expiry
cleanup after resume, not continued desktop responsiveness during the stall.
See the [runner instructions](README.md#held-input-lifecycle-faults).

## Recorded results

Each row passed with a balanced synthetic release, zero foreground pointer or
keyboard events, a preserved foreground grab, and no cleanup errors. Every row
required fresh operator approval and refused replay of the previous grant.
Disconnect and reload also refused the ended public session label; a new
transport used a new label to request authorization.

| Fault | Release latency | Action outcome | Original video duration |
| --- | ---: | --- | ---: |
| Operator Stop | 26.87 ms | `stopped` | 9.044 s |
| Agent MCP disconnect | 61.76 ms | Transport closed | 10.662 s |
| Lease expiry | 18.18 ms after resume | `lease_expired`; compositor stalled 1056.27 ms | 15.426 s |
| Plugin unload/reload | 60.58 ms | `plugin_shutdown`; new epoch after reload | 13.820 s |

The four original 1920 × 1080 H.264 recordings fully decoded, and representative
frames plus fresh final Driver snapshots were inspected. Video is supporting
evidence; application and wire timestamps establish the release bounds. Reload
briefly displayed an unknown-plugin-config warning while the module was absent;
it cleared after reload. This is not a claim of visually seamless reload.

Earlier unrecorded candidate runs also passed: Stop 6.78 ms, disconnect 19.57 ms,
expiry 27.39 ms after resume, and unload 39.16 ms. Those runs and the baseline
have trajectory snapshots, not videos. They are individual observations, not a
latency distribution or performance guarantee.

Raw operational logs, grants, and machine-specific evidence remain outside the
public repository. The retained private bundle includes both passing and failed
attempts. No raw traces or authorization packets are needed to run the committed
helpers against new synthetic fixtures.

## Supporting checks and failed setup attempts

- All 18 common lifecycle and nine Linux `hyprland_input` unit tests passed
  natively. The tested Rust source tree matched the committed native archive.
- All six native plugin CTests passed.
- All 44 Python helper tests passed locally and in the Fleet guest with the
  final helper. Rust formatting and diff checks passed.
- Initial candidate disconnect released promptly but failed a test assumption
  that a new transport could reuse an ended label. The corrected helper requires
  a new lifecycle and fresh approval.
- An initial expiry attempt exhausted its lease before pressing. The helper now
  reserves snapshot time and requires an actual press before injecting the fault.
- Initial trajectory runs omitted explicit video. The helper now requires
  `video_active: true` and a nonempty finalized video file when recording is
  requested. The fresh image also needed a task-local recorder dependency.
- The first recorded disconnect attempt used RPC entry for its early-fault
  threshold. The helper now uses the application's received-press timestamp.
- One Stop attempt timed out waiting for a grant and cleaned up. A new run with
  fresh approval passed; the timed-out attempt is not counted as evidence.

## Remaining gates

The later [desktop-state pass](desktop-state-validation.md) separately covers
target loss, geometry changes, lock, and DPMS with its own candidate and matched
controls. It does not change the scope of the lifecycle results recorded here.

These tests prove held-pointer cleanup only. Atomic key packets do not expose a
held-key stream. They do not establish physical-mouse behavior, target-loss or
mid-action geometry handling, lock/DPMS behavior, broad app/toolkit compatibility,
fractional scaling, multiple monitors, or the canonical cross-platform desktop
matrix.

Reload proves cleanup, epoch replacement, and refusal of stale authority. It
does not prove surviving clients automatically bind replacement seats or accept
new input. The bounded resident-handler workaround remains experimental.
Production resource lifetime, authorization UX, and the protocol/RFC decision
remain open. Keep the PR draft; this record does not authorize merge, release,
or rollout.
