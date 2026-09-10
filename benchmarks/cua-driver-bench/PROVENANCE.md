# Import provenance

## Source

- Source repository: `trycua/cua-driver-bench`
- Snapshot revision: `a820570ae02844d3615e784dfc366df16183701c`
- Base revision before Jainish Patel's contribution:
  `15186386cc94d0920458ee6c1aef775875c8c7a2`
- Imported authors: Francesco Bonacci and Jainish Patel
- Jainish Patel's source contribution: `trycua/cua-driver-bench#58`

The import is a new, allowlisted snapshot. It does not merge, filter, or expose
the source repository's Git history.

## License review

Only reusable material classified as MIT-licensed by the source repository's
`LICENSING.md` and `REUSE.toml` is eligible for this import. Imported files are
also covered by this repository's MIT license.

## Exclusions

The snapshot excludes the source repository's proprietary and confidential
benchmark material, including:

- executable benchmark tasks and private task manifests;
- task fixtures, hidden evaluators, expected-state definitions, and oracles;
- datasets, release payloads, results, traces, screenshots, and evidence;
- validation journals or reports that disclose protected benchmark evidence.

The public tree contains only the task-pack interface and synthetic conformance
fixtures. Benchmark task IDs can appear as opaque runner selectors, but no
target application, target record, success condition, or expected end state is
embedded in the public runtime or its tests. The held-out task pack remains in
its authorized location.

Protected macOS execution retains the generic helper and evidence apparatus.
The authorized task pack or prepared seed must supply the task-specific
mediator and application. The public import does not contain an implementation
that translates a held-out task into target-specific driver actions or
readbacks.

## Mechanical adaptations

The monorepo import renames the runtime import package from `cb` to
`cua_bench_runtime` and its executable from `cb` to `cdb`. This avoids a
collision with the existing `cua-bench` package and `cb` executable in this
repository. Relative paths and documentation links are adjusted for the
monorepo layout. The automated release-comparison runner requires an explicit
`--tasks-root` and uses the runtime's public `McpClient` and `merge_app_maps`
interfaces.

Runtime tests use only synthetic task, application, store, and participation
contracts. Tests that exercise a held-out task require an explicit authorized
task root and remain with that task pack.
