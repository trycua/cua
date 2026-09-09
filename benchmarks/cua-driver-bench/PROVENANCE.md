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

The public tree contains only an empty task-pack interface and synthetic
conformance fixtures. The held-out task pack remains in its authorized private
location.

## Mechanical adaptations

The monorepo import will rename the runtime import package from `cb` to
`cua_bench_runtime` and its executable from `cb` to `cdb`. This avoids a
collision with the existing `cua-bench` package and `cb` executable in this
repository. Relative paths and documentation links are adjusted for the
monorepo layout.

