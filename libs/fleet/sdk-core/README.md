# Cyclops SDK core

The Rust core owns typed pool and claim CRUD, porcelain defaults, claim polling, service endpoint resolution, OAuth token caching and refresh, authenticated HTTP execution, and one retry after `401`. Bindings never receive bearer tokens or service endpoint URLs.

Pool creation accepts `namespace + PoolSpec`, ensures the caller-owned namespace, and rolls the namespace back if pool creation fails. Pool deletion removes the pool before deleting its namespace. For nonzero replica pools, claim creation waits for an available sandbox before creating the claim. Zero-replica pools remain demand-driven. Claim creation accepts an optional generated `ClaimSpec`; otherwise, the core derives `<pool-name>-template`.

`wait_claim` returns sandbox identity and the service names declared by its pool. `service_request` accepts that sandbox, one declared service name, and an HTTP request with a relative path. The core validates every route component, resolves the private control-plane route, applies managed authorization, performs the request, and refreshes the token once after `401`. Unknown-service errors include the requested name and sorted available names. No exported operation accepts an application-constructed URL.

The native JSON-lines runner and WASM component expose the same typed resource and service HTTP operations. Language bindings provide transport adapters and idiomatic request surfaces only.
