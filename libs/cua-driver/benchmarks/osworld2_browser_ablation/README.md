# Cua Driver × OSWorld 2 browser-use ablation

This directory prepares a reproducible comparison of four observation/action
policies on the official OSWorld 2 release:

| Mode | Model-visible observation | Model-visible actions |
| --- | --- | --- |
| `screenshot_only` | Native window screenshot | Native pixel-addressed actions |
| `screenshot_ax` | Native screenshot and accessibility tree | Native pixel and accessibility actions |
| `cdp_only` | CDP `semantic_v2` outline and refs | Typed browser actions |
| `combined` | Native screenshot, accessibility tree, and CDP semantics | Native and typed browser actions |

The native window is always selected before browser binding. The runner must
continue only with `binding_quality: "exact"` and exactly one tab whose
`active` field is `true`. Hidden binding metadata is used only to preserve this
safety contract; it is not leaked into ablation observations.

CDP screenshots are deliberately excluded from every mode. The pinned OSWorld
image timed out on `Page.captureScreenshot`, while native Cua Driver
screenshots and CDP semantic/action calls remained reliable. This benchmark
therefore tests the release thesis directly: OS-native pixels/accessibility
paired with browser-native semantics and actions.

## Reproducibility pins

- Benchmark release: `osworld-v2-2026.06.24`
- Reviewed downloader bootstrap:
  `xlang-ai/OSWorld-V2@83f19747da485a29c2e406daf9e732219fef33a7`
- OSWorld code: `xlang-ai/OSWorld-V2@v2026.06.24`
- Gated tasks: `xlangai/osworld_v2_tasks@v2026.06.24`
- Gated assets: `xlangai/osworld_v2_assets_gated@v2026.06.24`
- QEMU archive: `osworld-v2-ubuntu-x86.qcow2.zip`
- QEMU SHA-256:
  `eb737ae70b49849e24af407de6a518439a23de05a8497096a948334ce0a909aa`
- Fleet pool size: exactly one VM

`manifest.json` repeats the official release contract and pins both resolved
Git commits. The reviewed bootstrap commit contains both official gated
downloaders; after downloading, `harness.py finalize` switches to the exact
release commit. The harness refuses a moved tag, task-hash mismatch, or
mismatched release manifest.

## Prepare everything public

From this directory:

```bash
python3 harness.py prepare
```

This clones the public OSWorld repository, checks out the reviewed downloader
bootstrap commit in detached state, validates the exact release tag and
official release manifest, writes `.work/status.json`, and exits with status
`2` if credentials are the only remaining blocker. It never creates a Fleet
namespace or VM.

Local infrastructure values belong in ignored `.work/local.json`; copy
`local.example.json` and fill it from the existing secret/image inventory.
Never commit credentials, signed URLs, private tokens, or task/evaluator
source.

## Credential handoff

Official execution requires:

```bash
export HF_TOKEN="..."                       # accepted access to both gated repos
export CUA_BENCH_MODEL_PROVIDER="openai"    # or anthropic / litellm / gateway
export CUA_BENCH_MODEL="..."
export OPENAI_API_KEY="..."                 # provider-specific
```

For an OpenAI-compatible LiteLLM gateway, use
`CUA_BENCH_MODEL_PROVIDER=litellm`,
`CUA_BENCH_OPENAI_BASE_URL=https://<gateway>/v1`, and
`LITELLM_API_KEY`. Instead of exporting a key, ignored `.work/local.json` may
name an existing scoped AWS Secrets Manager secret. Pin one model route for
all ablation cells, record the wire API and streaming requirement, and set
`model_route_verified: true` only after a credential-safe smoke succeeds.
Record the resolved model from every episode response and fail or quarantine
an episode if a fallback silently changes the underlying model.

Fleet authentication can use either `CUA_CLIENT_ID` +
`CUA_CLIENT_SECRET`, or an existing scoped AWS secret named by
`CUA_FLEET_SECRET_NAME`/`.work/local.json`. Secrets must be injected at
runtime; the harness never writes them.

Verify the boundary without side effects:

```bash
python3 harness.py preflight
```

After Hugging Face access is available, use the official scripts from the
prepared checkout:

```bash
cd .work/OSWorld-V2
uv run scripts/tools/download_osworld_v2_tasks.py \
  --benchmark-release osworld-v2-2026.06.24
uvx --from huggingface_hub hf download \
  xlangai/osworld_v2_tasks manifests/task_hashes.json \
  --repo-type dataset \
  --revision v2026.06.24 \
  --local-dir cache/osworld_v2_tasks_metadata
uv run scripts/tools/download_osworld_v2_assets.py \
  --benchmark-release osworld-v2-2026.06.24 \
  --target-dir cache/osworld_v2_assets
cd ../..
python3 harness.py finalize
cd .work/OSWorld-V2
uv sync --frozen
export OSWORLD_FILE_BASE_URL="$(pwd)/cache/osworld_v2_assets"
```

The `finalize` command first requires all 108 pinned task files, the gated task
hash manifest with its published SHA-256, and a non-empty asset cache. It then
checks out the exact OSWorld evaluation release commit.

Do not expose downloaded task classes, setup code, evaluator logic, or assets
to the evaluated model.

## Pilot matrix

Create a sealed text file containing exactly 20 task IDs, one per line. Choose
browser-heavy and OS→browser tasks only after the gated bundle is available;
do not guess from public task numbers.

```bash
python3 harness.py matrix \
  --tasks .work/pilot_tasks.txt \
  --output results/pilot-matrix.json
```

The fixed pilot is 20 tasks × 4 modes × 3 seeds = 240 episodes. Keep the model,
prompt, sampling parameters, step budget, image, task release, and evaluator
identical across modes. Report task success, steps, wall time, model tokens,
tool refusals, retries, and categorized failures.

## Fleet and browser integration contract

- Provision `replicas: 1`; confirm one available replica and one sandbox owner.
- Use the immutable private `containerDisk` digest derived from the pinned QEMU
  archive.
- Use the image-baked, release-pinned Cua Driver archive. Verify its version,
  archive SHA-256, and `/etc/cua-driver-osworld2-build.json` against
  `manifest.json` before starting the daemon.
- Invoke tools through the release-matched Python SDK. Until a Linux wheel is
  published, assemble it only from the full release archive and the exact
  release-tag Python sources, verify every SHA-256 in `manifest.json`, smoke
  the SDK against the pinned daemon, and retain that attestation in run
  provenance.
- The `0624` image exposes OSWorld control on `5000`, Chrome CDP on `1337`,
  noVNC on `8006`, and VLC on `8080`.
- Start Cua Driver as the desktop user with the guest display/session bus and
  `--dangerously-bypass-approvals`.
- Use `start_session` once. Take a fresh native snapshot before every native
  action and another after it.
- Let `browser_prepare` discover and attest the browser-owned CDP listener;
  never assume Fleet port `9222`.
- Rebind the exact native `(pid, window_id)` after preparation. Select only a
  uniquely proven active tab.
- Refresh `semantic_v2` refs after every browser mutation.
- Use explicit `input_route: "dom_event"` for Linux browser clicks when that
  synthetic semantic is acceptable; never silently downgrade a trusted click.
- Verify browser effects with a fresh semantic snapshot. When an effect also
  changes visible UI, verify the same exact native window again.
- Always call `end_session`, delete the Fleet namespace, and verify absence in
  a `finally` path.

The deterministic one-VM smoke established these ports and contracts, but it
is not an official benchmark score. Only runs using the gated, release-matched
tasks and assets may be reported as OSWorld 2 results.

## Canonical paired run

`run_paired_gpt55.py` is the repeatable single-task integration pilot. It
creates one Fleet VM through `fleet_pilot.py`, runs official asset-free
Task070 twice with fresh task tenants and Chrome profiles, and compares:

- control: native screenshot + accessibility tree;
- treatment: the identical native inputs plus CDP `semantic_v2` observations
  and typed browser actions.

The runner takes the new immutable image digest separately so the ignored
Fleet config does not need to be edited or copied:

```bash
CUA_OSWORLD2_WORK_DIR=/path/to/prepared/.work \
/path/to/prepared/.work/OSWorld-V2/.venv/bin/python run_paired_gpt55.py \
  --config /path/to/prepared/.work/local.json \
  --container-disk-image \
    '<account>.dkr.ecr.<region>.amazonaws.com/osworld-v2-ubuntu-x86@sha256:<digest>' \
  --env-file /path/to/private/.env.local \
  --model gpt-5.5 \
  --reasoning-effort xhigh \
  --max-steps 24 \
  --order control-first
```

The run refuses unpinned OSWorld code/tasks, a mutable image tag, mismatched
Driver metadata, degraded native accessibility, inexact browser binding,
model-snapshot drift, evaluator errors, task-reset drift, or unverified Fleet
cleanup. Raw screenshots, observations, model responses, action outcomes,
official scores, latency, usage, estimated cost, and provenance remain in the
ignored `.work/results` directory.

This one Task070 pair is integration evidence, not a population-level OSWorld
V2 estimate. The 240-episode matrix above remains the publishable follow-up.
