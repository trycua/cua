# docs

Public documentation content and assets live in this repository. The production
renderer, redirects, analytics, and site configuration live in `trycua/cloud`.
This app is a local MDX preview for contributors to the public repository.

Install the docs dependencies and run the local preview from this directory:

```bash
pnpm install --frozen-lockfile
pnpm dev
```

Open http://localhost:8090 with your browser to see the result. The docs app has
its own lockfile; installing dependencies at the repository root is not enough.

## Validate a change

Curated MDX pages use the content checks and production build:

```bash
pnpm docs:check-hygiene
pnpm docs:check-links
pnpm build
```

For generated reference changes, also run the owning component check:

```bash
pnpm docs:check:cua-driver
pnpm docs:check:lume
```

`pnpm docs:check` is the explicit full Cua Driver and Lume audit.

## Docs conventions

Public docs live in `content/docs/` and follow the Diátaxis modes:

- `tutorials/` teach a guided first success.
- `how-to-guides/` give steps for a specific goal.
- `concepts/` contains Diátaxis explanations of concepts, constraints, and
  tradeoffs.
- `reference/` is dry lookup: commands, APIs, contracts, limits.

Place content by what the reader is trying to do, not by topic. Do not mix modes in one page; move reference tables to reference pages and link to them from how-to guides or explanations.

## Maintainer runbooks

- [Release components with Release Please](component-release-workflow.md)
- [Backfill historical release metadata](release-backfill-runbook.md)

## Learn More

To learn more about Next.js and Fumadocs, take a look at the following
resources:

- [Next.js Documentation](https://nextjs.org/docs) - learn about Next.js
  features and API.
- [Learn Next.js](https://nextjs.org/learn) - an interactive Next.js tutorial.
- [Fumadocs](https://fumadocs.vercel.app) - learn about Fumadocs
