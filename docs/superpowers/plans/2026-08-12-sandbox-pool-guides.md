# Sandbox Pool Guides Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Fleets documentation into two directly nested Sandbox how-to guides: one for Terraform and one for Python.

**Architecture:** The public MDX content and local-preview redirects live in `trycua/cua`; production redirects live in `trycua/cloud`. The Terraform page preserves the provider-specific `fleets_pool` vocabulary while presenting the user-facing concept as a sandbox pool, and the Python page uses the supplied PEP 723 script unchanged except for surrounding instructional prose.

**Tech Stack:** MDX, Fumadocs metadata, Next.js redirects, TypeScript, Vitest, Terraform HCL, Python 3.11+

## Global Constraints

- Work only in `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs` and `/home/node/workspace/repos/cloud/.worktrees/sandbox-pool-redirect`.
- Put both new pages directly under `docs/content/docs/how-to-guides/sandbox/`; do not create a nested `pools/` directory.
- Preserve `/docs/how-to-guides/fleets/configure-run-cua-fleets` with a permanent redirect to `/docs/how-to-guides/sandbox/configure-pool-with-terraform`.
- Redirect the older `/docs/how-to-guides/(fleets)/configure-run-cua-fleets` alias directly to the same destination.
- Do not commit changes unless the user explicitly requests a commit.

---

### Task 1: Add Production Redirect Coverage

**Files:**

- Modify: `/home/node/workspace/repos/cloud/.worktrees/sandbox-pool-redirect/src/website/app/lib/docs/redirects.test.ts`
- Modify: `/home/node/workspace/repos/cloud/.worktrees/sandbox-pool-redirect/src/website/app/lib/docs/redirects.ts`

**Interfaces:**

- Consumes: `resolveDocsRedirect(slug: string): string | null`
- Produces: Permanent legacy mappings for both Fleets guide slugs.

- [ ] **Step 1: Add failing redirect cases**

Add these cases to the `it.each` table:

```ts
[
  'how-to-guides/fleets/configure-run-cua-fleets',
  '/docs/how-to-guides/sandbox/configure-pool-with-terraform',
],
[
  'how-to-guides/(fleets)/configure-run-cua-fleets',
  '/docs/how-to-guides/sandbox/configure-pool-with-terraform',
],
```

- [ ] **Step 2: Verify the new current-route case fails**

Run: `corepack pnpm test -- app/lib/docs/redirects.test.ts`

Expected: FAIL because `how-to-guides/fleets/configure-run-cua-fleets` has no redirect and the parenthesized alias still targets the removed Fleets route.

- [ ] **Step 3: Add both production redirects**

Add direct entries to `DOCS_REDIRECTS`:

```ts
'how-to-guides/fleets/configure-run-cua-fleets':
  '/docs/how-to-guides/sandbox/configure-pool-with-terraform',
'how-to-guides/(fleets)/configure-run-cua-fleets':
  '/docs/how-to-guides/sandbox/configure-pool-with-terraform',
```

- [ ] **Step 4: Verify the redirect test passes**

Run: `corepack pnpm test -- app/lib/docs/redirects.test.ts`

Expected: PASS with all redirect cases green.

### Task 2: Restructure Sandbox Pool Documentation

**Files:**

- Create: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/content/docs/how-to-guides/sandbox/configure-pool-with-terraform.mdx`
- Create: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/content/docs/how-to-guides/sandbox/create-pool-with-python.mdx`
- Delete: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/content/docs/how-to-guides/fleets/configure-run-cua-fleets.mdx`
- Delete: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/content/docs/how-to-guides/fleets/meta.json`
- Modify: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/content/docs/how-to-guides/meta.json`
- Modify: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/content/docs/how-to-guides/sandbox/meta.json`

**Interfaces:**

- Consumes: Existing Terraform guide content and the user-supplied `cua_sandbox.Image`/`Pool` script.
- Produces: `/how-to-guides/sandbox/configure-pool-with-terraform` and `/how-to-guides/sandbox/create-pool-with-python`.

- [ ] **Step 1: Verify the desired structure is absent**

Run:

```bash
test -f docs/content/docs/how-to-guides/sandbox/configure-pool-with-terraform.mdx &&
test -f docs/content/docs/how-to-guides/sandbox/create-pool-with-python.mdx &&
! test -d docs/content/docs/how-to-guides/fleets
```

Expected: FAIL because both new pages are absent and the Fleets directory exists.

- [ ] **Step 2: Move and reframe the Terraform guide**

Move the existing guide to `sandbox/configure-pool-with-terraform.mdx`, use the title `Configure a sandbox pool with Terraform`, replace general user-facing “fleet” wording with “sandbox pool,” retain literal provider/API identifiers such as `trycua/fleets`, `fleets_pool`, and Fleet authentication names, and keep the Linux/Windows, sizing, apply, verify, destroy, and troubleshooting sections.

- [ ] **Step 3: Add the Python guide**

Create `sandbox/create-pool-with-python.mdx` with:

```yaml
---
title: Create a sandbox pool with Python
description: Create a reusable warm sandbox pool, claim a sandbox, and run commands with the Cua Sandbox SDK.
---
```

Document Python `>=3.11,<3.14`, `uv`, Fleet authentication through `FLEETS_TOKEN` or `CUA_CLIENT_ID` plus `CUA_CLIENT_SECRET`, the default `https://run.cua.ai` endpoint, the supplied PEP 723 script, `uv run create_pool.py`, reusable pool behavior, claim release on context-manager exit, screenshot output, shell result checking, and optional `CUA_POOL_NAME`/`CUA_CLAIM_NAME` overrides.

- [ ] **Step 4: Flatten navigation metadata**

Remove `fleets` from `how-to-guides/meta.json`. Add `configure-pool-with-terraform` and `create-pool-with-python` to the Sandbox page list after `lifecycle` in `sandbox/meta.json`.

- [ ] **Step 5: Remove the old Fleets section**

Delete the now-empty `docs/content/docs/how-to-guides/fleets/` directory.

- [ ] **Step 6: Verify the structure check passes**

Run the Step 1 command again.

Expected: PASS.

### Task 3: Add Local Redirects and Validate

**Files:**

- Modify: `/home/node/workspace/repos/cua/.worktrees/sandbox-pool-docs/docs/next.config.mjs`

**Interfaces:**

- Consumes: The two retired Fleets URLs.
- Produces: Permanent redirects in the local Next.js preview matching production behavior.

- [ ] **Step 1: Add local-preview redirects**

Add permanent redirect objects for `/how-to-guides/fleets/configure-run-cua-fleets` and `/how-to-guides/(fleets)/configure-run-cua-fleets`, both targeting `/how-to-guides/sandbox/configure-pool-with-terraform`.

- [ ] **Step 2: Run Cua docs validation**

Run:

```bash
corepack pnpm docs:check-links
corepack pnpm docs:check-hygiene
corepack pnpm build
```

Expected: All commands pass; the static route list includes both new Sandbox guide paths and excludes the removed Fleets page.

- [ ] **Step 3: Run Cloud redirect validation**

Run: `corepack pnpm test -- app/lib/docs/redirects.test.ts`

Expected: PASS.

- [ ] **Step 4: Review both worktree diffs**

Run `git diff --check` and `git status --short` in both worktrees. Confirm no generated files, dependency lockfiles, or unrelated paths changed.
