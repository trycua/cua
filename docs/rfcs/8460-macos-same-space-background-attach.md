---
title: Background existing-profile attach on the user's Space (macOS)
authors:
  - abonneth
created: 2026-11-21
last_updated: 2026-11-21
status: review
discussion: https://github.com/trycua/cua/issues/8460
rfc_pr:
implementation:
  - https://github.com/trycua/cua/pull/7482
supersedes:
superseded_by:
---

# RFC: Background existing-profile attach on the user's Space (macOS)

## Summary

On macOS, let `browser_prepare` with `strategy.kind: "existing_profile"` and
background delivery attach to a Chrome or Edge window that is shown on the
user's Space, under the self-activation guard that #5052 added for windows on
another Space. Keep refusing for a minimized or hidden window and for a window
whose Space is unknown. This RFC fixes what stays true for the user's window
and keyboard during the attach, and how a failed attach is reported.

## Motivation

Today the attach refuses with `background_unavailable` whenever the browser
window is on the user's Space and another app is in front, which is the usual
desktop. The only way through is `delivery_mode: "foreground"`, which fails
when the user is typing (#8460):

| macOS 26 arm64, Chrome 157, TextEdit in front, a key every 40 ms | main, background | main, foreground |
| --- | --- | --- |
| attached | 0/10, refused | 0/10, refused |
| Chrome window above the user's | never | 10/10, 728 to 1159 ms |
| keystrokes that missed the user's document | 0/188 | 136/643 |

## Goals

- A background attach to a browser window shown on the user's Space.
- Guarantees for the user's window and keyboard that hold in every outcome,
  including a failed attach.
- A failed attach is a refusal with its effects rolled back, never a silent
  foreground step.

## Non-goals

- Windows and Linux. Their background attach is unchanged.
- Minimized or hidden windows: Chrome brings them back on screen when the
  setup page opens (measured below), so they keep the refusal.
- Making `delivery_mode: "foreground"` safe while the user types.

## Terminology

- **User's Space**: the Space shown on the display the user works on.
- **Shown window**: a layer-0 browser window that WindowServer reports
  on-screen, so neither minimized nor hidden.
- **Guard**: `SelfActivationGuard` in `platform-macos/src/browser/platform.rs`.
  It polls WindowServer's front process every 2 ms. When the browser comes in
  front, it gives key focus back to the user's window. When a browser window
  is ordered above the user's window, it raises the user's window again.
- **Foreground assist**: `with_foreground_assist`, which brings the browser to
  the front and sends global HID input.

## Current state

All references are to main at `b6b335c915`.

- The guard is requested for a background attach when the browser is not the
  front app
  ([`prepare.rs:1477-1487`](https://github.com/trycua/cua/blob/b6b335c915ed9c3b78a77707bebd76f3552a88d5/libs/cua-driver/rust/crates/cua-driver-core/src/browser/prepare.rs#L1477-L1487)).
  macOS grants it only when the window is known to be on another Space
  ([`platform.rs:294-296`](https://github.com/trycua/cua/blob/b6b335c915ed9c3b78a77707bebd76f3552a88d5/libs/cua-driver/rust/crates/platform-macos/src/browser/platform.rs#L294-L296)).
  Otherwise the attach refuses with `background_unavailable` and
  `background_attach_requires: "browser_window_on_another_space"`, having
  changed nothing
  ([`prepare.rs:1521-1561`](https://github.com/trycua/cua/blob/b6b335c915ed9c3b78a77707bebd76f3552a88d5/libs/cua-driver/rust/crates/cua-driver-core/src/browser/prepare.rs#L1521-L1561)).
- Under the guard, setup works through accessibility and events posted to the
  browser's pid: it opens a tab with `AXPress`, sets the address field to the
  fixed `chrome://inspect/#remote-debugging` URL, types it with pid-routed
  keys when `AXConfirm` is not offered, toggles the remote-debugging checkbox
  with `AXPress` or a pid-routed Space key, and presses the consent prompt's
  Allow button with `AXPress`.
- Three setup steps can still use foreground assist while the guard is
  active, in `platform-macos/src/browser/setup_ui.rs`:
  - navigation, when typing in the background failed and the address field
    offers no exact suggestion (line 1669);
  - Edge's trusted checkbox click, when `AXPress` did not toggle it (line 1859);
  - Edge's trusted rollback click (line 1178).
- Under the guard the driver does not wait for a person to answer the
  browser's consent prompt, because "it opens on another Space"
  ([`prepare.rs:1834-1839`](https://github.com/trycua/cua/blob/b6b335c915ed9c3b78a77707bebd76f3552a88d5/libs/cua-driver/rust/crates/cua-driver-core/src/browser/prepare.rs#L1834-L1839)).
  The driver-level grant (`--grant existing-profile`, or an approval in
  standard mode) is the consent.
- `end_session` turns remote debugging back off when the driver turned it on,
  under the same guard rule.

## Proposal

### Which windows attach in the background

| browser window | result |
| --- | --- |
| on another Space | guarded attach (unchanged) |
| shown on the user's Space | guarded attach (new) |
| minimized or hidden on the user's Space | refused, nothing changed |
| Space unknown, or not a layer-0 window | refused, nothing changed |
| browser is the front app | attach without the guard (unchanged) |

`end_session` cleanup follows the same table.

### What stays true for the user

During a guarded attach and its cleanup:

1. The app that was in front when the call started is in front when it
   returns, whatever the outcome.
2. Setup never uses foreground assist or global HID input. Its input goes
   through accessibility or events posted to the browser's pid. A step that
   has no such route rolls back and refuses (see below). This closes the three
   routes listed in Current state, for windows on another Space too.
3. The browser can bring itself in front while it opens the setup page or the
   prompt. The guard hands the front back, and that interval is the only
   disturbance. The test plan bounds it.
4. A key the user types in that interval can reach the browser. Setup commits
   the URL only when the address field reads back exactly the setup URL, and
   rolls back otherwise, so a stray key ends in a refusal. The typed route
   already checks before Return. The `AXConfirm` route reads the field back
   (`setup_ui.rs:1553`) but confirms (line 1560) before acting on that check
   (line 1564); the implementation checks first.
5. The driver never restores a minimized window, unhides an app, changes the
   Space, or moves the pointer.

### Consent

As on another Space today, the driver answers the browser's prompt with Allow
and does not wait for a person. The prompt is the browser's own dialog, and
the guard keeps it behind the user's window. The consent that counts is the
driver-level grant, checked before any setup starts.

### Failure reporting

| case | refusal | effects |
| --- | --- | --- |
| window not eligible (table above) | `background_unavailable`, `background_attach_requires: "browser_window_shown_or_on_another_space"` | none |
| a setup step has no background route | `background_unavailable`, `limitation: "setup_requires_foreground"`, `alternative_delivery_mode: "foreground"` | rolled back, listed in `setup_side_effects` |
| any other setup or consent failure | unchanged | rolled back, unchanged |

## Alternatives considered

- **Keep the refusal.** The caller's only way through is the foreground
  attach, which loses 136 of 643 keystrokes and fails while the user types.
- **Attach to minimized or hidden windows too.** Measured: a minimized window
  ends up above the user's for 525 to 578 ms in 6 of 6 attaches and stays
  unminimized; a hidden app comes back on screen in 6 of 6.
- **Move the browser window to another Space first.** Changes the user's
  desktop, which background delivery must not do.
- **CDP only, no setup UI.** Not possible the first time: remote debugging is
  off until the checkbox is toggled.

## Compatibility and migration

- The MCP schema does not change. `browser_prepare` attaches where it refused,
  and its description changes.
- `background_attach_requires` keeps its name and gets a new value. A caller
  that compares it to `"browser_window_on_another_space"` must update.
- `limitation: "setup_requires_foreground"` is a new value of an existing field.
- Rollback: revert the implementation; the refusal comes back.

## Security, privacy, and telemetry

- The permission boundary is unchanged: no setup starts without the
  driver-level existing-profile grant.
- The setup page and prompt are the browser's own UI; the driver reads only
  the controls it presses.
- No new telemetry.

## Implementation plan

1. #7482: the eligibility table, refusal detail, tool description and e2e rows.
2. In #7482 as well: under the guard, replace the three foreground-assist
   routes with a rollback and `setup_requires_foreground`, and check the
   address field before `AXConfirm`.

## Test and acceptance plan

Hosted `macos-26`, TextEdit in front with a key posted every 40 ms, front app
and window order polled every 3 ms, at least 40 attaches and 20 cleanups:

- attached in at least 95%;
- the browser window above the user's in at most 1 attach in 20, for at most
  200 ms each;
- `setup_side_effects.foregrounded_window` and `injected_global_input` false
  in every guarded call;
- minimized and hidden windows refused and left as they were, 6 of 6 each;
- the upstream hosted browser lane passes on Chrome and Edge.

#7482 measured, before the foreground-assist routes were closed:

| | branch, background |
| --- | --- |
| attached | 49/49 |
| Chrome window above the user's | 1/49, 137 ms |
| Chrome front process | 12/49, 5 to 44 ms (111 ms once) |
| keystrokes that missed the user's document | 8/4079 |

## Unresolved questions

- Is a front-process change of up to about 100 ms, handed back by the guard,
  acceptable for `delivery_mode: "background"`?
- On the user's Space a person could see the prompt if the browser window is
  not covered. Should the driver still answer it at once?
- Multiple displays and Spaces were not measured.

## Decision record

Pending review.
