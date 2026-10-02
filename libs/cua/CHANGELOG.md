# Changelog

## [0.2.0](https://github.com/trycua/cua/compare/cua-sdk-v0.2.0...cua-sdk-v0.2.0) (2026-10-01)


### Features

* merge updated sdk from cua-staging ([#4397](https://github.com/trycua/cua/issues/4397)) ([9166817](https://github.com/trycua/cua/commit/9166817485ae53f3966935c13878a8196d79a399))

## Changelog

Release notes for the cua SDK (`cua-sdk-v*`: PyPI `cua`, npm `@trycua/cua`, Swift `Cua`, the `cua` CLI).

## Release notes: the unified cua SDK (first cua-sdk release)

Release Please adds the version heading and commit list above this section
when it opens the cua-sdk release PR. These are the curated notes for that
first release (the unified cua SDK, the `cua` CLI and daemon, and Spaces on
the SDK); copy them into the release description when the release is
published.

### Behaviour changes

* **refs:** sandboxes and Spaces share one ref scheme: `local:<name>`, `cloud:<name>`, `direct:<host:port>`, `relay:<machine-id>`. `sb.id` (and `SandboxInfo.id`, the daemon's `Sandbox.id`, Space ids) is the qualified ref; every call that takes a sandbox name accepts a ref, and a bare name must be unique across locations, else the new typed `AmbiguousSandbox` error (`CuaError.AmbiguousSandbox`, cua-sandbox `AmbiguousSandbox` with `.candidates`, daemon reason `AMBIGUOUS_SANDBOX`) lists the qualified candidates. `local=` (CLI `--local` / `--cloud` on every NAME command) narrows a bare name. `parse_sandbox_ref`, `qualify_sandbox_ref` and `ambiguous_sandbox_candidates` are exported in every binding. Legacy spellings still parse (`space://{fleet,local,direct,relay}/...`, `fleet:<ns>:<claim>`, `url:<addr>`, URLs) and stored Spaces registries migrate on read; output always uses the new form. One address is one ref: a new direct connection to an address replaces the earlier one (with its token).
* **refs:** the `location` vocabulary is `local | cloud | direct | relay` everywhere: the Spaces provider (`SpaceInfo.provider` is `cloud`, not `fleet`; `fleet` still parses), the Spaces contract (0.3.0: `providers` use these words, `relay` added) and CLI output (`--on cloud`, `--on direct:<addr>`, with `fleet` and `url:<addr>` as aliases; `cua sb ls` prints an `ID` column of refs).
* **refs:** cloud sandbox names are unique within the account: creating a named cloud sandbox fails with `InvalidArgument` when another pool already has a claim of that name (the same pool still reattaches).
* **image:** the local image prefix naming a cloud pool's template is `pool:<name>`; `fleet:<name>` is a deprecated alias (it logs a warning), so `fleet:` never names an image.
* **fleet:** `cua fleet pool export --terraform` writes every attribute the fleets provider has (`command`, `args`, `env`, `process_mode`, `claim_secrets`, `idle_ttl_seconds`, `ttl_policy`, `ttl_seconds_after_created`) and, for a pool with a private-registry pull secret, a `fleets_registry_secret` resource whose username and password are Terraform variables. `sidecar` blocks (now with `args`, `cpu`, `memory`) stay commented until the provider has them.
* **sdk:** local is the default provider on every surface. `SandboxCreateOptions.provider` is optional: unset means local, unless `url` is set (direct) or cloud options are (`cloud`, or the deprecated flat Fleet fields), which imply Fleet. `provider: Local` with `cloud` is `InvalidArgument`. The daemon applies the same rule, and the `cua mcp` / `cua daemon mcp` `sandbox_create` tool defaults to `local=true` (a `pool` implies the cloud).
* **image:** `Image.fromRegistry` / `from_registry` and `resolve_image` are literal (`ubuntu:24.04` is `docker.io/library/ubuntu:24.04`). Aliases apply only through `Image.linux()/windows()/macos()` and the CLI's bare words (`linux`, `windows`, `macos[:v]`, bare `ubuntu`).
* **sdk:** `sandbox.service(name).public_url(ttl, label)` (TS `publicUrl`).
* **cli:** `cua sb mcp NAME SVC config` masks credential headers; `--show-secrets` prints them.
* **sdk:** listing shows every sandbox by default: `sandboxes().list()` (no provider) returns local, direct and live cloud (Fleet) sandboxes, each with `location`; a provider filters. Without Fleet credentials the cloud part is left out silently; when Fleet fails or takes over 5 s it is left out with a warning (`list_with_warnings`, daemon `ListSandboxesResponse.warnings`; `include_cloud` defaults to true). `list_all()` is a deprecated alias. `cua sb ls` lists everything (`--local`, `--cloud` filter; a Fleet failure is a warning, exit 0); MCP `sandbox_list` takes `location`. `cua do ls` is unchanged. JSON rows carry `location` (`where` is deprecated).
* **auth:** a non-secret session marker, `~/.cua/session.json` (`store`, `account`, `expires_at`; mode 0600), is written when a session is stored and removed at logout, so implicit calls (the default sandbox listing) read the OS keychain only when a session is there. **One-time step for sessions stored before this release:** run `cua auth status` (or any cloud command, such as `cua sb ls --cloud`); its successful read writes the marker, and the default listing includes cloud sandboxes from then on. `may_have_fleet_session()` exposes the check in every binding.
* **sdk:** cloud `suspend` / `restart` are `Unsupported` (Fleet cannot suspend one claim) and no longer release the claim or scale a user pool to zero; `resume` of a running cloud sandbox reattaches.
