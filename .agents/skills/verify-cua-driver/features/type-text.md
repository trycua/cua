# Type text

Text entry changes the addressed field and can submit it with a separate Enter action, without modifying another field.

## Sub-features

- `type-text`: replace the target field's text through AX or pixel addressing.
- `type-submit`: type, obtain a fresh handle, then press Return.
- `field-isolation`: keep the unrelated number input at `42`.
- `background-refusal`: reject unavailable delivery without target mutation.

## How to get to it (user POV)

Use MCP `type_text` with text and a window/element target, then `press_key` with `key: "return"` to submit. The CLI `call` surface exposes the same tool names. Select delivery explicitly rather than assuming the currently focused app.

## Driving it with MCP/testkit

Preconditions: [Launch and Doctor](../SKILL.md) succeed and the fixture has `txt-input`, `keyboard-input` and `number-input`.

- **Type:** run `CUA_E2E_CELL_FILTER=linux-electron-type-text-ax-foreground timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. The harness uses a fresh `txt-input` token and `text: "cua-ax-foreground"`. Require `mirror=cua-ax-foreground` and `number-input` still `42` in fixture evidence.
- **Submit:** run `CUA_E2E_CELL_FILTER=linux-electron-type-submit-ax-foreground timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require `key_state=submitted:cua-submit-ax-foreground`, with a new snapshot/token between typing and Return.
- **Refusal:** run `CUA_E2E_CELL_FILTER=linux-electron-type-text-ax-background timeout 1260 .agents/skills/verify-cua-driver/scripts/verify-linux.sh x11 --no-build`. Require `BackgroundUnavailable`, unchanged fixture state and stable sentinel/native observations. This is not a delivered-text proof.
- **Proof:** retain the type/submit requests and responses, before/after captures and independent fixture values. CLI entry remains unverified by the MCP harness.

## Gotchas

Linux Electron background text input is expected to refuse, even with an AX token; never select it as the positive Linux proof. A successful tool response alone does not prove the text or submission. Do not use CDP or a DOM setter to populate the target. Pixel entry is a distinct matrix row, not implicitly covered by AX.
