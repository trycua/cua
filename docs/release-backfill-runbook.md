# How to backfill historical release attribution

This guide shows release owners how to generate, review, and apply retrospective
release notes for the Rust Cua Driver and Lume.

## When to use this guide

Use this guide for the one-time historical backfill. New releases use the
forward Release Please and attribution workflow.

## Before you start

- Announce a Cua Driver and Lume release freeze for the inventory and apply
  windows.
- Fetch `origin/main` and every tag.
- Use a clean branch based on current `origin/main` for generation.
- Export `GH_TOKEN` with repository read access. The apply step also needs
  release write access.
- Keep journals and Claude stream logs outside the repository.
- Do not run `apply --execute` while another release owner is editing release
  notes.

Merging the implementation pull request does not edit GitHub releases. Only an
explicit `apply --execute` command can patch a release body.

## Generate the reviewed payload

### 1. Freeze the release catalog

```bash
python3 .github/scripts/release_backfill.py inventory
```

Review every `previousTag`, `rangeKind`, `pathEraIds`, and tag SHA in
`.github/release-backfill/catalog.json`. The catalog must contain 54 published
Rust Driver releases and 128 published Lume releases for the July 17, 2026
snapshot. A later intentional snapshot can have different counts.

### 2. Collect evidence

```bash
python3 .github/scripts/release_backfill.py collect
```

Review `.github/release-backfill/skip-ledger.json`. Resolve configuration or
identity errors only when public evidence proves the correction. Keep
same-SHA, empty-range, private security, and uncertain identity cases skipped.

### 3. Generate candidate prose

Claude Code generation incurs model charges. Estimate and approve the run cost
before starting it. The default adaptive limits cap each batch at 10 releases,
30 source records, and 120,000 input bytes so complex releases stay complete:

```bash
BACKFILL_CLAUDE_MODEL=sonnet \
python3 .github/scripts/release_backfill.py author \
  --model-id claude-sonnet-5 \
  --resume \
  python3 .github/scripts/release_backfill_claude.py
```

The author command checkpoints after each batch. Rerun the same command after
an interruption. It retains validated candidates and retries agent failures.

The agent may write summaries and group changes. It cannot supply contributor
identities, tag ranges, GitHub links, or mutation fields. The validator rejects
references outside the release's evidence packet. If the evidence contains only
version bumps or internal maintenance, an evidence-backed empty change list is
recorded as `no-product-changes` and retained across resumed runs.

### 4. Render and validate

```bash
python3 .github/scripts/release_backfill.py render
python3 .github/scripts/release_backfill.py validate
```

Review these files:

- `candidates.md` for the proposed retrospective notes;
- `dry-run-report.md` for coverage, ranges, skips, and hashes;
- `skip-ledger.json` for every omitted release;
- `rendered.json` for the exact apply payload.

Review every minor release and milestone release manually. Sample routine patch
releases across both products and every path era. Move any uncertain candidate
to the review-stage skip ledger.

### 5. Run the independent audit

Ask Claude Code Fable for a read-only review of the final pull request. The
review must cover tag adjacency, path eras, identity resolution, evidence-bound
claims, mutation methods, drift detection, journal integrity, idempotency, and
rollback. Resolve every blocking finding before merge.

## Apply the pilot

Check out the exact merged commit in a clean worktree. Record its full SHA:

```bash
git rev-parse HEAD
git status --short
```

Run a dry run for the six pilot releases:

```bash
python3 .github/scripts/release_backfill.py apply \
  --approved-commit FULL_40_CHARACTER_SHA \
  --journal /absolute/path/outside/repo/release-backfill-pilot.jsonl \
  --release-key cua-driver-rs:0.8.0 \
  --release-key cua-driver-rs:0.8.1 \
  --release-key cua-driver-rs:0.8.2 \
  --release-key lume:0.3.0 \
  --release-key lume:0.3.14 \
  --release-key lume:0.3.15
```

The dry run must report six proposed body patches and make no journal or GitHub
change. Add `--execute` only after the release owner approves the exact payload.

After execution, rerun the same command with `--execute`. It must report every
release as already applied and verified without sending another patch.

## Rehearse rollback

Choose one pilot release and run rollback without `--execute`:

```bash
python3 .github/scripts/release_backfill.py rollback \
  --approved-commit FULL_40_CHARACTER_SHA \
  --journal /absolute/path/outside/repo/release-backfill-pilot.jsonl \
  --release-key cua-driver-rs:0.8.2
```

After approval, add `--execute`, verify the original body hash, then reapply the
same release. Keep every journal line. Do not edit the journal by hand.

## Apply the remaining releases

Apply accepted releases in reverse chronological batches of 15 to 25 per
product. Pause between batches to inspect:

- the GitHub API result and re-fetch verification;
- preserved title, flags, tag SHA, and assets;
- the journal's prepared and verified entries;
- GitHub release feed behavior;
- contributor names and links.

Stop the run on the first drift, collision, validation, or GitHub API error.
Do not bypass the failing invariant. Investigate, update the reviewed payload
through a pull request, and resume from the last verified journal entry.

## Troubleshooting

**The command reports release state drift.** Another process changed the tag,
title, flags, body, or assets after inventory. Freeze release work, inspect the
change, and regenerate the catalog through review.

**The agent omits a release or returns an unsupported type.** The author command
records `agent-rejected`. Rerun with `--resume`; reduce the batch size if the
model omits results again.

**A release has no PR association.** Keep the direct commit as evidence. Credit
it only when GitHub returns a verified login or an approved coauthor mapping
exists.

**Rollback refuses the current body.** The release changed after backfill.
Preserve the journal and inspect the new body. Do not overwrite the newer edit.

## Related reference

- [Historical release backfill reference](../.github/release-backfill/README.md)
- [Release attribution and announcements plan](release-attribution-and-announcements-plan.md)
