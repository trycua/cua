# Cua-Bench manifest schemas

This directory contains the machine-readable Cua-Bench manifest contract. The
current schema line uses JSON Schema Draft 2020-12.

## Version convention

Each published schema has an immutable semantic version in both its directory
and `$id`. For example, task schema version `0.1.0` lives at
`v0.1.0/task.schema.json` and has the canonical identifier
`https://schemas.cua.ai/cuabench/v0.1.0/task.schema.json`. Instances declare the
same value in `schema_version`. Schema versions are independent of runtime and
dataset versions.

A patch version may clarify annotations or tighten validation for instances
that were already invalid. A minor version may add optional fields. A major
version may change accepted instance structure or meaning. References use an
exact schema version; schemas never redirect a versioned `$id` to newer rules.

Version 0.1.0 defines:

- `task.schema.json` for task identity, lifecycle policy, evaluation, and reset;
- `dataset.schema.json` for ordered task selection and dataset freezes; and
- `capability.schema.json` as the capability vocabulary shared by task and
  driver manifests;
- `driver.schema.json` for a shipped driver interface and its optional skill;
- `profile.schema.json` for the five canonical experiment conditions;
- `trial.schema.json` for one scheduled execution unit and its resolved inputs,
  delivery facts, result fields, and evidence; and
- `release.schema.json` for a content-addressed dataset, apparatus, candidate,
  trial, report, and tool-surface closure.

Version 0.2.0 preserves those contracts and adds two optional fields:

- task `participation_requirements` declare ordered semantic interactions with
  benchmark application surfaces; and
- trial `execution.participation` records the observer's non-scoring gate
  status and receipt digest, with an optional pinned receipt artifact.

The 0.1.0 files remain immutable. A 0.2.0 manifest resolves every reference
within the 0.2.0 schema directory.

Version 0.3.0 starts from the immutable 0.2.0 contract and adds manifests for
the complete agent system and benchmark-owned execution policy. Trials and
releases bind those declarations to independently observed execution facts.
The 0.1.0 and 0.2.0 files remain immutable; every 0.3.0 reference resolves
within the 0.3.0 schema directory.

### Agent-system and execution-policy manifests

`system.schema.json` identifies the harness build and configuration, all
declared primary/subagent/fallback model routes, the driver candidate and
profile, the exact tool contract, skill presentation, and the full enabled
tool/skill inventory. `native_harness_capabilities_enabled` is required to be
true: a comparison records full harness capability instead of disabling coding
or orchestration tools to normalize surface area. Vendor defaults, maintainer
defaults, and tuned systems have distinct identities.

`execution-policy.schema.json` holds benchmark-owned limits and rules apart
from the system identity. It fixes attempts, infrastructure retries, time,
tokens, cost, network, permissions, autonomy, workspace and target isolation,
cache and package behavior, credential-state profile, price table, and cost
basis. Controlled comparisons hold this policy digest fixed.

A 0.3.0 trial binds both manifest digests and records observed model routes,
cache-aware token totals, interventions, approval prompts, parallelism,
workspace/reset/cache evidence, package installations, persistent state,
applied network and permission policy,
self-modification findings, application versions, display facts, and the
observed accounting basis. Schema and semantic fixtures reject undeclared
routes, digest mismatches, autonomy contradictions, missing fresh-workspace
evidence, and price-table mismatches. These conditions affect comparison
eligibility without erasing the independently graded task outcome.

The trial environment's `resolved_seed_provenance_sha256` records the
host-controlled seed provenance seal captured when seed generation was
finalized. It does not claim byte identity for the current seed disk or a Lume
clone. Certifying runtime receipts bind the same value as
`seed_provenance_digest`; fresh-clone conformance is separately enforced by
the required, benchmark-selected guest-facts pristine fingerprint.

## Shipped and resolved driver data

The driver manifest records the bundle as shipped. Its ordered tool definitions
produce `tool_schemas_digest` using SHA-256 over RFC 8785 canonical JSON. This
digest names the tool definitions, not the containing manifest file. An optional
`skill` records the source, format, version, artifact bytes, and generator
version when generated.

The trial manifest records what reached the agent. Native trials bind the
resolved tools to the driver manifest. Bare-driver trials state that the skill
was removed. Normalized-facade trials record both the removed vendor skill and
the exact neutral guidance delivered by the harness. Each delivered guidance
record includes its channel, injection point, byte count, token count, content
digest, source artifact, and exact delivered-content artifact. The two artifacts
may differ for an on-demand or transformed delivery. Validators use these
records instead of inferring delivery from the agent trajectory. Token usage
keeps driver-skill and neutral-guidance tokens separate.

## Profile and trial rules

Profile identifiers select one fixed harness, driver presentation, guidance
policy, and decision-bearing status. A profile with an inconsistent combination
fails schema validation.

An eligible trial has one execution, a termination status, outcomes,
observables, and evidence. An ineligible trial has eligibility evidence and
none of those execution fields. Candidate trials bind the candidate manifest
and resolved driver surface. The harness-native reference omits both.

In schema version 0.2.0, an execution may include `participation`. The receipt
is independent of graded `outcomes`: a trial can reach the correct final state
and still fail the participation gate. The observer receipt points to
runtime-owned event hashes and may identify a verifier key; agent-authored
evidence cannot replace it.

`support_status` stays separate from eligibility. An unsupported eligible trial
lists the candidate capabilities it lacks and still records execution and
graded outcomes. A post-freeze trial binds the dataset freeze and benchmark
apparatus digests. A pre-freeze trial omits the dataset freeze digest and cannot
claim either finalized binding.

## Release closure

A decision-bearing release contains the four required profiles for every
candidate and pairing key. The validator loads each referenced manifest,
checks its file digest and identity, confirms paired coordinates remain fixed,
and rejects pre-freeze trials. It also checks the separate consolidated,
factored, and fine-grained tool-surface study. Optional driver-only diagnostic
trials may appear in the release, but they do not satisfy or alter the required
four-profile comparison.

The release digest is SHA-256 over canonical JSON for the complete `inputs`
object. That object binds the runtime source revision and artifact; dataset;
categorized apparatus provenance, harness builds, environment images,
observers, and evaluators; profiles; candidates; trials; report inputs; and each
tool-surface arm's contract, trial inventory, and result. The tool-surface study
also records its backend and the capability list that forms its digest preimage.
The validator checks every referenced file and cross-manifest closure before
computing the digest.

## Validator coverage

The repository validator checks schema definitions, valid examples, declared negative cases,
complete negative coverage of every schema's `required` groups, task and driver
identifiers, referenced artifact bytes, dataset and release digests, and
cross-manifest bindings. Semantic mutations cover tool digests, duplicate tool
and profile identifiers, candidate bindings, paired profile coverage, time
ordering, apparatus closure, v0.3 final-certification bindings, and release
digests. A report accepts `certified: true` only from the final apparatus
certification, not from participation alone. Digest mutations prove that
changing a dataset or release input changes its aggregate digest. RFC 3339 and
URI format checks are enabled by pinned validator dependencies.

Schema references resolve through the validator's in-repository registry. The
validator does not fetch versioned schemas from the network.

See the [task-pack interface](../../../../../benchmarks/cua-driver-bench/docs/reference/task-pack-interface.md)
for the executable validation boundary and the
[manifest reference](../../../../../benchmarks/cua-driver-bench/docs/reference/manifests.md)
for the cross-version overview.

## Dataset freeze digest

The freeze digest is SHA-256 over canonical UTF-8 JSON for the `freeze.inputs`
object. Object keys are sorted by Unicode code point, insignificant whitespace
is removed, and array order is retained. Strings use JSON escaping and
non-ASCII characters remain UTF-8. Digest input structures contain no JSON
numbers, which avoids
cross-language number serialization differences. Every input entry records the
SHA-256 digest of the exact file bytes. This aggregate convention is distinct
from the RFC 8785 tool-definition digest. The validator checks file digests
before computing the aggregate digest. Release inputs use the same canonical
JSON rules.

The input categories are fixed: ordered task manifests, fixture artifacts,
evaluator artifacts, the normalized facade contract, and exact neutral
instructions. The task-manifest array order must match the dataset's `tasks`
array. Dataset paths resolve relative to the dataset manifest. Paths declared
inside a task resolve relative to that task manifest. The validator resolves
both forms to the same confined file before comparing freeze coverage.
