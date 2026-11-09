# Gap audit: "one AI supercomputer" for Home agents

Goal: in Cua Spaces Local, a Home prompt such as "autoresearch a ~1B decision
model that classifies the safety of computer-use actions" makes the agent
(Claude, in a Linux Space) use GPU jobs on the user's pool as **children of its
tree**, with no orphans, and up to 10 such agents at once share the GPUs.

Evidence below is `path:line` in this repository at the audit commit. Status is
kept current by each fixing PR.

## Ranked gaps

| # | Gap | Blocks | Evidence | Status |
|---|---|---|---|---|
| G1 | A Space cannot reach the pool. The scoped child channel answers `deny()` to `pool_request` and `list_hosts`, and `cua run`, `cua jobs`, `cua computers` all go through `pool_request`. | everything | `cua-daemon/src/children/api.rs:1099-1105`; `cua-cli/src/runs.rs:152-158`; `cua-cli/src/lib.rs:1266-1273` | open |
| G2 | Jobs are not children of the submitter's tree: `JobSpec`/`JobRow` have no submitter, parent Space or root; `cj-*` Spaces are created without a parent; deleting or stopping the parent never touches them. | no orphans | `cua-cluster/src/model.rs:85-153`; `cua-spaces-cluster/src/coordinator/store.rs:88`; `cua-spaces/src/host_jobs.rs`; `cua-spaces/src/tree.rs:235` | open |
| G3 | Nothing teaches Claude to use the pool. The Home brief lists `spaces ls/create` only; no skill mentions `cua run`, `cua jobs`, `@app.function`, GPU. | agent behaviour | `apps/cua-spaces-web/src/lib/agent-home/prompt.ts:22-49`, `container.ts:21-43`; `libs/cua/skills/*` | open |
| G4 | No GPU in containers on a Windows PC: the built-in runtime is a WSL 2 distribution that has `/dev/dxg` and the driver libraries but the GPU option answered "no GPU access". | real GPU use | `cua-vmm/src/gpu.rs` (windows arm), `cua-daemon/src/local.rs` (`gpu_support`) | PR: feat/windows-wsl-gpu |
| G5 | The accelerator is a vendor string. No GPU model, VRAM, free VRAM, per-job VRAM request, admission against over-commit, priority/preemption (interactive over batch), or fair share across tenants. | noisy neighbours, 10 agents | `cua-cluster/src/model.rs:161,175`; `placement.rs:615` | open |
| G6 | `@app.function` takes `accelerator=` (`gpu|apple|nvidia`), not `gpu=`; no `volumes=`; job Spaces have no Cua Volume, so no shared HF/dataset cache. | Modal-like definition, caches | `jobs-runtime/cua/__init__.py:223-302`; `host_jobs.rs:27-28` | open |
| G7 | No UI for pool jobs: no runs under the parent, live logs, GPU use, cleanup state; the machines page has no accelerator column. | visibility | `apps/cua-spaces-web/src/components/ci/jobs-table.tsx` is CI-only; `routes/machines/index.tsx` | open |
| G8 | Budgets and roots: each Home request is its own tree root with 16 live children, 16 vCPU, 32 GiB, $2/h; depth 2. No pool-wide concurrency view or queue UI. | 10 concurrent agents | `cua-daemon/src/children/budget.rs:24-30`; `cua-spaces/src/tree.rs:38-40` | open |
| G9 | Host or daemon death: queued/running jobs outlive the parent; a host that never returns keeps its `cj-*` Spaces and bundle directories until a sweep. | no orphans | `host_jobs.rs:217-224`; `coordinator/mod.rs:13-20` | open (fault tests pending) |

### Found while running it

| # | Gap | Fix |
|---|---|---|
| G10 | A Space cannot put a window on the person's screen (the scoped channel refused the stream tools). | #6945: allowed for the Space itself and its children |
| G11 | `cua` inside a Space started a daemon of its own and answered "this cua daemon does not have them" (`ensure_daemon` ignored the spawn channel). | #6945 |
| G12 | The first `cua` command in a Space can run before the spawn channel is up (the first probe lands while cua-spacesd starts; the retry was 60 s). | a Space younger than 3 minutes is probed every 5 s |
| G13 | A Windows computer reported no memory, so no pool job fit it ("not enough memory free"). | #6943 |
| G14 | The built-in runtime on Windows refused every mount, and a job member needs its work, data, cache and tools directories. | the job directories are mounted into the distribution with `drvfs` (`/mnt/cua-host/<name>`) |
| G15 | OS invariance (audit below). | shim and CLI fixes in this series |
| G16 | A published image carries an older cua-spacesd than a daemon built from main wants after a version bump, so an agent's Space has no children channel until images are rebuilt. | release process; noted |

### OS invariance audit (Linux, macOS and Windows submitter, member and host)

A job's member is a Linux container Space on a Windows or Linux host, so its
paths and shell are Linux whatever the host OS. What follows the host or a
Windows guest:

| Assumption | Where | Status |
|---|---|---|
| `fcntl` in the shim's store lock: `import cua._store` raised on Windows, so every job died before user code | `cua/_store.py` | fixed (`msvcrt` fallback; test) |
| `fsync` on a read-only handle fails on Windows | `cua/_store.py` | fixed |
| Windows `file://` store URLs rejected | `cua/__init__.py`, `_store.py` | fixed (`file_url_path`) |
| Only SIGTERM stops a member | `_cua_entry.py` | fixed (SIGBREAK on Windows) |
| NVIDIA is detected only on Linux | `_cua_entry.py` | fixed (Windows too) |
| `python3` is the submitter's Python | `jobs_cmd.rs` | fixed (`python3`, `python`, `py -3` on Windows) |
| uv is pinned for macOS and Linux only; `member_env` joins PATH with `:` | `cua-host/uv.rs`, `member.rs` | open: only a native Windows member (kind `machine`) needs it; containers do not |
| MLX needs a macOS member: a Linux container on a Mac has no Metal | `host_jobs.rs` | by design: `accelerator=apple` runs in a macOS VM (Lume) |
| Apple GPUs report no free memory, so `gpu_memory` is not enforced there | `sysinfo.rs` | unified memory; noted |

## What already works

Gang scheduler with SQLite queue, retries and fences; checkpoint save/resume
(`cua/checkpoint.py`, `_store.py`); rank-0 results in `./cua-runs/<run>/`; cost
in `cua jobs <run>`; `cua computers` lists accelerators (this PC reports
`nvidia:RTX PRO 6000 Blackwell Workstation Edition:96 GB`); cascade delete of
registry children (`tree.rs:235`) and the orphan reaper (`children/reaper.rs`).

## Fix order

G4 (GPU works at all), G1 + G2 + G9 (a Space submits as its tree; cleanup),
G3 (skill and brief), G5 (admission and fair share), G6 (`gpu=`, `volumes=`),
G7 (UI), G8 (budgets). After each fix the real prompt runs in the real app and
the run is recorded; a run passes only if the agent is not confused or verbose,
nothing errors, nothing is left behind, the job used the GPU and produced a
trained ~1B classifier with an evaluation report.
