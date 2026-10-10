# How to export and report trial results

This guide shows you how to turn verified runtime directories into frozen v0.3
trial manifests and a deterministic report.

## Before you start

- Prepare one frozen trial template for each runtime directory.
- Prepare the referenced system and execution-policy manifests.

## Preregister the report block

Before executing any trial, validate and freeze the complete comparison plan:

```console
cdb preregister-report \
  --view system-track \
  --trial-template path/to/trial.template.cuabench.json \
  --system path/to/system.cuabench.json \
  --execution-policy path/to/execution-policy.cuabench.json \
  --signing-key /absolute/private/participation-signing-key \
  --verifier-key path/to/participation-verifier.pub \
  --out path/to/reports/system-track.preregistration.json
```

Repeat `--trial-template` and `--system` for every planned input. Preserve the
receipt with the run evidence. If the command rejects a fixed factor, correct
and refreeze the templates instead of changing their bindings after execution.
Use the same report parameters here that the final report will use.

Execute every trial named by the accepted preregistration before reporting.

## Verify each runtime directory

```console
cdb explain --verify-only /absolute/path/to/trials/trial-id
```

Do not export a directory that fails verification.

## Export the trial

```console
cdb export-trial \
  --trial-dir /absolute/path/to/trials/trial-id \
  --template path/to/trial.template.cuabench.json \
  --out path/to/results/trial.cuabench.json
```

The command stages referenced artifacts beside the manifest. Both destinations
must be absent before export.

## Build the report

```console
cdb report \
  --view system-track \
  --trial path/to/results/trial.cuabench.json \
  --system path/to/system.cuabench.json \
  --execution-policy path/to/execution-policy.cuabench.json \
  --preregistration path/to/reports/system-track.preregistration.json \
  --verifier-key path/to/participation-verifier.pub \
  --normalized-out path/to/reports/trials.jsonl \
  --out path/to/reports/system-track.json
```

Repeat `--trial` and `--system` for every input. Use the preregistered
`--bootstrap-samples`, `--seed`, `--pass-k`, and infrastructure-failure limit.
The command rejects a changed plan, parameter, system, policy, or trial cell,
and it verifies certifying apparatus signatures before aggregation.

## Check report eligibility

Inspect `decision_bearing` and `decision_bearing_violations`. A descriptive
report retains outcomes but cannot support the controlled claim named by its
view.

## Troubleshooting

**Export rejects a binding.** Compare the template's task, system, policy,
variant, and artifact hashes with the verified runtime directory.

**A controlled report rejects the inputs.** Confirm that every paired block
contains every arm and that fixed policy, task, environment, apparatus, price,
and pairing digests match.

**Preregistration says an attempt sequence is incomplete.** Provide exactly
the policy's `attempts_per_task` indices, starting at zero, for every task and
comparison arm.

**A system-track report says `apparatus_digest` has multiple values.** Check
that every trial binds the same benchmark-owned apparatus digest. Keep
system-specific guest-launch contracts in the system identity; do not rewrite
frozen trial bindings after execution.

**Token or cost counts are missing.** Preserve the missing values. Do not
convert unavailable accounting into zero.

## Related references

- [Manifest reference](../reference/manifests.md)
- [Comparison views](../reference/comparison-views.md)
- [Comparison design](../explanation/profiles-systems-and-comparisons.md)
