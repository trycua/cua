---
title: Explicit window-only video recording
authors:
  - f-trycua
created: 2026-09-08
last_updated: 2026-09-08
status: accepted
discussion: https://github.com/trycua/cua/issues/3644
rfc_pr: https://github.com/trycua/cua/pull/3667
implementation:
  - https://github.com/trycua/cua/pull/3667
supersedes:
superseded_by:
---

# RFC: Explicit window-only video recording

## Summary

Add an opt-in window-video mode to `start_recording`. The caller selects one
native window by PID and window ID. The macOS adapter records that window with
a desktop-independent ScreenCaptureKit filter. This mode writes video and
bounded recording metadata only: no trajectory screenshots, accessibility
trees, action arguments, or global cursor samples. Unsupported backends refuse
the request before capture. No failure widens capture to a display.

This is the capture-scope increment of #3644. It does not claim to resolve the
broader recorder-control and concurrent-recorders questions in that issue.

## Motivation

Recording one application should not require collecting the surrounding
desktop. Cropping a display recording does not provide the same boundary:
another window can cover the selected rectangle, and the original file still
contains unrelated pixels. A target-window stream avoids collecting those
pixels in the first place.

## Goals

- Produce a finalized H.264 MP4 containing only the selected native window.
- Keep capture in the daemon's existing OS permission identity.
- Preserve existing recording calls that omit `target`.
- Make capture scope, unsupported cases, and termination reasons observable.
- Verify exclusion of occluding and adjacent windows with synthetic fixtures.

## Non-goals

- Multiple simultaneous recorders or an owner-only replacement/stop redesign.
- Window-scoped trajectory recording, replay, or human demonstration capture.
- Agent-cursor compositing, audio, microphone capture, or browser-tab capture.
- Cropping out browser chrome or redacting content inside the selected window.
- Window-video implementations for Windows, X11, or Wayland in this increment.
- Installing, replacing, or restarting the released Driver to run development
  tests; release and local builds remain separate.

## Terminology

- **Legacy mode:** a call without `target`; its trajectory and optional display
  video behavior is unchanged.
- **Window-video mode:** a call with an explicit window target and
  `record_video: true`; only that window's video and recording metadata are
  collected.
- **Capture identity:** platform-attested window ownership and capture geometry,
  not a title match or a screen rectangle.

## Current state

At `4bc38a79237cc842a5039c310ab40132485b01cf`,
[`recording_tools.rs`](../libs/cua-driver/rust/crates/cua-driver-core/src/recording_tools.rs)
accepts `output_dir` and `record_video`. Video defaults to false. The shared
[`VideoBackendFactory`](../libs/cua-driver/rust/crates/cua-driver-core/src/video.rs)
receives only an output path.

The macOS
[`video_sckit.rs`](../libs/cua-driver/rust/crates/platform-macos/src/video_sckit.rs)
selects a display and records it with `SCStream` and `SCRecordingOutput`.
[`capture.rs`](../libs/cua-driver/rust/crates/platform-macos/src/capture.rs)
already validates window capture identity and uses a desktop-independent
`SCContentFilter::with_window` for screenshots.

The recorder is a singleton. Legacy manual stop is unconditional; session
disconnect teardown is owner-aware. PR #3632 separately addresses trajectory
turn attribution. This RFC neither duplicates nor supersedes that work.

## Proposal

### Public input and output

Extend `start_recording` with an optional `target`:

```json
{
  "output_dir": "/tmp/window-demo",
  "record_video": true,
  "target": {"kind": "window", "pid": 1234, "window_id": 5678}
}
```

The example IDs are placeholders for values from `list_windows`. Require a
positive, representable PID and window ID. Reject missing fields, unknown
target kinds, extra target fields, and a target combined with missing or false
`record_video`. Reject ambiguous legacy top-level PID/window fields rather
than guessing. Omitting `target` keeps the existing behavior; a new desktop
selector is not needed for this increment.

Add equivalent CLI flags to `recording start`: `--video --pid PID --window-id
WINDOW_ID`. Require the two window flags together and require `--video` for a
window request. Preserve the existing bare command's trajectory-only default.
Keep the generic SDK `call_tool` surface working and add a typed convenience
only where the SDK already exposes recording configuration.

Recording state and `session.json` identify `mode: "window_video"` and the
requested/resolved window target. Include pixel dimensions, backend identity,
whether finalization succeeded, and a stable termination/error classification.
Do not report video active merely because an in-memory backend exists after
its capture stream has stopped. Legacy metadata remains readable; new fields
are additive and do not reinterpret old recordings.

### Shared ownership and artifact boundary

The shared core owns target parsing, validation, mode selection, artifact
policy, and status. The platform backend receives a typed capture request.
Use a recording-specific target validator before authorization; do not route
recording through the input-action target normalizer. Both the observation
resource and backend request must retain the same validated PID/window pair.

Window-video mode suppresses trajectory turn reservation and global cursor
sampling from the start, including calls from the initiating session. It does
not write `turn-*` directories or `cursor.jsonl`. Disable system-cursor and
audio capture for the window stream. The live input-safety overlay remains
enabled during Driver actions, but is not included in this recording mode.

Use a new output directory or refuse if recorder artifacts already exist in
it. Do not overwrite an older recording or mix files from different modes.
Validation and capability checks must precede output mutation and teardown.
A failed window-video start is an error, not a successful trajectory-only
recording. Reserve the destination against competing recording starts before
creating encoder output; recheck the prepared target before stream start.

Keep one recorder. Starting a window recording while any recording is active,
or starting another mode while a window recording is active, returns
`recording_busy` without replacing it. Legacy-to-legacy replacement remains
unchanged. Manual stop remains explicitly daemon-wide; session disconnect
stops only its owned recording. Therefore window-video isolation is a content
boundary, not independent recorder control. A foreign stop can end a recording
but cannot widen its capture target. The broader ownership decision stays in
#3644.

### Native macOS behavior

Resolve and validate the exact PID/window pair using the existing capture
identity logic. Use `SCContentFilter::with_window`, not a display crop or an
application-wide filter. Derive even-sized encoder dimensions from the
filter's content rectangle and pixel scale without discarding edge content.
The requested PID must match both WindowServer and ScreenCaptureKit ownership.
The retained health fingerprint excludes position so ordinary movement is not
mistaken for a replacement window; ownership, layer, dimensions, and scale
remain checked.

An ordinary position change does not change the target. Occluding windows,
desktop, Dock, menu bar, and separate overlay windows are excluded. Content
inside the selected window, including tab bars, is included.

For this first version, output dimensions are fixed. A detected resize or
backing-scale change stops and finalizes the recording with a classified
reason; it does not silently distort the capture or start another file.
Closing, minimizing, losing shareability, or losing the attested owner also
stops capture. A missing, minimized, invalid, or unshareable initial target
fails before recording starts. A changed or reused window identity never
causes automatic retargeting.

Capture has a bounded health check and retains the recording output until
finalization completes. Native recording completion/error callbacks, rather
than transport success alone, determine finalization. Stop, disconnect,
startup failure, and health-check termination release owned stream resources.

### Platform behavior

| Backend | Window-video mode | Existing no-target mode |
| --- | --- | --- |
| macOS 15+ ScreenCaptureKit | Native exact-window capture, subject to OS consent and shareability | Unchanged |
| Windows | Explicit `window_recording_unsupported` | Unchanged |
| Linux X11 | Explicit `window_recording_unsupported` | Unchanged |
| Linux Wayland | Explicit `window_recording_unsupported` | Unchanged |

No unsupported backend substitutes display recording. Shared tests cover the
refusal; native support requires a later implementation and evidence.

## Alternatives considered

- **Display capture followed by cropping:** rejected because it collects
  unrelated desktop pixels and includes occluders.
- **A separate recorder executable:** rejected for the product feature because
  it creates another macOS capture-permission identity and duplicates lifecycle.
- **Window video plus legacy trajectory/global cursor collection:** rejected
  because the resulting artifact directory is not window-scoped.
- **Full per-session recorders and cursor compositing now:** deferred to avoid
  coupling the first exact-window stream to a larger concurrency redesign.
- **Dynamic stream resizing:** deferred; explicit termination is testable and
  avoids changing the dimensions of an active MP4 encoder.

## Compatibility and migration

The change is opt-in and requires no migration for no-target calls. Calls with
invalid or unsupported targets fail instead of entering legacy mode. CLI,
MCP, and SDK contracts expose the same semantics. Update skills and generated
tool documentation together, including the existing video-default wording.
Rollback removes the opt-in capability; it must not reinterpret window
requests as display requests. Keep the RFC and implementation in review until
the native evidence is attached; no release is implied by a draft PR.

## Security, privacy, and telemetry

Bind observation authorization to the platform-attested window and output
authorization to the exact destination. A window grant does not authorize
display discovery/capture. Preserve managed/user policy and OS consent gates.
Do not weaken permission checks or use unrestricted mode as a substitute for
capture-scope validation.

Do not collect window titles, URLs, page text, screenshots, or process command
lines in telemetry. Local metadata contains only the capture identifiers and
geometry needed to explain the file. Synthetic test evidence must not contain
real account data. Native window capture cannot redact sensitive information
that appears inside the selected window; callers remain responsible for its
content. The daemon-wide manual-stop limitation remains documented.

## Implementation plan

1. Review and record this scoped decision before implementation.
2. Extend common request/state/backend contracts and add fail-closed tests.
3. Implement macOS window selection, health checks, and finalization. Keep
   other backend refusals explicit.
4. Wire CLI/SDK/docs and add synthetic native capture tests. These changes can
   proceed in parallel after the shared contract is fixed.
5. Review the integrated diff, run focused tests, and verify native isolation
   on the exact candidate. Keep the PR draft; a later ready/merge action needs
   the repository's canonical platform certification and review gates.

## Test and acceptance plan

- Invalid targets, missing video opt-in, ownership mismatch, unsupported
  platforms, and occupied output directories cause no recorder replacement or
  out-of-scope capture.
- Window mode creates video/metadata only. Owner and foreign actions create no
  turns or global cursor data, including in-flight calls across mode changes.
- Two independent sessions prove the busy rule, daemon-wide manual stop,
  owner-disconnect teardown, and preservation of legacy no-target behavior.
- A synthetic moving window produces changing frames while an adjacent and
  occluding window has a distinct test marker absent from every captured
  frame. The input focus, window order, and real cursor remain unchanged by
  starting/stopping capture.
- Verify Retina dimensions, odd dimensions, movement, resize, close/minimize,
  stale target, and backing-scale policy with explicit result classifications.
- Decode the entire MP4 with ffmpeg, inspect representative frames, and verify
  its dimensions, timestamps, duration, and lack of audio with ffprobe.
- Repeated start/stop, failed starts, and disconnect leave no live capture
  workers or growing descriptor count attributable to the recorder.
- Shared/contract and macOS focused evidence is required for the draft's
  implementation claim. Full affected-platform certification is required
  before ready/merge, not replaced by a manual capture.

## Unresolved questions

Owner-only control, simultaneous recorders, agent-cursor compositing, dynamic
resizing, and additional native backends remain explicitly deferred. They do
not block evaluation of this video-only increment.

## Decision record

Accepted for the selected macOS-first implementation, not for release. The
additive window-video contract preserves no-target behavior and intentionally
defers concurrent recorders, owner-only manual stop, scoped trajectories, and
cursor compositing. It does not mark #3644 complete.

Independent contract and native-feasibility review identified four required
implementation safeguards: recording-specific pre-authorization target
validation, transactional startup without trajectory fallback, requested-PID
checks against both native ownership sources, and callback-backed live status
and finalization. These requirements are incorporated above. Ordinary window
movement must not invalidate a geometry fingerprint merely because its origin
changed. The selected resize policy is explicit termination, not a dynamic
encoder-size change.

Remaining risks are native shareability/permission behavior, finite health
check latency, same-process window-ID reuse, and callback failure or timeout.
They require synthetic native evidence and explicit limitations before the
draft can be considered ready. The decision is recorded on the discussion
issue; the draft PR remains the execution and evidence record.
