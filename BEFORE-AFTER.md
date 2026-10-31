# CI fixes: before and after

Measured on `trycua/cua` at `33a57bb0` (main, 2026-10-31). Raw data and the
script that produces every number here are in `measure/`.

```bash
python3 measure/fetch.py     # pull runs, jobs, logs from the Actions API (~33 MB, git-ignored)
python3 measure/measure.py   # print the table below + the evidence behind it
```

`measure/data/logs/` and `measure/data/evidence.json` are committed, so
`measure.py` reproduces the published numbers without re-fetching.

## The table

| metric | before | after | reduction | label |
|---|---|---|---|---|
| sccache hit rate, `libs-cua-driver-rust` | **0.26 %** (4 / 1563) | — | — | measured / n/a |
| sccache hit rate, `libs-cua-spacesd` | **97.71 %** | — | — | measured / n/a |
| `gate / Linux E2E / shared Electron + Tauri` | **34.55 min** | 20.47 min | **40.8 %** | measured / **projected** |
| `ci-gate` job duration | 2.42 min (median) | 2.42 min | 0.0 % | measured / measured |
| `ci-gate` timeout cap | 360 min | 180 min | **50 %** | measured / measured |
| heavy PR wall-clock (median) | 25.10 min | 25.10 min | 0.0 % | measured / unchanged |
| dependabot PR wall-clock (median) | 3.65 min | 3.65 min | 0.0 % | measured / unchanged |

## What is actually measured, and what is not

Only **one** row is a real before/after from this PR: the `ci-gate` timeout cap,
360 → 180 minutes. That is a ceiling on a wedged run, not a saving on a normal
PR — the worst gate observed across 98 runs was **94.8 min**, so 180 keeps ~47 %
headroom while stopping a hang from holding a required check for six hours.

Everything else is context:

- **The 40.8 % on the E2E leg is a projection, and it is not caused by this PR.**
  The arithmetic is real: the job executes 1,563 compiles with 4 cache hits at
  1.843 s each = 2,873 s of compiler work, which over the 3.4 effective cores
  `ci_cache_audit.py` assumes is ~14.1 min off a 34.55 min job. But that number
  is the ceiling *if the cache worked*, and this PR does not make it work. The
  job restores the family as an **exact hit** (373 MB, 405 files) and then misses
  1,542 times, which rules out "the cache is not seeded" as the cause. Do not
  quote 40.8 % as a result of this PR.
- **`libs-cua-spacesd` is already fixed on main.** `ci-cua-spacesd.yml:36` has
  carried `push: branches:[main]` before this PR; 14 push runs have used it, and
  Linux jobs hit **98.9 %** (4,083 / 4,129) and macOS **96.5 %**. The audit's
  0.14 % figure predates that trigger.
- **The two PR wall-clock rows are unchanged on purpose.** This PR does not touch
  the fast or the heavy path.

## What this PR changes

**1. Nine `save-if` guards** (`.github/workflows/e2e-rust-linux.yml`,
`e2e-rust-windows.yml`). The old guard was
`github.ref_type == 'branch' && needs.source.outputs.sha == github.sha`; on a
pull request `ref_type` is also `branch`, so it never gated a PR save.

The audit's diagnosis needs one correction. For eight of the nine sites the
guard was **inert**: `.github/actions/rust-cache` decides `save` itself
(`refs/heads/main`, `workflow_dispatch`, or `pr-save`) and only drops a save when
`save-if` is the literal `"false"`. Verified against the live cache inventory:
of 376 entries, **zero** sccache entries exist under `refs/pull`, so no PR was
writing one. Those eight lines are now
`github.event_name != 'pull_request'`, which preserves the documented
`workflow_dispatch` path (the action's own comment calls it "how a change to a
workflow's cache is measured before it merges") while making the intent explicit.

The ninth site, `e2e-rust-linux.yml:113`, is `Swatinem/rust-cache`, which *does*
honour `save-if` itself — and it is the only site that really wrote PR-scoped
data (`refs/pull/5957/merge`, 1006 MB, key `v0-rust-e2e-shared-verify-skill-*`).
It now reads `github.ref == 'refs/heads/main' || github.event_name == 'workflow_dispatch'`.

**2. `ci-gate` timeout 360 → 180 minutes.** The only change with a measured
before/after.

## What was dropped, and why

These were in the audit's list. Each is either already done or would regress
something, so none of them ship:

| proposed | why not |
|---|---|
| add `push:` to `ci-cua-spacesd.yml` | already on main (line 36) since before this analysis; 14 push runs, 98.9 % hits |
| `ci-gate` `sleep 120` → 20 s | **unsafe.** Run-creation fan-out across 37 measured bursts: p90 11 s, p95 101 s, p99 119 s, **max 119 s** — the window is sized to that. At 20 s the first verdict lands ~30 s in, and 3 of 37 bursts still had runs appearing after that, so the two-identical-listings rule would confirm an incomplete set and verdict early. `test_gate_catches_a_failing_run_that_appears_inside_the_window` exists precisely to catch this. |
| cap `poll_interval()` 60 s → 20 s | breaks the repo's own API budget: `test_gate_poll_schedule_backs_off_and_stays_within_the_api_budget` asserts `set(intervals) == {10, 30, 60}` and `len(sleeps) <= 100`. A 128-min PR would go from ~113 polls to ~339. Upside is ~40 s. |
| `concurrency:` on `ci-release-metadata.yml` | **would block merges.** `test_installer_compatibility_never_cancels_a_superseded_caller` documents why: a cancelled call fails the required `validate` check on that head, and a failed duplicate blocks the merge. The duplicates are real (4 overlapping runs across 28 heads) but cancelling them is the wrong remedy. |
| `concurrency:` on `release-on-merge.yml` | no evidence: 8 runs, 8 distinct heads, 0 duplicates. |

## Re-running this measurement after the fixes are live

The `after` columns become measurable only once the fixes are on `main` **and**
main has run the affected workflows. Then:

```bash
cd ci-fixes
python3 measure/fetch.py

# 1. cache hit rate. Pick a main run of each workflow, read the job log:
gh api repos/trycua/cua/actions/runs/<run_id>/jobs --jq '.jobs[]|"\(.id)\t\(.name)"'
gh api repos/trycua/cua/actions/jobs/<job_id>/logs \
  | grep -E "sccache enabled:|Compile requests executed|Cache hits |Cache hits rate"

# 2. E2E leg duration
gh api repos/trycua/cua/actions/workflows/383939642/runs?per_page=20 \
  --jq '.workflow_runs[]|"\(.id)\t\(.head_sha[0:8])\t\(.created_at)\t\(.conclusion)"'
gh api repos/trycua/cua/actions/runs/<run_id>/jobs?per_page=100 \
  --jq '.jobs[]|select(.name|endswith("Linux / shared Electron + Tauri"))|"\(.started_at) \(.completed_at)"'

# 3. regenerate the table
python3 measure/measure.py --md
```

To confirm the driver-rust guard took effect, check that no new
`refs/pull/*` sccache entries appear:

```bash
gh api repos/trycua/cua/actions/caches --paginate \
  --jq '.actions_caches[]|select(.key|test("sccache"))|"\(.ref)\t\(.key)"' | sort -u
```

Expect only `refs/heads/main` rows. Re-running `measure.py` after those runs will
replace the `n/a` and `projected` cells with `measured` values; until then any
`after` number for the cache families would be invented.

## If you want the bigger number

The 40.8 % is a ceiling, and the real cause sits in `libs-cua-driver-rust`
restoring an exact hit and then missing 1,542 times — a cache-key/input mismatch
between how main seeds that family (dev profile, `ci-rust-linux.yml`) and how the
E2E leg compiles (release profile, `--features portal-input`). Fixing that is a
separate change and is the one lever that would move the 34.55 min leg.
