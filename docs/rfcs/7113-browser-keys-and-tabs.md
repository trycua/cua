---
title: Background keys and tabs for bound browsers
authors:
  - abonneth
created: 2026-11-10
last_updated: 2026-11-10
status: review
discussion: https://github.com/trycua/cua/issues/7113
rfc_pr: https://github.com/trycua/cua/pull/7257
implementation:
  - https://github.com/trycua/cua/issues/7111
  - https://github.com/trycua/cua/issues/7113
supersedes:
superseded_by:
---

# RFC: Background keys and tabs for bound browsers

## Summary

Add two browser tools that act over the driver's existing DevTools binding:
`browser_press_key` sends a key or chord to a bound tab, and `browser_new_tab`
opens a tab in a bound browser and returns its `tab_id`. Neither changes the OS
foreground. They close the last two browser actions that still raise the
user's browser.

## Motivation

- On Windows, `press_key` and `hotkey` to a Chromium window that is not in
  front return `background_unavailable`
  (`platform-windows/src/tools/hotkey.rs`): Chromium reads modifier state live,
  so posted chords do nothing. The only path is `delivery_mode: "foreground"`,
  which raises the browser over the user's app.
- No tool opens a tab. `browser_navigate` replaces the page the user may be
  reading. Hosts relaunch the browser executable with a URL, and Chromium's
  launcher hands the running instance the foreground.

Measured on Edge 156, Windows 11 arm64, a host window in front
([run](https://github.com/abonneth/cua-fork/actions/runs/42710143076),
[script](https://github.com/abonneth/cua-fork/blob/ab/chords/ab/cdp_chords.py)):

| Action | Reps | Effect | Edge took the foreground |
|---|---|---|---|
| Relaunch `msedge.exe <url>` (today's workaround) | 5 | tab opened 5/5 | 4/5 |
| `Target.createTarget`, `background: true` | 5 | tab opened 5/5 | 0/5 |
| `Target.createTarget`, `background: false` | 5 | tab opened 5/5 | 0/5 |
| `Input.dispatchKeyEvent` `ctrl+t` | 5 | tab opened 5/5 | 0/5 |
| `Input.dispatchKeyEvent` `ctrl+w` | 1 | tab closed | no |
| `Input.dispatchKeyEvent` `f5`, `ctrl+r` | 1 each | page reloaded | no |
| `Input.dispatchKeyEvent` `ctrl+a` in a field | 1 | 5 of 5 chars selected | not measured |

## Goals

- Keys and chords reach a bound tab without a foreground change, on every
  platform.
- A tab opens in a bound browser without a foreground change and comes back
  with a `tab_id`.
- Both tools use the binding, session, and protected-resource checks that
  `browser_navigate` and `browser_type` use.

## Non-goals

- Native `press_key` and `hotkey` on unbound windows: unchanged.
- Dedicated close, reload, or select-tab tools (`browser_close_tab`,
  `browser_reload`, `browser_select_tab`).
- Browser UI that DevTools cannot observe (omnibox focus, menus).

## Current state

- The existing-profile allowlist (`cua-driver-core/src/browser/cdp_ws.rs`,
  `EXISTING_PROFILE_METHODS`) has `Input.dispatchKeyEvent` and
  `Browser.getWindowForTarget`, not `Target.createTarget`.
- `browser_type` with `mode: "keystrokes"` sends characters to an editable
  ref, without modifiers.
- The cursor classification (`cua-driver-contract/src/cursor.rs`) already
  names `browser_press_key` and `browser_new_tab`; no tool implements them.

## Proposal

### `browser_press_key`

```text
browser_press_key {target_id, tab_id, session, key, modifiers?, ref?}
```

- `key` and `modifiers` match native `press_key`: key names (`return`, `tab`,
  `escape`, arrows, `f1`-`f12`, a letter or digit) and modifiers
  `ctrl`/`shift`/`alt`/`meta`.
- Sends `rawKeyDown` for each modifier, `rawKeyDown`/`keyUp` for the key with
  the modifier bitmask, then the modifier `keyUp`s, on the tab's CDP session.
- `ref` (optional) focuses that element first, with the ref checks
  `browser_type` uses. Without it, keys go to the page's current focus.
- Annotations: `destructive: true` (a chord can close or reload the tab),
  `idempotent: false`.
- Result: `{status, target_id, tab_id, key, modifiers, refs_invalidated: true}`.
- Protected-resource scope: the tab resource `browser_type` uses, with
  `action_class: "key_input"`.

### `browser_new_tab`

```text
browser_new_tab {target_id, tab_id, session, url} -> {tab_id, url}
```

- `tab_id` names an existing bound tab. It anchors the protected-resource
  scope (the `browser_navigate` shape: `action_class: "navigation"`,
  `requested_origin` from `url`) and the window.
- Calls `Target.createTarget {url: "about:blank", background: true}`, checks
  with `Browser.getWindowForTarget` that the tab is in the anchor's window,
  then `Page.navigate` to `url` (http, https, or about, as in
  `browser_navigate`).
- Mints and returns the new `tab_id` on the same target.
- Adds `Target.createTarget` to `EXISTING_PROFILE_METHODS`.

## Alternatives considered

- **`browser_navigate` without `tab_id` opens a tab.** One tool with two
  scopes and two results. Rejected for clarity.
- **`ctrl+t` through `browser_press_key`.** No allowlist change, but it returns
  no id and depends on the browser's key bindings.
- **Background chords in native `hotkey`.** Chromium reads modifier state from
  the live keyboard, so posted chords cannot work without the foreground.
- **Hosts call the DevTools port directly.** This bypasses the consent model,
  and the port is not discoverable when the driver turned remote debugging on.

## Compatibility and migration

Additive: two tools and one allowlist entry. Rollback removes them. Generated
tool specs, `BROWSER.md`, and the permissions guide gain the two tools.

## Security, privacy, and telemetry

- Exact bindings only; heuristic bindings are refused, as for the other
  mutating browser tools.
- `browser_press_key` reaches any browser shortcut the page's focus allows,
  including closing the tab, hence `destructive: true` and its own action
  class.
- `Target.createTarget` only ever opens `about:blank`. The URL goes through
  `Page.navigate`, so the origin checks of `browser_navigate` apply.
- Telemetry records key names and modifiers, as `press_key` does. No page
  content.

## Implementation plan

1. `browser_press_key`: core tool, mock-CDP tests, docs.
2. `browser_new_tab` and the allowlist entry: core tool, mock-CDP tests, docs.

Each PR is independently revertible.

## Test and acceptance plan

- Mock-CDP unit tests: the event sequence and modifier bitmask, refusal on a
  heuristic binding, unknown or stale `tab_id`, window mismatch on
  `browser_new_tab`.
- Windows e2e with Edge on an existing profile, a host window in front, 20
  reps per case:
  - `browser_press_key`: `ctrl+a` selects a field, `f5` reloads, and the
    foreground is unchanged 20/20.
  - `browser_new_tab`: the tab opens in the bound window and the foreground
    is unchanged 20/20.
- The same e2e once on macOS and Linux.

## Unresolved questions

- `Target.createTarget` takes no window id: with several browser windows, the
  tab opens in the last active one. On a mismatch, refuse and leave the tab
  open, or close it (adds `Target.closeTarget` to the allowlist)?
- Keys to an inactive tab: do they need `Emulation.setFocusEmulationEnabled`,
  as `browser_type` keystrokes do? The probe covered the active tab of a
  background window only.
- Should chords that open native OS dialogs (print, save, open) be refused?
- `key_input` as a new action class, or reuse `page_input`?

## Prior art

[browser-harness](https://github.com/browser-use/browser-harness) (MIT) opens
tabs with `Target.createTarget(background=True)` then navigates, and sends
chords as `Input.dispatchKeyEvent` with a modifier bitmask.

## Decision record

Pending review.
