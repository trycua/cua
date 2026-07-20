# Browser Cross-Platform Hardening Journal

This journal records implementation and validation evidence for the browser
support hardening branch. It is not a public support contract.

## 2026-07-19: Scope and baseline

- Branch created from exact `origin/main` commit `6b6ad1e0`.
- Fable and local source review agreed that Linux Chromium-family product
  certification is achievable in this work window.
- Safari existing-profile attachment needs a WebKit/WebDriver-specific engine.
  Firefox existing-profile attachment cannot enable its Remote Agent on an
  already-running ordinary process; Mozilla requires launch-time enablement.
- Generic Wayland remains refused until a compositor can attest exact window
  identity. Sway and the maintained GNOME helper are the accepted native
  Wayland identity routes.
- macOS and Linux trusted Chromium pointer delivery remains an exact refusal
  when Chromium would activate the standalone browser. Synthetic ref-targeted
  `dom_event` delivery is a distinct explicit route, not trusted input.

## Implemented locally

- Added exact `CUA_E2E_BROWSER_PRODUCTS` selection. Explicit certification
  sets fail on unknown, duplicate, or missing products; the historical default
  selection is unchanged.
- Added a source-bound `browser-provenance.jsonl` artifact populated from each
  launched browser's CDP version endpoint. Hosted lanes now declare their exact
  mandatory product list instead of using a broader display label.
- Added the stable Microsoft Edge Linux executable name.
- Corrected the public evidence claim: Linux X11 accepted Chrome; native Sway
  accepted Chromium.
- Enriched unsupported Gecko/WebKit refusals with bounded engine, product,
  protocol, and lifecycle detail before native-window probing.
- Enriched pre-dispatch trusted-input refusals with the explicit synthetic
  alternative and a proof that no trusted delivery was attempted.
- Added a real-browser generic-Wayland refusal row. It withholds all
  compositor-specific identity from the driver and proves the browser window
  and fixture state are unchanged when exact identity is unavailable.
- Made the shared loopback fixture complete a real HTTP readiness round trip
  before it is returned to browser tests.
- Corrected the existing-profile setup row to model the user workflow: launch
  an ordinary browser page without CDP, prepare that exact native window, then
  navigate to the oracle fixture through the newly attached browser route.
- Added protocol and compositor identity design records.
- Added a GNOME Wayland browser-identity route using WinRects helper API
  v4. Browser-sensitive calls now prove the immutable D-Bus owner is the
  current user's system-installed GNOME Shell process, use exact compositor
  window identity, and restore the previously focused Shell window after each
  bounded setup or consent operation.

## Local evidence

- `cargo test -p cua-driver-core browser::`: 132 passed, 0 failed.
- `cargo test -p cua-driver-testkit`: 49 passed, 0 failed.
- `cargo test -p cua-driver --test standalone_browser_behavior_test --no-run`:
  compiled successfully at the current working tree.
- `cargo test -p platform-linux --no-default-features`: platform code compiled
  on macOS; Linux-only helper tests are scheduled on the GNOME/Sway workers.

## Representative evidence

- GNOME Wayland with Google Chrome at source `2ca38aea`: 13 delivered, 3
  expected policy refusals, 0 failures, 0 skips, and 16 playable videos. This
  accepts the GNOME helper route for Chrome without making a generic Wayland
  claim.
- Native Sway with Chromium at source `2ca38aea`: 13 delivered, 3 expected
  policy refusals, 0 failures, 0 skips, and 16 playable videos.
- Native Sway with Google Chrome and Microsoft Edge at source `2ca38aea`: 26
  delivered, 6 expected policy refusals, 0 failures, 0 skips, and 32 playable
  videos. Together these runs accept Chrome, Chromium, and Edge on Sway.
- Linux X11 with Microsoft Edge at source `2ca38aea`: 13 delivered, 3 expected
  policy refusals, 0 failures, 0 skips, and 16 playable videos. Snap Chromium
  could not start its CDP endpoint in an SSH-created Xvfb session because the
  snap required a real login-session cgroup; that preflight failure is not a
  Cua Driver behavioral result.
- macOS Tahoe canonical desktop matrix at source `f3413ac0`: 140 delivered, 8
  expected refusals, 0 failures, 0 skips, and playable video for every declared
  row. Later branch changes before `2ca38aea` affect only Linux test paths.
- Generic native Wayland at source `2f48888d`: one real Chromium existing-profile
  setup attempt refused with `browser_route_unavailable`, 0 failures, 0 skips,
  and playable video. The harness retained Sway IPC only as an out-of-band
  focus and z-order oracle; the Cua Driver child received an unusable socket
  path and could not use compositor-specific identity.
- macOS Tahoe existing-profile setup at source `75c40845`: the corrected Chrome
  row completed setup, exact attachment, typed navigation, external fixture
  mutation, and video/report validation with no failure or skip.
- macOS Tahoe standalone Chrome and Edge matrix at source `86d799b8`: 26
  delivered, 4 expected policy refusals, 0 failures, 0 skips, and 30 playable
  videos. Both ordinary-profile setup and isolated-profile launch passed for
  both products after the harness began from an ordinary `about:blank` window
  and navigated through the prepared browser route.

## Evidence still pending

- Windows interactive desktop: replay Chrome and Edge from the exact branch.
- Linux X11: final hosted Chrome regression at the pushed source commit.
- Non-Sway Wayland: record only fail-closed evidence unless an exact compositor
  identity adapter is implemented and separately accepted.
