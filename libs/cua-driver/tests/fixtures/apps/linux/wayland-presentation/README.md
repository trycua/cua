# Wayland presentation-timestamp latency fixture

A repository-owned Wayland client whose visible state changes in response to a
Cua Driver action, and which records presentation feedback for that exact
content update.

It exists to answer one question: **which side of the presentation boundary
owns a slow action?** Driver dispatch, application paint, compositor
scheduling, and Driver-owned post-action waiting all look identical from the
outside. Without feedback tied to one submitted surface update, a screenshot
poll or a sleep is the only available stand-in, and neither can distinguish
them.

## Why a raw Wayland client and not GTK

The fixture owns its own `wl_surface`. That is the whole point:

- `wp_presentation.feedback` is bound to the *next commit* on a surface, so the
  process that commits must be the process that asks for feedback;
- a toolkit owns its own commits and may coalesce several state changes into
  one content update, so a toolkit-hosted fixture cannot prove which commit
  carried the mutation being measured.

GTK3/GTK4 coverage lives in the sibling fixtures and remains the right place
for accessibility and widget-tree behavior.

## Regions

The fixture publishes its surface-local region map in its `startup` journal
record, so the runner clicks published centers instead of guessing. The map
moves with the surface: when the compositor resizes the window the fixture
re-lays-out and republishes it as a `layout` record, so a consumer must follow
the most recent one. The fixture commits the resized frame after acknowledging
the configure. The canonical Sway lane makes this the normal case, since
it resizes `CuaTestHarness` windows by title right after they map.

| Region      | Behavior |
| ----------- | -------- |
| `active`    | Mutates the counter and submits exactly one content update. |
| `inert`     | Receives the input, changes nothing, commits nothing. |
| `supersede` | Queues two commits before one flush; the first update is superseded. |

`inert` is what makes "this action was supposed to change nothing" checkable:
a delivered-but-inert input produces a typed `no_mutation` row and can never be
reported as a presented mutation. `supersede` exercises the compositor's
`discarded` path against a real compositor.

## Evidence

Two channels, deliberately separate:

- `--journal <path>` — JSONL. `startup`, `mapped`, `layout`, `input`, `state`,
  `sample`, and `shutdown` records. The `sample` records are the causal timing
  rows.
- `--state <path>` — application-owned state (counter, `last_update_id`, colour,
  last action), replaced atomically. The window title mirrors both the counter
  and update ID as `CuaTestHarness Presentation [n=<counter>] [u=<update_id>]`,
  which the Driver can read back through `list_windows`.

The runner asserts the mutation from the state channel and the title, never
from the timing rows, so a bug in the timing path cannot manufacture a passing
state assertion, and a bug in the state path cannot manufacture a presented
row.

## Clocks

Every fixture timestamp is `CLOCK_MONOTONIC`, reported as `clock_id: 1`. The
compositor's presentation clock is recorded as advertised by `wp_presentation`
and compared explicitly. If it is not the fixture clock, the row is
`clock_mismatch` and every presentation-relative delta is withheld rather than
computed across clock domains. `surface_commit_ns` is sampled beside the
`wl_surface.commit` request, before the socket flush. `feedback_received_ns`
is sampled locally when the callback runs; it is separate from the compositor's
`presented_ns`.

## Outcomes

| `fixture_outcome` | Meaning |
| ----------------- | ------- |
| `verified`        | State changed and this update's own feedback reported `presented` in a comparable clock. |
| `discarded`       | The compositor discarded or superseded this content update. |
| `timeout`         | No feedback arrived before the fixture gave up. |
| `clock_mismatch`  | `presented` arrived in a clock the fixture cannot compare. |
| `implausible`     | `presented` preceded its own commit. |
| `no_mutation`     | Input was delivered but changed no state and committed nothing. |

Only `verified` counts as a presented mutation.

The update ID is assigned before the counter changes. Feedback for an unknown
or already accounted ID is recorded as `unmatched_feedback` and cannot claim
another update's row. A refused Driver action is checked separately: it must
produce neither input nor a sample.

`discarded` is ordinary compositor behaviour: a superseded update was never
shown. Its row retains callback receipt and state change, but no presentation
time. The runner requires one such row from the `supersede` control.

## Building

`cargo` alone is enough: `wayland-client`'s default backend is the pure-Rust
protocol implementation, so no system Wayland library is needed to build. The
Wayland dependencies are declared for `cfg(target_os = "linux")` only, which
keeps the accounting, layout, and journal modules — and their unit tests —
building on every host.

## Usage

```bash
libs/cua-driver/tests/fixtures/apps/linux/wayland-presentation/build.sh

CuaTestHarness.WaylandPresentation \
  --journal /tmp/presentation.jsonl \
  --state /tmp/presentation-state.json \
  --deadline-ms 1000
```

Options: `--journal` (required), `--state`, `--title`, `--deadline-ms`,
`--width`, `--height`, `--exit-after`, `--probe`.

`--probe` answers whether this compositor can attribute a content update to a
presentation at all, so a runner can decide between measuring and recording a
limitation:

```bash
CuaTestHarness.WaylandPresentation --journal /tmp/probe.jsonl --probe
```

The probe commits one content update with feedback requested and waits for the
compositor to complete feedback for that probe's own update ID. Binding
`wp_presentation` is deliberately not the
answer, because advertising the global does not mean feedback ever arrives:

| Compositor                        | Advertises | Completes feedback | Clock |
| --------------------------------- | ---------- | ------------------ | ----- |
| sway 1.9 / wlroots 0.17, headless | yes        | yes                | `CLOCK_MONOTONIC` |
| sway 1.7 / wlroots 0.15, headless | yes        | **no**             | `CLOCK_MONOTONIC` |
| sway nested on a parent compositor | yes       | yes                | parent's clock |

A headless wlroots 0.15 output never reaches a real presentation, so every
action would time out and read as a slow Driver rather than as a lane that
cannot see presentation. The probe reports that as a limitation instead. The
hosted lane's sway 1.9 completes feedback in `CLOCK_MONOTONIC`, so its rows are
directly comparable with the fixture's own stamps. A different clock produces
a typed `clock_mismatch` lane limitation without an invalid subtraction.

Exit codes: `0` normal, `2` bad arguments or non-Linux host, `3` this
compositor cannot attribute a presentation — either `wp_presentation` is
unavailable or it completed no feedback (a typed environment limitation, not a
measurement), `1` any other failure.

The runner retains every raw row in `rows.jsonl` and outcome counts in
`summary.json`. One presented action is enough for this first evidence slice;
it reports no latency distribution statistics.

## Not in scope

Physical panel click-to-photon measurement, production telemetry, any change
to Driver action results, and cross-compositor performance claims from one
lane.
