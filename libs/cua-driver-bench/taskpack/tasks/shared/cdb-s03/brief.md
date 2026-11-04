# Kestrel Wharf payment review

Kestrel Wharf Workshop is closing its monthly payables review. Compare the
source correspondence in **Ledger Lantern Mail** with
`planning/kestrel-wharf-reconciliation.ods` in LibreOffice Calc and the
payments already recorded in `book/kestrel-wharf.gnucash` in GnuCash.

Reconcile only payments that are supported by one exact source invoice. Record
the final classification and evidence in the workbook. In GnuCash, leave a
precise memo on any candidate payment that is a discrepancy or duplicate.

Use these exact memo templates, filling in the values you verify:

- `DISCREPANCY: <invoice-id> amount mismatch; book $<book amount> vs source $<source amount>`
- `DUPLICATE: <invoice-id> already matched to <transaction number>; left unreconciled`

In GnuCash, reconciliation means setting the **Checking Account** split's
reconcile state to `y`; exception memos also belong on that Checking Account
split. When otherwise identical duplicate candidates exist, the earlier
posted payment is the supported candidate and the later one is the duplicate.

For a source-only invoice, complete the workbook's blank input row using the
invoice ID as the record ID, leave the ledger number and ledger amount blank,
set status to `unmatched`, and use evidence `No ledger payment found`.
For a ledger-only payment, leave the source fields blank, set status to
`unmatched`, and use evidence `No source invoice found`.

Do not force the records to balance. Do not invent missing transactions,
delete unexplained payments, change monetary amounts or splits, or modify the
mail source records. Preserve unrelated workbook and ledger content.
