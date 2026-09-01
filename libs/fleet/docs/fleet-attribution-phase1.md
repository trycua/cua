# Fleet Attribution Phase 1

This public contract defines bounded first-touch campaign binding and workload qualification.

## Browser capture

Before Keycloak bootstrap, the SPA captures first-touch values from the current URL: `campaign_id`, `content_id`, `utm_source`, `utm_medium`, and `utm_campaign`. Values use a conservative ASCII set (`A-Z`, `a-z`, `0-9`, `.`, `_`, `~`, `-`), are non-empty, and are at most 128 characters. The serialized version-1 record is limited to 2048 UTF-8 bytes and expires after 7 days. Unknown query keys are ignored; repeated allowed keys, malformed values, and oversized records are rejected. The first valid record is write-once in browser `sessionStorage` and is removed after a successful bind.

The five stable fields are acquisition-channel neutral. They can represent email, SEO, partners, future channels, and organic or paid social traffic without provider-specific behavior. For example, both `utm_source=x&utm_medium=organic-social` and `utm_source=x&utm_medium=paid-social` are ordinary valid values. Provider SDKs, pixels, credentials, arbitrary provider payloads, and provider-specific success semantics are outside this contract. Pixels are optional downstream adapters and are not part of Phase 1.

## Binding and qualification

After Keycloak authentication, the SPA submits the validated record to the authenticated `/api/analytics/attribution` endpoint. The backend repeats the version, size, age, key, and value checks and accepts only the Fleet SPA principal. Stable authenticated identities emit `fleet_attribution_bound` unless verified `@trycua.com` evidence classifies them as internal. Missing or unverified email evidence is treated as external for growth measurement; unauthenticated identities or identities without a stable user ID remain unknown. PostHog receives the five fields only as set-once first-touch person properties under the keyed-HMAC pseudonym. The endpoint never accepts a URL, referrer, email, raw identity, arbitrary property, or provider payload, and attribution delivery cannot affect authentication or application readiness.

`QualifiesFleetAttribution` is a deterministic, side-effect-free predicate over explicit facts. Qualification requires authenticated namespace authorization, a persisted Bound claim whose sandbox identity matches the requested service, service exactly `<sandbox>-server`, an exact namespace pool, and a 2xx upstream response. Probe paths, redirects, upgrades, HEAD/OPTIONS, non-service routes, and Kubernetes/orchestration routes are excluded.

The backend derives the claim from Kubernetes state: the requested service identifies the sandbox, the sandbox's controller owner reference identifies the claim, and the persisted claim must point back to that sandbox. No SDK header or client-side claim inference is required. `qualifies` is a qualification verdict only; it is not an activation claim or emission.
