// Hidden docs prelude `team-pool`: the dedicated pool a page claims from,
// named by CUA_POOL_NAME (team-pool.subst.json replaces `my-team-desktop`).
import { embedded as __cuaDocsE, Pool as __cuaDocsPool, poolOptions as __cuaDocsOpts, sandboxSpec as __cuaDocsSpec } from '@trycua/cua';

await __cuaDocsPool.apply(
  __cuaDocsE().fleet(),
  process.env.CUA_POOL_NAME!,
  __cuaDocsSpec('ghcr.io/trycua/linux:24.04', { services: { env: 3211 } }),
  __cuaDocsOpts({ replicas: 1 })
);
