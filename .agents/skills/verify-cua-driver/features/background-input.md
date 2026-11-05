# Background versus foreground input

Delivery mode makes the intended effect on the user's desktop explicit: background operations must not steal focus, move the real pointer, raise the target or leak input into the foreground app.

## Sub-features

- `background-delivered`: AX click reaches a fully occluded target.
- `foreground-delivered`: explicit foreground click activates the target path.
- `no-stolen-input`: native observer and sentinel independently detect desktop changes.
- `typed-refusal`: unsupported delivery fails without mutating the fixture.

## How to get to it (user POV)

Set `delivery_mode` explicitly to `background` or `foreground` on the desktop input tool. Address the intended window/element, not whichever app happens to own keyboard focus. MCP, CLI `call` and the typed SDK expose input operations.

## Driving it with MCP/testkit

Preconditions: [Launch and Doctor](../SKILL.md) succeed, and the sentinel completely occludes the target before capture and dispatch.

- **Background:** run `timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require both AX click cells delivered with `last_action=left_click`, a live sentinel heartbeat, no input/blur events, stable native focus and pointer, unchanged background z-order, and supported oracles.
- **Foreground:** run `CUA_E2E_CELL_FILTER=linux-electron-left-click-ax-foreground timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require the target click marker. Foreground input is allowed to activate the target; do not apply a background guarantee to this row.
- **Unavailable route:** run `CUA_E2E_CELL_FILTER=linux-electron-type-text-ax-background timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require the typed refusal, unchanged fixture journal and stable desktop, not delivery.
- **Proof:** retain sentinel JSONL and native observation alongside the call, images and fixture state. Missing/unsupported observation is failure, not evidence of no change. CLI delivery parity is not covered by the MCP transcript.

## Gotchas

Linux Electron permits background AX left click and child-window activation in this matrix; other background actions are expected to refuse. Sentinel setup may intentionally manipulate focus before the recording boundary; only the drained action boundary certifies no stolen input. Fixture journal state is independent of AX text, which can lag. X11 results do not prove nested Wayland or other platforms.
