# Click

Click activates the intended app control without changing an unrelated app or stealing input during background delivery.

## Sub-features

- `left-click-ax`: activate the border target using a fresh AX handle.
- `click-px`: address the same target through screenshot-local pixels.
- `right-click` and `double-click`: preserve their distinct action semantics.
- `sdk-identity`: reject stale tokens and mismatched windows before valid delivery.

## How to get to it (user POV)

Call MCP `click`, `right_click` or `double_click`; use the CLI `cua-driver call` surface or typed SDK `click` for the same native-window operation. Select the app window first and explicitly choose `background` or `foreground` delivery.

## Driving it with MCP/testkit

Preconditions: [Launch and Doctor](../SKILL.md) succeed, with the Electron fixture staged.

- **AX background and SDK:** run `timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Both left-click AX background cells must deliver. The MCP request uses the fresh `border-click-target` element token; `fixture-state.json` must contain `last_action=left_click` and observer evidence must show no stolen focus/pointer/input.
- **Pixel foreground:** run `CUA_E2E_CELL_FILTER=linux-electron-left-click-px-foreground timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require the same marker and screenshot-local targeting in the transcript.
- **Right click:** run `CUA_E2E_CELL_FILTER=linux-electron-right-click-ax-foreground timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require `last_action=right_click`.
- **Double click:** run `CUA_E2E_CELL_FILTER=linux-electron-double-click-ax-foreground timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require `last_action=double_click`.
- **Proof:** inspect both turn screenshots, the tool response and fixture journal. The SDK sibling must reject stale/mismatched targets without fixture mutation, then deliver a valid click. CLI transport remains separately unverified by these commands.

## Gotchas

A new snapshot invalidates old element tokens. Do not substitute tree indices for tokens. Pixel coordinates are local to the captured window, accounting for origin and screenshot scale; the existing harness converts them. Linux background right/double/pixel click cells expect typed refusals, not successful delivery. The default substring filter intentionally selects two cells, including `-typed-sdk`.
