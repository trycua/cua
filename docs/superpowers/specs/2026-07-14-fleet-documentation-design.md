# Fleet Documentation Design

## Goal

Add public documentation for Fleet, CUA's managed cloud sandbox platform, while preserving the documentation site's existing Diátaxis information architecture.

## Audience

The pages serve developers who need to understand Fleet's orchestration model, prepare reusable sandbox images, connect to sandbox services, automate Fleet through the SDK, and build or evaluate computer-use agents at scale.

## Information Architecture

Fleet content remains under the site's existing reader-intent sections rather than introducing a new top-level product section.

### Concepts

Create `docs/content/docs/concepts/fleet/` with:

- `index.mdx`: Fleet's role and the relationship between images, pools, claims, autoscaling, and services.
- `images.mdx`: Immutable starting environments, image identity, versioning, and reuse.
- `pools.mdx`: Groups of compatible sandboxes, desired capacity, placement, and lifecycle ownership.
- `claims.mdx`: How workloads request and hold a sandbox from a pool, including release and failure behavior.
- `auto-scaling.mdx`: How demand, warm capacity, limits, and scale-down interact.
- `services.mdx`: How a claimed sandbox exposes HTTP, desktop, MCP, and other workload endpoints.
- `meta.json`: Ordered Fleet concept navigation.

Add `fleet` to `docs/content/docs/concepts/meta.json`.

### How-to Guides

Create `docs/content/docs/how-to-guides/fleet/` with:

- `index.mdx`: Entry point that directs readers to each Fleet task.
- `build-a-linux-image.mdx`: Build and validate a Linux image suitable for Fleet pools.
- `build-a-windows-image.mdx`: Build and validate a Windows image suitable for Fleet pools.
- `connect-to-a-service.mdx`: Claim a sandbox, discover a service endpoint, authenticate, and connect.
- `use-the-sdk.mdx`: Authenticate, create or select Fleet resources, claim a sandbox, inspect status, and release it.
- `meta.json`: Ordered Fleet how-to navigation.

Add `fleet` to `docs/content/docs/how-to-guides/meta.json`.


## Page Conventions

Every page uses MDX frontmatter with a concise `title` and `description`. Public product terminology uses “Fleet” and “sandbox”; “SBX” does not appear as a product name.

Concept pages explain relationships, boundaries, lifecycle, and tradeoffs without becoming procedural guides. How-to guides start with prerequisites, solve one concrete task, include verification, and stop after the desired result.

Code examples use the actual Fleet interfaces available in `trycua/cloud/cyclops-cs` and the current CUA repositories. If a requested workflow has no stable public API, the page states the supported boundary and uses only verified commands or configuration rather than inventing an interface.

## Cross-Linking

The Fleet index pages link between concepts and how-to guides. Individual pages link to prerequisite concepts, related tasks, and existing Sandbox SDK reference pages where appropriate. Existing pages are changed only when a Fleet link materially improves navigation.

## Validation

Run the repository's documentation checks after authoring:

```bash
cd docs
pnpm docs:check-hygiene
pnpm docs:check-links
pnpm build
```

The change is complete when all requested pages appear in navigation, internal links resolve, MDX builds successfully, examples match verified interfaces, and no generated or unrelated files are committed.
