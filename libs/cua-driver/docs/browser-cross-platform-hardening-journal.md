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
  identity. Sway remains the only accepted native-Wayland browser setup route.
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
- Added protocol and compositor identity design records.

## Local evidence

- `cargo test -p cua-driver-core browser::`: 132 passed, 0 failed.
- `cargo test -p cua-driver-testkit`: 49 passed, 0 failed.
- `cargo test -p cua-driver --test standalone_browser_behavior_test --no-run`:
  compiled successfully at the current working tree.

## Representative evidence pending

- macOS Tahoe Lume worker: install exact source, verify TCC, run the standalone
  Chrome and Edge matrix in the logged-in Aqua session.
- Windows interactive desktop: replay Chrome and Edge from the exact branch.
- Linux X11: Chrome regression plus explicit Chromium and Edge attempts.
- Linux Wayland/Sway: Chromium regression plus explicit Chrome attempt.
- Non-Sway Wayland: record only fail-closed evidence unless an exact compositor
  identity adapter is implemented and separately accepted.
