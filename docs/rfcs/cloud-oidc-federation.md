---
title: "Connect a cloud: OIDC federation and sealed tokens"
authors:
  - cua-spaces
created: 2026-10-24
last_updated: 2026-10-24
status: accepted
discussion:
rfc_pr:
implementation:
  - libs/cua-spacesd/crates/cua-oidc (issuer, connection registry, token policy, audit)
  - libs/cua-spacesd/crates/cua-relay (GET /internal/v1/devices/{id})
supersedes:
superseded_by:
---

# RFC: Connect a cloud: OIDC federation and sealed tokens

Linear: CUA-2046 (federation, no stored keys) and CUA-2053 (link once, observability, billing).

## Goals and rules

- A cloud is linked once per account. Every signed-in device, and every agent running in a Space on those devices, can then create machines there. Nobody has to sign the cloud's own CLI in on each device.
- **No usable customer cloud secret is ever stored server-side.** AWS, Google Cloud and Azure use OIDC federation: Cua signs short-lived tokens and the customer's cloud trusts them. Hetzner and Modal have no OIDC, so their API token is sealed end-to-end to the account's devices, and the servers only ever hold ciphertext.
- Cua touches only what it creates (the `cua-managed=true` tag, as today). It never changes pre-existing customer resources.
- A monthly budget is required per cloud before the first machine. Limits that an agent sets only take effect after a person approves them.
- Older clients and today's per-device links keep working.

## 1. The Cua OIDC issuer

**Service.** `cua-oidc` is a new, small Rust service. It is server-only, lives at `libs/cua-spacesd/crates/cua-oidc` and is built and signed like `cua-relay`. It is deliberately **not** the relay and not Keycloak:
- The relay is a mesh coordinator, and compromising it should not grant cloud access.
- Keycloak can't mint per-device, per-agent and per-purpose claims, and gives no control over the key.

It is deployed with GitOps in the `cloud` repo: staging at `oidc.staging.cua.ai` first, then prod at `oidc.cua.ai`. It runs Traefik plus a cert-manager Let's Encrypt certificate, which AWS STS, GCP and Entra can all fetch without a pinned thumbprint, and it has its own CNPG Postgres.

**Endpoints.**
- `GET /a/{acct}/.well-known/openid-configuration` returns the discovery document, one per account (see "Issuer per account").
- `GET /a/{acct}/.well-known/jwks.json` and `GET /.well-known/jwks.json` return the same key set.
- `POST /v1/token` mints a token. Its body is `{connection, purpose, audience, agent?}`. It is authorized as described under "Who may get a token".
- Connections:
  - `POST /v1/connections`, `GET /v1/connections`, `GET /v1/connections/{id}`;
  - `POST /v1/connections/{id}/approve`, `/deny`, `/limits` and `/revoke`;
  - `DELETE /v1/connections/{id}`;
  - `POST /v1/connections/{id}/spend` `{month, usd}`: what this device spent there this month, its running total (see "The spend ledger"). `GET` answers carry `spent_month_usd`, the sum over the account's devices.
  - These hold metadata only, never a usable secret (see "Connection registry").
- `GET /v1/templates/aws.yaml` serves the CloudFormation template that the quick-create link points at. A static copy also ships in the repo.

**Issuer per account.** The issuer URL is `https://oidc.cua.ai/a/{acct}`, where `{acct}` is a stable, opaque, 12-character ID derived from the account. A customer's AWS OIDC provider, GCP pool or Entra credential therefore trusts tokens for their own account only. A token for account A carries `iss=…/a/A`, and account B's trust doesn't name that issuer, so it can't be replayed there even if the `sub` check were mistyped. The `sub` check is a second line of defence.

**Claims.** A JWT signed with RS256. RS256 is the only algorithm all three clouds accept: Entra requires RSA.

| claim | value |
|---|---|
| `iss` | `https://oidc.cua.ai/a/{acct}` |
| `sub` | `account:{acct}`: stable, used in the cloud's trust condition |
| `aud` | per cloud: `sts.amazonaws.com`; `//iam.googleapis.com/projects/{n}/locations/global/workloadIdentityPools/cua/providers/cua`; `api://AzureADTokenExchange` |
| `iat`, `nbf`, `exp` | `exp - iat` = 15 min (AWS minimum session 15 min) |
| `jti` | random, logged |
| `cua_conn` | connection id |
| `cua_device` | `dev_…` of the requesting device |
| `cua_agent` | agent or routine name when an agent is the caller, else absent |
| `cua_purpose` | `create` \| `manage` (stop/start/delete/sweep) \| `check` \| `cost` |

The AWS role session name is built from the claims as `cua-{device}-{agent|user}-{purpose}`, truncated to 64 characters. The same name shows in CloudTrail and in the app's Activity list. GCP and Azure carry the extra claims through attribute mapping (`attribute.device`, `attribute.purpose`) and their audit logs.

**Keys and rotation.** The signing key is an AWS KMS asymmetric key (`RSA_2048`, `RSASSA_PKCS1_V1_5_SHA_256`) in Cua's infra account. The private key never leaves KMS; the service calls `kms:Sign` through IRSA or the instance role and holds no key file.
- `kid` is the SHA-256 thumbprint of the public key.
- The JWKS always lists the active key, the next key and the previous key.
- Rotation runs every 90 days. GitOps creates the next KMS key and publishes it in the JWKS at least 24 h before signing switches to it. Signing then switches, and the old key stays in the JWKS for 24 h. Because tokens live 15 minutes, nothing breaks.
- Emergency revocation removes a `kid` from the JWKS. AWS, GCP and Entra refetch on unknown `kid` and on their cache expiry, so this takes about an hour on the cloud side. Removing a connection on our side is instant (see "Revocation").
- A `FileSigner` with a PKCS#8 RSA key exists only for tests and for self-hosted issuers.

**Who may get a token** (`POST /v1/token`). The caller must present both of these:
1. A Keycloak access token from `cyclops-cs` with audience `cua-oidc`. This is the account's normal sign-in, and the issuer checks it against the realm's JWKS.
2. A **device proof**: a JWS from the device's enrolled P-256 key over `{htu, htm, iat, jti, connection, purpose, agent}`, following DPoP (RFC 9449). The issuer checks the device is enrolled and not revoked in the relay's device registry through a new, cluster-internal, read-only relay endpoint, `GET /internal/v1/devices/{id}`, authenticated with a service token from an ExternalSecret.

This ties issuance to an enrolled device and not only to a bearer token. A stolen access token alone mints nothing.

Agents never call the issuer. The device's daemon calls it on the agent's behalf, through the existing cloud tools, and stamps `cua_agent`. An agent can't name a different connection or purpose than the tool call it made, and it never sees the cloud credentials: the daemon does the STS exchange and the cloud calls.

**Policy checks at issuance.** The issuer refuses to mint a token when:
- the connection is `pending` and the purpose is anything other than `check`;
- the connection is `revoked`;
- the account's server-side ledger shows this month's budget for that connection is used up and the purpose is `create` (stop and delete stay allowed, so you can always clean up);
- the device is not enrolled.

**The spend ledger.** Each device reports what it spent on each OIDC or sealed connection this month: list price times runtime of the machines it created, as its own records estimate it. A report replaces that device's earlier one for the month, and the connection's month spend is the sum over the account's devices (table `oidc_spend`, keyed by account, connection, month and device). Reports need the device proof (purpose `connection.spend`), so a revoked device or another account can't move the figure. Devices report when a machine is recorded or forgotten, when a create is checked and when `cloud_status` runs, in the background; they never block or fail what was asked. Before a new machine, the client checks the larger of its own figure and the account's, so sealed clouds (no tokens to refuse) stop at the budget too. An older issuer without the route, or one that can't be reached, leaves the device's own figure in charge.

Each refusal is a typed error that the CLI and the app show plainly. Every issuance is written to an append-only audit table with the `jti`, device, agent, purpose and connection, and these rows feed the Activity view.

**Revocation.** `POST /v1/connections/{id}/revoke` or Disconnect stops minting immediately. No live credential lasts longer than the remaining STS session (15 min max). Users who also want to remove the trust delete the stack or pool themselves; Cua never deletes customer IAM.

## 2. Connection registry

The server stores one record per linked cloud, with no secrets:
- the account and connection id;
- `cloud` (aws/gcp/azure/hetzner/modal);
- `auth` (`oidc` | `sealed`);
- the cloud's identifiers: AWS account id and role ARN; GCP project number, pool and provider names, and service account (optional); Azure tenant id, client id, subscription and resource group; Hetzner project name; Modal workspace;
- region, `budget_usd_month`, `max_size` (vCPU and GiB) and `ttl_hours`;
- `status` (`pending` | `connected` | `revoked`), `created_by` (device and agent), `approved_by` (user and device, with time), `pending_agent` (the agent whose raise waits in `pending_limits`);
- for `sealed` connections, `sealed_token` (ciphertext and the pool key generation; see section 5).

Role ARNs and client ids are identifiers, not secrets. With them, someone can only ask the cloud to trust a Cua-signed token for this account, and only Cua's KMS key can sign one.

**Approval.** A connection created or changed by an agent, meaning the daemon's request carries `cua_agent`, starts `pending`, as do limits raised by an agent. The app shows "Approve <cloud> for your agent?". Approving needs a user session and an enrolled device. On Macs it sits behind the same user-presence check as Keyvault (Touch ID). An agent can lower limits, but it can't raise a budget or size and can't approve.

## 3. Trust setup per cloud

**AWS.** `templates/aws/cua-spaces.yaml` is a CloudFormation stack (quick-create link, CLI deploy, or the Terraform module `trycua/spaces-trust/aws` with the same content). It creates:
- `AWS::IAM::OIDCProvider` for `https://oidc.cua.ai/a/{acct}` with client id `sts.amazonaws.com`, and no thumbprint, since the certificate is from a public CA.
- `AWS::IAM::Role cua-spaces`, whose trust policy allows `sts:AssumeRoleWithWebIdentity` with `StringEquals {oidc.cua.ai/a/{acct}:aud: sts.amazonaws.com, oidc.cua.ai/a/{acct}:sub: account:{acct}}`, `MaxSessionDuration 3600`.
- The inline policy is today's least-privilege `policy/aws-policy.json`, scoped to `cua-managed=true`, plus `ce:GetCostAndUsage` (read-only, for `cost`). It still has no `iam:` actions.
- `Outputs.RoleArn`. The role name is fixed (`cua-spaces`), so the client derives the ARN from the account id and needs no callback. `cua cloud connect aws --check` (or the app, polling) tries the exchange; when it succeeds, the connection records the ARN. People who renamed the role paste the ARN instead.
- A drift test keeps the template's policy byte-identical to `aws-policy.json` and to the docs.

**Google Cloud.** A Cloud Shell script `cua-trust.sh`, a gcloud snippet and a Terraform module create:
- a workload identity pool `cua` with an OIDC provider `cua`: issuer `https://oidc.cua.ai/a/{acct}`, allowed audience the provider resource name, `attribute.account=assertion.sub`, `attribute.device=assertion.cua_device`, and the attribute condition `assertion.sub=='account:{acct}'`;
- a custom role `cuaSpaces` with today's 15 `gcp::PERMISSIONS` plus `billing.resourceCosts.get`;
- a binding of `principalSet://…/pools/cua/attribute.account/account:{acct}` to that role, with an IAM condition limiting it to `resource.labels.cua-managed == 'true'` where the API supports it. Creates are label-checked by the client and by `cua cloud audit`.

The token exchange is STS `token` (urn:ietf:params:oauth:grant-type:token-exchange), then direct federated access. This needs no service account and no impersonation.

**Azure.** A "Deploy to Azure" template (ARM/Bicep), an az CLI snippet and a Terraform module create:
- the resource group `cua-spaces`;
- a user-assigned managed identity `cua-spaces` with a federated credential: issuer `https://oidc.cua.ai/a/{acct}`, subject `account:{acct}`, audience `api://AzureADTokenExchange`;
- a role assignment of `Virtual Machine Contributor` plus `Network Contributor`, scoped to that resource group only. The group is the boundary. Cua also tags everything `cua-managed`.

The token exchange is the Entra `client_credentials` grant with `client_assertion_type=jwt-bearer`.

**Hetzner and Modal** have no OIDC. They use an API token (section 5).

## 4. Token exchange on the device (cua-byoc)

- `Connection` (`state.json`, `model.rs`) gains `auth: Auth`. `Cli` is the default, so old files read as today. The other variants are `Oidc { conn_id, role_arn | gcp provider | azure ids }` and `Sealed { conn_id }`. It also gains `budget_usd_month` and `max_size`. All new fields use `serde(default)`.
- `cua-byoc/src/federation.rs`:
  - an `IssuerClient` that gets a token from `cua-oidc`, using the session access token plus a device proof signed with the `DeviceKey` from `cua-host`;
  - a `ProvideCredentials` implementation for aws-config that runs `AssumeRoleWithWebIdentity` with the minted token and caches it until 2 minutes before expiry;
  - the GCP STS exchange and the Azure client-assertion exchange, both reqwest-based and cached the same way.
- The provider code (`aws.rs`, `gcp.rs`, `azure.rs`) asks a `CredentialSource` for credentials: CLI sign-in (today), OIDC, or sealed. Nothing else changes.
- Server connections are merged into `cua cloud status`. A device with no local connection uses the account's `oidc` or `sealed` connections automatically, which is the "link once" behaviour.

## 5. Sealed tokens (Hetzner, Modal, others without OIDC)

- On the device that pastes it, the token is sealed with the **pool data key** of the current generation: XChaCha20-Poly1305, under the HPKE-wrapped bundle each enrolled device already gets in `cua-spaces-cluster/src/pool/seal.rs`. The AAD is `{acct, conn_id, cloud, generation}`.
- The ciphertext and its generation are stored on the connection record. The server holds no key that opens it.
- Any enrolled device or pool host opens it locally when it needs to call the cloud. Agents use it through the daemon (Keyvault-style use, no read). The token is never printed, logged or returned by any tool.
- When a device is revoked, the pool moves to a new generation. The holders that remain re-seal the token on their next run, and the server swaps the blob for one under the new generation (compare-and-swap on the generation).
- **Trade-off, as stated in the decision:** unattended scaling on these clouds needs at least one holder online. The cloud page lists the holders ("Can open it").
- **Revoke:** delete the token in the cloud's console, or Disconnect, which deletes the blob. Replacing the token re-seals it and swaps it in.
- Today's local token (OS vault or 0600 file, `byoc-hetzner-token`) stays as the `Cli` path for old clients.

## 6. The agent self-guide surface

- `cua cloud connect <cloud> --region R --budget USD --max-size 4x16 [--project|--subscription …] [--token-stdin] [--json] [--check]`:
  - It never prompts and is idempotent: it creates the connection record or updates it in place.
  - Every run ends with `status` (`needs-trust` | `checking` | `waiting-for-approval` | `connected` | `failed`), `next` (an exact command for the agent, or a URL for a person) and `verify` (the command to confirm). With `--json` it prints `{status, next: {run?, open?, why}, verify, checks[]}`.
  - Exit codes: 0 when connected, 10 when the next step is the agent's, 11 when it waits on a person, 1 on error.
- The MCP tool `cloud_connect` takes the same new fields (`budget_usd_month`, `max_size`, `auth: "oidc"|"sealed"|"cli"`) and returns the same `{status, next, verify, checks}`.
  - `cloud_status` reports `auth`, `status`, `budget`, `spent_month`, `running` and `approval`.
  - A new tool `cloud_approve` (`provider`, `deny`) exists, but only for the app and a user's session. It is in the contract's `PERSON_ONLY_TOOLS`: no MCP server lists or runs it, the daemon and `cua cloud approve` refuse when an agent calls, and the issuer refuses an agent's proof. `cloud.approve` is never granted to agents.
- The agent docs page `docs/content/docs/agents/connect-a-cloud.mdx` gives plain steps for each cloud, the exact commands, the verify command, and what to ask the human.

## 7. App

- `machines_view` gains a `clouds` group, built in the app core and drawn by web, Electron and SwiftUI: name, logo id, short line ("3 running", "Budget reached", "Not trusted", "Token refused"), health and actions.
- The cloud detail view shows:
  - status ("Connected", "Budget reached", "Not trusted", "Token refused");
  - spend against budget, with a progress bar;
  - running machines (name, who started them, cost);
  - settings (region, budget, largest machine);
  - for sealed clouds, the holders;
  - Activity, from the issuer audit plus the local audit;
  - Export…;
  - Disconnect.
- Picker and sheets follow the storyboard: OIDC or API token, one primary button, a read-only prompt field, and the vendors' logos.
- Follow-ups: move the machine page to hairline rows, and show the Run an agent dialog's prompt in a read-only field.

## 8. Usage and billing (CUA-2053 milestone 1)

- The estimate is list price × runtime from the ledger, per Space, agent and day. This already exists as `spent_since`; the work adds a per-connection breakdown.
- Actuals come from the customer's own billing data, through the same federated access with purpose `cost`:
  - AWS Cost Explorer, filtered by tag `cua-managed`;
  - GCP billing export, when the customer enables it;
  - Hetzner, from the invoice API through the sealed token.
- Alerts fire at 50, 80 and 100 % of each connection's budget. At 100 %, `create` stops (issuer and client); stop and delete still work.
- `cua cloud cost --by space|agent|day --since … --format csv|json` and Export in the app. FOCUS comes next.

## 9. Migration and compatibility

- Existing `state.json` connections read as `auth: Cli` and behave exactly as today. `cua cloud connect aws --oidc` upgrades one in place; the app shows "Switch to OIDC" on a CLI-sign-in AWS, GCP or Azure link.
- Older clients (cua 0.6.x/0.7.x, Spaces 0.12/0.13) never see server connections and keep their per-device links. Cloud resources are tagged the same way, so sweep and audit work on both.
- New fields on MCP inputs and outputs are optional. `cloud_connect` without `auth` keeps today's CLI-sign-in behaviour. The contract manifest is additive; there is no `buf breaking` impact.
- The issuer is new, so nothing depends on it until a user opts in. When `oidc.cua.ai` is unreachable, OIDC connections fail closed with "Can't reach Cua's sign-in for AWS", and CLI-sign-in links are unaffected.

## 10. Rollout (PRs)

1. `cua-oidc` crate (issuer, signer trait with KMS and file signers, registry on Postgres (tokio-postgres) with an in-memory test store, token policy, audit) and the relay's internal device lookup, with tests. *Opus.*
2. Cloud repo GitOps PRs: KMS key (Terraform), CNPG, Deployment, IngressRoute, DNS for `oidc.staging.cua.ai`, ExternalSecrets, and the build workflow pin. Staging first; prod waits for sign-off.
3. cua-byoc federation for AWS: `Auth`, `IssuerClient`, the credential provider, the CloudFormation template plus drift test, the non-interactive `connect` with status/next/verify, MCP field changes, and the agent docs page. Real test on `cua-byoc-sandbox` (us-west-2, `cua-owner=claude-oidc`, TTL ≤ 4 h). *Opus.*
4. App: the Clouds group in Computers, the cloud page, sheets and logos, approval, plus both follow-ups. *Sonnet*, previews rendered before merge.
5. Sealed tokens for Hetzner and Modal on pool keys. *Opus.*
6. Google Cloud and Azure federation (code plus templates; real tests only with sign-off).
7. Usage, per-connection budgets and alerts, CSV export.

## Open points (decided)

- **Signer:** a dedicated `cua-oidc` service with an AWS KMS RSA key. Not the relay, not Keycloak.
- **Replay across accounts:** prevented by the per-account issuer path plus `sub`, and AWS, GCP and Azure each also check `aud`.
- **Self-hosted relays as issuers:** later. The crate runs with a `FileSigner`, and the client takes `CUA_OIDC_ISSUER_URL`, so a self-hoster can point their own trust at their own issuer.

## Appendix: the issuer's HTTP API (as built)

Base URL `https://oidc.cua.ai` (staging `https://oidc.staging.cua.ai`). Errors
are `{"error": "<code>", "message": "<plain words>"}`.

**Public:** `GET /a/{acct}/.well-known/openid-configuration`,
`GET /a/{acct}/.well-known/jwks.json`, `GET /.well-known/jwks.json`,
`GET /v1/templates/aws.yaml?acct={acct}`, `GET /healthz`, `GET /readyz`.

**Signed in** (`Authorization: Bearer <Keycloak access token>`, audience
`cua-oidc`): `GET /v1/account` -> `{account, issuer, subject}`;
`GET /v1/connections` -> `{connections: [...]}`; `GET /v1/connections/{id}`;
`GET /v1/audit?connection=&limit=` -> `{events: [...]}` (newest first).

**Signed in plus a device proof** (`DPoP: <proof>`):

| request | proof `connection` / `purpose` | answer |
|---|---|---|
| `POST /v1/token {connection, purpose, audience, agent?}` | the connection / the token purpose | 200 `{token, issuer, subject, audience, jti, expires_at, expires_in}` |
| `POST /v1/connections {cloud, identifiers, region?, limits?, sealed_token?}` | `""` / `connection.create` | 201 connection (`pending` when an agent asks) |
| `POST /v1/connections/{id}/limits {budget_usd_month?, max_size?, ttl_hours?}` | id / `connection.limits` | 200 `{connection, approval_required}` |
| `POST /v1/connections/{id}/approve` | id / `connection.approve` (no agent) | 200 connection |
| `POST /v1/connections/{id}/deny` | id / `connection.deny` (no agent) | 200 connection: an agent's raise dropped (`pending_limits` and `pending_agent` cleared), a `pending` link `revoked` |
| `POST /v1/connections/{id}/revoke` | id / `connection.revoke` | 200 connection (a sealed token's ciphertext is dropped) |
| `POST /v1/connections/{id}/sealed-token {sealed_token, expected_version?, expected_generation?}` | id / `connection.seal` | 200 connection; 409 `conflict` when the connection is no longer the version, or its token no longer the generation, the caller read (at least one is required); an agent's token sets `pending` |
| `DELETE /v1/connections/{id}` | id / `connection.delete` | 204 |

**Device proof:** a compact JWS, header `{"typ": "dpop+jwt", "alg": "ES256",
"jwk": {"kty": "EC", "crv": "P-256", "x", "y"}}` (the device's enrolled key),
claims `{htu, htm, iat, jti, ath, connection, purpose, agent?}`: `htu` the
request URL without query on the public base URL, `htm` the method, `iat`
within 60 s, `jti` 16-128 characters and never reused, `ath` the base64url
SHA-256 of the access token, `agent` absent or null when a person acts. The
device id is `dev_` + the first 24 hex characters of SHA-256 over the
uncompressed public key, as the relay and `cua-host` form it.

**Account id:** `acct` = the first 12 characters of lowercase, unpadded
base32 of HMAC-SHA256(issuer secret, `"cua-oidc-account/v1\n"` + Keycloak
account claim). Stable while the secret is kept, not reversible without it.

**Error codes:** 400 `invalid_request`, `audience_not_allowed`,
`connection_not_oidc`, `connection_not_sealed`; 401 `invalid_token`, `invalid_proof`,
`proof_replayed`; 403 `device_not_enrolled`, `device_revoked`,
`connection_pending`, `connection_revoked`, `budget_required`,
`budget_exhausted`, `agent_cannot_approve`; 404 `connection_not_found`; 409
`connection_exists`, `nothing_to_approve`, `conflict`; 503
`storage_unavailable`, `device_directory_unavailable`, `signing_unavailable`.
