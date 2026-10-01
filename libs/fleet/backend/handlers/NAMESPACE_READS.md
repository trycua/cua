# Namespace read rollout

`GET /api/namespaces` retains its existing JSON response and authorization.
Terraform seeds `/feature-flags/cyclops-cs/ff-list-ns-read-pg` with `true`,
so production starts with PostgreSQL reads. Runtime flag changes remain
operator-controlled. Missing flags and evaluation errors still select Kubernetes.

- Disabled, missing, or failed flag evaluation: read Kubernetes only.
- Enabled: query namespace objects from `k8s_api.current_resources` using the
  existing tenant-role executor and PostgreSQL row-level security.
- PostgreSQL success: return the result, including an empty `[]`.
- PostgreSQL failure (including unavailable executor, deadline, or invalid row):
  discard all partial results, log an error with the trace ID, then perform the
  existing tenant-selected, impersonated Kubernetes read.
- Apply the existing GitHub namespace allowlist to either backend's result.
- If Kubernetes also fails, preserve its error/status behavior.

The PostgreSQL attempt has a five-second context deadline. Database cleanup
has separately bounded contexts (up to three seconds each for rollback and
connection close). Kubernetes fallback uses the original request context, not
PostgreSQL's expired context. Flag evaluation has a three-second deadline.

## Enablement

In development, with the development OpenFeature provider:

```sh
CYCLOPS_CS_FF_LIST_NS_READ_PG=true
```

In production, new SSM parameters start at `true`. Set the flag to `false`
to force Kubernetes reads, or back to `true` to resume PostgreSQL reads.
Terraform retains `ignore_changes = [value]`, so subsequent applies do not
overwrite manual toggles. If the parameter already exists with `false`, changing
the Terraform starting value does not flip it; set that existing flag explicitly
when ready. PR previews also seed the flag on as described below.

The state reader requires `STATE_QUERY_DATABASE_DSN` and
`STATE_QUERY_TENANT_PASSWORD`. Its initialization is independent of the
application database configured by `DATABASE_URL`.
Verify projector health, namespace rows, and tenant-role registration before
turning the flag on. A stale or empty projection that queries successfully does
**not** trigger fallback. The query uses the tenant-visible namespace rows in
this deployment's state store; it does not add a cluster filter.

## Trace shape

```text
namespaces.list [read.backend, read.fallback, namespaces.count]
  feature_flag.evaluate [feature_flag.key, feature_flag.result.value]
  identity.resolve_tenant
  namespaces.read.postgres                 (when enabled)
    db.execute [db.system=postgresql]
      db.connect
      db.transaction.begin [db.transaction.read_only=true]
      db.query
      db.rows.read_decode [db.response.returned_rows]
      db.transaction.rollback
      db.connection.close
  namespaces.read.kubernetes               (disabled or PostgreSQL failed)
    kubernetes.request [method, status, connection reuse]
    namespaces.decode
  namespaces.filter [input, returned, excluded counts]
  response.encode_write
```

The Kubernetes request span includes timestamped connection acquisition, DNS,
TCP, TLS, and first-response-byte events when those phases occur. Reused
connections naturally omit DNS/TCP/TLS setup events. `db.query` measures the
initial query call; result fetching, row decoding, and result-writer work are
measured in `db.rows.read_decode`.

A recovered PostgreSQL failure marks the PostgreSQL child as failed, not the
successful request. The parent records `read.fallback=true`,
`read.fallback.reason=postgres_error`, and a `namespaces.read.fallback` event.
Query text, credentials, HTTP headers, and returned objects are not attached to
these new spans. Error details remain in the fallback log; spans record safe
stage status and error types.

These changes do not instrument authentication middleware or change namespace
creation/deletion. PostgreSQL stage spans also cover `QUERY /api/state/query`,
since both endpoints use the same executor.

## PR previews

Previews seed `ff-list-ns-read-pg=true` into the process-local feature flag
management store. The flag appears in the preview admin feature flags UI and
can be toggled off/on there. Changes affect only that preview pod, never
production SSM, and reset to the seed when the pod restarts.

`cyclops-preview-state-query` imports only `state_query_dsn` and
`state_query_tenant_password` from the existing state database secret. These
credentials connect as the authenticated tenant's read-only RLS role, not as
an application, writer, migration, or role-admin user. Namespace data comes
from the existing production projection, matching the production Kubernetes
cluster that previews already query. The production projector and tenant-role
controller populate that store; previews do not run duplicate controllers or
write production state. The preview's `DATABASE_URL` remains its isolated
`cyclops-preview-database` application connection.

The query credential references are optional so a missing ExternalSecret does
not prevent the backend from starting. Missing credentials leave the executor
unavailable and trigger the normal Kubernetes fallback. Secret values are
injected as environment variables, so restart the preview pod after credentials
first appear or rotate. Once query credentials are present, namespace requests should report `read.backend=postgres`;
force the flag off to compare against `read.backend=kubernetes`.

Only trusted same-repository PR previews receive these credentials, following
the existing preview secret isolation/fork guard. This change does not modify
production flag values, credential values, or production database grants.
