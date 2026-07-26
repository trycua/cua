# Historical release backfill reference

This directory contains the reviewed inputs and outputs for retrospective Cua
Driver and Lume release notes. The production release workflow does not read
these files.

## Files

| File | Contents |
| --- | --- |
| `config.json` | Product tag rules, path eras, managed-block version, and security exclusions. |
| `catalog.json` | Frozen GitHub release state, explicit tag adjacency, tag SHAs, paths, body bytes and hashes, flags, and asset inventory. |
| `evidence.json` | Normalized commits, pull requests, direct commits, issues, files, and verified contributor identities. |
| `candidates.json` | AI-authored summaries and categorized changes with prompt and evidence provenance. |
| `skip-ledger.json` | One reason for every release that has no candidate. |
| `rendered.json` | Exact managed blocks and final release bodies accepted for application. |
| `candidates.md` | Human-readable candidate review file. |
| `*.schema.json` | JSON Schema contracts for the catalog, evidence, candidates, skip ledger, and rendered payload. |

## Command reference

The canonical command is:

```text
python3 .github/scripts/release_backfill.py COMMAND [OPTIONS]
```

### `inventory`

Reads local tags and published GitHub releases. Writes `catalog.json`.

Required environment: `GH_TOKEN` with repository read access.

### `collect`

Reads the frozen catalog, local git ranges, and public GitHub metadata. Writes
`evidence.json` and the evidence-stage skip ledger.

Required environment: `GH_TOKEN` with repository read access.

### `author`

Passes bounded evidence packets to an external JSON-in/JSON-out agent command.
The agent receives no GitHub token. `--batch-size` accepts 1 through 25.
`--batch-max-sources` and `--batch-max-bytes` prevent complex releases and long
pull request bodies from overloading one response. `--resume` retains completed
candidates and retries authoring-stage failures. The command checkpoints after
every batch. An evidence-backed empty change list is recorded as a durable
`no-product-changes` skip instead of publishing maintenance-only prose.

The repository includes `release_backfill_claude.py`, a streaming Claude Code
adapter. It disables Claude tools and stores raw streams and debug logs outside
the repository. `BACKFILL_CLAUDE_MODEL` selects the model.

### `render`

Validates candidate claims against their release evidence, renders managed
blocks, and writes the exact final bodies and human-readable candidate review.

### `validate`

Validates every JSON artifact against its checked-in schema, then rebuilds the
accepted output from the catalog, evidence, candidates, and skip ledger. It
fails when rendered files differ, coverage is incomplete, or any schema, hash,
or evidence reference is invalid.

### `apply`

Defaults to a read-only dry run. `--execute` also requires a full
`--approved-commit`, a clean worktree at that commit, one or more
`--release-key` values, and a journal path. The command can patch only the
GitHub release body. It verifies the release ID, tag SHA, title, body hash,
draft flag, prerelease flag, and asset inventory before each write and after
each patch.

### `rollback`

Defaults to a read-only dry run. It restores exact pre-image body bytes from a
verified apply journal. Rollback stops if the current body has changed since
the apply operation.

## Managed block

The renderer appends a block with versioned boundary markers:

```text
<!-- cua-release-backfill:v1:start evidence-sha256=... -->
...
<!-- cua-release-backfill:v1:end -->
```

An existing marker is a collision. The tool never replaces or nests a managed
block.

## Mutation boundary

The backfill GitHub client has no delete, upload, tag, title, draft,
prerelease, or latest-release mutation method. Its sole write operation is:

```json
{"body": "the exact reviewed final body"}
```

See [the operator runbook](../../docs/release-backfill-runbook.md) for the
generation, review, pilot, batching, and rollback procedure.
