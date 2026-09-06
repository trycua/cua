# Fleet Attribution Phase 1

This public contract defines bounded first-touch campaign binding and workload qualification.

## Browser capture

Before Keycloak bootstrap, the SPA captures first-touch values from the current URL: `campaign_id`, `content_id`, `utm_source`, `utm_medium`, and `utm_campaign`. Values use a conservative ASCII set (`A-Z`, `a-z`, `0-9`, `.`, `_`, `~`, `-`), are non-empty, and are at most 128 characters. The serialized version-1 record is limited to 2048 UTF-8 bytes and expires after 7 days. Unknown query keys are ignored; repeated allowed keys, malformed values, and oversized records are rejected. The first valid record is write-once in browser `sessionStorage` and is removed after a successful bind.

The five stable fields are acquisition-channel neutral. They can represent email, SEO, partners, future channels, and organic or paid social traffic without provider-specific behavior. For example, both `utm_source=x&utm_medium=organic-social` and `utm_source=x&utm_medium=paid-social` are ordinary valid values. Provider SDKs, pixels, credentials, arbitrary provider payloads, and provider-specific success semantics are outside this contract. Pixels are optional downstream adapters and are not part of Phase 1.

Use `content_id` for placement detail, not `utm_content` (an ignored unknown key). For the GitHub README CTA, use this capture-compatible URL:

```text
https://run.cua.ai/?utm_source=github&utm_medium=referral&utm_campaign=fleet_activation&content_id=repo_readme
```

## Binding and qualification

After Keycloak authentication, the SPA submits the validated record to the authenticated `/api/analytics/attribution` endpoint. The backend repeats the version, size, age, key, and value checks and accepts only the Fleet SPA principal.

For stable authenticated identities, verified `@trycua.com` email evidence or trusted admin-owner membership classifies the identity as internal. Without verified company-domain evidence, unresolved admin membership leaves the identity unknown. Unauthenticated identities and identities without a stable user ID also remain unknown. Resolved non-admin identities without verified company-domain evidence are external, including those with missing or unverified email evidence. Internal and unknown identities are discarded without emitting `fleet_attribution_bound`. External identities are eligible for that event, but configured subject exclusions suppress analytics delivery. PostHog receives the five fields only as set-once first-touch person properties under the keyed-HMAC pseudonym. The endpoint never accepts a URL, referrer, email, raw identity, arbitrary property, or provider payload, and attribution delivery cannot affect authentication or application readiness.

`QualifiesFleetAttribution` is a deterministic, side-effect-free predicate over explicit facts. Qualification requires authenticated namespace authorization, a persisted Bound claim whose sandbox identity matches the requested service, service exactly `<sandbox>-server`, an exact namespace pool, and a 2xx upstream response. Probe paths, redirects, upgrades, HEAD/OPTIONS, non-service routes, and Kubernetes/orchestration routes are excluded.

The backend derives the claim from Kubernetes state: the requested service identifies the sandbox, the sandbox's controller owner reference identifies the claim, and the persisted claim must point back to that sandbox. No SDK header or client-side claim inference is required. `qualifies` is a qualification verdict only; it is not an activation claim or emission.

## Qualification lookup diagnostics

Fact reads run after the workload response under one shared 250 ms deadline. A lookup failure remains a rejection: it does not emit a qualifying workload or activation, extend the deadline, or bypass ownership checks.

For `fleet_workload_qualification_rejected`, `reason` retains `binding_lookup_failed` or `pool_lookup_failed`. Two optional properties distinguish fact-read failures:

- `qualification_lookup_stage`: `sandbox`, `claim`, `pool`, or `unknown` when a reader cannot identify the binding stage.
- `qualification_error_class`: `deadline_exceeded`, `canceled`, `timeout`, `unauthenticated`, `forbidden`, `not_found`, `rate_limited`, `upstream_5xx`, `unexpected_status`, `invalid_response`, `invalid_binding`, `missing_identity`, `invalid_request`, `request_failed`, or `unknown`.

Both properties must be present together, use allowlisted values, and match a lookup-rejection reason. `request_failed` covers request construction, client availability, or transport errors that have no more specific classification. `unknown` does not imply a timeout. Pool HTTP 404 still produces `pool_missing`, without lookup-error diagnostics.

`status_code` and `error_class` describe the workload's upstream response, not the Kubernetes fact read. An HTTP-200 request with rejected qualification is neither a proven failed workload nor a proven qualifying success. The diagnostic fields contain no raw errors, response bodies, URLs, resource names, email addresses, or user IDs. Existing pseudonymization and internal/unknown classification and exclusion rules still apply. Historical rejections without the new fields cannot establish the underlying failure cause.
