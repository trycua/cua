# Fleet Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish complete Fleet concepts, how-to guides, and tutorials within the existing CUA documentation information architecture.

**Architecture:** Add a `fleet` subgroup beneath each existing Diátaxis mode: concepts, how-to guides, and tutorials. Ground Fleet resource semantics and service connection examples in `trycua/cloud/cyclops-cs`, and ground agent, benchmark, and reinforcement-learning workflows in the corresponding `trycua/cua` packages. Keep every page focused on one documentation mode and link across modes instead of duplicating explanations.

**Tech Stack:** MDX, Fumadocs `meta.json` navigation, Python examples, Kubernetes-style Fleet resources, Cua Driver, Cua Bench, pnpm documentation checks.

**Source baselines:**

- CUA docs repository: current `trycua/cua` checkout.
- Fleet implementation: `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs` at cloud commit `1506bb47f`.
- Public terminology: use “Fleet”, “sandbox”, “image”, “pool”, “claim”, and “service”; do not present “SBX” as a product name.
- Git commits are excluded from this plan because creating commits requires separate explicit user authorization.

---

### Task 1: Add Fleet navigation and landing pages

**Files:**
- Create: `docs/content/docs/concepts/fleet/index.mdx`
- Create: `docs/content/docs/concepts/fleet/meta.json`
- Create: `docs/content/docs/how-to-guides/fleet/index.mdx`
- Create: `docs/content/docs/how-to-guides/fleet/meta.json`
- Create: `docs/content/docs/tutorials/fleet/index.mdx`
- Create: `docs/content/docs/tutorials/fleet/meta.json`
- Modify: `docs/content/docs/concepts/meta.json`
- Modify: `docs/content/docs/how-to-guides/meta.json`
- Modify: `docs/content/docs/tutorials/meta.json`

- [ ] **Step 1: Add ordered navigation metadata**

Create these exact page orders:

```json
{ "title": "Fleet", "pages": ["index", "images", "pools", "claims", "auto-scaling", "services"] }
```

```json
{ "title": "Fleet", "pages": ["index", "build-a-linux-image", "build-a-windows-image", "connect-to-a-service", "use-the-sdk"] }
```

```json
{ "title": "Fleet", "pages": ["index", "hermes-and-cua-driver", "surf-the-web", "computer-use-sub-agent-with-mcp-sampling", "benchmark-codex-and-claude-code-with-cua-bench", "reinforcement-learning-for-computer-use"] }
```

Insert `fleet` into each parent `meta.json` after the closest existing Sandbox entry, without reordering unrelated pages.

- [ ] **Step 2: Write the Concepts landing page**

Use frontmatter title `Fleet concepts` and a description that names managed cloud sandboxes. Explain the five-resource mental model in one paragraph and provide direct links to Images, Pools, Claims, Auto Scaling, and Services. Link task-oriented readers to `/how-to-guides/fleet` and guided learners to `/tutorials/fleet`.

- [ ] **Step 3: Write the How-to landing page**

Use frontmatter title `Fleet how-to guides`. State that these pages assume the reader already understands Fleet concepts. Link to all four requested tasks and to `/concepts/fleet` for the resource model.

- [ ] **Step 4: Write the Tutorials landing page**

Use frontmatter title `Fleet tutorials`. Present the tutorials in increasing complexity: web surfing, Hermes with Cua Driver, MCP Sampling sub-agent, Codex versus Claude Code benchmarking, then reinforcement learning. Include prerequisites shared by the tutorials: Fleet access, a user API credential, Python 3.12 or 3.13, and workload-specific model credentials.

- [ ] **Step 5: Verify navigation parses**

Run:

```bash
cd docs
pnpm docs:check-links
```

Expected: the checker may report missing Fleet destination pages that are created in later tasks, but it must not report malformed JSON or invalid landing-page links outside the planned Fleet files.

---

### Task 2: Document the Fleet resource model

**Files:**
- Create: `docs/content/docs/concepts/fleet/images.mdx`
- Create: `docs/content/docs/concepts/fleet/pools.mdx`
- Create: `docs/content/docs/concepts/fleet/claims.mdx`
- Create: `docs/content/docs/concepts/fleet/auto-scaling.mdx`
- Create: `docs/content/docs/concepts/fleet/services.mdx`

**Primary sources:**
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/claim_and_connect.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/create_pool_and_list_tools.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/src/api/pools.ts`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/backend/handlers/svc.go`
- `/home/node/.codex/worktrees/f61f/cloud/nixos/pool-services/`
- `docs/content/docs/concepts/how-sandboxes-work.mdx`

- [ ] **Step 1: Write Images**

Explain that a Fleet image is the immutable boot artifact and software contract for pool replicas, distinct from the public `cua.Image` builder abstraction. Cover image references, Linux BIOS versus Windows EFI expectations, preinstalled services, readiness dependencies, version pinning, and why mutable `latest` tags reduce reproducibility. Link to both image-building how-to guides.

- [ ] **Step 2: Write Pools**

Define a pool as a named group of compatible sandbox replicas created from one template. Explain replicas, `minAvailable`, requested CPU/RAM, firmware, readiness port, declared services, warm capacity, and the one-pool/one-namespace convention used by the current Fleet API. Separate desired capacity from currently available capacity.

- [ ] **Step 3: Write Claims**

Describe `OSGymSandboxClaim` as the workload's lease on one sandbox from a pool. Explain Pending, Bound, and Failed phases; `sandboxTemplateRef`; binding to a warm replica; exclusive use; deletion as release; guest reset before reuse; and why per-user credentials are required for claim creation. Do not include a procedural claim script on this concept page.

- [ ] **Step 4: Write Auto Scaling**

Explain how desired replicas, warm capacity (`minAvailable`), outstanding claims, maximum capacity, provisioning latency, readiness, and scale-down interact. State that a bound but rebooting guest may not have ready services yet. Describe operational tradeoffs between zero-idle cost and low claim latency without promising undocumented timing or capacity limits.

- [ ] **Step 5: Write Services**

Explain declared pool services, service labels/names, ports, health paths, MCP paths, auxiliary services, and authenticated proxy routing through `/api/svc/{namespace}/{service}/`. Distinguish claim binding from service readiness and explain why 502/503/504 responses can occur during guest startup. Link to the connection how-to.

- [ ] **Step 6: Check documentation mode**

Read the five pages together and remove command sequences, installation walkthroughs, and tutorial narrative. Each page must answer what the resource is, how it relates to other resources, its lifecycle boundaries, and its principal tradeoffs.

---

### Task 3: Add Fleet image-building guides

**Files:**
- Create: `docs/content/docs/how-to-guides/fleet/build-a-linux-image.mdx`
- Create: `docs/content/docs/how-to-guides/fleet/build-a-windows-image.mdx`

**Primary sources:**
- `/home/node/.codex/worktrees/f61f/cloud/.github/workflows/build-desktop-workspace-duo.yml`
- `/home/node/.codex/worktrees/f61f/cloud/.github/workflows/build-windows-desktop-workspace.yml`
- `/home/node/.codex/worktrees/f61f/cloud/images/`
- `/home/node/.codex/worktrees/f61f/cloud/windows-desktop-workspace/`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/linux-mini-swe.yaml`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/windows-mini-swe.yaml`

- [ ] **Step 1: Write the Linux image guide**

Provide prerequisites, identify the repository image directory and build workflow, explain how the disk becomes a KubeVirt `containerDisk` OCI image, require a stable registry tag, and show the verified Fleet pool fields for Linux: image reference, CPU, RAM, BIOS firmware, readiness port, and services. Include verification of the image manifest and a pool readiness check. Clearly distinguish an infrastructure image build from `Image.linux()` in the public Sandbox SDK.

- [ ] **Step 2: Write the Windows image guide**

Provide prerequisites for a Windows installation source and image build, explain unattended provisioning and the `containerDisk`/PVC deployment boundary used by the current workflow, require EFI firmware for the documented image, and show the verified Fleet pool fields for Windows. Include service startup and readiness verification before scaling the pool. Avoid publishing internal registry account identifiers; use `REGISTRY/IMAGE:TAG` placeholders whose replacement is explicitly explained.

- [ ] **Step 3: Validate YAML examples**

Extract each pool YAML code block to a temporary file and parse it:

```bash
python - <<'PY'
from pathlib import Path
import yaml

for path in [Path('/tmp/fleet-linux-pool.yaml'), Path('/tmp/fleet-windows-pool.yaml')]:
    yaml.safe_load(path.read_text())
    print(f'{path.name}: valid')
PY
```

Expected: both files print `valid`. Install or use an existing YAML parser only if already available; do not add a docs runtime dependency solely for this check.

---

### Task 4: Add service and SDK how-to guides

**Files:**
- Create: `docs/content/docs/how-to-guides/fleet/connect-to-a-service.mdx`
- Create: `docs/content/docs/how-to-guides/fleet/use-the-sdk.mdx`

**Primary sources:**
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/claim_and_connect.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/create_pool_and_list_tools.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/lanes_fan_out.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/cua_train/_convenience.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/openapi3.yaml`

- [ ] **Step 1: Write Connect to a service**

Document installation of `cua-train` from the CUA wheel index, `CUA_CLIENT_ID`/`CUA_CLIENT_SECRET`, per-user credential requirements, claim creation, polling until Bound, selecting a declared service, constructing the `/api/svc/{namespace}/{service}/` path, polling through transient proxy errors, and deleting the claim in `finally`. Include a compact MCP `initialize` then `tools/list` verification path for a Cua Driver service and a plain HTTP health-check variant.

- [ ] **Step 2: Write Use the SDK**

Use `TrainClient.from_key` as the supported authentication entry point. Show one complete script that lists or selects a pool, creates a claim, waits for Bound, inspects the assigned sandbox and service information, then releases the claim. Explain transparent token refresh and the difference between generated SDK endpoints and authenticated `httpx` calls to Kubernetes proxy endpoints. Link to the service guide for protocol-specific connection details.

- [ ] **Step 3: Syntax-check Python examples**

Extract all complete Python scripts from these pages and run:

```bash
python -m py_compile /tmp/fleet-connect-service.py /tmp/fleet-use-sdk.py
```

Expected: exit code 0 and no output.

---

### Task 5: Add introductory Fleet tutorials

**Files:**
- Create: `docs/content/docs/tutorials/fleet/hermes-and-cua-driver.mdx`
- Create: `docs/content/docs/tutorials/fleet/surf-the-web.mdx`
- Create: `docs/content/docs/tutorials/fleet/computer-use-sub-agent-with-mcp-sampling.mdx`

**Primary sources:**
- `docs/content/docs/tutorials/drive-your-first-app.mdx`
- `docs/content/docs/how-to-guides/driver/connect-your-agent.mdx`
- `docs/content/docs/reference/cua-driver/mcp-tools.mdx`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/claim_and_connect.py`
- `/home/node/.codex/worktrees/f61f/cloud/cyclops-cs/python-sdk/examples/linux-mini-swe.yaml`
- The published Model Context Protocol Sampling specification, used only for protocol fields not defined in these repositories.

- [ ] **Step 1: Write Hermes and Cua Driver**

Guide the reader through selecting a Fleet pool that exposes Cua Driver, claiming a sandbox, connecting Hermes to the proxied MCP endpoint, reloading Hermes MCP servers, asking Hermes to complete one desktop task, verifying the result through a screenshot or visible state, and releasing the claim. Reuse the current Hermes configuration shape from `drive-your-first-app.mdx`; replace local Cua Driver transport with the authenticated Fleet service URL.

- [ ] **Step 2: Write Surf the web**

Create a beginner tutorial that claims a Linux browser sandbox, verifies browser and Cua Driver readiness, asks an agent to navigate to a harmless public site and report visible information, verifies the final URL or page state, and releases the claim. Include a warning not to place credentials into untrusted sites or prompts.

- [ ] **Step 3: Write MCP Sampling sub-agent**

Explain the architecture: host MCP client owns model access, the Fleet-connected computer-use MCP server requests `sampling/createMessage`, and the host approves and returns model output. Provide a minimal host-side example with explicit capability negotiation, request handling, bounded iterations, tool result forwarding, and claim cleanup. State that MCP Sampling availability depends on the host client; do not claim that all clients support it.

- [ ] **Step 4: Verify cross-page progression**

Ensure the web tutorial can be completed before the Hermes tutorial, and the Hermes tutorial introduces enough Fleet and MCP vocabulary for the Sampling tutorial. Link concepts instead of duplicating resource definitions.

---

### Task 6: Add benchmarking and RL tutorials

**Files:**
- Create: `docs/content/docs/tutorials/fleet/benchmark-codex-and-claude-code-with-cua-bench.mdx`
- Create: `docs/content/docs/tutorials/fleet/reinforcement-learning-for-computer-use.mdx`

**Primary sources:**
- `libs/cua-bench/README.md`
- `libs/cua-bench/cua_bench/cli/commands/run.py`
- `libs/cua-bench/cua_bench/config/schema.py`
- `libs/cua-bench/cua_bench/runners.py`
- `libs/cua-bench/cua_bench/workers/worker_server.py`
- `libs/cua-bench/cua_bench/workers/dataloader.py`
- `libs/cua-bench/cua_bench/trainer/off_policy/tinker/rl_loop.py`
- `libs/cua-bench/cua_bench/trainer/off_policy/tinker/grpo.py`

- [ ] **Step 1: Write the Codex versus Claude Code benchmark tutorial**

Define one deterministic computer-use task with an evaluator, pin the same Fleet image and resource shape for both agents, run multiple trials per agent, retain trajectories and rewards, and compare success rate, average reward, latency, and token/cost data only where the harness emits it. Separate agent configuration from environment configuration so the comparison changes one variable. State that small samples are illustrative rather than statistically conclusive.

- [ ] **Step 2: Write the reinforcement-learning tutorial**

Guide the reader through defining a Cua Bench task, implementing reset/setup and reward evaluation, provisioning parallel Fleet workers, collecting trajectories, loading episodes through the replay/dataloader layer, running the repository's supported off-policy GRPO/Tinker loop, evaluating a held-out split, and cleaning up workers. Explain reward leakage, environment determinism, train/eval separation, checkpointing, and cost controls.

- [ ] **Step 3: Verify CLI and Python names**

Check every named command, decorator, class, and module against the current Cua Bench source with `rg`. Remove or clearly label pseudocode where no stable public CLI exists. Do not present an internal module invocation as a supported public command unless `libs/cua-bench/README.md` or the CLI exposes it.

---

### Task 7: Cross-link and validate the complete documentation set

**Files:**
- Modify only as needed: `docs/content/docs/concepts/how-sandboxes-work.mdx`
- Modify only as needed: `docs/content/docs/tutorials/your-first-cloud-sandbox.mdx`
- Modify only as needed: `docs/content/docs/reference/sandbox-sdk/index.mdx`
- Review: all files under `docs/content/docs/concepts/fleet/`
- Review: all files under `docs/content/docs/how-to-guides/fleet/`
- Review: all files under `docs/content/docs/tutorials/fleet/`

- [ ] **Step 1: Add minimal entry links from existing Sandbox docs**

Add one short Fleet link where readers naturally move from a single sandbox to managed pooled capacity. Do not rewrite existing Sandbox SDK content or imply that `cua.Image` and Fleet infrastructure images are the same object.

- [ ] **Step 2: Run terminology and placeholder checks**

Run:

```bash
rg -n "\bSBX\b|FIXME|REGISTRY/IMAGE:TAG" docs/content/docs/{concepts,how-to-guides,tutorials}/fleet
```

Expected: no `SBX`, planning placeholders, or unexplained registry placeholders. A registry placeholder is acceptable only on lines whose surrounding text explicitly tells the reader how to replace it.

- [ ] **Step 3: Run docs hygiene and links**

Run:

```bash
cd docs
pnpm docs:check-hygiene
pnpm docs:check-links
```

Expected: both commands exit 0. The hygiene command prints `Public docs hygiene check passed.` and the link checker reports no invalid internal links.

- [ ] **Step 4: Build the docs site**

Run:

```bash
cd docs
pnpm build
```

Expected: Next.js completes a production build with all Fleet MDX pages generated and no MDX, TypeScript, navigation, or route errors.

- [ ] **Step 5: Review the final diff**

Run:

```bash
git diff --check
git status --short
git diff --stat
```

Expected: no whitespace errors; only the Fleet docs, navigation metadata, approved cross-links, design spec, and implementation plan are changed.
