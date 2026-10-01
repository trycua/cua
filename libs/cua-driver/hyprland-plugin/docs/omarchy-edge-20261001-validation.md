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

The existing bounded app smoke does not pass yet. Its one-second accessibility
walk times out on this environment; a longer observation completes the tree.
Its initial selection predicate also expects closed-menu children and an
object-panel row that are absent from the current initial UI projection.
These attempts stop before input and report `inspection_only`; they establish
neither successful background delivery nor an input-routing defect.

## Remaining gates

- Complete unchanged native Linux all-suite and the two manual Hyprland suites.
- Review any bounded smoke grounding repair and replay it natively.
- Two independent app lanes, traced overlap/capacity/refusal and cleanup proof.
- Independent primary-input isolation and negative control on production bytes.
- Live removal/reinstallation, restart, upgrade and rollback evidence.
- Bind any published kit/package to the exact passing evidence.
- Omarchy-owned downstream acceptance and package promotion.

No native-certified profile or release asset is published by this record.
