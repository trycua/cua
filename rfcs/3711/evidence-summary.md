# Native capture geometry: investigation evidence

This is a sanitized decision input for [RFC #3711](../3711-native-capture-geometry.md),
not product acceptance evidence or an assertion that the proposed implementation
has shipped. It contains no desktop images, raw application transcripts,
credentials, private machine paths, or instructions to approve OS consent.

## Provenance and limits

- Original public report: [#3631](https://github.com/trycua/cua/issues/3631),
  authored by [@f-trycua](https://github.com/f-trycua). It describes release
  0.23.2, a roughly 31×97 Calculator capture in the Stage Manager strip, and
  independently successful background AX calculation.
- Follow-up investigation: 2026-09-10, isolated Apple Virtualization/Lume guest,
  macOS 26.5.2 build 25F84, arm64, 1024×768 display at 1× backing scale.
- Tested installed driver reported local version 0.25.0, standard permission
  mode. Its SHA-256 was
  `d510c089b187eeb19fb286c86bc56f7bb785ea1dea61d84f413043e16110ee4e`.
  It was not replaced during the investigation. Its exact source commit was
  not established; do not infer one from a workspace name or version string.
- Source reviewed at
  [`ed289df50257bd6a65f9ee7964bb842777a1a10a`](https://github.com/trycua/cua/tree/ed289df50257bd6a65f9ee7964bb842777a1a10a).
  The inspected guest copy of `platform-macos/src/capture.rs` was byte-identical
  to that revision's file, with SHA-256
  `cba6b1f0a6bc31f31c4187ad2f422c4d455420bc2fe1441e2a6367ea77c25d5d`.
  This is file-level provenance, not proof that the whole installed binary was
  built from that commit.
- A separate locally signed Swift probe exercised native capture APIs without
  replacing the installed app. Direct SSH execution failed read-only permission
  preflights and performed no capture; LaunchServices routing provided the
  expected accessibility/capture attribution.
- Despite passing those preflights, an additional OS private-window-picker
  bypass consent dialog appeared. It was **not approved**, and no TCC grants or
  permission profiles were changed. APIs still returned the measured images.
  The outstanding dialog was present during both the active/working and
  strip/non-working pointer-control cases. This caveat prevents treating these
  results as permission-ready release certification.
- Earlier locked-host and interrupted borrowed-guest attempts, and an initial
  probe using an ID from before a reboot, are excluded from the findings.

The local diagnostics retained native JSON, source probes, event logs, and PNGs.
Those raw artifacts are not a public review prerequisite. The following method,
configurations, numerical observations, and required future acceptance tests
provide the portable decision record. Re-run on an exact candidate and a
properly authorized desktop before making a product support claim.

## Collection method

For each exact native window:

1. Read its owner, layer, on-screen state, and bounds through
   `CGWindowListCopyWindowInfo`, selecting the exact CGWindowID.
2. Resolve that same window through AX window enumeration and
   `_AXUIElementGetWindow`; read `AXPosition`, `AXSize`, and `AXMinimized`.
   Do not choose by title, maximum area, or process-wide main-window heuristics.
3. Resolve the same ID from `SCShareableContent`; read its frame and owner.
   Build `SCContentFilter(desktopIndependentWindow:)` and read `contentRect`
   and `pointPixelScale`.
4. Record each requested configuration and actual image dimensions. Re-read
   compositor geometry afterward and record foreground identity before/after.
5. Repeat across Stage Manager off, active stage, strip, and restored state.
   Stage changes are explicit driver actions outside passive capture.
6. Independently decode PNG RGBA pixels and compare nontransparent content,
   rather than assuming that larger output dimensions mean greater fidelity.

The native fixture used two ordinary titled AppKit windows. One had a 90×102
outer frame; the other had a 300×232 outer frame with a checkerboard, text, red
center marker, and an application-owned mouse-down counter/event log. Its
logical geometry and received events were independent of driver response claims.

## Geometry measurements

AX, CG, and filter dimensions below are logical points; output dimensions are
pixels. All were 1×. Each row compares the same exact window across sources.

| Window/state                                 | AX outer frame | CG bounds | Fresh filter content rectangle | Default PNG |
| -------------------------------------------- | -------------- | --------- | ------------------------------ | ----------- |
| Calculator, Stage Manager off                | 230×408        | 230×408   | 230×408                        | 230×408     |
| Calculator, strip, tutorial popover closed   | 230×408        | 31×102    | 31×102                         | 31×102      |
| Calculator, Stage Manager restored off       | 230×408        | 230×408   | 230×408                        | 230×408     |
| Small control, settled active stage          | 90×102         | 90×102    | 90×102                         | 90×102      |
| Same small control, strip                    | 90×102         | 47×105    | 47×105                         | 47×105      |
| Normal control, settled active stage         | 300×232        | 300×232   | 300×232                        | 300×232     |
| Same normal control, first strip arrangement | 300×232        | 66×109    | 66×109                         | 66×109      |
| Same normal control, later strip arrangement | 300×232        | 74×118    | 74×118                         | 74×118      |

An earlier Calculator sample with its tutorial popover present measured 26×84.
That popover was dismissed through its exact AX Close control before the main
comparison. Child-window content can affect the representation and is not an
excuse to compare different logical targets.

Immediately after activating the small control, one sample measured 91×103 at
x=249 in the compositor/filter and 90×102 at x=250 in AX. Settled measurements
matched exactly. This supports bounded transition handling, but does not by
itself validate any proposed tolerance or timeout constant.

The original follow-up reproduction, before this backend investigation, also
collected three 31×103 Calculator PNGs with `screenshot_frame_valid:true`.
Fresh-token AX clicks computed 7×6=42 while screenshots stayed thumbnail-sized.
Selecting the same window into the active stage restored a 230×408 image showing 42. The varying thumbnail dimensions do not change the underlying discrepancy.

## Output-size and backend experiments

For the settled 31×102 Calculator representation:

| Configuration/path                                                    | Result                                                                                         |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Fresh desktop-independent filter; `captureImage`; filter-sized output | 31×102 transformed thumbnail.                                                                  |
| Explicit single-window shadow exclusion                               | Same thumbnail; the observed native default already excluded single-window shadows on this OS. |
| Request AX size, 230×408, with `scalesToFit=false`                    | Same thumbnail pixels on a larger transparent canvas.                                          |
| AX-sized output with capture resolution `.best`                       | Same padded thumbnail pixels.                                                                  |
| AX-sized output with `scalesToFit=true`                               | Enlarged perspective thumbnail, not reconstructed native content.                              |
| macOS 26 `captureScreenshot`; default size; child windows excluded    | 31×102 thumbnail.                                                                              |
| macOS 26 API; AX-sized output                                         | Same padded thumbnail.                                                                         |
| macOS 26 API; AX-sized output and canonical display intent            | Same padded thumbnail.                                                                         |
| Shell `screencapture -l <exact-id> -x -o <output>`                    | Thumbnail matching the default native capture in decoded pixels.                               |

Default, AX-sized, `.best`, both AX-sized macOS 26 variants, and shell capture
contained 2,931 nontransparent pixels in a 31×101 alpha bounding box. After
cropping transparent padding, decoded RGBA SHA-256 was identical:

```text
2c1ee7c86f685576f43a0d377102b1a18c5883e00d086ae7e1986f67f69a206e
```

This establishes padding rather than restored detail. Scale-to-fit produced
more output pixels, but visually enlarged the same perspective-distorted
representation and retained the thumbnail's aspect rather than restoring the
logical window.

The installed driver was not instrumented to label every request's backend.
Both its native API path and shell fallback were independently exercised and
both exposed the limitation, so selecting between these tested paths does not
provide a demonstrated recovery mechanism.

## Retained filter and live stream

A 30-sample sequence kept the same filter created while the 300×232 control
window was active and simultaneously sampled an existing `SCStream`:

- Before the explicit stage switch, CG, AX, filter, and image agreed at 300×232.
- After the switch, CG became 74×118 while the cached filter continued reporting
  300×232. One-shot captures using that cached filter contained a transformed
  thumbnail on the configured output canvas.
- The already-running stream also changed to transformed thumbnail content;
  it did not preserve untransformed backing pixels.
- Stream attachments continued reporting 300×232 content/bounding rectangles
  and content scale 1 while their screen rectangle became 74×118. The images
  contradicted any inference that full content rectangles meant full-window
  image fidelity.

This is direct native-filter/stream evidence, not instrumentation of the
installed driver's cache hit counter. Preserve the driver's existing
owner/frame cache invalidation; keeping apparently full-sized cached metadata
is not a supported workaround.

## Controlled input observation

With the normal fixture active, a background pixel click at screenshot
`(150,132)` produced an app-observed mouse-down at content-view `(150,100)`,
the intended red center below a 32-point title bar.

With the same window in its 74×118 strip representation, a fresh screenshot
placed the red marker near `(22,68)`. Clicking that observed marker in the
background produced no additional mouse-down event in either fixture window.
The driver reported `window_pointer: available`, `screenshot_frame_valid:true`,
and returned `effect: unverifiable`, `route: synthetic_events`. The post-action
snapshot and the application event log remained unchanged.

This supports refusing to treat the current observation as proof of usable
window-pointer mapping. It does not demonstrate wrong-target input, establish a
vulnerability, or prove that all thumbnail clicks are no-ops. Future acceptance
uses actual refusal-before-dispatch and application/focus/cursor oracles rather
than generalizing from one failed click.

## What these results establish

- The currently compared PNG and compositor bounds can agree while both refer
  to a transformed representation of a different-sized logical window.
- Request sizing, the tested alternative APIs, shell fallback, caching, and a
  retained stream did not recover full native content on this guest.
- Stable exact AX/compositor disagreement is a useful candidate limitation
  signal. Small size, on-screen status, global Stage Manager state, and stream
  output dimensions are not sufficient by themselves.
- A mismatch is not a universal cause classifier: AX may be unavailable/stale,
  transitions exist, and the smaller fixture's strip height increased.
- Semantic usability and capture/pointer limitations must remain independent.

These results do not establish release 0.23.2 behavior independently of the
original report, other macOS versions, Retina/multi-display support, a complete
minimized/off-Space classifier, or full desktop certification. No supported
recovery path was found among the tested APIs; this is not a proof about every
possible Apple or private API.

Stage Manager was restored to off, separate Spaces left enabled, the installed
driver hash stayed unchanged, and the owned guest was stopped without deletion.
The additional OS consent dialog was never approved. The RFC's acceptance plan
must repeat the relevant behavior with exact source/binary provenance and no
unresolved permission prompt before implementation can be declared complete.
