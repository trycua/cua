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

| Case                             | Result                                                                            | Evidence           | Limitation                                          |
| -------------------------------- | --------------------------------------------------------------------------------- | ------------------ | --------------------------------------------------- |
| Initial attempts                 | Portal readiness timeout; zero emission and A/B events                            | Operations 1–4     | Harness exit 0 is not delivery                      |
| A, without worker gates          | A received 1 press/release; B zero                                                | Operation 5; video | Ordinary delivery, not race safety                  |
| B, without worker gates          | B received 1 press/release; A zero                                                | Operation 6; video | Ordinary delivery, not race safety                  |
| Precheck focus takeover          | `closed/stale/inactive target`; zero emission and events                          | Operation 7        | Deterministic precheck gate                         |
| Helper reload / stale generation | Same KWin owner, new generation; `owner/generation changed`; zero emission/events | Operation 8        | Reload revocation, not every lifetime transition    |
| Worker reply lost                | A received one pair; result `unknown; never retry`                                | Operation 9        | Not outer SDK reply loss                            |
| Replay of operation 9            | Exclusive ledger refused before Driver construction                               | `replay-9.log`     | Parent ledger, not daemon-wide deduplication        |
| Background                       | Exact code `background_unavailable`; zero submission/emission/events              | Operation 10       | Background delivery, not a third mode               |
| Postcheck focus takeover         | **A zero, B received one pair intended for A**                                    | Operation 11       | Deterministic postcheck gate; no video of this case |

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
both returned 101. The same assertion also occurs on the clean baseline;
this does not establish its root cause or exclude every patch interaction.
It does not make the full library green. No full suite was repeated.

Unrun: pointer/drag/hotkeys, live AX smoke,
outer SDK acknowledgement loss, and comprehensive desktop/session revocation.
Production input remains disabled. `InputDeliveryMode` has only
[`Foreground` and `Background`](../../libs/cua-driver/rust/crates/cua-driver-contract/src/inputs.rs#L608-L611).
Exact identity is the request's `ActionTarget::Window { pid, window_id }`,
not a delivery mode. The exercised request is
`CuaDriver::press_key(PressKeyInput)` in
[`sdk_harness.rs`](driver-prototype/sdk_harness.rs#L8-L11), with the
compile-only foreground field carried by the sidecar patch; the background
case changes that field before the same SDK call. There is no separate
`exact` enum value or additional exact-mode experiment owed here.

## Reproduction and provenance

### Historical execution (not commands to rerun with the consumed IDs)

Source archive: `2b99a150d00aa35ded968b251901b6dee1ce3504`.
Driver patch base: `93f7afc89a09e7f9d39da22990e26036a74adfbd`.
Runner additions are published at `df0bbcef078fa30341bad0f2836de5ff73213190`;
the guest runner used those changes before that documentation commit.
SHA-256:

| Artifact                              | SHA-256                                                            |
| ------------------------------------- | ------------------------------------------------------------------ |
| `helper-identity.patch`               | `a7ba8d55b85dae206e8d57b74b53f1259ae86a921e203960922fd9ee17e88b82` |
| `driver-prototype/driver.patch`       | `3994b717302a799f1f032c313415f1f037f605be5524985151cf3ae270378b95` |
| Guest `target/debug/examples/rfc3506` | `060ccc4edf639560d4bcfe4b36ca1051e0347e01d1b38136e3747994adc41d59` |
| Guest helper module                   | `d1a04b96a2597ed7b771ea2abb24b9cfe076376b7d9fc4781773995620fd3540` |

Packages recorded: `kwin-wayland`/`kwin-dev` `4:6.6.4-0ubuntu1`,
`plasma-workspace` `4:6.6.4-0ubuntu2`, `qt6-base-dev` `6.10.2+dfsg-7`,
`xdg-desktop-portal-kde` `6.6.4-0ubuntu1`, `xdg-desktop-portal`
`1.21.1+ds-1ubuntu3`, `pipewire` `1.6.2-1ubuntu1.2`.
Build/runtime dependencies: CMake, C++20 compiler, ECM, Qt6/KF6/KWin
development packages, Rust/Cargo, clang/libclang, pkg-config, GLib/GTK3,
AT-SPI, PAM/X11/Wayland development libraries; fixture/runner use
`python3-dbus`, `python3-gi`, `gir1.2-gtk-3.0`. Package logs record the
distro Rust/Cargo 1.93 packages; exact `rustc -Vv` output was not retained.
Do not substitute a different compositor ABI or silently upgrade it.

Guest desktop wrapper `/work/desktop.sh` exported these values, not host
session variables:

```sh
export XDG_RUNTIME_DIR=/run/user/1000
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
export WAYLAND_DISPLAY=wayland-0
export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland
export QT_QPA_PLATFORM=wayland GDK_BACKEND=wayland TMPDIR=/work/tmp
```

Historically executed runner invocations (in that guest's synthetic fixture
session, with portal consent handled there):

```sh
runner=/work/cua/rfcs/3506/run_input_case.py
binary=/work/cua/libs/cua-driver/rust/target/debug/examples/rfc3506
sh /work/desktop.sh python3 "$runner" --binary "$binary" --helper-name cua_kwin_target_helper --directory /work/evidence --seq 5 --case A --ungated
sh /work/desktop.sh python3 "$runner" --binary "$binary" --helper-name cua_kwin_target_helper --directory /work/evidence --seq 6 --case B --ungated
sh /work/desktop.sh python3 "$runner" --binary "$binary" --helper-name cua_kwin_target_helper --directory /work/evidence --seq 7 --case precheck_takeover
sh /work/desktop.sh python3 "$runner" --binary "$binary" --helper-name cua_kwin_target_helper --directory /work/evidence --seq 11 --case postcheck_takeover
```

Cases 8/9/10 used the same arguments with `stale_generation`, `drop_ack`,
and `background` respectively. All actual expectations, activation observations,
monotonic emission/flush and fixture receipt records are in the linked JSON.
The recording is QEMU framebuffer capture, not a movie synthesized from logs.
The fault cases have no correlated film because they happened after it ended.

### Proposed reproduction in a restored isolated guest (not executed here)

The retained QEMU launch script boots a live ISO, not an installed guest root.
The ISO SHA-256 is
`95ce9cf68f13015b9a88bd1ef86fcf7eda77c99979fda48c69e28aa0a84f88ac`.
Recreating lost guest provisioning is prerequisite work, not a verified
one-command resume. Inside an already restored 6.6.4 session, create a fresh
source archive directory; never trust `prepare.py`'s existing `Cargo.toml`
cache as proof of its source SHA. Apply both published patches to that archive,
then use the following source-derived build commands (the original helper
configure command was not retained verbatim). The proposed checkout
`/work/review/cua` must contain the published commits. Archive `df0bbcef` to
include the runner additions; its production Driver/helper sources have no
diff from the tested `2b99a150d`. Do not overwrite the retained `/work/cua`:

```sh
mkdir /work/repro
mkdir /work/repro/cua
git -C /work/review/cua archive df0bbcef078fa30341bad0f2836de5ff73213190 | tar -x -C /work/repro/cua
cd /work/repro/cua
git apply --check rfcs/3506/helper-identity.patch
git apply rfcs/3506/helper-identity.patch
git apply --check rfcs/3506/driver-prototype/driver.patch
git apply rfcs/3506/driver-prototype/driver.patch
cmake -S libs/cua-driver/kwin-target-helper -B /work/helper-repro -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib/x86_64-linux-gnu
cmake --build /work/helper-repro --parallel 4
sudo cmake --install /work/helper-repro
cp rfcs/3506/driver-prototype/sdk_harness.rs libs/cua-driver/rust/crates/cua-driver-sdk/examples/rfc3506.rs
cd libs/cua-driver/rust
RUSTFLAGS='--cfg cua3506_prototype --check-cfg=cfg(cua3506_prototype)' cargo build -p cua-driver-sdk --features portal-input --example rfc3506
```

Only inside the guest: load the module through
`qdbus6 org.kde.KWin /Effects org.kde.kwin.Effects.loadEffect cua_kwin_target_helper`,
check `GetVersion == 1`, and run the binary's `--identity` trust checks.
Start `python3 /work/repro/cua/rfcs/3506/fixture.py --directory /work/evidence-followup`
through the desktop wrapper, arrange A/B side by side, and start an actual
framebuffer recording before input. Use a new directory and fresh operation
IDs greater than 11 (derive them from all retained journals), then the same
runner forms above with `runner=/work/repro/cua/rfcs/3506/run_input_case.py`,
`binary=/work/repro/cua/libs/cua-driver/rust/target/debug/examples/rfc3506`
and `--directory /work/evidence-followup`. Approve the guest portal explicitly; do not reuse a host
portal or bypass owner/UID/generation validation. Record frame monotonic
timestamps and operation timestamps in the same guest/host clock mapping,
inspect the takeover/receipt moment, copy journals/video out before shutdown,
and stop at the first wrong-window delivery. Never replay an uncertain ID.

The existing video is attachment-ready but **not public evidence until uploaded**.
Single manual step: drag `plasma-6.6.4-AB.webm` from the retained
`plasma66-env` directory into the edit box of the existing PR comment, let
GitHub insert its uploaded URL, retain the operations 1–6-only caption, and save.

## Earlier source review

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

Earlier Ubuntu source review and initial guest introspection preceded the
completed input experiment above. The guest is now stopped; its live root
overlay was RAM-only, while the attached `/work` disk retains sources,
binaries and journals. A supplementary short fault-case video was not made:
the guest SSH endpoint refuses connections and restoring its lost SSH,
runtime dependencies and helper installation would require reprovisioning.
The retained work disk is not a saved running desktop. No new guest was
created and no host input was sent to work around this limitation.

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

## Identity bridge used by this experiment

The current [Cua helper](../../libs/cua-driver/kwin-target-helper/kwin_target_helper.cpp)
maps `KWin::Window::internalId()` to a monotonically allocated numeric `token`
in its private `m_tokens` map. `GetWindows()` exports the numeric token and
PID, but **does not export `internalId` or the mapping**. Its v1 API has no
activation method. The [Rust adapter](../../libs/cua-driver/rust/crates/platform-linux/src/wayland/kwin_helper.rs)
verifies the helper owner, KWin process/session ownership, UID, and protocol.

Consequently, the token is not a UUID, X11 window ID, or index into
`workspace.windowList()`. Matching title, app ID, geometry, or PID alone does
not complete the requested proof, especially for two windows of one process.

The separately applied [read-only prototype patch](helper-identity.patch)
provided a generation-aware token-to-UUID bridge for both the host and guest
experiments, not shipped production code. It resolves the adapter-selected
`(pid, token, generation)` to the exact live KWin object.
Preserve `GetVersion() == 1` discovery and
do not substitute private C++ activation internals for the requested supported
interface. A UUID manually chosen in the scripting console can test the KWin
API in isolation, but cannot satisfy the trusted-adapter requirement.

A D-Bus unique owner alone cannot identify every helper reload within the same
KWin process. The bridge must also reject old tokens across unload/reload,
window destruction/replacement, and generation changes. Stop the proof at this
gate if exact identity cannot be established.

## Supported API calls used by the probe

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

## Review boundary

Use the completed result table at the top, not an obsolete host-only matrix.
The genuine A/B video still needs manual GitHub attachment and does not show
the postcheck fault. Machine-readable operation 11 is the counterexample
evidence presently available in Git. Maintainer disposition is pending;
RFC stays `review` and production raw KWin input remains unavailable.
