# Cua Driver backlog by platform and test batching, 2026-08-23

Status: point-in-time, read-only scheduling and test-infrastructure plan

This companion to the
[backlog reconciliation](backlog-reconciliation-2026-08-23.md) reorganizes the
same active work by operating-system lane and area of interest. It answers a
narrow optimization question: which candidates can reuse one provisioned VM
image or desktop session **sequentially** without weakening exact-source
evidence?

This plan does not select or authorize implementation, review, merge, release,
branch composition, or issue disposition. A batch reuses infrastructure. It
does not combine code changes, branches, contributor ownership, or evidence.
Every candidate keeps its own exact SHA and ledger, and no result certifies a
different SHA.

Evidence was rechecked at 2026-08-23 11:54 CDT against `origin/main` at
`737dc2a069528abadee67526d138a907e1c52061` and live GitHub state. Evidence
levels use the reconciliation report's **Deep**, **Standard**, and **Inventory**
scale. The five public Cua Sandbox and Fleet pages cited below were rechecked at
2026-08-23 12:40 CDT.

## Public Cua Fleet pilot

If Francesco selects priority Windows reproduction, use one reusable public Cua
Fleet pool for the two sequential **W1** candidate segments. Reuse the pool and
its configuration, not candidate state. Candidate B must start from the pinned
image independently of every filesystem, registry, process, account, profile,
daemon, and GUI mutation made by candidate A.

### What the public documentation establishes

A Cua Sandbox is one isolated computer whose code and GUI interfaces share the
same filesystem, processes, and operating-system state.[1] An image is the
immutable contract for a fresh sandbox's starting state; a running sandbox then
accumulates its own writable state. Snapshots can create images, and forks from
one snapshot have independent writable disks.[1]

The public Terraform provider documents reusable Linux and Windows pools on
`run.cua.ai`, with either a static warm-replica target or claim-driven
autoscaling.[3] Its Windows example uses KubeVirt, EFI, the public
`cua-windows-2022` image, computer-server port `8000`, and a TCP readiness probe.
Those settings establish the public example's boot and service shape; readiness
does not prove a clean candidate boundary.[3]

The public Python guide uses `Pool.apply()` to create or reconcile a pool and
`pool.claim()` to create or reconnect to a claim. Exiting the claim context
releases the claim while leaving the pool and its one warm replica available.[4]
The lifecycle guide likewise distinguishes disconnecting from destroying a
sandbox: disconnect preserves the running machine and its state, while destroy
deletes it.[2]

These public pages do **not** say that releasing a Fleet claim cleans, reimages,
or replaces its replica. A new claim identity is therefore not proof of a new
sandbox, and neither release-plus-rewarm nor claim recreation is a documented
clean boundary. Autoscaling with `min_pool_size=0` is also not such proof: the
Python guide says it scales an idle pool to zero and makes the next claim cold
start, but does not promise a wipe or reimage.[4]

### Recommended W1 shape

Prefer one static Windows pool with `replicas=1`. It is the smallest reusable
shape documented by the Python guide and keeps the two-item pilot sequential.[4]
Broad parallel scale-out is unnecessary: the public concurrency guide supports
multiple asynchronous sandboxes but recommends starting with low concurrency
and increasing it only after observing resource use.[5]

Infrastructure reuse and state reuse are separate decisions:

- keep one reviewed pool name, image digest, replica target, service definition,
  and teardown owner for the bounded pilot;
- give candidate A and candidate B separate claim and sandbox identities and
  separate evidence ledgers; and
- before candidate B runs, prove that its sandbox was freshly created from the
  pinned image digest and cannot contain candidate A state. If the selected
  public Fleet operation cannot prove that boundary, abort W1. Do not substitute
  a released claim, a cold start, or an absent known artifact for that proof.

The pilot therefore validates the clean boundary before it executes either
backlog candidate. Pool reuse is the proposed optimization; fresh candidate
state is a non-negotiable acceptance condition.

### Phase 0: no-cost preparation

Phase 0 is documentation and read-only preparation only. It must not authenticate
to Fleet, inspect credentials, create or reconcile a pool, claim a sandbox, run
contributor code, or create billable capacity.

1. Revalidate and pin candidate A to current main
   `737dc2a069528abadee67526d138a907e1c52061`, candidate B to #3275 head
   `1429831d5a27246ea241f55b4f0f991884e43f5b`, and each candidate's base SHA.
2. Select the reviewed Windows image and record its immutable digest. Do not use
   the public example's mutable `:latest` tag as candidate evidence.
3. Review the following Terraform and Python shapes against the selected account
   and current public SDK/provider versions. They are drafts, not executed
   commands or committed configuration.
4. Define the strict Windows interactive-desktop preflight, clean-boundary
   rehearsal, candidate-specific ledgers, abort conditions, artifact hashing,
   cost observation, and teardown checklist.

Draft Terraform shape, adapted from the public Windows example:[3]

```hcl
resource "fleets_pool" "w1_windows" {
  name                 = var.pool_name
  cpu_cores            = 4
  memory               = "4Gi"
  container_disk_image = var.windows_image_digest
  runtime              = "kubevirt"
  firmware             = "efi"
  replicas             = 1

  readiness_probe_json = jsonencode({
    tcpSocket          = { port = 8000 }
    initialDelaySeconds = 60
    periodSeconds       = 5
    timeoutSeconds      = 3
    failureThreshold    = 120
  })

  service {
    name        = "computer-server"
    target_port = 8000
    protocol    = "TCP"
  }
}
```

Draft SDK control-flow shape, adapted from the public `Pool.apply()` and
`pool.claim()` example:[4]

```python
import os

from cua_sandbox import Image, Pool

pool = await Pool.apply(
    Image.from_registry(os.environ["CUA_WINDOWS_IMAGE_DIGEST"]),
    name=os.environ["CUA_POOL_NAME"],
    replicas=1,
    cpu=4,
    memory_mb=4096,
    services={"computer-server": 8000},
)

async with pool.claim(
    name=os.environ["CUA_CLAIM_NAME"],
    service="computer-server",
    time_to_start=900,
) as sandbox:
    # Record identity and preflight only after the clean boundary is proven.
    ...
```

Phase 0 does not assert that this control flow creates a clean second sandbox.
That unresolved behavior is the first Phase 1 acceptance question.

### Phase 1: billable and human-gated

Do not apply the pool or claim a sandbox until a human records all of these
approvals:

- accountable Fleet account owner and budget ceiling;
- maximum wall-clock duration and cost/spend observation method;
- globally unique pool name and immutable Windows image digest;
- credential source, without copying credentials into this report, source
  control, Terraform state, shell history, or logs;
- teardown authority, teardown deadline, and an alternate owner if the operator
  becomes unavailable; and
- explicit approval to execute untrusted contributor pull-request code for
  candidate B.

Approval to prepare this report is not approval to spend money, create Fleet
resources, retrieve credentials, or execute either candidate.

### Acceptance, candidate order, and aborts

1. **Prove the clean boundary first.** In a harmless rehearsal, record the pool,
   claim, and sandbox identities; pinned image digest; boot and uptime evidence;
   claim times; and a preflight designed to detect prior-segment state. Obtain a
   public or repository contract for the operation that creates the fresh
   sandbox. Abort before candidate execution if the evidence cannot prove a
   newly created sandbox from the pinned image.
2. **Candidate A:** in its own claim, sandbox, and ledger, reproduce
   [#3329](https://github.com/trycua/cua/issues/3329) at current main. Record the
   daemon, MCP shim, session label, owner disconnect, process generation, and
   next-call result. Collect and hash artifacts, then end the segment.
3. **Revalidate the boundary:** candidate B receives a different claim and
   sandbox identity and must repeat the clean-boundary proof. Abort if candidate
   A state cannot be excluded.
4. **Candidate B:** at exact #3275 head
   `1429831d5a27246ea241f55b4f0f991884e43f5b`, replay focused diagnostics for
   both failing owners: shared Electron/Tauri and native
   WPF/WinUI3/WebView2. Keep its ledger and artifacts separate.
5. Run the selector-free complete Windows harness only after a repaired, stable
   exact candidate SHA exists. Destroy the pool by the approved deadline; do not
   infer successful cleanup from claim release alone.

Abort the pilot on an unproven clean boundary, failed interactive-desktop
preflight, stale candidate SHA, image-digest drift, missing ledger identity,
credential leakage, budget or wall-clock limit, teardown-owner loss, or any
unexpected cross-candidate state.

For every segment, record pool, claim, and sandbox identity; image digest;
candidate and base SHA; boot and uptime evidence; claim start and release times;
preflight; exact commands; exit status; artifact hashes and paths; cleanup state;
and cost/spend observation. End each ledger with: “This evidence applies only to
candidate SHA `<SHA>` in session class `WIN-GUI`.”

### Cleanup and teardown evidence

The Phase 0 checklist must define these Phase 1 records before any resource is
created:

- claim-release time and result, recorded as lifecycle evidence rather than
  proof that the replica was cleaned;
- the approved deletion path—`await pool.delete()` or `terraform destroy`—plus
  its operator, start and completion times, exit status, and redacted error if
  it fails.[3][4]
- post-delete verification that the pool and its documented backing namespace
  and template no longer exist, with the verification method and time; and
- final cost/spend observation, cleanup deadline result, and escalation owner and
  evidence if deletion or verification is incomplete.

The public Python guide says `pool.delete()` deletes the pool and template, and
the Terraform guide says `terraform destroy` deletes the pool and same-named
namespace.[3][4] Record the observed result; do not mark teardown complete from
the requested operation alone.

The exact-head
[#3275 Windows E2E run](https://github.com/trycua/cua/actions/runs/32428179703)
failed both the shared and native jobs, not only the native jobs. Its current PR
check rollup shows 21 successes and two skips but omits that failed E2E workflow;
the direct exact-head action run remains the relevant failure evidence.

The reconciliation's earlier total of 31 successful jobs and two skips counted
job-level results across the exact-head workflows at 02:14 CDT, including
successful jobs inside the failed Windows E2E workflow. At 11:54 CDT, GitHub's
PR `statusCheckRollup` returned 21 successes and two skips because it omitted
that workflow. The difference is API scope, not a new candidate or a recovered
E2E result.

[#3318](https://github.com/trycua/cua/pull/3318) remains in the no-VM review
queue. Its exact head is green, but it has no human review. A Fleet slot becomes
useful only if focused review identifies a confirmation gap.

The public Fleet pool guide documents Linux and Windows pool examples.[3] This
pilot covers only the Windows W1 diagnostic shape. It does not provide or replace
the logged-in, TCC-authorized macOS Lume harness, and it does not certify any
Linux or macOS lane.

## Evidence authority and batching rules

Apply these rules before every batch:

1. Recheck the item state, head SHA, assignments, reviews, linked work,
   dependencies, and recent comments. A stale head or competing claim cancels
   the scheduled segment.
2. Pin one candidate SHA and one base SHA in the ledger before running anything.
   A GitHub rollup is inventory; the direct run and retained artifacts are the
   evidence.
3. Use focused unit, protocol, or native diagnostics while a change is still
   moving. Run a complete platform matrix once the candidate is stable and near
   readiness.
4. Never carry behavioral evidence across candidate SHAs. A rebase, repair, or
   composition creates a new candidate and invalidates prior certification for
   the changed source.
5. Preserve the contributor's pull request and authorship. VM efficiency is not
   a reason to replace external work or copy it into a maintainer branch.
6. Stop batching when a session cannot prove its class, preflight, clean
   baseline, or reset boundary.

The canonical complete commands remain:

```text
Windows: .\scripts\ci\windows\run-rust-e2e.ps1 -RequireGui
Linux:   scripts/ci/linux/run-rust-e2e.sh
macOS:   libs/cua-driver/tests/runners/macos-lume/run-all.sh
```

Windows requires an interactive user desktop. Linux requires the intended real
X11 or Wayland session. macOS requires the logged-in, TCC-authorized Lume
maintainer wrapper. Installed-browser scope also requires the repository's
standalone-browser coverage; on macOS, pass `--standalone-browser` to the Lume
wrapper.

## Session classes

A session class is part of the evidence, not a provisioning detail. Reusing an
image across classes does not make the results interchangeable.

| Class ID             | Required environment                                                                                                       | Eligible work                                       |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------- |
| `WIN-GUI`            | Interactive Windows user desktop that passes `-RequireGui`; never Session 0, locked, or disconnected                       | #3329, #3275, and the #3263/#3264/#3265 series      |
| `WIN-REVIEW`         | No desktop VM; source, checks, protocol, and error-contract review                                                         | #3318 before any focused confirmation               |
| `LX11-GNOME`         | Representative GNOME/X11 session with a live focus oracle and fixture-owned mutation oracle                                | #3237 and #3238 current-main diagnostics            |
| `LX11-CRD`           | XFCE under Chrome Remote Desktop, explicit `DISPLAY` and `XAUTHORITY`, Firefox/Snap provenance                             | #3334 only                                          |
| `LWAYLAND-NATIVE`    | Native Wayland compositor lane with compositor-specific identity and focus oracles                                         | Future #2206 Wayland slice after contract decisions |
| `LXWAYLAND-ROOTLESS` | Live rootless XWayland topology; not ordinary X11 and not native Wayland/Sway                                              | #3295 acceptance                                    |
| `MAC-LUME-BROWSER`   | Logged-in, TCC-authorized Lume guest with Retina capture, existing Chrome, focus sentinel, and standalone-browser coverage | #3331                                               |
| `RELEASE-SOLO`       | Fresh release-candidate environments with no preceding candidate segments                                                  | #3269 only after its failed check is resolved       |
| `SECURITY-PRIVATE`   | Authorized private security process and private evidence destination                                                       | #2016; excluded from public batches                 |

## Reset checkpoints

Use two reset levels:

- **Light reset** stays within one candidate SHA and one session class. Stop the
  driver and fixture processes, clear candidate-owned temporary artifacts,
  create fresh session labels and fixture reset tokens, restore the foreground
  sentinel, and rerun the strict environment preflight. Record every retained
  package, profile, permission, account, portal grant, and compositor setting.
- **Full snapshot revert** returns to a named, verified clean snapshot and reruns
  provisioning and strict preflight. Use it between different candidate SHAs
  only where the environment documents and proves that operation,
  after an unrecoverable or intentionally corrupted session, after install,
  upgrade, rollback, account, permission, browser-profile, portal, compositor,
  or remote-desktop mutation, and whenever contamination cannot be disproved.

A session-class change requires the image or snapshot that defines the new
class, not a light reset. Public Fleet claim release is not a documented full
snapshot revert; W1 uses the clean-boundary acceptance gate above. Release
certification starts from a fresh `RELEASE-SOLO` environment and runs alone.
Record the proven boundary identity and time in both adjacent candidate ledgers.

## Windows lane

### Session and daemon lifecycle

- [#3329](https://github.com/trycua/cua/issues/3329) is open, unassigned, has no
  comments or linked fixing PR, and needs evidence on current main
  `737dc2a069528abadee67526d138a907e1c52061`. The reporter is
  `bzthm964yk-dotcom`. Readiness: **Needs evidence**. Evidence: **Standard**.
  Reproduce the shared-daemon/per-client-shim lifecycle with focused Windows
  protocol tests. The complete Windows GUI harness is required only if a future
  fix changes GUI-session ownership.
- [#3318](https://github.com/trycua/cua/pull/3318), by `injaneity`, remains open
  and non-draft at `bbebabcd59090e7e7f64b4e9bdb937a3ba8db3ce`. Its current
  rollup has 29 successes, two skips, no failures, and no human review.
  Readiness: **Ready for maintainer review**, not merge. Evidence: **Deep**.
  Review the same-SID named-pipe error and CLI exit contract without a VM first;
  add focused reachable, absent, and inaccessible endpoint confirmation only if
  review finds a gap.

### Discovery and transport

- [#3275](https://github.com/trycua/cua/pull/3275), by `injaneity`, remains open
  and non-draft at `1429831d5a27246ea241f55b4f0f991884e43f5b`. Preserve their
  ownership and existing maintainer coauthor credit. Readiness: **Blocked**.
  Evidence: **Deep**. The direct exact-head Windows E2E run failed shared
  Electron/Tauri and native WPF/WinUI3/WebView2 while installer/update and
  capture/desktop-scope passed. Diagnose both failing owners, then run the
  complete Windows harness only at a repaired stable exact SHA.

### Accessibility and tree completeness

- [#3263](https://github.com/trycua/cua/pull/3263), by Ethan Blake, is an open
  draft at `c5cabe28e1d6c9ab7bdec280ea531a222653f537` for effective enabled
  state. Readiness: **Needs confirmation**. Evidence: **Standard**.
- [#3264](https://github.com/trycua/cua/pull/3264), by Ethan Blake, is open and
  non-draft at `15d7e63f7b99beecc76eb333639692310f81fb36`, with 23 successful
  checks, one skip, and an exact-head approval from `injaneity`. Readiness:
  **Needs confirmation**. Evidence: **Standard**. The PR body still identifies
  native WPF replay as pending.

### Coordinates and input

- [#3265](https://github.com/trycua/cua/pull/3265), by Ethan Blake, is an open
  draft at `359d5d98beacecf0a79ff4841e548486e1fdfcab` for Windows window-state
  coordinate semantics. Readiness: **Needs confirmation**. Evidence:
  **Standard**.

The three UIA candidates may reuse one `WIN-GUI` image only after a maintainer
decides their landing order and acceptance bar. Keep separate SHA-specific
segments and fully revert between them. Focused WPF/UIA diagnostics come first;
the complete Windows harness runs only for each stable candidate that is
actually approaching readiness. One sibling's result does not certify another.

### Proposed Windows batches

- **`W0-REVIEW`:** #3318 source and contract review, with no VM reservation.
- **`W1`:** #3329 current-main reproduction, a proven fresh-sandbox boundary,
  then #3275 exact-head shared and native diagnostics. This is the recommended
  first public Fleet pilot if selected and billable execution is approved.
- **`W2-UIA`:** #3263, #3264, and #3265 as separate, full-revert-delimited
  segments after the landing-order decision. Draft or soon-to-be-rebased heads
  receive focused diagnostics, not premature certification.

If selected, the Cua open-source maintainer remains accountable for `W0`, `W1`,
and `W2`. Route the #3329 reproduction segment to the Cua Driver dogfooder and
the concrete #3275 or UIA diagnostic segments to the software engineer. Those
execution handoffs do not transfer contributor ownership or authorize a fix.

## Linux X11 lane

### Discovery and transport

- [#3334](https://github.com/trycua/cua/issues/3334), reported by `coleopter`,
  is open, unassigned, and has no linked fixing PR. Readiness: **Needs
  evidence**. Evidence: **Standard**. Its XFCE-under-Chrome-Remote-Desktop
  topology, `DISPLAY`, `XAUTHORITY`, Firefox process/window identity, and Snap
  provenance define `LX11-CRD`; ordinary X11 cannot substitute. Reproduce on
  current main `737dc2a069528abadee67526d138a907e1c52061`, then require the
  complete Linux harness at any stable fixing SHA. Add standalone-browser
  coverage if the fix changes installed-browser behavior.

### Coordinates and input

- [#3237](https://github.com/trycua/cua/issues/3237), reported by Evgeny Zotov,
  remains open and unassigned with no linked fixing PR. Readiness: **Needs
  evidence**. Evidence: **Standard**. Pin current main and use a representative
  GNOME/X11 session with a GTK fixture-owned pixel mutation oracle to compare
  screenshot and input frames. A stable fix requires the complete Linux X11
  harness.

### Focus and window state

- [#3238](https://github.com/trycua/cua/issues/3238), also reported by Evgeny
  Zotov, remains open and unassigned with no linked fixing PR. Readiness:
  **Needs evidence**. Evidence: **Inventory**. Pin the same current-main SHA and
  use a representative GNOME/X11 session with an external active-window oracle
  to verify launch response, focus, and restoration. A stable fix requires the
  complete Linux X11 harness.

### Proposed Linux X11 batches

- **`LX1-GNOME`:** at one pinned current-main SHA, diagnose #3237 first, perform
  a light reset, then diagnose #3238. The two issues share an image and SHA, not
  implementation scope. Separate ledger rows and oracles remain mandatory.
- **`LX2-CRD`:** #3334 alone in `LX11-CRD`. Its remote-desktop, browser package,
  and authentication state are incompatible with `LX11-GNOME`.

If selected, the Cua open-source maintainer remains accountable and the Cua
Driver dogfooder owns the environment reproduction. A software-engineer handoff
begins only after reproduction identifies a bounded implementation or test gap.

## Linux Wayland and rootless XWayland lane

### Discovery, transport, focus, and window state

- [#3295](https://github.com/trycua/cua/pull/3295), by `injaneity`, remains an
  open draft at `0e7470a7f4a9d133d2dfecf0bf351b1ca41f2e84`, with 22 successful
  checks, one skip, and an exact-head approval from `f-trycua`. Readiness:
  **Blocked by acceptance evidence**. Evidence: **Standard**. Preserve the
  external contribution and its lineage. Run it alone as **`LW1-ROOTLESS`** in
  a live `LXWAYLAND-ROOTLESS` session that proves exact PID/window identity,
  bounded budget refusal, and no regression to ordinary EWMH and native-Wayland
  selection. Green unit and Nix checks do not prove that topology.

Native Wayland/Sway evidence does not certify rootless XWayland, and rootless
XWayland evidence does not certify native Wayland or ordinary X11. The live
rootless-XWayland acceptance is a separate, required gate. The current
repository guidance does not name rootless XWayland as a canonical complete
lane, so also run the selector-free Linux harness in its supported canonical
session for a stable candidate, and record the two results without treating
either session class as a substitute for the other.

If selected, the Cua open-source maintainer remains accountable for
`LW1-ROOTLESS`; the Cua Driver dogfooder owns the live acceptance environment.

## macOS lane

### Accessibility, coordinates, input, focus, and window state

- [#3331](https://github.com/trycua/cua/issues/3331), reported by
  `r33drichards`, remains open and unassigned with no linked fixing PR.
  Readiness: **Needs evidence**. Evidence: **Inventory**. Run **`M1-BROWSER`**
  alone on current main `737dc2a069528abadee67526d138a907e1c52061` in
  `MAC-LUME-BROWSER`. The reproduction must target an already-running Chrome
  profile, enforce background-only routing, use a foreground sentinel, and
  check AX completeness, Retina screenshot/action coordinates, postconditions,
  and focus preservation. Any stable fix requires
  `libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser` at
  its exact SHA.

TCC grants, browser profiles, and active-app state are persistent evidence
inputs. Fully revert before another candidate uses the Lume guest.

If selected, the Cua open-source maintainer remains accountable for
`M1-BROWSER`; the Cua Driver dogfooder owns the reproduction and exact
environment ledger.

## Cross-platform lane

### Focus and window state

- [#2206](https://github.com/trycua/cua/issues/2206), reported by Francesco
  Bonacci, remains the broad foreground action-scoped activation and restoration
  contract. Readiness: **Blocked**. Evidence: **Inventory**. It needs a recorded
  public-contract decision or RFC and platform slices before implementation.
  Do not batch one implementation across Windows, macOS, X11, and Wayland. Each
  accepted slice needs its native session class and exact-SHA harness; the final
  stable cross-platform candidate needs the complete affected-platform matrix.

### Accessibility and tree completeness

- [#3307](https://github.com/trycua/cua/pull/3307), by `Wangxiaoxiaoa`, remains
  open at `eb49aa5a1db35f135bdf8265c7a5204b00f9399e` with changes requested.
  Readiness: **Blocked**. Evidence: **Inventory**. Maintainer review requires a
  linked issue or RFC for the public output change, a focused shared
  cross-platform scope, and native Windows, macOS, and Linux evidence. It stays
  in **`X0-DECISION`**, which uses no VM, until those gates are resolved.

Cross-platform grouping describes contract ownership. It does not authorize a
shared branch, hide platform limitations, or let one platform's result certify
another.

## Separately held security and release work

### Private security hold

- [#2016](https://github.com/trycua/cua/pull/2016) remains open and merge-dirty
  at `4f33280c88bc89e012f8e6c7117cab56ed3f292d`, with a failing contributor
  attribution check and no review. Readiness: **Private-security review hold**.
  Evidence: **Deep**. Keep it outside ordinary public batching as
  **`S-PRIVATE`**. This document intentionally includes no exploit details,
  private observations, or public execution plan.

### Packaging and release

- [#3269](https://github.com/trycua/cua/pull/3269), the release-bot Cua Driver
  0.22.0 PR, remains open at
  `02070849730d8d6b080f1de4f3739dcac809d04e` with one failed `test` check and
  no review. Readiness: **Blocked**. Evidence: **Standard**. Triage the failed
  check before provisioning. Any later exact release candidate runs as
  **`R-SOLO`** on fresh, isolated release environments with no other candidate
  before or after it. Release publication remains separately authorized work.

## Compatibility matrix

`Main@737dc2a` means a focused reproduction pinned to full main SHA
`737dc2a069528abadee67526d138a907e1c52061`, not a fixing candidate.

| Item                                               | OS                | Area                  | Contributor or reporter | Candidate                                  | Session class        | Persistent mutation                      | Reset                             | Gate                                                                        | Batch          | Blocker                                     | Evidence  |
| -------------------------------------------------- | ----------------- | --------------------- | ----------------------- | ------------------------------------------ | -------------------- | ---------------------------------------- | --------------------------------- | --------------------------------------------------------------------------- | -------------- | ------------------------------------------- | --------- |
| [#3329](https://github.com/trycua/cua/issues/3329) | Windows           | Session/daemon        | `bzthm964yk-dotcom`     | Main@737dc2a                               | `WIN-GUI`            | Daemon, shim, labels, processes          | Proven fresh sandbox after        | Focused protocol first; Windows complete only if GUI ownership changes      | `W1`           | Current-main reproduction                   | Standard  |
| [#3318](https://github.com/trycua/cua/pull/3318)   | Windows           | Session/daemon        | `injaneity`             | `bbebabcd59090e7e7f64b4e9bdb937a3ba8db3ce` | `WIN-REVIEW`         | None for review                          | None                              | Maintainer review; focused pipe confirmation if needed                      | `W0-REVIEW`    | No human review                             | Deep      |
| [#3275](https://github.com/trycua/cua/pull/3275)   | Windows           | Discovery/transport   | `injaneity`             | `1429831d5a27246ea241f55b4f0f991884e43f5b` | `WIN-GUI`            | Builds, apps, browser/listener processes | Proven fresh sandbox before/after | Failed-owner diagnostics; complete Windows at repaired SHA                  | `W1`           | Exact-head shared and native E2E failures   | Deep      |
| [#3263](https://github.com/trycua/cua/pull/3263)   | Windows           | Accessibility/enabled | Ethan Blake             | `c5cabe28e1d6c9ab7bdec280ea531a222653f537` | `WIN-GUI`            | Builds and UIA fixtures                  | Full between SHAs                 | Focused WPF/UIA; complete Windows when stable                               | `W2-UIA`       | Draft; landing order                        | Standard  |
| [#3264](https://github.com/trycua/cua/pull/3264)   | Windows           | Tree completeness     | Ethan Blake             | `15d7e63f7b99beecc76eb333639692310f81fb36` | `WIN-GUI`            | Builds and UIA fixtures                  | Full between SHAs                 | Native WPF confirmation; complete Windows when stable                       | `W2-UIA`       | Landing order and production error coverage | Standard  |
| [#3265](https://github.com/trycua/cua/pull/3265)   | Windows           | Coordinates/input     | Ethan Blake             | `359d5d98beacecf0a79ff4841e548486e1fdfcab` | `WIN-GUI`            | Builds and UIA fixtures                  | Full between SHAs                 | Focused coordinate oracle; complete Windows when stable                     | `W2-UIA`       | Draft; landing order                        | Standard  |
| [#3237](https://github.com/trycua/cua/issues/3237) | Linux X11         | Coordinates/input     | Evgeny Zotov            | Main@737dc2a                               | `LX11-GNOME`         | Fixture/window geometry                  | Light within same SHA; full after | Focused GTK oracle; complete Linux X11 at fix SHA                           | `LX1-GNOME`    | Current-main source validation              | Standard  |
| [#3238](https://github.com/trycua/cua/issues/3238) | Linux X11         | Focus/window state    | Evgeny Zotov            | Main@737dc2a                               | `LX11-GNOME`         | Launched apps and focus                  | Full after                        | Focused focus oracle; complete Linux X11 at fix SHA                         | `LX1-GNOME`    | Current-main source validation              | Inventory |
| [#3334](https://github.com/trycua/cua/issues/3334) | Linux X11         | Discovery/transport   | `coleopter`             | Main@737dc2a                               | `LX11-CRD`           | CRD session, browser, Xauthority         | Full                              | Exact-topology repro; complete Linux plus browser scope at fix SHA          | `LX2-CRD`      | Exact environment validation                | Standard  |
| [#3295](https://github.com/trycua/cua/pull/3295)   | Rootless XWayland | Discovery/focus       | `injaneity`             | `0e7470a7f4a9d133d2dfecf0bf351b1ca41f2e84` | `LXWAYLAND-ROOTLESS` | Compositor and XWayland topology         | Full                              | Live rootless acceptance plus complete Linux in a supported canonical class | `LW1-ROOTLESS` | Accepted live fixture/environment           | Standard  |
| [#3331](https://github.com/trycua/cua/issues/3331) | macOS             | AX/coordinates/focus  | `r33drichards`          | Main@737dc2a                               | `MAC-LUME-BROWSER`   | TCC, Chrome profile, focus               | Full                              | Lume complete plus standalone browser at fix SHA                            | `M1-BROWSER`   | Current-source reproduction                 | Inventory |
| [#2206](https://github.com/trycua/cua/issues/2206) | Cross-platform    | Focus/window state    | Francesco Bonacci       | None                                       | Per-platform         | Contract and desktop state               | Full per platform                 | RFC/decision, slices, then affected complete matrices                       | `X0-DECISION`  | Public contract and platform slicing        | Inventory |
| [#3307](https://github.com/trycua/cua/pull/3307)   | Cross-platform    | Accessibility/output  | `Wangxiaoxiaoa`         | `eb49aa5a1db35f135bdf8265c7a5204b00f9399e` | Per-platform         | Builds and native AX providers           | Full per platform                 | Issue/RFC, shared tests, and native three-platform evidence                 | `X0-DECISION`  | Changes requested                           | Inventory |
| [#2016](https://github.com/trycua/cua/pull/2016)   | Held              | Private security      | `ddupont808`            | `4f33280c88bc89e012f8e6c7117cab56ed3f292d` | `SECURITY-PRIVATE`   | Private                                  | Private process                   | Private review only                                                         | `S-PRIVATE`    | Security hold, conflicts, attribution       | Deep      |
| [#3269](https://github.com/trycua/cua/pull/3269)   | Release           | Packaging/release     | Cua release bot         | `02070849730d8d6b080f1de4f3739dcac809d04e` | `RELEASE-SOLO`       | Full install/release state               | Fresh and solo                    | Fix failed check; full release certification at later SHA                   | `R-SOLO`       | Failed `test` check and no review           | Standard  |

## Anti-batching conditions

Do not place items in one reusable session segment when any of these conditions
applies:

- the candidate SHAs differ and no full snapshot revert separates them;
- the session classes differ, including GNOME/X11 versus XFCE/CRD, X11 versus
  native Wayland, or native Wayland versus rootless XWayland;
- account, SID, TCC, browser profile, portal grant, package, installer,
  compositor, remote-desktop, or persistent daemon state might cross the
  boundary;
- a failure intentionally leaves a daemon, shim, session, fixture, desktop, or
  browser in a corrupted or indeterminate state;
- the candidate is draft, likely to rebase, missing a landing-order decision, or
  lacks a stable acceptance oracle, and the proposed run is expensive
  certification rather than a focused diagnostic;
- private security scope would enter public logs, artifacts, or scheduling;
- the work is release certification, which must run alone;
- the proposed ledger would combine contributors, branches, SHAs, or direct-run
  evidence; or
- the environment can provide only a rollup, one-off smoke, or historical run
  instead of the required exact-source evidence.

## Per-candidate evidence ledger

Record at least these fields for every candidate segment:

- issue or pull request, title, OS lane, area, contributor or reporter, and
  accountable test owner;
- full candidate SHA, base SHA, branch or pull-request head, and revalidation
  time;
- session class, OS and window-system versions, VM image, clean snapshot ID, and
  preflight result;
- reset level before and after, snapshot-revert time, processes stopped, and
  state intentionally retained;
- exact command, focused diagnostic or complete gate, direct run URL, job names,
  start and end times, and artifact locations;
- expected behavior, observed behavior, external oracle, result, and structured
  refusal where applicable;
- packages, accounts, permissions, profiles, portal grants, display variables,
  compositor settings, daemon labels, and other persistent mutations;
- check, review, and blocker state; evidence level; remaining gap; next action;
  and owner; and
- the explicit scope statement: “This evidence applies only to candidate SHA
  `<SHA>` in session class `<CLASS>`.”

## Decisions required from Francesco

1. Select or decline **`W1`** as a public Fleet pilot and, separately, approve or
   withhold its billable Phase 1 gates: priority current-main #3329 reproduction,
   a proven fresh-sandbox boundary, then #3275 exact-head shared and native
   diagnostics.
2. Confirm whether #3318 should enter formal maintainer review now. Keep it out
   of the VM queue unless that review identifies a focused confirmation gap.
3. Decide the landing order and acceptance bar for #3263, #3264, and #3265
   before provisioning **`W2-UIA`** or spending a complete-harness run.
4. Decide whether #3295 requires an automated live rootless-XWayland fixture or
   may use a named, reproducible representative environment.
5. Decide the public-contract/RFC boundary and platform slicing for #2206 and
   #3307 before any cross-platform implementation or native VM allocation.
6. Confirm that #2016 remains in the private security process and that #3269
   release certification remains isolated and separately authorized.

These are selection and acceptance decisions. Until they are recorded, the
batches remain planning advice rather than scheduled work.

## Validation limits

This document uses current repository guidance, source inspection, and live
GitHub metadata, checks, reviews, heads, and direct action-run evidence. It does
not rerun Windows, Linux, or macOS desktop behavior. No backlog issue, pull
request, assignment, label, review, merge, release, or workflow was mutated to
produce this plan.

## Sources

[1] https://cua.ai/docs/concepts/how-sandboxes-work — How sandboxes work | Cua docs
[2] https://cua.ai/docs/how-to-guides/sandbox/lifecycle — Sandbox lifecycle | Cua docs
[3] https://cua.ai/docs/how-to-guides/sandbox/configure-pool-with-terraform — Configure a sandbox pool with Terraform | Cua docs
[4] https://cua.ai/docs/how-to-guides/sandbox/create-pool-with-python — Create a sandbox pool with Python | Cua docs
[5] https://cua.ai/docs/how-to-guides/sandbox/scale-out — Run sandboxes in parallel | Cua docs
