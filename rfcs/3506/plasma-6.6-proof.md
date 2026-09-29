# Plasma 6.6 foreground feasibility proof

Status: **Plasma 6.6.4 executed; post-confirmation input leaked to B.**

## Completed isolated 6.6.4 run

The existing sidecar was built inside a QEMU/KVM Kubuntu guest (8 GiB,
4 vCPU), from `2b99a150d00aa35ded968b251901b6dee1ce3504`, with the
separately applied helper/Driver patches. Running KWin and Plasma were
6.6.4, Qt 6.10.2, guest UID 1000, Wayland `wayland-0`. Helper, fixture,
portal and libei transport shared the guest session. No host helper was
installed and no production source was changed; RFC remains `review`.
The existing supported scripting and portal calls below were used unchanged.
The runner gained `--binary`, `--helper-name`, `--ungated` and a stale-generation
case; those sidecar-only changes are included in this PR.

| Case                             | Result                                                                            | Evidence           | Limitation                                                                    |
| -------------------------------- | --------------------------------------------------------------------------------- | ------------------ | ----------------------------------------------------------------------------- |
| Initial attempts                 | Portal readiness timeout; zero emission and A/B events                            | Operations 1–4     | Harness exit 0 is not delivery                                                |
| A, without worker gates          | A received 1 press/release; B zero                                                | Operation 5; video | Ordinary delivery, not race safety                                            |
| B, without worker gates          | B received 1 press/release; A zero                                                | Operation 6; video | Ordinary delivery, not race safety                                            |
| Precheck focus takeover          | `closed/stale/inactive target`; zero emission and events                          | Operation 7        | Deterministic precheck gate                                                   |
| Helper reload / stale generation | Same KWin owner, new generation; `owner/generation changed`; zero emission/events | Operation 8        | Reload revocation, not every lifetime transition                              |
| Worker reply lost                | A received one pair; result `unknown; never retry`                                | Operation 9        | Not outer SDK reply loss                                                      |
| Replay of operation 9            | Exclusive ledger refused before Driver construction                               | `replay-9.log`     | Parent ledger, not daemon-wide deduplication                                  |
| Background                       | Exact code `background_unavailable`; zero submission/emission/events              | Operation 10       | Separate exact-mode request not run: existing harness exposes background only |
| Postcheck focus takeover         | **A zero, B received one pair intended for A**                                    | Operation 11       | Deterministic postcheck gate; no video of this case                           |

[Machine-readable evidence](evidence/plasma-6.6.4.json) retains all operations,
activation observations, guest monotonic timing, versions and artifact hashes.
Ungated check-to-emission spans were 15.119 and 15.367 microseconds.
The forced postcheck takeover span was 12.893644 ms. These are measured
client-side spans, not safety thresholds; flush is not application receipt.

The original QEMU framebuffer recording completed cleanly: 500 s, 2500 frames,
full decoding without errors. Inspected frames at 400 s and 495 s show both
independent counters at A=1/1, B=1/1. A four-second control clip was inspected
before input. The final cropped, silent video is retained locally at
`/home/netbos/Documents/GitHub/cua-evidence/3506/plasma66-env/plasma-6.6.4-AB.webm`
for GitHub attachment. It covers operations 1–6 only: recording had already
ended before operations 7–11; no second recorder was started. Do not describe
it as visual proof of the focus-race counterexample. All fixture journals
were copied to the host before guest shutdown. Failed attempts are retained.

The focused Rust overlay test compiled and failed the same assertion,
`started.elapsed() >= ARRIVAL_WAIT_CAP`, in both the prototype and clean
base `93f7afc89a09e7f9d39da22990e26036a74adfbd`. Each ran one test
with `portal-input`, the same RUSTFLAGS and `--exact --test-threads=1`;
both returned 101. This isolates the observed failure from the sidecar patch;
it does not make the full library green. No full suite was repeated.

Unrun: separate exact-mode request, pointer/drag/hotkeys, live AX smoke,
outer SDK acknowledgement loss, and comprehensive desktop/session revocation.
Production input remains disabled. The remainder records the earlier
source review and requirements; its prior outstanding/read-only status is
superseded by the measured run above, not by a passing safety proof.

## Earlier source review and procedure

The [host result](host-6.7.5-result.md) supplies additive identity and isolated
Driver prototype patches, exact API calls, event journals and timing evidence.
It observed wrong-window input after a post-confirmation focus takeover, so it
is not a passing target-safety proof. Its recording also lacks visual sentinel
coverage (both composed panes showed A). No production input was enabled.

The exact-version procedure below is a procedure and source
review for [RFC #3506](../3506-kwin-target-input.md), not certification of a
working transport. The [2026-09-29 maintainer request](https://github.com/trycua/cua/issues/3506#issuecomment-5896356295)
requires the recording and exact tested API calls on
[PR #3507](https://github.com/trycua/cua/pull/3507).

The original source-review environment was Ubuntu 24.04 without a live KWin
desktop. A separate, isolated Kubuntu 26.04 live guest is now reachable as its
normal UID 1000 user. It runs KWin and plasmashell **6.6.4** on Wayland, with
`kwin-wayland 4:6.6.4-0ubuntu1`, `plasma-workspace 4:6.6.4-0ubuntu2`,
`xdg-desktop-portal-kde 6.6.4-0ubuntu1`; its RemoteDesktop portal reports
version 2 and keyboard/pointer device mask 7. Guest D-Bus introspection shows
`loadScript`, `unloadScript`, and `isScriptLoaded`. The official ISO SHA-256 is
`95ce9cf68f13015b9a88bd1ef86fcf7eda77c99979fda48c69e28aa0a84f88ac`.
These were **read-only environment checks**: no helper was built or installed
in the guest, no synthetic fixture, consent, activation, input or recording was
run on 6.6.4. The host's observed wrong-window portal/libei delivery makes
repeating unsafe raw input in the guest unjustified without a new destination-
bound primitive. Production code, AX actions and exact background refusals
are unchanged.

## What source review establishes

KDE's [public scripting API documentation](https://develop.kde.org/docs/plasma/kwin/api/)
describes Plasma 6 APIs, but identifies itself as generated for KWin 6.0. Check
the installed 6.6 build as well. The following candidates were checked against
the KDE `v6.6.0` source tag, commit
`e53bb3be7ecf1735c7739b797f94d66de422ab4d`:

| Purpose                               | Exact API                                                                                             | Source                                                                                                                                                                                                                                               |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Enumerate live script-visible windows | `workspace.windowList()`                                                                              | [workspace_wrapper.h](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/scripting/workspace_wrapper.h)                                                                                                                   |
| Identify the compositor object        | `window.internalId` (UUID), `window.pid`                                                              | [window.h](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/window.h)                                                                                                                                                   |
| Request activation                    | `workspace.activeWindow = target`                                                                     | [workspace_wrapper.cpp](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/scripting/workspace_wrapper.cpp)                                                                                                               |
| Read active window / observe changes  | `workspace.activeWindow`, `workspace.windowActivated.connect(callback)`                               | [workspace_wrapper.h](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/scripting/workspace_wrapper.h)                                                                                                                   |
| Load and unload a JavaScript probe    | `org.kde.kwin.Scripting.loadScript(filePath, pluginName)`, `unloadScript(pluginName)` at `/Scripting` | [scripting.h](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/scripting/scripting.h)                                                                                                                                   |
| Run one loaded probe                  | `org.kde.kwin.Script.run()` at `/Scripting/Script<ID>`                                                | [scripting.cpp](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/scripting/scripting.cpp), [D-Bus XML](https://github.com/KDE/kwin/blob/e53bb3be7ecf1735c7739b797f94d66de422ab4d/src/scripting/org.kde.kwin.Script.xml) |

These APIs establish a candidate for requesting and observing activation.
They do not expose an atomic "verify target and deliver this portal burst"
operation. The existence of a setter, a signal, or a successful D-Bus reply is
not proof of activation, application receipt, or delivery-time target binding.
The signal must be observed before requesting activation; an already-active
target may not cause a new signal. Re-read the exact live object before each
burst rather than treating an earlier notification as durable authority.

## First blocker: preserve the adapter identity

The current [Cua helper](../../libs/cua-driver/kwin-target-helper/kwin_target_helper.cpp)
maps `KWin::Window::internalId()` to a monotonically allocated numeric `token`
in its private `m_tokens` map. `GetWindows()` exports the numeric token and
PID, but **does not export `internalId` or the mapping**. Its v1 API has no
activation method. The [Rust adapter](../../libs/cua-driver/rust/crates/platform-linux/src/wayland/kwin_helper.rs)
verifies the helper owner, KWin process/session ownership, UID, and protocol.

Consequently, the token is not a UUID, X11 window ID, or index into
`workspace.windowList()`. Matching title, app ID, geometry, or PID alone does
not complete the requested proof, especially for two windows of one process.

Before an end-to-end scripting experiment, the spike must provide a reviewed,
generation-aware bridge from the adapter-selected `(pid, token, generation)`
to the exact live KWin object. An additive read-only token-to-UUID mapping is
one candidate to investigate; a supported exact-token activation interface is
another. The separately applied [read-only prototype patch](helper-identity.patch)
now supplies the former for the host experiment, not shipped production code.
Preserve `GetVersion() == 1` discovery and
do not substitute private C++ activation internals for the requested supported
interface. A UUID manually chosen in the scripting console can test the KWin
API in isolation, but cannot satisfy the trusted-adapter requirement.

A D-Bus unique owner alone cannot identify every helper reload within the same
KWin process. The bridge must also reject old tokens across unload/reload,
window destruction/replacement, and generation changes. Stop the proof at this
gate if exact identity cannot be established.

## Exact calls to exercise on the test desktop

Use a disposable Plasma 6.6 Wayland session (Kubuntu 26.04 is acceptable), two
synthetic fixture windows with independent key/click counters, and the helper
built for that installed KWin/Qt ABI. Record versions and the candidate SHA.
Run commands inside that desktop user's session bus, not a separate root bus.

Read-only prerequisites:

```bash
kwin_wayland --version
plasmashell --version
printf '%s\n' "$XDG_SESSION_TYPE"
git rev-parse HEAD
qdbus6 org.cua.KWinTarget /org/cua/KWinTarget org.cua.KWinTarget.GetVersion
qdbus6 org.cua.KWinTarget /org/cua/KWinTarget org.cua.KWinTarget.GetWindows
qdbus6 org.kde.KWin /Scripting org.freedesktop.DBus.Introspectable.Introspect
```

The snapshot commands are diagnostics, not a substitute for the Rust adapter's
owner/PID/UID verification. Do not publish unrelated window titles or user data.

After the identity bridge exists, the activation probe should use these exact
scripting operations. This fragment deliberately requires bridge-provided
arguments and returns only an activation observation; it does not send input
or authorize a later burst:

```javascript
function observeExactActivation(expectedUuid, expectedPid) {
  const windows = workspace.windowList();
  const matches = windows.filter(
    (window) => String(window.internalId) === expectedUuid && window.pid === expectedPid
  );
  if (matches.length !== 1) {
    return { active: false, reason: 'exact_window_missing_or_ambiguous' };
  }
  const target = matches[0];
  workspace.activeWindow = target;
  const active = workspace.activeWindow;
  const confirmed =
    active !== null &&
    active === target &&
    String(active.internalId) === expectedUuid &&
    active.pid === expectedPid;
  return {
    active: confirmed,
    reason: confirmed ? 'activation_observed' : 'activation_not_confirmed',
  };
}
```

The spike must invoke this logic with freshly verified bridge data, observe
asynchronous activation within a bounded deadline if required, and carry a
fresh confirmation over a Driver-owned channel. A timeout or malformed/missing
confirmation means no portal input. A `print()` log or `run()` exit status is
not that channel. Activation itself changes desktop state even if the later
input request is refused.

For a completed probe file, use the following load/run calls, with `PROBE_JS`
set to its absolute path. Do not run this sequence with an incomplete bridge or
treat the function definition above as an end-to-end probe. Use a fresh probe
name so an existing script is never replaced:

```bash
: "${PROBE_JS:?Set the absolute path of the completed activation probe}"
probe_name="cua-3506-proof-$(date +%s)-$$"
script_id=$(qdbus6 org.kde.KWin /Scripting \
  org.kde.kwin.Scripting.loadScript "$PROBE_JS" "$probe_name") || exit 1
case "$script_id" in
  ''|*[!0-9]*) echo 'Probe load failed; do not dispatch input' >&2; exit 1 ;;
esac
trap 'qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.unloadScript "$probe_name"' EXIT
qdbus6 org.kde.KWin "/Scripting/Script${script_id}" org.kde.kwin.Script.run
```

Check the live introspection against these source-derived calls. Record any
distro-specific difference rather than guessing a different object path or
calling every script through a global start operation.

## Guarded portal burst: prototype failed target-safety check

The [separately applied host prototype](driver-prototype/README.md) connected
fresh confirmation to the existing portal/libei transport under normal Driver
admission, not a new `guardedBurst` method. It delivered to the wrong window
when focus changed after confirmation. The following remain acceptance
requirements for a different, destination-bound implementation. No portal
session or background-input capability is created by activation alone.

For each burst, capture on a common monotonic timeline: admission, exact target
and generation validation, active-window confirmation, transport submission,
acknowledgement, and fixture receipt. Record the check-to-submission interval,
burst duration/event count, observation latency, and the point after which
delivery becomes uncertain. A later read-back cannot undo an earlier event.
Do not claim an arbitrary millisecond threshold is safe; report the measurements
and residual race for maintainer review.

Force a focus change both before confirmation and between confirmation and
input processing. Refuse before sending any input when confirmation fails;
after dispatch may have started, stop further work and report partial/unknown
with no retry. A confirmed active top-level window does not by itself certify
pointer hit-testing, popup/modal routing, or release of held input. Test those
separately before proposing those operations. Any observed non-target delivery
must be reported as a failed case, never hidden behind a later successful focus
check or relabeled as a zero-dispatch refusal.

## Live recording and acceptance matrix

Record the desktop and correlate it with synthetic fixture event counters and
dispatch traces. The recording must identify the actual OS, Plasma/KWin/Qt
versions, Wayland session, helper build, Driver/probe commit, fixture, and exact
commands. A terminal recording alone cannot establish which window received
input. Capture with an available external recorder if necessary; do not make
the proof depend on #4034's separate Cua recording release fix.

| Case                            | Required visible and machine-observed evidence                                                                                                          | Current result                                                                               |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Exact selection                 | Adapter token resolves to the same live UUID/PID/generation; two same-process windows remain distinguishable                                            | 6.7.5 only: observed with additive helper patch                                              |
| Positive foreground canary      | Activate A from B, confirm A, send one admitted bounded burst; only A's counter changes; then explicitly select B and repeat                            | 6.7.5 only: fixture A and B each received one pair in separate operations; not a safety pass |
| Activation/confirmation failure | Missing/stale target, wrong active window, timeout, or lost confirmation: zero input submissions and a refusal; report any focus change separately      | 6.7.5 only: activation-only refusals and precheck input refusal; not all revocations covered |
| Focus takeover                  | Change focus before confirmation and again in the check-to-delivery gap; show both window counters, timing, stop behavior, and any residual misdelivery | 6.7.5: **postcheck leak to B**; unsafe                                                       |
| Lifetime change                 | Close/recreate target and unload/reload helper; old identity cannot authorize input even if PID/token or bus owner is reused                            | 6.7.5 activation-only checks, no input attempted after loss                                  |
| Lost acknowledgement            | Dispatch once, lose the final reply, reconnect/re-resolve: partial/unknown outcome and no replay; a genuinely new admitted action is distinct           | 6.7.5 worker reply dropped and parent ledger blocked replay; outer SDK reply loss untested   |
| Scope preservation              | AX actions retain their behavior; exact/background requests retain their exact existing refusals; no fallback to unguarded global input                 | 6.7.5 background refused; AX source unchanged, no live AX smoke                              |

Upload a suitable recording and sanitized report to #3507 with the tested commit,
probe source, exact API calls (including the actual burst transport), measured
timings, fixture counts, and results for every row. Keep failed and unrun rows
explicit. Documentation checks, a successful activation setter, and a mock test
do not close this gate. Request the maintainer's disposition only after the
real evidence is reviewed; until then the RFC stays `review` and production
raw KWin input remains unavailable. The local host recording cannot fulfill
the requested simultaneous A/B visual coverage: both panes showed A.
