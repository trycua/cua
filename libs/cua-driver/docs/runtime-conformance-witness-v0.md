# Runtime conformance witness v0

The generated cua-driver contract and live `tools/list` surface answer what a
build **advertises**. `health_report` answers whether the surrounding runtime
is broadly usable. Neither artifact proves that one concrete build/platform
actually completed a representative tool call without hanging, crashing, or
returning an invalid success shape.

`RuntimeWitnessV0` fills only that gap.

It is intentionally small and metadata-only:

- binds the witness to driver, contract, tools-list, and capability versions;
- records the platform and runtime host;
- distinguishes whether `tools/list` and `health_report` were actually
  observed;
- records one deterministic row per exercised read-only tool;
- distinguishes pass, fail, skip, and timeout;
- records whether successful structured content passed the existing shared
  output validator;
- records elapsed time and an optional bounded diagnostic code.

It does **not** store raw tool payloads, screenshots, application names,
window titles, accessibility text, clipboard contents, or other desktop data.

## Why not another capability registry?

The capability vocabulary already has an owner. The live runtime already owns
authorization, platform-specific tool availability, and `health_report`.

A witness must consume those surfaces, not fork them.

The intended relationship is:

```text
generated/static contract
        ↓
live tools/list advertisement
        ↓
health_report
        ↓
read-only smoke calls
        ↓
RuntimeWitnessV0
```

The witness is therefore evidence about one observed runtime, not a second
source of truth.

## v0 safety boundary

v0 accepts read-only probes only. Acting probes such as click, hotkey, typing,
dragging, window movement, or clipboard writes require a future explicit
opt-in design. They must not silently appear in a metadata conformance run.

This boundary also keeps CI and downstream fleet qualification safe: a default
conformance job may inspect capabilities, but it cannot interact with a user's
desktop.

## What this can catch

A runtime witness is useful for regressions where static schema parity remains
green but the platform implementation is not actually usable, for example:

- a read-only platform call hangs until a timeout;
- a platform API exists but is unavailable on one OS version;
- a success payload no longer satisfies its advertised output schema;
- the driver reports degraded health but a downstream harness accidentally
  treats the environment as fully qualified.

It is not a substitute for end-to-end action tests or task benchmarks.

## Future runner

This PR deliberately defines the portable contract before choosing a runner.
A later runner can call `tools/list`, `health_report`, then a small
platform-safe read-only probe set and emit `RuntimeWitnessV0`.

The runner should reuse the existing runtime output validators and must never
copy raw desktop results into the witness artifact.
