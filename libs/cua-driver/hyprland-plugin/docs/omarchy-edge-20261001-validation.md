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

The bounded app smoke does not pass yet. Initial attempts stopped before raw
input because the default accessibility walk was truncated or the grounding
expected absent closed-menu children. The repaired harness requires a complete
15-second-budget observation, prepares the Objects panel through accessibility,
and accepts exact semantic geometry labels without relaxing numeric values.
The latest native attempt selected the rectangle, moved it two pixels right,
and saved the SVG, but failed its immediate post-save identity observation
during a compositor/AT-SPI title transition. Saved-file evidence alone does not
pass the smoke. A bounded observation-only retry now preserves strict identity;
native replay remains required. Failures after raw input are reported as failed,
not inspection-only. The portable harness suite passes 619 tests, with two skips.

The unchanged complete native Linux baseline finished with failures in Tauri
page scrolling, keyboard-first cursor placement, GTK3 pixel geometry, desktop
scope, and encrypted history setup. Product repairs are isolated in #4396;
the guest function-key helper and Secret Service setup were also corrected.
The manual foreground safety suite passed three tests. The manual observation
test also passed its identity and input assertions, but does not call the
behavioral-video boundary; its missing video remains an evidence gap, not proof
that the test skipped its actions.

## Remaining gates

- Pass the complete native Linux all-suite on the final repaired candidate and
  retain the manual observation test's behavioral video.
- Review any bounded smoke grounding repair and replay it natively.
- Two independent app lanes, traced overlap/capacity/refusal and cleanup proof.
- Independent primary-input isolation and negative control on production bytes.
- Live removal/reinstallation, restart, upgrade and rollback evidence.
- Bind any published kit/package to the exact passing evidence.
- Omarchy-owned downstream acceptance and package promotion.

No native-certified profile or release asset is published by this record.
