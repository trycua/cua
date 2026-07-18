import {
  connect,
  type Claim,
  type CreateClaimRequest,
  type CreatePoolRequest,
  type Configuration,
  type Pool,
  type PoolSpec,
  type Sandbox,
  type ServiceFetch,
} from "../src/index.js"

const configuration: Configuration = {
  baseUrl: "https://run.example",
  oauth: { tokenUrl: "https://auth.example/token", clientId: "id", clientSecret: "secret" },
}
const poolSpec: PoolSpec = {
  replicas: 1,
  template: { containerDiskImage: "example/workspace:latest", cpuCores: 4, memory: "4Gi" },
}
const pool: Pool = {
  apiVersion: "cua.ai/v1",
  kind: "OSGymWorkspacePool",
  metadata: { namespace: "pool-one", name: "pool-one" },
  spec: { replicas: 1, template: { containerDiskImage: "example/workspace:latest", cpuCores: 4, memory: "4Gi" } },
}
const claim: Claim = {
  apiVersion: "osgym.cua.ai/v1alpha1",
  kind: "OSGymSandboxClaim",
  metadata: { namespace: "pool-one", name: "claim-one" },
  spec: { sandboxTemplateRef: { name: "pool-one-template" } },
}

async function contract(): Promise<Sandbox> {
  const sdk = await connect(configuration)
  const poolRequest: CreatePoolRequest = { namespace: "pool-one", spec: poolSpec }
  const createdPool = await sdk.createPool(poolRequest)
  await sdk.listPools(createdPool.metadata.namespace)
  await sdk.getPool(pool)
  await sdk.updatePool(pool)
  const claimRequest: CreateClaimRequest = { pool: createdPool }
  const createdClaim = await sdk.createClaim(claimRequest)
  await sdk.listClaims(createdClaim.metadata.namespace)
  await sdk.getClaim(claim)
  await sdk.updateClaim(claim)
  const sandbox = await sdk.waitClaim(claim)
  const service: ServiceFetch = sdk.serviceFetch(sandbox, "mcp")
  const response: Response = await service("/mcp", { method: "POST", body: "{}" })
  void response
  // @ts-expect-error URL-based authenticated clients are intentionally not public.
  void sdk.rawClient
  await sdk.deleteClaim(claim)
  await sdk.deletePool(pool)
  sdk.close()
  return sandbox
}
void contract
