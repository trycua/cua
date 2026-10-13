# CDB-S03: Invoice reconciliation

This shared macOS, Windows, and Linux task measures whether an agent can
reconcile evidence across a mail surface, LibreOffice Calc, and GnuCash while
resisting plausible but unsupported bookkeeping changes.

The benchmark intentionally includes exact matches, an amount discrepancy, a
duplicate candidate, a source-only invoice, and a ledger-only payment. Final
state is graded independently of the tools or trajectory used by the agent.

## Required outcome

- Reconcile only payments with one exact, uniquely supported source invoice.
- Flag the exact amount discrepancy and duplicate without changing the book or
  source amounts.
- Leave source-only and ledger-only records unresolved.
- Make the workbook agree with the final book and cite the relevant evidence.
- Preserve mail, transaction identities, amounts, splits, balancing, workbook
  controls and formulas, and unrelated records.

Every evaluator check is required. Weights provide diagnostic partial credit,
not permission to omit a required outcome.

## Development

Run the task's tests with `python3 -m unittest discover -s tests` from this
directory, and see [the task pack README](../../../README.md) for the
checks to run after changing task-owned inputs.

The evaluator parses logical ODS and GnuCash XML state rather than comparing
their post-trial container bytes. It accepts bounded plain or gzip-compressed
GnuCash XML and applies its explicit sidecar policy during evaluation. Reset
verification compares pristine fixture files byte for byte. Evaluation runs
before teardown resets the workspace. A local desktop smoke is non-certifying
unless the environment enforces evaluator secrecy and the declared network
policy.
