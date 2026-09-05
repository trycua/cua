# Concurrent real-app input validation

On 2026-09-05, two independent Cua Driver MCP connections edited LibreOffice
Calc and Inkscape in an Omarchy Fleet VM while a foreground fixture retained
the primary pointer grab. Concurrent drags overlapped for 1.49–1.77 seconds.
The parked and moving-pointer cases recorded no uncommanded primary motion,
focus changes, foreground input leakage, or unexpected button releases.

This is a nonshipping compatibility experiment in [draft
#3572](https://github.com/trycua/cua/pull/3572), not production or bare-metal
certification. The normal plugin build remains discovery-only. This record
extends the earlier [single-seat GTK3 fixture result](input-validation.md);
it does not change that result's tested revision or historical limitations.

## Exact artifacts

Native binaries came from committed source. The later proof-helper commit
changes test orchestration and assertions, not either native binary.

| Component | Tested identity |
| --- | --- |
| Native source | `571bbe356e41d531e13182363575d566b68b5e16` |
| Proof helper source | `354c934ec6705a3fe039f4c2ae1da49e8fd22ec9` |
| Omarchy | `4.0.1-1` |
| Hyprland | `0.56.2-1`, commit `efb50993780079460b0cbed1363e2166a2de1d9f` |
| Portal | `xdg-desktop-portal-hyprland 1.4.1-1` |
| Driver source and installed version | `0.23.2`, task-local source build |
| Plugin | `0.1.0`, explicitly enabled input experiment |
| Compiler | GCC `16.1.1`, matching the compositor; shared C++ runtime |
| LibreOffice | Arch package `26.2.5-3`, native Wayland |
| Inkscape | Arch package `1.4.4-6`, native Wayland |
| Recorder | `wf-recorder 0.6.0-2`, invoked through Driver recording |
| Display | One 1920 × 1080 output, scale 1 |

The artifact SHA-256 values are:

- Native source archive: `b3abafc90a02a38c75c074191e493687f5845c1847c6119076adf26ca04a8b47`.
- Plugin: `f4828207ac3af02425a901180bed922bd16ac5cec32f78aa28eb06ff69e53269`.
- Driver: `2c924c7bd63bc167d4ac9c805932ff20a0d4ceeb06cc8f6e7a858788ce945dc9`.
- `realapp_proof.py`: `b1545cf159ec9e97e1cd2f9e831db71259e07470804aab4474ba16d7be71b236`.

The loaded plugin's ABI fingerprint matched
`efb50993780079460b0cbed1363e2166a2de1d9f_aq_0.14_hu_0.14_hg_0.5_hc_0.1_hlg_0.6`.
The installed executable and the running service's `/proc/PID/exe` had the
same Driver digest. Installation used the repository's `install-local.sh`
with task-local guest paths. Existing Driver installations were preserved;
no host installation, Fleet image publication, merge, or release occurred.

## What the tests measure

[`realapp_proof.py`](realapp_proof.py) runs reviewed, deterministic plans through
two separate MCP processes. Each owns a private lifecycle, an independently
leased synthetic seat, and a visible agent cursor. Public session labels are
not credentials. An external operator signs short-lived grants bound to the
connection, target, and epoch; the private signing key never enters the VM.

The desktop uses Omarchy's tiled layout: a foreground raw-event fixture beside
Calc and Inkscape. Every background input call is bracketed by fresh Driver
window snapshots. Plans pin the observed window identities and geometry and
refuse a stale layout. The canvas and spreadsheet gestures deliberately
exercise the raw plugin route. Every delivered action must report
`route: synthetic_events`; the test cannot substitute an AT-SPI or foreground
action and still pass.

The independent app oracles read the files saved by Ctrl+S. They assert the
numeric value of Calc cell B3 and a predeclared translation range for Inkscape's
`agent-shape` rectangle, with unchanged width and height. Before/after files are
retained. A successful transport response or cursor animation is not task proof.

[`primary_grab.c`](primary_grab.c) supplies an independent virtual primary
pointer, holding the foreground button throughout the measurement. The moving
case commands a loop inside that window. This is not a physical mouse or a
human gameplay test.

[`primary_trace.py`](primary_trace.py) analyzes event-level compositor
instrumentation, not periodic screenshots alone. It requires a complete,
ordered trace with working hooks, start/stop markers, and no overflow or
timeout. Incomplete telemetry is inconclusive. The checks cover:

- Every observed primary cursor position, including transient warp-and-return.
- Exact ordered correspondence with commanded motion in the moving case.
- Primary pointer/keyboard focus and enter/leave events.
- Foreground button, key, and scroll leakage, including unexpected releases.
- Actual overlapping synthetic drag intervals, not overlapping animations.
- Balanced synthetic button/key press and release counts in each lane.

The foreground fixture's independent raw-event journal and compositor
identity/workspace readback supplement the trace. HUD counters are cumulative;
the harness compares baseline deltas for each run.

## Results

Each positive row has complete telemetry, no primary focus/leak/release
violations, balanced synthetic input, and an empty cleanup-error list.

| Case | Trace events | Primary motion | Drag overlap | App or control result | Video duration |
| --- | ---: | --- | ---: | --- | ---: |
| Parked primary | 168 | None | 1491.820 ms | Eight actions; B3 = 96; shape moved and saved | 37.083 s |
| Moving primary | 731 | All 276 commands matched; no extra motion | 1537.106 ms | Eight actions; B3 = 75; shape moved and saved | 37.231 s |
| Warp-and-return control | 7 | One uncommanded 50 px excursion detected | Not applicable | Detector correctly failed isolation despite identical endpoints | 17.441 s |
| Cancel one lane | 91 | None | 337.783 ms | Calc drag cancelled; Inkscape finished and saved | 22.222 s |
| Global Stop | 50 | None | 316.710 ms | Both in-flight drags returned `stopped` | 22.154 s |
| Parked repeat after Stop | 181 | None | 1771.956 ms | Eight actions; B3 = 96; shape moved and saved | 37.097 s |

Cancel and Stop were issued only after the trace showed both drags had started.
After Cancel, further input on that lane required new operator approval while
the other lane remained usable. After Stop, both lanes required new approval.
Neither left a synthetic button or key held. These are test-operator protocol
controls, not a claim of finished user-facing cancellation UX.

Stop is not undo: partial app changes made before cancellation remain. The
repeat plan accounted for the observed unsaved partial shape movement and
declared its saved-file translation range before execution.

The negative-control test passes only when isolation detection fails for the
intended excursion. It issues no background app actions and must not be counted
as a successful isolation run.

## Recordings and retained evidence

Every case has an uninterrupted original `recording.mp4`, MCP before/after
snapshots, a guest-monotonic action timeline, the complete compositor trace,
analysis JSON, and cleanup results. Successful app-edit cases also retain the
saved documents. The recordings contain synthetic test content. Raw operational
logs and authorization packets are not committed to the public repository.

Video start-call and start-response timestamps bracket recorder startup; they
do not provide frame-exact synchronization with the compositor trace. The live
HUD provides visual correlation. All six original H.264 files decoded fully,
and representative interaction and result frames were inspected. These are
variable-frame-rate recordings. To check them without introducing null-muxer
timestamp-rounding warnings, preserve the source timebase:

```bash
ffmpeg -v error -i recording.mp4 -fps_mode passthrough -enc_time_base demux -f null -
```

## Supporting checks

- Six native plugin CTests passed: protocol, status, Linux transport, both
  mocked API variants, and the CMake API probe.
- Native Rust tests passed: 43 common runtime-isolation, four window-target,
  seven `hyprland_input`, three `experimental_`, and two `primary_seat` tests.
- All 36 Python helper tests passed with the cryptography dependency available.
- The final module passed [`nested_lifecycle.py`](nested_lifecycle.py): two
  owned nested compositors, six discovery mutation refusals per start, clean
  restart, old socket/connection closure, fresh socket/epoch, sibling liveness,
  and clean shutdown. The parent module was not reloaded by this test.
- Final inspection found the original parent and app identities intact, both
  leases inactive, the primary button released, and the intended Driver still
  running. Fresh Driver desktop capture verified the scene after lifecycle
  cleanup.

These checks supplement the real-app evidence. They do not replace the
repository's complete canonical desktop E2E matrix, which has not run for this
candidate.

## Setup failures and remaining limits

Two failed attempts were retained rather than counted as passes:

1. Approval before establishing the foreground hold allowed primary focus to
   revoke the app lease. Input refused with `pending_operator_approval`. The
   committed helper establishes the foreground hold before applying approval.
2. Apps that survived an earlier plugin reload did not bind the replacement
   synthetic input resources. Input refused with `client_not_bound`. Saving
   the test document and restarting both apps fixed the test setup. App
   survival after reload is not proof that input can resume without a restart.

Exact-window foreground hotkeys also refused with `foreground_unavailable`
because Hyprland exact-address activation is not implemented. Explicit Driver
desktop input was used only for authorized setup, outside the background proof.

This candidate supports two bounded experimental lanes, not arbitrary numbers
of synthetic seats. It conservatively rejects primary/agent conflicts within
the same Wayland client. Its resident `NODELETE` handlers remain a test-only
unload workaround with a bounded reload count, not a production lifetime design.

Unproven areas include Chromium/Ozone, Electron, GTK4, Qt6, XWayland,
subsurfaces/popups, Unicode/IME/raw text, modified pointer gestures, fractional
scaling, multiple monitors, target loss, mid-action geometry changes,
lock/DPMS transitions, and held-state expiry/disconnect/unload. The earlier
single-seat grant/replay tests are historical evidence, not fresh coverage of
every two-lane interaction. Broader protocol tests, permission UX, resource
lifetime review, [RFC #3550](https://github.com/trycua/cua/issues/3550), and the
physical Omarchy acceptance gate remain open. Keep the implementation draft.
