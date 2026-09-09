# Browser bash chat

Cyclops includes an authenticated chat UI whose agent loop runs inside the
browser. The model may request a single `bash` function tool implemented with
`just-bash/browser`; the browser executes it in a temporary virtual filesystem
and returns the result to the backend for the next model turn. Registered Bash
commands bridge that isolated shell to the authenticated Cyclops SDK and the
read-only CUA documentation and versioned code MCP service.

The Go backend owns conversation history, authentication, authorization, and
LiteLLM access. The current conversation store is in memory, so history is lost
when the backend process restarts. The store interface is intended to support a
PostgreSQL implementation later.

## Security boundary

The bash runtime is browser-local and isolated:

- It cannot access the host filesystem.
- It cannot make network requests.
- Its virtual filesystem is temporary and exists only in browser memory.
- Each conversation receives a separate shell and virtual filesystem.
- Refreshing or closing the page discards the browser-local filesystem, while
  persisted transcript history remains available until the backend restarts.
- Tool-provided `timeout_ms` and `max_output_chars` values are clamped by the
  browser runtime before execution.
- Requests are capped at 256 KiB, individual messages at 128 KiB, retained
  conversations at 256 messages and 1 MiB of model context, and streamed model
  output at 128 KiB. The in-memory backend also caps each owner at 100
  conversations and the process-wide transcript store at 64 MiB.

There is no Playwright tool, browser automation, Chromium process, Node
sidecar, or arbitrary Bash networking. Four explicit read-only MCP commands are
registered for CUA documentation and code lookup. The frontend does not receive
LiteLLM credentials.

## Command help skills

Every registered SDK and MCP command supports `-h` and `--help`. Help is the
command's progressively disclosed skill: it describes purpose, usage,
arguments, output shape, user-facing presentation, safety, and examples without
calling the backing SDK or MCP service.

The system prompt contains only the compact command catalog. Before first use
of a command in a conversation, the agent runs `<command> -h` in a separate
Bash call and follows the returned guidance. It does not repeat help for that
command in the same conversation unless it needs a refresher.

Presentation rules live with command execution metadata. For example,
`listPools -h` requires a Markdown table with `Pool`, `Replicas`, `Available`,
and `Phase`; omits the redundant Namespace column; and links each pool name to
`/pools/<namespace>/<name>` using URL-encoded path segments.

## Local development

Install dependencies and run the frontend:

```bash
cd cyclops-cs
corepack pnpm install
corepack pnpm dev
```

Run the Go backend with chat enabled in a separate terminal:

```bash
cd cyclops-cs/backend
ENVIRONMENT=development \
CYCLOPS_CS_CHAT_ACCESS=all \
LITELLM_BASE_URL=http://localhost:4000/v1 \
LITELLM_MODEL=large \
LITELLM_API_KEY='<virtual-key>' \
go run .
```

Use an OpenAI-compatible LiteLLM endpoint and a restricted virtual key. Never
put `LITELLM_API_KEY` in Vite variables, frontend configuration, or browser
storage.

## Backend configuration

- `/feature-flags/cyclops-cs/chat-access` selects `disabled`, `restricted`, or `all`. Missing or invalid values default to `restricted`.
- `/feature-flags/cyclops-cs/chat-subs` is a JSON array of non-admin Keycloak `sub` values used in `restricted` mode; admins are enabled automatically.
- `LITELLM_BASE_URL` is the OpenAI-compatible API base URL, including `/v1`.
- `LITELLM_MODEL` selects the model alias and defaults to `large`.
- `LITELLM_API_KEY` is the backend-only LiteLLM virtual key.

Production resolves both chat flags from AWS SSM through OpenFeature and uses
the in-cluster LiteLLM service with a backend-only virtual key synced from AWS
Secrets Manager. The `SimpleEnvProvider` maps the flags to
`CYCLOPS_CS_CHAT_ACCESS` and `CYCLOPS_CS_CHAT_SUBS` for development and
previews. Set both `LITELLM_BASE_URL` and `LITELLM_API_KEY`, or leave both unset
to run without a configured chat model.

## API routes

All routes require the existing Cyclops authentication and authorization
middleware:

- `POST /api/chat/conversations` creates a conversation.
- `GET /api/chat/conversations` lists the calling user's conversations.
- `GET /api/chat/conversations/{id}` loads one owned conversation.
- `POST /api/chat/conversations/{id}/turns` appends user or tool messages and
  streams the assistant response as server-sent events.

In production, the backend stores user, assistant, tool-call, and tool-result
messages in the shared Cyclops Postgres database, so conversations survive
replica changes and pod restarts. The React UI keeps only transient rendering
and run state; it reloads canonical conversation history from the backend. If a browser run is stopped or rejects a
tool call after the assistant request is stored, the next retry or user prompt
records synthetic failed tool results before continuing, so the conversation is
not stranded.

## Preview environments

Chat is also enabled in the preview base at
`clusters/kopf-k3s/cyclops-cs-previews-base`. Each preview has a Flux
`GitRepository` pinned to the PR head SHA, so manifest and secret-wiring changes
are rendered from the same commit as the frontend/backend images. The backend
sidecar receives the four LiteLLM environment variables. The nginx/frontend
container receives none of them.

The `cyclops-cs` pull-request label is the trust gate for this backend secret.
Maintainers must apply it only to reviewed, same-repository PRs whose backend
code is trusted to receive the preview credential. Fork PRs are blocked from
publishing Cyclops images, and a base-branch `pull_request_target` workflow
automatically removes the label if it is applied to a fork PR. Remove the label
and revoke the credential if any enabled preview becomes untrusted.

An `ExternalSecret` named `cyclops-cs-litellm` reads property `api_key` from
AWS Secrets Manager path `kopf-k3s/cyclops-cs-browser-agent-litellm` and creates
a namespace-local Secret with the same name. Production syncs the same
restricted browser-agent credential into its own namespace.

The stored value must be a dedicated LiteLLM virtual key restricted to model
alias `large`. Configure a strict spend budget and an expiry, or use a documented
short rotation interval if expiry is unavailable. Rotate the key on schedule
and revoke it immediately if an enabled preview is no longer trusted. Never use
a general-purpose or unrestricted gateway key for previews.

## Validation

Before adding preview wiring, the source-tree assertion was:

```bash
rg -n "CYCLOPS_CS_CHAT_ACCESS|CYCLOPS_CS_CHAT_SUBS|LITELLM_BASE_URL|LITELLM_API_KEY|litellm-credentials.yaml" \
  clusters/kopf-k3s/cyclops-cs-previews-base
```

Expected baseline before wiring: no matches and exit status 1, proving the
preview chat configuration was not already present.

Validate the browser agent and UI:

```bash
cd cyclops-cs
corepack pnpm test:agent
corepack pnpm test:e2e -- agent-chat.spec.ts
corepack pnpm typecheck
corepack pnpm build
```

Verify the preview trust contract:

```bash
clusters/kopf-k3s/cyclops-cs-previews/tests/security-contract.sh
```

Render and inspect the preview manifests:

```bash
kubectl kustomize clusters/kopf-k3s/cyclops-cs-previews-base \
  > /tmp/cyclops-chat-preview.yaml
rg -n \
  "kind: ExternalSecret|CYCLOPS_CS_CHAT_ACCESS|CYCLOPS_CS_CHAT_SUBS|LITELLM_BASE_URL|LITELLM_MODEL|LITELLM_API_KEY" \
  /tmp/cyclops-chat-preview.yaml
```

Confirm the rendered environment variables appear only on
`cyclops-cs-backend`, the ExternalSecret references the expected AWS path and
property, and no secret value is present in the output.
