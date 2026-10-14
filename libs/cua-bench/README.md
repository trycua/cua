# cua-bench

Run computer-use benchmark datasets on local and cloud sandboxes.

**[Documentation](https://cua.ai/docs/cuabench)** - Installation, guides, and API reference.

## Registry tasksets

Browse the tasksets at [cua.ai/cuabench/registry](https://cua.ai/cuabench/registry) and run
one by name on a desktop sandbox:

```bash
cb dataset list                                       # tasksets and versions
cb run cua-bench-basic --task-filter click-button     # oracle check, local desktop
cb run cua-bench-basic --agent cua-agent --model anthropic/claude-sonnet-4-20250514 -j 4
```

A name (or `name@version`) resolves through the bundled index to a pinned commit, fetched
once into `~/.cua/cbregistry`. `CUA_BENCH_REGISTRY` points at another index.

## Running tasks

One command runs a task or a dataset on this machine or in the cloud. There is no pool or
provider to set up.

```bash
cb run example_tasks/hello_file_env                   # local gVisor container (default)
cb run datasets/cua-bench-basic -j 8 --on cloud       # 8 at a time on cua cloud
cb run my_task --agent cua-agent --model anthropic/claude-sonnet-4-20250514
cb run my_task --dry-run                              # what would run, nothing starts
```

`cb run <path>` runs one task (a directory with `main.py`, variant `--variant-id`) or a
dataset (every task and variant). It runs in the foreground with per-variant status; add
`--detach` to return at once and follow with `cb run watch <id>`.

| Flag | Meaning |
| --- | --- |
| `--on local\|cloud` | Where. `local`: sandboxes on this machine through the cua SDK. `cloud`: managed Fleet pools. Default: `CUA_DEFAULT_ON`, then `cua config set default.on`, then `local`. |
| `--kind auto\|container\|vm` | What kind. `auto` (default): Linux runs as a container unless the task or the image's variant index says VM; Windows, macOS and Android are VM-only. Default: `CUA_DEFAULT_KIND`, then `default.kind`. |
| `--runtime auto\|gvisor\|runc\|qemu\|lume\|kubevirt` | Which engine, and it implies the kind. Local containers: `gvisor`, `runc`; local VMs: `qemu`, `lume`; cloud: `gvisor` (containers), `kubevirt` (VMs). `auto` (default) lets the SDK pick. A combination that does not exist is an error listing the valid values. Default: `CUA_DEFAULT_RUNTIME`, then `default.runtime` (skipped for a task it does not fit). |
| `--image REF` | Image for every task: an OS alias (`linux`, `windows`, `macos:tahoe`), `ghcr.io/trycua/<os>`, any registry ref or a digest, or `pool:<name>` to claim from an existing Fleet pool (`fleet:<name>` is a deprecated alias). Otherwise the task's `setup_config.image`, then `CUA_BENCH_IMAGE`, then the canonical image for the OS. |
| `--cpu`, `--memory` | Resources per sandbox (`--memory 4G`), local and cloud. |
| `-j`, `--max-parallel` | Variants at once. In the cloud this is also the pool's max size. |
| `--warm` | Cloud: keep one replica ready when the pool is first created. |
| `--claim-ttl` | Cloud: how long a claim outlives a crashed run (default 15m, renewed while running). |
| `--attempts N` | Run every variant N times (fresh sandbox each); `summary.json` reports pass@k. |
| `--retries N` | Retry a variant that failed before evaluation (sandbox start, connection), with backoff. |
| `--dry-run` | Print each variant's image, kind, variant and backend, then exit. |

Deprecated 0.2.11 flags still parse, with a notice naming the new ones: `--platform
linux-docker` (`--kind container`), `linux-qemu` (`--kind vm --runtime qemu --image
linux`), `windows-qemu` (`--kind vm --runtime qemu --image windows`), `android-qemu`
(`--kind vm --image android`), all local; `--provider-type native` has no effect.

Results land in `~/.local/share/cua-bench/runs/<run id>/<task>_v<n>/` (`run.log`,
`result.json`, `task_<n>_trace/`, `trajectory.json`) with a `summary.json` per run.
`cb run list | info | watch | logs | stop` inspect runs; `cb dataset build <run dir>`
exports traces (aguvis-stage-1, gui-r1).

- `result.json` records what ran (`on`, `backend` such as `local-gvisor` or
  `cloud-kubevirt`, the `sandbox` ref such as `local:<name>` or `cloud:<name>`,
  `kind`, `runtime` (the engine, when one was chosen), `image_ref`,
  `image_variant`, `image_digest` as the pinned `repo@sha256`, `arch`) and carries
  Harbor's trial fields (`task_name`, `trial_name`, `verifier_result.rewards`,
  `exception_info`, `agent_info`, per-phase timing).
- `trajectory.json` is an ATIF-v1.8 trajectory (Harbor's format) with screenshots under
  `imgs/`.
- `summary.json` adds `pass_at_k` (with `--attempts`), `stats` and a `targets` breakdown.
  New keys are additive; `schema_version` is 1.

### Datasets

`cb run <name>[@version]` runs a dataset from the registry. Each version is pinned to a
git commit and fetched once (shallow and sparse) into `~/.cua/cbregistry`; a plain name
means the latest version. `cb dataset list` shows them. `CUA_BENCH_REGISTRY` points at
another index (a path or URL), including a Harbor `registry.json`.

### Images

Tasks run on the canonical `ghcr.io/trycua/{linux,windows,macos}` images unless they name
another registry image. Each canonical image is one index with a rootfs (containers) and
a `-disk` containerDisk (VMs); `cb image list` shows them.

### Cloud

Sign in once with `cua auth login` (or `cb login`), or set `CUA_CLIENT_ID` and
`CUA_CLIENT_SECRET` (`cua auth keys create`), or `FLEETS_TOKEN`. The task image must be in a
registry Fleet can pull.

- Each image gets one managed pool (`cua-auto-*`), created on first use and reused by later
  runs. It autoscales from zero: the first sandbox of an idle pool takes about a minute.
- A batch claims one sandbox per running variant, up to `-j`, and releases each claim when
  the variant ends, on Ctrl-C or on `cb run stop`. If the process dies, the claim expires
  after its TTL.
- Idle pools scale to zero and are deleted after a day without use. `cb env ls` lists them,
  `cb env gc --idle 30m` deletes idle ones now.

### Tasks

A task declares its environment in `computer`:

```python
cb.Task(
    description="...",
    computer={
        "provider": "native",  # a real sandbox (the only provider since 0.3)
        "setup_config": {
            "os_type": "linux",  # linux | windows | macos (local only) | android (local only)
            "image": "ghcr.io/me/my-desktop:docker-1",  # optional
            "kind": "container",  # optional preference: container | vm
            "kinds": ["vm"],  # optional requirement (e.g. the task needs a kernel)
            "server_port": 8000,  # optional: the image's own daemon, used for readiness
        },
    },
)
```

Setup, the oracle or agent, and `evaluate` run in the `cb` process and drive the sandbox
through cua-spacesd (shell, files, screen, input), so tasks that use them need an image
with cua-spacesd. Agents are loaded in-process (`--agent` or `--agent-import-path`).
`cb task create <dir>` scaffolds a Linux container task.

### A coding-agent harness as the agent

`--agent harness` runs a coding-agent harness (Claude Code, Codex, Gemini CLI, OpenCode,
Goose, ...) inside the task's own sandbox through the cua SDK's agents API: "harness X in
image Y", with the image chosen as above. The harness gets the task description as its
prompt and the sandbox's own MCP tools (screen, input, windows); the task's `evaluate`
scores the sandbox afterwards.

```bash
CUA_BENCH_HARNESS=claude-code CUA_BENCH_HARNESS_KEYS=ANTHROPIC_API_KEY \
  cb run example_tasks/hello_file_env --agent harness
```

Settings are environment variables (or constructor kwargs): `CUA_BENCH_HARNESS`
(`cua agent harnesses` lists them), `CUA_BENCH_HARNESS_KEYS` (provider key variables
forwarded from this process), `CUA_BENCH_HARNESS_MODEL` (or `--model`),
`CUA_BENCH_HARNESS_BASE_URL` (a proxy or compatible endpoint),
`CUA_BENCH_HARNESS_MCP` (`NAME=URL,...`), `CUA_BENCH_HARNESS_TIMEOUT` (seconds, default
1800). The session must be a cua-spacesd sandbox; for a legacy computer-server session set
`CUA_BENCH_SPACESD_URL` and `CUA_BENCH_SPACESD_TOKEN`. Events and the result land in the
task's agent log directory (`harness-events.jsonl`, `harness-result.json`).

## Upgrading from 0.2

- The simulated (Playwright) provider is gone. A task that still declares
  `provider: "simulated"` runs on the Linux container with a deprecation warning
  (`CUA_BENCH_STRICT=1` makes it an error). HTML pages opened with
  `session.launch_window(html=...)` need bench-ui in the image.
- `cb image create|shell|clone|delete`, `cb platform`, `cb prune --docker` and
  `cb agent build|push` are deprecated no-ops; use `--image <registry ref>`.
- RL workers, the dataloader and trainers moved to
  [cua-bench-rl](../cua-bench-rl) (`cua_bench.workers` still imports, with a warning).
- `RemoteDesktopSession` keeps 0.2.7 semantics for code written against it (for example
  Agents' Last Exam): `run_command` reports `return_code` 0 unless
  `strict_exit_codes=True`, there is no shell timeout unless `timeout=` is set, `api_url`
  sessions speak to computer-server (or cua-spacesd: port 3211 or a sandbox ref such as
  `local:<name>`, `cloud:<name>`, `direct:<host:port>`), and `session.interface`
  keeps the cua-computer method set. `scripts/ale_compat/check_ale.py` checks ALE against
  a checkout.

## Running Tests

```bash
uv sync --extra dev
uv run pytest -q cua_bench/tests --ignore=cua_bench/tests/live
```

The suite is hermetic: cua-sandbox is replaced by an in-memory sandbox
(`cua_bench/tests/fakes.py`) and a loopback computer-server stand-in, so nothing starts a
container, VM or cloud claim. It includes the CLI parser golden
(`tests/golden/cli_tree.json`), export schema snapshots and the ALE compatibility tests.
After a deliberate, additive CLI or schema change, regenerate the goldens with
`CUA_BENCH_UPDATE_GOLDENS=1` and review the diff.

Opt-in live tests (they start real sandboxes):

```bash
CUA_BENCH_E2E_LOCAL=1 uv run pytest -s cua_bench/tests/live/test_local_live.py

set -a; source ~/.env; set +a
CUA_E2E_FLEET=1 uv run pytest -s cua_bench/tests/live/test_cloud_live.py
```

The ALE check runs agents-last-exam's own loader and harness against this cua-bench:

```bash
python scripts/ale_compat/check_ale.py --ale ../agents-last-exam --level 3
```
