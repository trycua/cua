# `cdb` command reference

`cdb` is the command-line interface for validation, trial execution, evidence
verification, trial export, and report generation.

## Commands

| Command | Description |
| --- | --- |
| `cdb validate` | Validate manifests and repository schema examples |
| `cdb run` | Execute one runtime trial |
| `cdb explain` | Verify and explain one runtime trial directory |
| `cdb export-trial` | Export a verified runtime trial as a v0.3 manifest |
| `cdb preregister-report` | Validate and freeze a comparison plan before execution |
| `cdb report` | Build a deterministic report from v0.3 trials |

## `cdb validate`

```text
cdb validate [--kind KIND] [--json] [paths ...]
```

`KIND` is one of `task`, `dataset`, `driver`, `profile`, `system`,
`execution-policy`, `trial`, or `release`. Without paths, the command validates
the repository's registered schemas and examples.

## `cdb run`

```text
cdb run --task TASK --agent AGENT --out OUT [options]
```

### Required arguments

| Option | Meaning |
| --- | --- |
| `--task` | Task manifest path |
| `--agent` | Agent executable or launcher path |
| `--out` | Trial output root |

### Trial options

| Option | Meaning |
| --- | --- |
| `--trial-id` | Trial directory identifier |
| `--seed` | Registered attempt seed |
| `--timeout` | Agent wall-time limit |
| `--env` | Environment adapter |
| `--json` | Machine-readable command output |

Environment values are `local`, `local-fail-cleanup`, `local-cua-smoke`,
`task-local-smoke`, `task-local-cua-smoke`, `lume-macos`, and
`lume-macos-certifying`.

### System and policy options

| Option | Meaning |
| --- | --- |
| `--system` | Frozen v0.3 system manifest |
| `--execution-policy` | Frozen v0.3 execution-policy manifest |
| `--credential-file` | One-shot credential lease source for an authenticated route |
| `--agent-brief` | Explicit production input override for an apparatus check |
| `--apparatus-check` | Marks a deterministic apparatus probe and excludes it from reports |
| `--debug` | Retains bounded content-free progress and makes the protected trial non-certifying and non-exportable |

### Protected macOS options

| Option | Meaning |
| --- | --- |
| `--lume-config` | Host-private Lume configuration |
| `--guest-launch` | Frozen guest-launch contract |
| `--participation-signing-key` | Host-private receipt signing key |
| `--participation-verifier-key` | Pinned public verifier key |

### Evaluator runtime options

| Option | Meaning |
| --- | --- |
| `--evaluator-node` | Absolute evaluator-owned Node executable |
| `--evaluator-node-sha256` | Expected lowercase SHA-256 of that executable |
| `--evaluator-node-version` | Expected version string |

## `cdb explain`

```text
cdb explain [--json] [--verify-only] TRIAL_DIR
```

`--verify-only` checks integrity without printing the narrative explanation.

## `cdb export-trial`

```text
cdb export-trial --trial-dir TRIAL_DIR --template TEMPLATE --out OUT
```

The destination manifest and its adjacent artifact directory must not exist.
Debug-mode and apparatus-check trials are not exportable.

## `cdb preregister-report`

```text
cdb preregister-report --view VIEW --trial-template TEMPLATE \
  --system SYSTEM --execution-policy POLICY --signing-key PRIVATE_KEY \
  --verifier-key PUBLIC_KEY [--bootstrap-samples N] [--seed N] \
  [--pass-k N] [--max-infrastructure-failure-rate RATE] --out OUT
```

`VIEW` accepts the same values as `cdb report`. Repeat `--trial-template` and
`--system` to describe the complete planned comparison block.

The command validates the frozen declarations before execution. It requires a
complete attempt sequence for every task and arm, rejects fixed-factor
confounds such as per-system apparatus digests, and writes a deterministic
receipt containing the input hashes, report parameters, and plan digest. The
receipt is signed under the report-preregistration SSHSIG namespace and
immediately verified with the pinned public key. `OUT` must not already exist.
`system-track` may preregister one arm only for a descriptive non-ranking run;
controlled comparison views still require at least two arms.
The four optional report parameters default to the same values as `cdb report`
and are signed into the plan; pass the same values when generating the report.

## `cdb report`

```text
cdb report --view VIEW --trial TRIAL --system SYSTEM \
  --execution-policy POLICY --preregistration PREREGISTRATION \
  --verifier-key PUBLIC_KEY --out OUT [options]
```

`VIEW` is `system-track`, `harness-comparison`, `model-comparison`,
`driver-profile`, or `tool-surface`. `--trial` and `--system` may be repeated.
An eligible one-arm `system-track` remains descriptive and non-ranking; it
cannot become decision-bearing without a second preregistered arm.

Optional controls are `--normalized-out`, `--bootstrap-samples`, `--seed`,
`--pass-k`, and `--max-infrastructure-failure-rate`.

The report verifies the signed preregistration, requires the exact frozen
trials, systems, policies, and parameters, and cryptographically verifies every
certifying apparatus receipt with the same pinned public key. Missing
comparison-eligibility evidence fails closed as ineligible.

## Exit behavior

A completed lifecycle returns zero when the evaluator grades a task failure.
Infrastructure faults, validation errors, timeouts, interruption, and cleanup
failures use separate nonzero exit codes.

## Related references

- [Trial artifacts](trial-artifacts.md)
- [Protected debug mode](protected-debug-mode.md)
- [Comparison views](comparison-views.md)
