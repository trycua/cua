# docs

Public documentation content and assets live in this repository. The production
renderer, redirects, analytics, and site configuration live in `trycua/cloud`.
This app is a local MDX preview for contributors to the public repository.

Run the local preview:

```bash
pnpm dev
```

Open http://localhost:8090 with your browser to see the result.

## Docs conventions

Public docs live in `content/docs/` and follow the Diátaxis modes:

- `how-to-guides/` give steps for a specific goal.
- `explanation/` explains concepts, constraints, and tradeoffs.
- `reference/` is dry lookup: commands, APIs, contracts, limits.

Place content by what the reader is trying to do, not by topic. Do not mix modes in one page; move reference tables to reference pages and link to them from how-to guides or explanations.

## Learn More

To learn more about Next.js and Fumadocs, take a look at the following
resources:

- [Next.js Documentation](https://nextjs.org/docs) - learn about Next.js
  features and API.
- [Learn Next.js](https://nextjs.org/learn) - an interactive Next.js tutorial.
- [Fumadocs](https://fumadocs.vercel.app) - learn about Fumadocs
