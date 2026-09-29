# Live host feasibility result: KWin 6.7.5

**Outcome: a post-confirmation focus takeover delivered the key to the wrong
window. Production input remains disabled; this is not a passing safety proof.**
The RFC remains `review`. This does not unblock #3972 or #1982.

## Source and environment

Base checkout: `93f7afc89a09e7f9d39da22990e26036a74adfbd`, already containing
upstream main `fe9b0c6d3c0307537bccbfd69f4f801ec219adc0`.
The separately applied helper and Driver patches, fixture, scripts and tested
binary have SHA-256 identities in [the machine-readable evidence](evidence/host-6.7.5.json).
No shipped production source was changed. `kwin_helper::available()` remains
false, AX routes are unchanged, and exact/background input remains refused.

Actual running host: CachyOS, KDE **Wayland**, desktop UID 1000; KWin/Plasma
**6.7.5**, Qt **6.11.2**, KCoreAddons **6.30.0**, portal **1.22.1**, KDE portal
**6.7.5**, PipeWire **1.6.9**. These are **not Plasma 6.6 results**.
KWin owned both `org.kde.KWin` and `org.cua.KWinTarget` as `:1.8`, PID 1139.
Builds and probes ran as the desktop user, not root.

Recovery backup: `/home/netbos/Documents/GitHub/cua-recovery/20260929-203231/`.
It contains verified Git bundle, refs, staged/unstaged patches, untracked archive
and Git state; its potentially sensitive contents stay local.

## Bridge and supported interfaces actually exercised

[helper-identity.patch](helper-identity.patch) adds only
`org.cua.KWinTarget.GetIdentitySnapshot()` returning helper-instance generation
and exact `internal_id` UUID alongside the existing v1 records. It does not add
an activation or input method. `GetVersion() == 1` and `GetWindows()` remain
compatible. A fresh UUID on every helper construction prevents token reuse
under the same KWin D-Bus owner from becoming authority.

[activation_probe.py](activation_probe.py) exercised:

- `/Scripting`, `org.kde.kwin.Scripting.loadScript(filePath, pluginName)`;
- `/Scripting/Script<ID>`, `org.kde.kwin.Script.run()`;
- `workspace.windowList()`, `window.internalId`, `window.pid`;
- `workspace.windowActivated.connect(callback)` before requesting activation;
- `workspace.activeWindow = exactObject`, followed by exact-object read-back;
- supported scripting `callDBus` to a nonce-specific observer, verifying the
  callback sender against the trusted KWin unique owner;
- `unloadScript(pluginName)` and `isScriptLoaded(pluginName)` cleanup checks.

Title labels were used only to initially choose A/B from the trusted adapter
list. Mutation then carried PID, numeric token, helper generation and UUID;
there is no title, PID-only, geometry or window-list-order fallback.

The running compositor did not discover the user-local proof module. Following
explicit user approval, the separately named `cua_kwin_3506_proof.so` was
installed temporarily in the system Qt/KWin plugin prefix. It was loaded with
`org.kde.kwin.Effects.loadEffect`, later unloaded and removed from both prefixes.
Existing helper modules were not replaced; the desktop was not restarted.

## Admission, transport and chronology

The [separately applied Driver prototype](driver-prototype/README.md) is enabled
only in its scratch build with `--cfg cua3506_prototype` and `portal-input`.
It invokes normal typed `CuaDriver::press_key` once with explicit foreground
mode. Standard Driver admission precedes `PressKeyTool`; no dummy admission or
external raw-input call substitutes for it.

Transport is the existing ashpd RemoteDesktop `CreateSession`,
`SelectDevices(Keyboard | Pointer, ExplicitlyRevoked)`, `Start`, `ConnectToEIS`,
and reis/libei keyboard path. `LIBEI_SOCKET` was unset; all XDG configuration,
state and data written by the input harness were isolated in its scratch area.
Portal readiness and consent completed before the parent requested activation.
Only one unmodified `a` press/release pair was emitted per operation; the
requested pointer capability was never used.

At the worker seam, after keyboard readiness, a new trusted unique-owner
snapshot validates exact identity, generation, active and minimized state.
Closure/deadline is checked again immediately before the first key request.
The worker marks `may_start` before requesting the press, frames both
transitions and checks client flush. A flush acknowledgement is not compositor
or application receipt. Independent GTK raw-event journals provide receipt.
All timeline timestamps use CLOCK_MONOTONIC nanoseconds.

## Observed cases

Two native GTK3 windows shared fixture PID 1615294 and had independent counters.

| Case                                            | Observation                                                                                            |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Explicit A selection, B initially active        | A received one press/release; B received zero                                                          |
| Explicit B selection                            | B received one press/release; A received zero                                                          |
| Focus takeover before final worker confirmation | Refused `closed/stale/inactive target`; both received zero                                             |
| Closed local operation                          | Refused `closed`; zero emission and zero fixture events                                                |
| Expired operation                               | Refused `expired`; zero emission and zero fixture events                                               |
| Exact/background operation                      | Existing `background_unavailable`; zero events                                                         |
| Lost worker acknowledgement                     | Actual reply suppressed after emission; returned `unknown; never retry`; A received one pair           |
| Reconnect/re-resolution of that operation       | Parent sequence 9 could not be reopened: exclusive ledger failed before Driver construction; no resend |
| Genuinely new admitted operation                | Sequence 10 re-resolved A and received exactly one new pair                                            |
| Focus takeover after final confirmation         | **A received zero, sentinel B received one pair**; retained as wrong-window delivery                   |

The activation-only run additionally exercised missing/closed token refusal,
window destruction/recreation, dropped confirmation timeout, wrong active
window, helper unload/reload within the same KWin process, stale generation
refusal, and deliberate fresh selection after recovery. It called no input
transport. Duplicate identity rejection has headless coverage, not an invented
live duplicate UUID. Initial discovery-readiness and misplaced prototype-hook
failures were corrected before input emission; their initial traces are
preserved locally. Operations 2/3 encountered the unchanged production refusal,
not the intended closed/expiry seam; operations 4/5 exercised that seam after
correction. The contract check now verifies interception of `PressKeyTool`,
not its `TypeTextTool` sibling.

### Timing

For successful A/B and new-operation runs, final worker-confirmation to first
key emission was **20.749–32.140 microseconds**. Emission to client-flush
acknowledgement was **42.740–82.795 microseconds**. These are client-side spans,
not a claimed safe threshold or compositor processing time. The retained
fixture journal provides press/release receipt times independently.
The deliberate post-check takeover inserted **9.653827 ms** between final
confirmation and emission and leaked the pair to B. Removing that deliberate
wait does not make a focus snapshot and later global delivery atomic.
Activation request/callback/final-read timestamps and every operation are
included in the linked JSON, without conflating setter return, confirmation,
submission, flush and application effect.

## Recording and limitations

External recording used real portal/PipeWire window streams, no monitor source
or audio, composed using GStreamer/VP8. The user confirmed selecting Synthetic
A followed by Synthetic B. **Frame inspection nevertheless showed A in both
composed panes.** The genuine recording therefore documents A and the test
interval but does **not** provide simultaneous visual sentinel evidence.
This recorder limitation is not hidden by the successful B event journal.

Local video (419 seconds, 1280×480, VP8, 13,602,567 bytes):
`/home/netbos/Documents/GitHub/cua-evidence/3506/20260929-host-6.7.5/host-6.7.5-proof.webm`.
No private desktop source was requested. The inspected frame shows only the
synthetic fixture. The video is kept local pending supported GitHub attachment;
no unrelated release or public file-host upload was created.

Not certified: pointer hit-testing, dragging, scroll, hotkeys, popups, multi-key
bursts, held-key cleanup under interruption, concurrent Driver-session
revocation during portal startup, daemon-wide durable deduplication, independent
activation-denial behavior, and actual loss of the outer SDK acknowledgement.
The lost-ack test covers the worker reply, not the latter. Separate-process
windows were not exercised; same-process exact selection was. Pointer testing
and additional raw-input experiments stopped after the observed keyboard leak.
AX source and unit contracts remain unchanged; no separate live AX smoke is
claimed. An isolated Plasma/KWin 6.6.4 Wayland guest was subsequently verified
read-only (package versions, portal properties and scripting introspection),
but no helper, synthetic input, or recording was run there. The observed
global-delivery leak does not become safer by changing the compositor version;
exact-version input proof requires a different target-bound primitive.

## Verification and cleanup

- Seven headless Python checks pass; structural patch check and SDK self-check
  pass; two focused Rust prototype checks pass; both helper and SDK builds pass.
- Full `platform-linux --features portal-input --lib`: **583 passed, 1 failed,
  9 ignored**. The unchanged
  `overlay::tests::unreported_arrival_releases_the_waiter_after_the_cap` failed
  its `started.elapsed() >= ARRIVAL_WAIT_CAP` assertion, also with one test
  thread. No unrelated production fix was made and this is not reported green.
- Production source diff is empty. No expensive desktop matrix was run or
  misrepresented as certification.
- Recorder exited cleanly and closed its portal sessions. Fixture quit;
  per-operation input processes exited. Temporary scripts and helper unloaded;
  the separately named installed proof module was removed. Source, journals,
  hashes and video were preserved outside transient scratch storage.

The supported APIs establish exact activation and observation, and ordinary
bursts reached their chosen synthetic windows. They do **not** establish
race-safe destination binding for global portal/libei input. Request a
maintainer decision on these observed limits; do not infer acceptance.
