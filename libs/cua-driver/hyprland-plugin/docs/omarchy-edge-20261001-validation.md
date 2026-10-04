# Omarchy Edge profile candidate: 2026-10-01

Refs [#4216](https://github.com/trycua/cua/issues/4216).

This is an in-progress qualification record, not a release or compatibility
claim. The target is an upgraded disposable x86_64 Omarchy Edge guest, not a
fresh ISO installation or Omarchy's independent Omabot acceptance run.

## Candidate identity

- Driver source: `5272e492d61b96caf08e3bf434d91126c1f3dccc` (`0.31.0`).
- Profile tooling: `54805907461376c8574996583b948500ce038d1a`, merged in #4383.
- Hyprland: `0.56.2-3`; Aquamarine: `0.15.1-1`.
- GCC: `16.2.1 20260810`; package `16.2.1+r23+gd564253eb6c8-1`.
- Inkscape: `1.4.4-6`. Use the documented Inkscape-only app profile;
  installed LibreOffice `26.8.0-2` is outside background admission.
- Profile SHA-256: `a7289e169914eda41ee4bb49e2077c5e4af36cb228390cd9104f2a24ceb02891`.
- Kit provenance SHA-256: `07f9f2462af9db1b5d8d076212eb4e26c34c5fcec4262e7802a19a82b720fe8d`.
- Trace-disabled module SHA-256: `1469e1f8aaf141199b4080b5bd755d68e56e8eac245730e7f893c717519d72cb`.

## Evidence so far

The reviewed-profile recipe builds without rewriting the source archive or
relaxing ABI checks. Its bundled CTests pass. The consumer verifier passes
after installation, and a fresh compositor loads the module with matching
compiled/runtime ABI hashes and enabled input endpoints.

The isolated ALPM lifecycle runner passes install, removal, reinstallation,
and rejection of a mismatched Hyprland dependency. Its rebuilt module matches
the installed module hash. The package archives have distinct identities:
the initial package is `02fe5dc2293eb5b66a7ec9c837d1f9a5dd149aff624ffdd4d82d218ecef6fbc4`,
and the lifecycle rebuild is `c689cc41a9cbece8b67cdfff18074269dac21c6ea745fe789484301753586d0d`.
Do not substitute one archive's lifecycle evidence for the other's native proof.

The bounded single-app production smoke now passes. Initial attempts stopped before raw
input because the default accessibility walk was truncated or the grounding
expected absent closed-menu children. The repaired harness requires a complete
15-second-budget observation, prepares the Objects panel through accessibility,
and accepts exact semantic geometry labels without relaxing numeric values.
An intermediate attempt selected the rectangle, moved it two pixels right,
and saved the SVG, but failed its immediate post-save identity observation
during a compositor/AT-SPI title transition. Saved-file evidence alone did not
pass the smoke. The native replay with bounded observation-only retries passes
strict identity, semantic geometry and saved SVG checks on the published Driver
and trace-disabled module. This proves the recorded single-app operations, not
two-lane isolation or concurrency. Failures after raw input are reported as failed,
not inspection-only. The portable harness suite passes 623 tests, with two skips.

The unchanged complete native Linux baseline finished with failures in Tauri
page scrolling, keyboard-first cursor placement, GTK3 pixel geometry, desktop
scope, and encrypted history setup. Product repairs are isolated in #4396;
the guest function-key helper and Secret Service setup were also corrected.
The manual foreground safety suite passed three tests. The updated manual
observation test starts behavioral recording; its latest replay stopped before
input because two fixture windows overlapped with ambiguous compositor z-order.
A non-overlapping fixture layout is required for that separate replay.

The first repaired candidate, `420c87d87fbd423d5f9da098b437946bc13098ea`,
passes the complete hosted X11 matrix and every native Hyprland behavioral
suite, including GTK3 37/37, capture 4/4, desktop scope 5/5 and encrypted history.
Its final native evidence validator fails because an intentional stale-zoom
refusal was classified as an unverifiable click and expected a click marker.
The shared producer fix in candidate
`e20386e4551c791c66a61e7e4693909cfdda430a` preserves legacy fields and reports
refused-before-dispatch; the validator is unchanged. That exact candidate passes
the complete native Hyprland Linux runner, including final video validation:
90 delivered, 42 expected refusals, zero failed or skipped cases. The complete
hosted X11 and Windows gates also pass. Hosted macOS passes as supplemental
evidence; the canonical Lume gate remains in progress.
Final manual foreground safety passes 3/3 and native observation passes 1/1
with behavioral video after the fixture windows are arranged without overlap.
Focused XWayland geometry on
the prior candidate matches an independent AT-SPI Screen oracle and delivers
a background accessibility click.

Two-lane production setup exposed an observer assumption that rejected a stable
primary locked-modifier mask despite no held keys. The proof now records that
baseline mask, still refuses depressed/latched modifiers and nonzero groups,
and still rejects every keyboard event during the interval, including a lock
change and return. The native production replay passes with two independent
Inkscape processes and Driver runtimes: each selects, moves and saves its own
rectangle, and both saved SVGs verify a two-pixel horizontal translation.
Independent primary-client observation finds no interference. The separate
negative control detects a deliberate pointer excursion and return. This does
not substitute for diagnostic overlap, compositor attribution or cleanup proof.

The first diagnostic overlap attempt stops before dragging: semantic Inkscape
geometry labels did not match the legacy numeric-label parser, and the Fill and
Stroke panel made the blue-rectangle pixel oracle ambiguous. The parser now
accepts only exact axis-specific semantic labels while retaining cross-projection,
numeric, visibility and uniqueness checks. Fresh setup closes the unrelated
panel through accessibility. A later run verifies both drags and saved outputs,
but fails final attribution because identical window-local coordinates match
both lanes. Harness `1b4c066ce` binds each agent to the lane independently
observed during its serial selection and verifies retained epoch/reservation
before subsequent input. A fresh run passes with 1501 ms of drag overlap,
per-lane wire endpoints, saved SVG effects, primary isolation and cleanup.
Capacity also passes: two persistent owners are admitted and a third receives
`lane_busy` without dispatch. Runtime cancellation passes with the sibling
finishing and synthetic input released; reacquisition remains unproven.
The integrated portable suite passes 628 tests with two skips.

The replacement Edge guest has Mesa `1:26.2.3-2`, while the first profile pins
`1:26.2.3-1`. The old recipe correctly refuses that mismatch. The newly measured
profile differs only in that runtime package plus its explicit labels:
`omarchy-edge-20261001-mesa2`, kit `1.2.1`, package release `3`.
Its profile digest is
`d3ff786deb7bbde5d2368dcb58966d9ddf0c705c37e51d0aac60901d7d983c84`;
kit provenance is `28a3ccebccb539fc625b972082d3426f41a403e872d358b8eff130a4f80913f8`.
The rebuilt production module is byte-identical to the original module, but
the package is distinct:
`fa68bd906e8bfe6473d790d28728e35efad978f548f48cf61d08a668d3841a30`.
Its live install, upgrade to release 4, rollback to release 3, removal and
reinstallation pass with the compositor stopped for every package mutation.
Fresh sessions after each installed stage map the production module with matching
ABI and enabled input. Removal leaves no module or loaded plugin; the retained
test-only enable setting reports an unknown key until reinstallation and
deliberate activation clear it. The current-profile production smoke passes
on published Driver 0.31.0 bytes, with strict semantic and saved-SVG readback.
Full reboot verification passes on kernel `7.2.7-arch1-1`: a new boot identity,
fresh compositor activation, matching ABI, enabled input, and the consumer
verifier all pass. The post-reboot production smoke also passes on published
Driver 0.31.0, including semantic readback and the saved two-pixel SVG translation.
The complete unchanged Linux runner also passes on the updated Mesa and kernel
with exact candidate `e20386e4551c791c66a61e7e4693909cfdda430a`: 90 delivered,
42 expected refusals, zero failures, and zero skips. Final video validation
passes. The retained private evidence archive has SHA-256
`34518c7cfc25653d299611e22763e3ceb473cb3a588aea58b725fafde76a6269`.
Published-source package smoke and candidate Driver matrix results remain
separately identified; neither changes the source contained in the package.

## Remaining gates

Additional diagnostic fault runs on the replacement guest pass primary-client
initial refusal with a new-action recovery, and same-client passive-hover
agent conflict with owner recovery. These do not establish active-drag primary
conflict, active-lease contention, other-lane recovery, or same-process sibling
window behavior. Separate floating-window move and resize episodes pass
mid-drag cancellation, own-seat input release, continuous primary isolation,
and freshly grounded recovery. Initial target-destruction episodes fail because
the tiled replacement changes bounds after target removal. A fresh episode with
fixed, non-overlapping floating windows passes exact-target destruction during
a drag, connection retirement, and a new action on a prepared distinct process.
The test uses an advertised 2560x1080 virtual display mode because each Inkscape
window expands to a 1078-pixel minimum width with the fixture panels open.
Same-client recovery, PID/address reuse and active-sibling behavior remain
unproven by this bounded case.

A second current-environment package, release `4`, builds with all 20 CTests
passing. Its measured profile differs from release `3` only in explicit labels
and package revision. Package SHA-256 is
`4bf905f05590a9f2bd4b25fa7d302c63dc17c71ebd2bd726b67235248d561982`;
kit provenance is `0947837380c9af82ca9dc0d2ac4f7b1e0238a74f7677495fee4685278c8af0d3`.
Its live upgrade and rollback checks pass as described above; it is not a
published release asset.

- Canonical Lume macOS verification of the shared refusal-metadata repair.
  Canonical hosted Linux and Windows pass; hosted macOS is supplemental only.
- Fault evidence is limited to the cases stated above; it is not arbitrary
  application or physical-hardware certification.
- Bind any published kit/package to the exact passing evidence.
- Omarchy-owned downstream acceptance and package promotion.

No native-certified profile or release asset is published by this record.
