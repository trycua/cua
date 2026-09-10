# How to diagnose a protected trial timeout

This guide shows maintainers how to retain bounded, content-free progress from
a protected production trial without making the run certifying or exportable.

## When to use this guide

Use debug mode when a production harness reaches the provider and drives the
protected application but does not reach a terminal task result. Run the normal
apparatus gates first. Debug mode is not a replacement for certifying evidence.

## Before you start

- Prepare the same frozen task, production system, execution policy, Lume
  configuration, guest launch, and participation keys used by the protected
  cell.
- Keep the registered production timeout and all isolation gates unchanged.
- Use the `lume-macos-certifying` environment. Debug mode is unavailable for
  local runs, `lume-macos` apparatus runs, and explicit apparatus checks.

## Run the protected cell in debug mode

Add `--debug` to the normal protected production command:

```console
cdb run \
  --task path/to/task.cuabench.json \
  --agent path/to/guest-agent-launcher \
  --system path/to/system.cuabench.json \
  --execution-policy path/to/execution-policy.cuabench.json \
  --out /absolute/path/to/trials \
  --trial-id diagnostic-trial-id \
  --env lume-macos-certifying \
  --lume-config /absolute/private/lume.json \
  --guest-launch /absolute/path/to/frozen-guest-launch.json \
  --participation-signing-key /absolute/private/signing-key \
  --participation-verifier-key /absolute/path/to/verifier.pub \
  --debug
```

Supply the normal one-shot credential file when the frozen route requires one.
Do not change `--timeout` merely to obtain diagnostics.

## Verify the trial

```console
cdb explain --verify-only /absolute/path/to/trials/diagnostic-trial-id
cdb explain /absolute/path/to/trials/diagnostic-trial-id
```

The explanation reports `debug mode: yes`, `certifying: no`, and an ineligible
comparison policy. Cleanup, participation, network sealing, stopped-disk
collection, and pristine-state checks still run normally.

## Inspect the content-free progress summary

```console
jq '{termination, token_totals, provider_activity, events: .events[-20:]}' \
  /absolute/path/to/trials/diagnostic-trial-id/artifacts/agent.debug.json
```

Use the event sequence to distinguish a provider stall, repeated tool
activity, a missing terminal event, or a harness-reported failure. The artifact
contains no raw prompts, responses, reasoning, tool arguments, stdout, stderr,
screenshots, or credentials.

Do not run `cdb export-trial` on this trial. The runtime rejects debug trials at
the export boundary.

## Troubleshooting

**`--debug` is rejected.** Confirm that the command uses a frozen production
system with `--env lume-macos-certifying` and does not use `--apparatus-check`.

**The artifact reports `debug_capture_unavailable`.** Preserve the verified
trial. The original timeout remains authoritative; the diagnostic read failed
without replacing the lifecycle result.

**Provider activity is null.** Check the protected result for a provider-proxy
seal failure. Debug mode does not bypass or synthesize provider evidence.

## See also

- [Protected debug mode reference](../reference/protected-debug-mode.md)
- [`cdb` command reference](../reference/cli.md)
- [Trial directory reference](../reference/trial-artifacts.md)
