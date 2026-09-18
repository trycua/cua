# Cua-S1

Cua-S1 is a research project for studying small, specialist computer-use
models. The project is intentionally scoped around models that perform a
defined class of interface tasks rather than a generally capable computer-use
agent.

The first checkpoint in the project family is `cua-s1-form-v0`, a specialist
checkpoint for research on form-oriented user-interface tasks. It should not be
treated as a general-purpose assistant or as evidence of reliable performance
outside its evaluated task and environment boundaries.

## Project status

Cua-S1 is at an early research stage. This component includes Python model,
synthetic-data, training, evaluation, and optional Cua Driver integration code.
It does not include or download model weights, datasets, demo binaries, or
recordings. No checkpoint performance claim is established by this source-only
release.

Before evaluating or using a checkpoint, read [`MODEL_CARD.md`](MODEL_CARD.md)
for its intended scope and limitations and [`SECURITY.md`](SECURITY.md) for
deployment guidance.

The source code in this component is available under the repository's MIT
license. That license does not apply to future official model weights,
datasets, hosted services, or Cua trademarks. A future checkpoint may permit
research and evaluation while requiring a separate agreement for commercial
production use; its release must state those artifact-specific terms clearly.

## Python package

The Python distribution is named `cua-s1`, and its import name is `cua_s1`.
This source-only change does not publish the distribution to a package index.

Install the standalone development environment from this component:

```bash
uv sync --project libs/cua-s1/python --extra pdf --group test
uv run --project libs/cua-s1/python pytest libs/cua-s1/python/tests
```

The package exposes research primitives without downloading a model:

```python
import cua_s1

print(cua_s1.__version__)
```

Loading a checkpoint requires a local `safetensors` file and matching JSON
configuration. Pickle-based PyTorch checkpoints are rejected.

## Safety boundary

Planning and execution are separate. The optional runtime defaults to a dry
run, requires one unambiguous target window, uses snapshot-bound element
tokens, and reobserves the window after each mutation. `execute` and `submit`
are independent opt-ins. PDF access is confined to configured allowed roots.
Without explicit configuration, the library and MCP server use their current
working directory as the allowed root. Production deployments should use a
dedicated, least-privilege directory.

Submission is deliberately narrow: `submit=true` permits at most one
high-confidence `Button` or `AXButton` whose normalized label is exactly
`Submit` or `Submit Form`. Other click decisions are omitted. Inspect the
dry-run plan before enabling both execution flags.

## Optional MCP server

The `cua-s1-mcp` command is an advanced integration surface, not a configured
model service. Install the optional dependencies before running it:

```bash
uv sync --project libs/cua-s1/python --extra mcp --extra pdf
```

The server uses the MCP stdio transport and requires these host settings:

- `CUA_S1_PLANNER_FACTORY` identifies trusted Python code in
  `module:attribute` form. Importing the factory executes code with the server
  process's privileges, so do not point it at untrusted modules.
- `CUA_S1_ALLOWED_PDF_ROOTS` is an operating-system path-separated list of
  directories that the server may read. If it is unset, the server uses its
  current working directory.
- `CUA_S1_DRIVER_BINARY`, `CUA_S1_DRIVER_TRANSPORT`, and `CUA_S1_SESSION` can
  override the Cua Driver executable, transport, and session.

The connected Cua Driver must provide exact-window snapshots, snapshot-bound
element tokens, and confirmed action effects. The portable Cua Driver contract
does not currently expose `set_value`, so fill execution fails closed unless
the connected runtime explicitly advertises compatible token-based value
mutation. Planning remains available without executing mutations.

Treat MCP tool results and stdio logs as sensitive. They can contain values
extracted from PDFs, form labels, window metadata, and values selected for
entry.

## Checkpoints

| Checkpoint | Scope | Status |
| --- | --- | --- |
| `cua-s1-form-v0` | Form-oriented computer-use research | Profile defined; weights not distributed |

Checkpoint-specific release materials should document the exact artifact,
runtime requirements, evaluation setup, results, and applicable terms. Do not
assume that results transfer across applications, operating systems, languages,
layouts, accessibility settings, or task distributions.

## Evaluation

The included offline metrics distinguish accuracy, abstention, coverage, wrong
actions, wrong targets, and actions taken when the expected behavior was to
abstain. Synthetic train, validation, and test splits are separated by form
signature. A future checkpoint release must add an untouched holdout, artifact
hashes, exact environment details, and independently reproducible results.

## Responsible use

Run computer-use models in isolated environments with least-privilege
credentials, explicit action boundaries, and independent verification of
important outcomes. Require human review before consequential, irreversible,
financial, legal, medical, account, permission, or external-communication
actions.

Report suspected vulnerabilities through the process in
[`SECURITY.md`](SECURITY.md).
