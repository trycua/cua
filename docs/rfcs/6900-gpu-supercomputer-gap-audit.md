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

## Audit: every way a Space can make or run another Space

Invariant: anything a Space creates or runs is its **child**: tagged with its parent, listed under it, cleaned up with it, and bound by its network and egress rules (never wider). Enforcement is in the daemon's scoped channel (`children/api.rs`, authorized by the Space's capability), not in the client library.

How it was checked: code reading of every handler the scoped channel serves (`SpaceService`, `DaemonService`, `ShareService`), the existing in-process red-team suite (`children/tests.rs`), the new test for runs, and a probe run from inside a real Space (`libs/cua/scripts/space-escape-probe.py`).

| Pathway (from inside a Space) | Before | Escaped? | Now |
|---|---|---|---|
| `create_space`, `sandbox_create`, `cua sb create` | `child_request`: parent forced to the caller, network cut to the caller's, egress the caller's, `widen`, forged `cua.parent` labels, `parent`, `reuse`, `durable` refused, depth and budget checked | No | Unchanged, covered by `a_create_becomes_a_child_or_a_plain_refusal` |
| Create on another computer or a cloud (`location`) | Allowed within budget; the network goes with the request cut to the caller's; labels carry the tree | No | Unchanged |
| `add_space`, `remove_space`, `leave_spaces`, `provision_local_space`, `release_space`, volumes (`list/create/delete_volume`), `list_hosts`, `get_gpu_support`, `list_orphans`, builds | Denied | No | Unchanged (`owner_calls_are_refused...`) |
| `fork_space` | Own subtree only, depth checked | No | Unchanged |
| `set_space_network/egress/clipboard/volume`, `get_space_network(_log)`, tree budget | Denied or read-only; "the owner's, never an agent's" | No | Unchanged |
| `call_space_tool` / `cua mcp` tools | Allowlist (`tool_target`): stop/start/keep_awake, bash, write, upload/download, list/call tool, window streaming; everything else (share_space, cloud_connect, create_space, agent_start, teleport, prewarm, checkpoint) refused by name | No | Unchanged |
| `create_share` | TCP port of itself or its children only, no consent, own shares listed | No | Unchanged |
| **`cua run` / jobs / gangs** (`pool_request submit`) | The job's Space was created with the default `internet` network and **no parent**: a Space with `none` ran a job with the internet, and the job never showed under it | **Yes, high** | Daemon stamps parent, root, depth and the parent's network over the request (also `jobs[].spec`); the host records a child (tree record on this computer, labels and the cut-down network on another). Refused with a reason: a run deeper than the tree allows |
| **`kind=machine` jobs** | Run natively on a dedicated host, outside every network rule | **Yes, high** | Refused from a Space with network rules; allowed from a plain `internet` Space |
| **Job `services`** (host services such as `compat:e2b`, which create Spaces on the host's own terms) | Accepted from a Space | **Yes, medium** | Refused |
| **Job `on` = a cloud provider** (`modal`, `aws`, ...) | Accepted: spends the owner's cloud outside the tree's budget | **Yes, medium** | Refused |
| `cua compat` e2b/daytona/modal shims | A gateway process in the Space calling the SDK: goes through the scoped channel, so its sandboxes are children | No | Unchanged; the host-side service form is refused above |
| In-Space `cua`/SDK with no spawn channel | Fell back to finding or starting a daemon of its own (G11) | Availability bug; a Space-local daemon only has what the Space has | `ensure_daemon` refuses inside a Space (`CUA_SPACE`) with a clear message. Client-side courtesy: the guarantee is the scoped channel |
| Raw routes: engine socket, WSL interop, the host's addresses and the daemon, relay, SMB and RPC ports, credentials, the daemon's token files | Not reachable (probe) | No | Kept by `space-escape-probe.py` (gateway and LAN address time out even for listening ports; no `docker.sock`, `/mnt/c`, `/run/WSL`; no daemon token or key in the Space; `get_info` returns none) |
| Not individually tested | prewarm, checkpoint/restore, teleport (refused by the allowlist, covered only by the by-name test of other owner tools); persistent agents (`agent_start` refused) | | Follow-up |

## Model: the tree is logical; placement is physical

A Space, a run or a job is a **child of the Space that made it, wherever it runs**. The tree the person sees (child list, "In this Space", counts, search, budgets) is the *logical* tree; the computer it executes on is a secondary detail ("on mac-m2max"), never a different place in the tree.

**Fields (carried on every record wherever it lives):** `parent` (the id of the Space that made it, by id never by name), `root` (the top-level Space of the tree), `depth`, `origin` (`child`, `job`, ...). A Space record keeps them in `SpaceMeta` (and mirrors `cua.parent`/`cua.root`/`cua.depth` labels so another computer that holds only the labels can see them); a run keeps them in the **run ledger** of the computer that holds the parent (`spawn/runs.json`: `space`, `root`, name, GPU), and the job's environment carries `CUA_PARENT_SPACE`, `CUA_ROOT_SPACE`, `CUA_TREE_DEPTH`, `CUA_PARENT_NETWORK` to whichever host executes it.

**Who owns the truth.** The computer that holds the *parent* owns the tree: its registry and its run ledger decide what is a child. Another computer owns only the execution of a child and records the labels it was given. The child list is built by merging, in this order: (1) local Space records with a parent link, (2) the run ledger (`job-rows.json`: one row per run, from the ledger and the pool's own state for the run; no Space record is needed, so a job on another computer or a native machine job still shows), (3) remote hosts' records for children made with `create_space --on host:...` (labels `cua.parent`). One row per run: if a job's own Space also exists locally for the same run label, it is the same row. A gang is one row with a member count.

**Policy crosses hosts by value.** The child's network is the parent's, cut down (`intersect`), computed on the parent's computer and sent with the request (`CUA_PARENT_NETWORK` for jobs, the request's `network_settings` for Spaces) and enforced by the *executing* host's egress layer. A host that cannot enforce it refuses (never runs the child open). A native machine job runs outside every network rule, so a Space with rules cannot start one (refused with the reason).

**Cleanup crosses hosts.** Deleting or stopping the parent cancels its runs wherever they run (`reap_runs`, from the ledger) and deletes remote children by the label link. If the parent's computer sleeps or leaves, children keep their durable parent link; after a grace period an orphan sweep on the executing host ends children whose parent has not been seen (open: the cross-host sweep, below), never silently losing results (a result is kept with the run for 7 days).

**Failure cases.** Host dies mid-job: the pool places the run again and the row shows Queued, then Running, never stuck at Running (`job_row` treats a waiting gang as queued); the parent's host sleeps: the run continues, the row is gone from view until it wakes (ledger on disk); relay or tunnel flaps: rows keep the last state and the pool answers again; the executing host leaves for good: the run fails with "its computer was lost" and the row shows Failed with why.

Implementation here: `children/runs.rs` (`job_row`, `job_facts`, `JobRowOut`; tests for this PC, another host, a native machine job, a gang, a host lost mid-job, a finished/failed run), `children/bridge.rs` (`write_job_rows`), the app's `spaces.tree` (jobs as nodes with `job`), `child-spaces-section.tsx` (the row).

## Second pass (after the recordings): current status and what is left, ranked

Re-tested against the current builds (PC and Mac):

| # | Item | Status |
|---|---|---|
| G1-G3, G10-G14 | pool from a Space, children tagged, skill and brief, windows, memory, mounts | working (end to end on both computers; NCA on CUDA and MLX) |
| G5 | VRAM admission | working for NVIDIA (PC); Apple reports no free memory (unified), noted |
| G7 | UI: job row | working for a job's Space on this computer; this pass adds the ledger-driven row for any host (tests; real-app check on the Mac run pending a native rebuild) |
| N1 | The thread said "Let me look at it" (process narration) in the MLX run. Cause: the model's habit before a tool call; the brief alone does not hold. | fixed structurally: later stretches that narrate a plan are held back (`is_process_narration`), plus the brief |
| N2 | An outsized event line in an agent's log made the reader stall and the final message never reach the chat | fixed (#7004) |
| N3 | F: full corrupted the runtime distro; no guard in the daemon | FIXED (#6945): every local create checks the free-space floor (`CUA_DISK_MIN_FREE`, default 5 GiB; pulls and VM creates already did) and refuses with the way out; unit tests with a fake probe |
| N4 | `cua run` followed in the foreground is killed by the agent shell after ~2 min and cancels the run | FIXED (#6945): when stdout is not a terminal (and not `--json`) the follow stops after 100 s (`CUA_RUN_FOLLOW_SECS`, 0 = never), says the run keeps going and how to follow or cancel it, exits 0 like `--detach`; a terminal follows to the end |
| N5 | The agent retrains to "polish" (3 runs, 15 min) | mitigated by the brief and a 3-minute limit; the quality check is still the agent's own |
| N6 | Children on another host: orphan sweep and child list | VERIFIED live (PC and Mac, non-GPU `sleep` jobs submitted from a home Space with `--on host:`): one row per run under the parent, each with its host; deleting the parent canceled both runs within ~10 s and the rows left the list |
| N7 | A host leaving mid-job | MEASURED live: the Mac host process killed mid-job; the row stayed Running about 3 minutes (the pool's detection time), then went queued while the pool placed it again, then failed with the reason (an Apple GPU job has no other computer). Never stuck at Running; the 3-minute window is the pool's, not the row's |
| N8 | 10 concurrent agents | RE-RUN with CPU jobs: 10 concurrent `cua run --on auto` jobs across the PC and the Mac all succeeded (Mac 20 s each, PC ~68 s each: the PC's member start is the slow part). A job's row now counts the gang size, not every attempt |
| N9 | The relay/tunnel is an SSH tunnel; a flap makes submits fail | FIXED (#6945): a submit retries for about a minute and says once, plainly, that the computers did not answer and nothing was started; a follow says it once and keeps trying |
| N10 | Windows native member PATH | FIXED (#6945): `member_env` joins `PATH` with `;` for a Windows guest (drive-letter tools directory), `:` elsewhere; tested |
| N11 | Stream/log/command routing to a remote child | BY DESIGN for now: results come back to the home Space and the window opens there (the demo's window is in the person's Stream); the job's own Space is not streamed. Logs follow through `cua jobs logs -f` across hosts |

Ranked next: all of N3-N11 are fixed, verified or documented as by design; what is left is N5 (the agent's own quality check) and a faster host-loss detection than the pool's ~3 minutes.
