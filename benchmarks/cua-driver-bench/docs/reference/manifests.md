# Manifest reference

Cua-Bench manifests are versioned JSON documents validated by schemas under
`schemas/v<version>/`.

## Manifest kinds

| Kind | Canonical suffix | Purpose |
| --- | --- | --- |
| Task | `task.cuabench.json` | Lifecycle, fixture, evaluation, reset, side effects, participation |
| Dataset | `dataset.cuabench.json` | Ordered task selection and freeze inputs |
| Driver | `driver.cuabench.json` | Candidate tools, capabilities, skill, and source identity |
| Profile | `profile.cuabench.json` | Harness and driver presentation condition |
| System | `system.cuabench.json` | Frozen harness, model routes, driver, tools, skills, and configuration |
| Execution policy | `execution-policy.cuabench.json` | Benchmark-owned limits, isolation, network, credentials, and accounting rules |
| Trial | `trial.cuabench.json` | One registered execution and its resolved evidence |
| Release | `release.cuabench.json` | Content-addressed closure over dataset, apparatus, candidates, trials, and reports |

## Schema lines

| Version | Additions |
| --- | --- |
| `0.1.0` | Task, dataset, driver, profile, trial, release, and capability contracts |
| `0.2.0` | Ordered driver-participation requirements and receipts |
| `0.3.0` | Complete agent systems, execution policies, observed routing, comparison eligibility, and report bindings |

Every schema version is immutable. References resolve to an exact version in
the repository and never redirect to newer rules.

## Identity and digests

Manifest identity is separate from file identity. Artifact records bind exact
bytes with SHA-256. Driver tool schemas use RFC 8785 canonical JSON. Dataset
freeze and release input digests use the repository's canonical UTF-8 JSON
rules.

The seed provenance digest authenticates the host-controlled image
finalization seal. Fresh-clone conformance uses a separately measured pristine
guest-facts fingerprint.

## Trial decisions

The trial schema keeps outcome, participation, certification, and comparison
eligibility separate. A task can pass while another decision fails.

## Examples and negative fixtures

`schemas/examples/` contains valid manifests and freeze inputs.
`schemas/fixtures/` contains structural and semantic negative cases, digest
fixtures, and version-transition cases.

## Authoritative detail

The full field and closure rules remain in the runtime's
[schema reference](../../../../libs/cua-bench-runtime/src/cua_bench_runtime/schemas_data/README.md),
the versioned JSON Schemas, and
[capability vocabulary](../../../../libs/cua-bench-runtime/src/cua_bench_runtime/schemas_data/capabilities.md).
