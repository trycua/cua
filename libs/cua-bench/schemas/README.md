# CUA Bench CI Receipt v1 — external consumer guide

This is a **test-result integrity envelope**. It is not an independent oracle, a signed software supply-chain attestation, or proof that an agent performed desktop actions.

## Implemented integration
- `pytest-json-report` (MIT) produces `core-report.json` and `full-report.json` from real pytest executions.
- `jsonschema` validates the published Draft 2020-12 contract at `schemas/ci-receipt-v1.schema.json`.
- `scripts/ci_receipt.py` binds the reports to their SHA-256 checksums, passing test counts and Git commit, and refuses altered or failed reports.
- GitHub Actions publishes these three files as `cua-bench-pytest-evidence` when available.

## Generate in CI

```sh
cd libs/cua-bench
uv run --extra dev --with 'jsonschema>=4.20,<5' python scripts/ci_receipt.py --commit "$GITHUB_SHA"
```

## Independently verify downloaded artifacts

Place the downloaded `core-report.json`, `full-report.json`, and `ci-receipt-v1.json` in the same directory, then run:

```sh
cd libs/cua-bench
uv run --extra dev --with 'jsonschema>=4.20,<5' python scripts/ci_receipt.py \
  --verify /path/to/artifacts/ci-receipt-v1.json \
  --evidence-dir /path/to/artifacts
```

The verifier checks the contract, two distinct report kinds, safe local basenames, exact report checksums, passing outcomes and mutually consistent counts. It does not download external URLs or execute downloaded code.

## Limits and outstanding User Stories
1. Commit and SHA-256 metadata are unauthenticated; consumers must establish CI origin independently (e.g. official GitHub run URL, permissions and repository owner).
2. Reported tests may use mocks. This is not a full Computer-Use Execution Witness and contains neither per-action state observations nor independent oracle replays.
3. No real VM crash/cleanup inventory is included. Real desktop VM fault injection remains a separate P0 gate.
4. Compatibility: versioned schema `cua-bench-ci-receipt/v1`; breaking changes require a new schema version.
5. Evidence retention is currently 30 days. Export elsewhere before expiry if it is used as a long-lived credential.

Success means an **authentic test run can be inspected with the raw reports and the same verifier**, not that the underlying benchmark's real-world task success is guaranteed.
