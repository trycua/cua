// Hidden docs prelude `pool-applied`: the `fleet` and pool (POOL_NAME) an
// earlier section applied.
import { embedded as __cuaDocsE, Pool as __cuaDocsPool, poolOptions as __cuaDocsOpts, sandboxSpec as __cuaDocsSpec } from '@trycua/cua';

const POOL_NAME = (process.env.CUA_POOL_NAME ??= 'cua-e2e-docs-pool');
const fleet = __cuaDocsE().fleet();
await __cuaDocsPool.apply(fleet, POOL_NAME, __cuaDocsSpec('python:3.12-slim', {}), __cuaDocsOpts({ replicas: 1 }));
