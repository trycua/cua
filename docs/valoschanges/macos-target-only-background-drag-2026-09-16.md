# macOS target-only background drag

## Original bug

The macOS `drag` tool refused every window-scoped background request even though the native layer already implemented pid/window-routed press-drag-release events. ProcessOn therefore required a CDP browser route or a foreground activation. The expected behavior is an exact-window drag that leaves ValOS-sit frontmost and does not move the real pointer or raise Chrome.

## Reproduction

- Target: a signed-in ProcessOn diagram in Google Chrome on macOS.
- Foreground app: ValOS-sit (`com.xingin.valos.sit`).
- Preconditions: Chrome remains behind ValOS-sit; the ProcessOn circle fixture is visible in a captured target-window snapshot.
- Prompt/workflow: run the no-LLM ProcessOn workflow, drag the circle left with `delivery_mode:"background"`, verify its new position, then reverse the same drag and verify restoration.

Before this change, the first `drag` call returned `background_unavailable` without posting input.

## Root cause

The tool rejected background mode before resolving its exact window. The lower-level `SLEventPostToPid` drag primitive existed, but Chromium can discard that stream unless its process-local active/key routing state is established. Foreground HID delivery supplied that state by visibly activating Chrome; the background path did not have an equivalent bounded transaction.

## Change

An exact macOS background drag now begins the target-only synthetic-focus route contributed in #3530, posts the complete pid/window-routed gesture, waits for the target renderer to consume mouse-up, and tears down only the target's synthetic state. Missing `window_id` or unavailable private SPI refuses before mouse-down. Foreground drag remains global HID.

This is a macOS private-SPI capability and does not claim portability to Windows, X11, or Wayland. Callers must still verify the business postcondition because queued native input is not proof that an application accepted the drag.

## Evaluation

Candidate SHA is pending the runtime commit.

- `cargo test -p platform-macos --locked`: 373 passed, 0 failed, 2 ignored.
- `cargo test -p cua-driver --test harness_appkit_test --locked --no-run`: compiled successfully; only pre-existing warnings were emitted.
- `cargo test -p cua-driver --test harness_appkit_test --locked harness_appkit_slider_drag_px_background -- --ignored --nocapture`: the case did not execute because the separately installed `CuaDriver.app` daemon was not running. The failure occurred during daemon proxy setup before any drag call.
- `npx --yes tsx scripts/docs-generators/runner.ts --library cua-driver --check`: blocked because `docs/node_modules/tsx/dist/cli.mjs` is absent. The generated `drag` reference was updated manually to match the schema text.

The complete Cua Driver evaluation has not been run. Local ValOS-sit smoke evidence and artifact paths will be recorded after deployment. A maintainer must run `libs/cua-driver/tests/runners/macos-lume/run-all.sh` on the exact final candidate before the pull request is made ready.
