# Private account lookup

The lookup index maps `(realm, identity-key fingerprint, pseudonym)` to a Keycloak
subject. It stores no email, username, or profile. The backend fetches current
account details from Keycloak only for an authorized exact lookup. Analytics
events continue using the existing `POSTHOG_IDENTITY_KEY` pseudonym.

Migration `000012_private_account_lookup.sql` creates a private schema owned by
the migration role. Only `cyclops_app` receives runtime schema access. Reporting
and tenant roles receive no access. Audit records contain actor, time, and a
fixed outcome only; they contain no lookup target or result. A PostgreSQL row
lock enforces ten admitted requests per actor in a rolling minute across replicas.

The API requires an authenticated `cyclops-cs-spa` human principal and a fresh
admin-membership decision for each request. CLI, API-key, proxy, and automation
principals cannot use it, even when their owner is an admin. Successful lookups
fail closed if the private audit cannot be written. Authenticated denials are
audited on a best-effort basis without reading the lookup body.

Future validated SPA, CLI, desktop, and normalized user-key identities enter a
bounded background queue, including analytics-excluded accounts. Successful
writes are deduplicated in memory for one hour. A full queue, database outage,
or process exit can lose observations; a later request or controlled backfill
can repair them. Neither authentication nor workload success depends on this
index. No new startup/readiness dependency is added. Before migration, the
lookup returns 503 while other routes keep serving.

## API contract

This feature is API-only; it adds no admin page or navigation entry.
Send `POST /api/admin/account-lookup` with `Content-Type: application/json` and
an `Authorization: Bearer` header containing a valid human admin access token
issued for `cyclops-cs-spa`. The HTTP caller does not need a custom UI, but CLI
client tokens, service accounts, and customer API keys remain denied. This
change does not add a token-issuance flow or expand client authorization.

For an exact account ID, send this synthetic example body:

```json
{"kind":"account_id","value":"synthetic-account-123"}
```

To resolve an analytics identity, set `kind` to `pseudonym` and `value` to the
exact `u_`-prefixed, 64-character lowercase hexadecimal HMAC value. Send the
identifier in the JSON body, not the URL. Requests are bounded to 1 KiB and
reject unknown fields. Email search, username search, fuzzy search, and bulk
lookup are not supported.

A successful HTTP response has `status`, `backfill_complete`, and, when found,
an `account` object:

| Status | Meaning |
| --- | --- |
| `found` | One current Keycloak account was retrieved. |
| `mapping_missing` | No mapping exists for that pseudonym and configured key. This does not prove the account never existed. |
| `account_missing` | Keycloak returned no current account for the exact subject. Historical events may still exist. |

The account contains `id`, `username`, `email`, `email_verified`, optional
`created_at`, `pseudonym`, `identity_class`, and `excluded`. Classification and
exclusion reflect current account/configuration evidence, not historical event
classification. The existing rule treats a verified `@trycua.com` email as
internal; other authenticated IDs default to external. That default is not
proof of customer ownership.

`backfill_complete` means an observed scan starting at offset zero reached the
end; it does not guarantee exhaustive coverage. The limitations are described
in the backfill section.

HTTP 400 indicates invalid input; 401/403 indicates missing or denied
authorization; 429 indicates the shared rate limit, with `Retry-After: 60`.
The lookup returns 503 when its database, directory, or required audit is
unavailable. Upstream authorization failures can also return a generic 5xx.
Responses use `Cache-Control: no-store`. Callers must not send input or returned
account details to analytics, public logs, shell history, or shared artifacts.

## Controlled historical backfill

Use an approved environment with the existing `POSTHOG_IDENTITY_KEY` and explicit
`KC_BASE_URL`, `KC_REALM`, `KC_ADMIN_CLIENT_ID`, and `KC_ADMIN_CLIENT_SECRET`.
Never paste credentials into command arguments or logs. Execute mode additionally
requires `DATABASE_URL` for the application role. This command intentionally
does not initialize telemetry or the serving application.

From `cyclops-cs/backend`, preview the bounded scan:

```sh
go run ./cmd/account-lookup-backfill --max-pages 100 --page-size 100
```

The default is read-only and prints counts and offsets only. After approval of
the target environment and scope, add `--execute` to write mappings. Repeated
runs are idempotent; a conflicting subject fails closed. `--offset N` resumes
work, but a resumed invocation never marks coverage complete. To establish the
observed scan marker, rerun from zero with enough `--max-pages` to reach a short
or empty final page. Interrupted, failed, dry-run, and capped scans do not create
a marker. Previously successful markers remain historical observations.

The marker means a scan starting at zero observed the end of Keycloak's paged
listing. The listing is not a snapshot: concurrent account changes can shift
offsets. It does not prove exhaustive current or historical coverage. Missing
mappings remain explicitly missing, even after a successful scan. Deleted users
cannot be reconstructed. Key rotation scopes new rows and scan status separately;
retain an approved key-to-history strategy before rotating the analytics key.

The backend release image includes `/app/account-lookup-backfill`. Operators can
run the same bounded command inside the deployed backend container with its
existing environment; no additional secret export or credential is required.
Run the read-only preview first and keep output limited to the command's counts.

## Rollout gates

1. Verify the additive migration on an isolated PostgreSQL cluster, including
   initial apply, immediate no-op, application behavior, and denied tenant and
   reporting access.
2. Deploy the migration through the normal immutable migration Job and verify
   its ledger and no-op result while serving remains available.
3. Verify authorization, current Keycloak read permissions, exact lookup, safe
   audit behavior, and rate limiting before enabling operator access.
4. Run the read-only backfill preview, approve the production target and scan bounds, then execute
   the backfill and verify observed scan status. A local test is not deployment
   or production backfill evidence.

Merge, deployment, production migration, backfill execution, and any access-grant
change each remain human-gated. The checked-in realm export declares user-read
roles for the backend client, but it is not evidence of current live grants.
No privilege expansion is part of this change.

Rollback the serving release to remove the endpoint, stop any backfill,
and leave the additive private schema in place. Do not delete or edit migration
12 or its ledger entry. Removing private data requires a separately approved
retention/deletion operation, not a schema rollback. The analytics key, event
contract, dashboard queries, and activation counts are unchanged.

The private tables are sensitive operational data. Apply the same restricted
backup and access policy as the account directory. Audit and inactive limiter
rows require an operator-defined retention policy; this change schedules no
automatic deletion.

## Local verification (September 4, 2026)

The implementation was tested in the supplied worktree based on `d1a5abee`.
No production secret, account directory, or database was used.

- Backend tests passed for `accountlookup`, `auth`, `handlers`, `keycloak`,
  `productanalytics`, the backfill command, and the main router package.
- Race checks passed for the lookup, handlers, Keycloak, and backfill packages.
- The full `database` suite passed against disposable loopback PostgreSQL 16.14,
  including version 11 to 12 (`pending=1`, `applied=1`), its immediate no-op,
  recovery after migration, and reporting/tenant denial assertions.
- Go vet, the serving-availability manifest contract, and whitespace checks passed.

The earlier UI, client wrapper, browser-only tests, and local visual preview
were removed when the scope changed to API-only. Their browser verification is
not evidence for the delivered API-only change. Backend tests exercise exact
lookup, strict request decoding, authorization and revocation, safe failures,
restricted auditing, and normalized identity mapping without a browser.

Changed-file scope: the `accountlookup` package and backfill command; append-only
migration 12 and database tests; Keycloak read wrappers; the lookup handler,
router wiring, OPA policy and authorization fixtures; and this API runbook.
The existing backend Dockerfile also packages the backfill binary.
No analytics event implementation, dashboard, deployment manifest, credential,
or existing migration was changed. The pre-existing `output/` directory was
left untouched.
