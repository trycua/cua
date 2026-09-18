# ADR 0001: Public monorepo boundary

## Status

Accepted for the initial import.

## Decision

Import Cua Driver Bench into `trycua/cua` as an allowlisted snapshot without
private Git history. Keep the reusable runtime in `libs/cua-bench-runtime/` and
the public benchmark definition in `benchmarks/cua-driver-bench/`.

Rename the runtime import package to `cua_bench_runtime` and its executable to
`cdb`. The existing `libs/cua-bench` project retains the `cua_bench` import and
`cb` executable.

Do not publish held-out tasks, fixtures, evaluators, expected state, datasets,
results, releases, or evidence. Callers provide an authorized task root
explicitly.

## Consequences

- the public repository can test schemas and lifecycle behavior with synthetic
  conformance fixtures;
- local driver comparisons remain possible for authorized task-pack holders;
- a wheel contains the schemas needed by `cdb validate`;
- publishing or relicensing the held-out task pack requires a separate decision;
- source revisions and contributor credit remain visible through
  `PROVENANCE.md` and commit trailers without exposing private history.
