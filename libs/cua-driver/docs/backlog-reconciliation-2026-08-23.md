# Cua Driver backlog reconciliation, 2026-08-23

Status: point-in-time, read-only maintainer recommendation

This report reconciles the historical backlog review recovered from commit
`3251536e645fa148c7cd9bfdba42575f47a1a19c` with `trycua/cua` at
2026-08-23 02:14 CDT. The historical document is context only: every issue,
pull request, check, review, and merge statement below was rechecked against
GitHub or current `origin/main`.

## Evidence scale

- **Deep**: current item, files or diff, checks, reviews, and relevant source or
  issue evidence were inspected.
- **Standard**: current item, state, head, checks, reviews, claims, and competing
  work were inspected, but no complete native behavior replay was performed.
- **Inventory**: current state and repository containment were checked without a
  fresh behavioral or line-by-line source review.

## Current repository state

- Repository: [`trycua/cua`](https://github.com/trycua/cua)
- Base and current revision: `737dc2a069528abadee67526d138a907e1c52061`
- Report branch: `bot/cua-driver-backlog-resume-2026-08-23`
- Live inventory at 2026-08-23 02:09 CDT: 340 open issues and 361 open pull
  requests. Their sum, 701, matched GitHub's repository open-item count.
- Available validation in this reconciliation: source inspection, current
  GitHub metadata, checks and reviews. No Windows, Linux, or macOS desktop E2E
  was rerun; historical native evidence is not promoted to current evidence.

## Recommended decision surface

An open issue remains intake, not selected work. The ordering applies impact and
readiness gates before age or popularity.

### 1. Review the narrow Windows daemon-status fix in #3318

- Item: [#3318](https://github.com/trycua/cua/pull/3318) at exact head
  `bbebabcd59090e7e7f64b4e9bdb937a3ba8db3ce`.
- Readiness: **Ready for maintainer review**, not merge. Evidence: **Deep**.
- Impact: `cua-driver status` currently can call a named pipe "running" after
  detecting only that the pipe exists, even when the caller's Windows account
  cannot open it. The two-file change requires a real daemon protocol request
  and documents the same-SID boundary.
- Current state: open, non-draft, 29 successful checks, two skips, no failed
  checks, no human review, and no unresolved requested changes observed.
- Evidence: the PR records same-account and different-account OpenSSH replay;
  the diff keeps the account-private pipe policy and changes status reporting
  rather than weakening access.
- Smallest next action: perform focused maintainer review of the error path and
  CLI exit contract, then require a deterministic status test for absent,
  reachable, and inaccessible endpoints if current coverage does not exercise
  those branches.
- Risk: the body reports live evidence but no local focused Cargo run on the
  Windows workspace.

### 2. Reproduce the unrecoverable Windows MCP session state in #3329

- Item: [#3329](https://github.com/trycua/cua/issues/3329), reported against Cua
  Driver 0.21.0 on Windows with a shared daemon and per-client MCP shims.
- Readiness: **Needs evidence**. Evidence: **Standard**.
- Impact: the report says all calls become permanently `session_ended` after an
  MCP shim exits and that `start_session` reports success without restoring the
  next call. The only successful recovery was replacing the shim process.
- Current claim state: open, unassigned, no comments, no linked or competing
  pull request found.
- Smallest next action: reproduce on current `main` while binding the exact MCP
  shim, daemon, session label, owner disconnect, and next-call generation. Add a
  deterministic transport/session test before selecting implementation.
- Validation fit: Windows protocol tests first; canonical Windows desktop E2E is
  needed only if the fix crosses into GUI session ownership.
- Risk: the symptom is severe, but one release report is not yet proof of the
  current root cause.

### 3. Repair and recertify the Windows listener contribution in #3275

- Item: [#3275](https://github.com/trycua/cua/pull/3275) at exact head
  `1429831d5a27246ea241f55b4f0f991884e43f5b`, linked to
  [#3251](https://github.com/trycua/cua/issues/3251).
- Readiness: **Blocked**. Evidence: **Deep**.
- Impact: replaces locale-dependent `netstat` parsing with native IPv4/IPv6 TCP
  owner tables while preserving loopback, allowed-PID, process-generation, and
  ambiguity checks.
- Contributor ownership: preserve `injaneity` as the contribution author and
  retain existing coauthor credit on maintainer-added hardening tests.
- Current state: open, non-draft, no review. At the exact head, 31 checks
  succeeded, two skipped, and the canonical Windows shared Electron/Tauri and
  native WPF/WinUI3/WebView2 jobs failed.
- Smallest next action: inspect those exact-head failure logs, determine whether
  they are product, fixture, or infrastructure failures, fix or rerun only with
  evidence, then obtain review. Do not rely on earlier-head retries.
- Risk: the source direction and malformed-table bounds are coherent, but green
  prior-head evidence cannot override red exact-head native lanes.

### 4. Reconcile the Windows UIA completeness series around #3264

- Item: [#3264](https://github.com/trycua/cua/pull/3264) at exact head
  `15d7e63f7b99beecc76eb333639692310f81fb36`, with sibling drafts
  [#3263](https://github.com/trycua/cua/pull/3263) and
  [#3265](https://github.com/trycua/cua/pull/3265).
- Readiness: **Needs confirmation**. Evidence: **Standard**.
- Impact: makes `elements_complete` truthful for bounded Windows UIA traversal;
  this affects whether callers may treat absence as definitive.
- Contributor ownership: preserve Ethan Blake's commits and authorship across
  any dependency-order rebase or composition.
- Current state: open, non-draft, 23 successful checks, one skip, and an
  exact-head approval from `injaneity`. The PR body still says native WPF replay
  is pending, while #3263 and #3265 remain separate drafts.
- Smallest next action: a maintainer must confirm landing order and whether
  #3264's current tests exercise production provider-read failures rather than
  only synthetic walk state. Rebase and rerun affected exact-head Windows
  evidence only after that decision.
- Risk: merging one sibling independently could publish a completeness signal
  without the safety and coordinate semantics expected by the series.

### 5. Keep the rootless XWayland contribution in draft until live acceptance

- Item: [#3295](https://github.com/trycua/cua/pull/3295) at exact head
  `0e7470a7f4a9d133d2dfecf0bf351b1ca41f2e84`.
- Readiness: **Blocked by acceptance evidence**. Evidence: **Standard**.
- Impact: bounded fallback discovery for rootless XWayland sessions whose EWMH
  client lists are empty.
- Contributor ownership: preserve the existing external contribution and its
  recorded lineage.
- Current state: open draft, 22 successful checks, one skip, and an exact-head
  maintainer approval.
- Smallest next action: provide a live rootless-XWayland fixture or accepted
  representative replay showing exact PID/window identity, budget exhaustion
  refusal, and no regression to ordinary EWMH and native-Wayland selection.
- Risk: green unit and Nix checks do not prove the compositor topology that
  motivated the fallback.

## Important blocked items and risks

- [#2016](https://github.com/trycua/cua/pull/2016) remains a **private-security
  review hold**. Current state is open, merge-dirty, unchanged at
  `4f33280c88bc89e012f8e6c7117cab56ed3f292d`, attribution-failing, and
  unreviewed. Do not conflict-repair or merge it as ordinary backlog work. Any
  specific authorization, private-observation, or output-destination defect
  belongs in the private process; this public report intentionally does not
  restate exploit details. Evidence: **Deep**.
- [#3269](https://github.com/trycua/cua/pull/3269), the Cua Driver 0.22.0 release
  PR, is blocked with one failed `test` check and no review. Release publication
  is separately authorized work and is not part of this reconciliation.
  Evidence: **Standard**.
- [#2206](https://github.com/trycua/cua/issues/2206) remains the broad
  cross-platform foreground-activation/restoration contract. It is too broad
  for immediate implementation without platform slices and exact focus oracles.
  [#3238](https://github.com/trycua/cua/issues/3238) is a bounded Linux launch
  slice; [#3331](https://github.com/trycua/cua/issues/3331) is new macOS intake
  that still needs current-source validation. Evidence: **Inventory**.
- [#3237](https://github.com/trycua/cua/issues/3237) remains an unclaimed Linux
  screenshot/click coordinate mismatch with no linked fixing PR. The report is
  reproducible-looking, but current-main source validation and a GTK4
  fixture-owned mutation oracle are still required. Evidence: **Standard**.
- [#3334](https://github.com/trycua/cua/issues/3334) is new Linux X11/Chrome
  Remote Desktop intake. It reports that fresh `list_windows` identities are
  immediately rejected as stale, but has no claim or linked PR. Validate the
  exact display, Xauthority, Firefox process/window identity, and capture path
  before ranking it above confirmed work. Evidence: **Standard**.

## Historical items no longer active

These historical conclusions were replaced with current evidence:

- [#3196](https://github.com/trycua/cua/issues/3196) is closed, and current main
  contains the narrow Windows executable-path correction from #3299 at
  `a68289551`.
- [#3257](https://github.com/trycua/cua/pull/3257),
  [#3280](https://github.com/trycua/cua/pull/3280),
  [#3291](https://github.com/trycua/cua/pull/3291), and
  [#3293](https://github.com/trycua/cua/pull/3293) are merged.
- [#3037](https://github.com/trycua/cua/pull/3037),
  [#3277](https://github.com/trycua/cua/pull/3277), and
  [#2888](https://github.com/trycua/cua/pull/2888) are contained in current
  main; they are not backlog candidates.
- [#3172](https://github.com/trycua/cua/pull/3172) is closed. Current successor
  [#3307](https://github.com/trycua/cua/pull/3307) is open with changes
  requested, so the structured AT-SPI action field is not ready.
- Historical heads, approvals, check runs, and release claims not listed above
  remain historical only and were not used to rank current work.

## Next bounded workstream

The top bounded execution candidate is a **maintainer review of #3318** at exact
head `bbebabcd59090e7e7f64b4e9bdb937a3ba8db3ce`. It is the smallest current,
reviewable correction with a clear user-facing failure, a two-file scope, live
Windows evidence, and green exact-head checks. The accountable owner should be
the Cua open-source maintainer; Windows test follow-up can be routed to the
software engineer only after review identifies a concrete gap.

In parallel, #3329 should be routed to the Cua Driver dogfooder for reproduction,
not selected for implementation yet. A current-main reproduction would promote
it above #3318 because an unrecoverable multi-client session is higher impact.

## Decisions required

1. Confirm whether to select #3318 for formal maintainer review.
2. Confirm whether #3329 should receive priority Windows reproduction before
   other Driver implementation work.
3. Decide the landing order and acceptance bar for the #3263/#3264/#3265 Windows
   UIA series.
4. Decide whether #3295 requires an automated rootless-XWayland fixture or may
   proceed with a named, reproducible representative environment.
5. Keep #2016 in private security review rather than ordinary public review.

## Validation and limitations

- Recovered historical source with `git show` and verified its SHA-256 as
  `0f455c6541b0434dfd8e7b980049ae3d641af0c811342d7350fea8151463b09a`.
- Fetched `origin/main` before and after evidence collection; it remained
  `737dc2a069528abadee67526d138a907e1c52061`.
- Queried issue and pull-request totals independently and reconciled them to the
  repository open-item count.
- Queried focused current issue/PR state, exact heads, files, checks, reviews,
  comments, draft state, and mergeability. Inspected the #3275 and #3318 diffs.
- Checked current component history for the merged historical corrections.
- No GitHub issue, pull request, label, assignment, comment, review, merge,
  release, or workflow was mutated during reconciliation.
- No native desktop matrix was run. Readiness statements are therefore review
  recommendations, not merge or release certification.
