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
record, so the runner clicks published centers instead of guessing:

| Region      | Behavior |
| ----------- | -------- |
| `active`    | Mutates the counter and submits exactly one content update. |
| `inert`     | Receives the input, changes nothing, commits nothing. |
| `supersede` | Mutates twice back to back, so the first update can be superseded. |

`inert` is what makes "this action was supposed to change nothing" checkable:
a delivered-but-inert input produces a typed `no_mutation` row and can never be
reported as a presented mutation. `supersede` exercises the compositor's
`discarded` path against a real compositor.

## Evidence

Two channels, deliberately separate:

- `--journal <path>` — JSONL. `startup`, `mapped`, `input`, `state`, `sample`,
  and `shutdown` records. The `sample` records are the causal timing rows.
- `--state <path>` — application-owned state only (counter, colour, last
  action), replaced atomically. The window title also mirrors the counter as
  `CuaTestHarness Presentation [n=<counter>]`, which the Driver can read back
  through `list_windows`.

The runner asserts the mutation from the state channel and the title, never
from the timing rows, so a bug in the timing path cannot manufacture a passing
state assertion, and a bug in the state path cannot manufacture a presented
row.

## Clocks

Every fixture timestamp is `CLOCK_MONOTONIC`, reported as `clock_id: 1`. The
compositor's presentation clock is recorded as advertised by `wp_presentation`
and compared explicitly. If it is not the fixture clock, the row is
`clock_mismatch` and every presentation-relative delta is withheld rather than
computed across clock domains.

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

`discarded` is ordinary compositor behaviour rather than a fault: when a later
commit supersedes an earlier one within the same refresh, the earlier update is
dropped and never shown. Such a row is retained as evidence but measures no
presentation, so the runner repeats the action rather than counting it. Driving
the fixture with five key events inside one frame reproduces this directly:
state reaches `n=5` and every update is accounted for, while only the updates
the compositor actually showed carry presentation deltas.

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
compositor to complete it. Binding `wp_presentation` is deliberately not the
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
directly comparable with the fixture's own stamps.

Exit codes: `0` normal, `2` bad arguments or non-Linux host, `3` this
compositor cannot attribute a presentation — either `wp_presentation` is
unavailable or it completed no feedback (a typed environment limitation, not a
measurement), `1` any other failure.

## Statistics

The fixture keeps raw observations and derives only the deltas its own
evidence supports. It introduces no percentile machinery: small fixture runs
report every sample, median, max, and deadline-miss counts. Extreme
percentiles belong to larger benchmark runs with enough observations to make
their tails meaningful.

## Not in scope

Physical panel click-to-photon measurement, production telemetry, any change
to Driver action results, and cross-compositor performance claims from one
lane.
