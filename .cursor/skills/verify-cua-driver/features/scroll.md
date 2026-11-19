# Scroll

Scroll advances the intended viewport and verifies movement independently of the tool response.

## Sub-features

- `scroll-ax`: address a scroll container by its fresh AX token.
- `scroll-px`: address it by screenshot-local position.
- `scroll-distance`: two requested pages advance at least one viewport.
- `background-refusal`: unavailable background delivery leaves state unchanged.

## How to get to it (user POV)

Call MCP `scroll` or the CLI `call` surface with a window target, delivery mode, direction, unit and amount. The fixture's tall scroll region represents a real app viewport.

## Driving it with MCP/testkit

Preconditions: [Launch and Doctor](../SKILL.md) succeed; `scroll-tall` is present in the fixture AX tree.

- **AX:** run `CUA_E2E_CELL_FILTER=linux-electron-scroll-ax-foreground timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. The tool arguments include the `scroll-tall` token, `direction: "down"`, `by: "page"`, `amount: 2`. Require the independent fixture journal to show at least one viewport of movement within two seconds.
- **Pixels:** run `CUA_E2E_CELL_FILTER=linux-electron-scroll-px-foreground timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require the same distance contract and before/after images.
- **Background:** run `CUA_E2E_CELL_FILTER=linux-electron-scroll-ax-background timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. On Linux Electron expect `BackgroundUnavailable`, no fixture mutation and no sentinel/native side effects, not delivered scrolling.
- **Proof:** preserve request/response, visible viewport change and `fixture-state.json`. CLI entry is not exercised by these MCP cells.

## Gotchas

A response acknowledgment is not movement. Do not loosen the viewport-distance assertion to make a small wheel twitch pass. Background delivery differs by platform; AX does not imply availability. Pixel coordinates require the existing window-origin and screenshot-scale conversion.
