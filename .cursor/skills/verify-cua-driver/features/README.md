# Cua Driver verification map

Read [Launch, Doctor, Evidence and Cleanup](../SKILL.md) before every journey. These recipes use a disposable desktop, source-built driver and real Electron fixture; do not drive a user's existing session. Each invocation creates its own daemon socket, app instances and evidence directory. The GUI session is isolated, but simultaneous Cargo builds share the normal Cargo lock.

## Features

- [Click](click.md): AX/pixel targeting, left/right/double click, SDK handle validation.
- [Type text](type-text.md): text entry, submit, isolation from unrelated fields.
- [Scroll](scroll.md): target-local wheel movement and state read-back.
- [Window discovery](window-discovery.md): list windows, select identity and obtain AX state.
- [Background versus foreground input](background-input.md): explicit delivery, no stolen input, typed refusals.

## Proof contract

Each feature lists sub-features and user entry points; selecting one cell does not certify the rest. MCP tools and the CLI `call` surface address the same daemon, but only the default click also exercises the typed SDK. CLI transport parity is not proven by an MCP result.

Run commands from the repository root. `scripts/verify-linux.sh` is the strict proof gate: it always runs the two background AX click cells and its checker requires both. Other journeys are diagnostics: run the canonical runner directly, as the recipes below do, inside a disposable X11 desktop (for example `xvfb-run -a dbus-run-session -- openbox` with a window manager and AT-SPI as in the helper) after a first `verify-linux.sh x11` has built and staged the fixture. Diagnostic runs are not the proof gate. `CUA_E2E_CELL_FILTER` is a substring filter.

Capture tool requests/responses, before/after images and independent state. A delivered mutation must change the expected fixture marker, not an unrelated field. A typed refusal must leave fixture state unchanged. Background actions additionally require native desktop and sentinel evidence. Missing prerequisites, skipped cells, unsupported observations and unreachable entry points are failures or explicit coverage gaps, never success through a substitute path. Evidence survives cleanup at the absolute path printed by the helper.
