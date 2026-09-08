import {
  CreateClaimRequestBuilder,
  type Claim as SdkClaim,
  type CreateClaimRequest,
  type CyclopsClient,
  type Pool,
} from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import { withClient } from "../auth/cyclops-client"
import type { Claim } from "./models"
import { isLocalVisualPreview } from "../local-visual-preview"

export function buildClaimRequest(pool: Pool): CreateClaimRequest {
  return new CreateClaimRequestBuilder().pool(pool).build()
}

function claimModel(claim: SdkClaim): Claim {
  return {
    name: claim.metadata.name,
    namespace: claim.metadata.namespace,
    templateRef: claim.spec.sandboxTemplateRef.name,
    warmpool: claim.spec.warmpool ?? "default",
    phase: claim.status?.phase ?? "Pending",
    sandboxName: claim.status?.sandbox?.name,
    sandboxService: claim.status?.sandbox?.service,
    ttlSecondsAfterCreated: claim.spec.ttlSecondsAfterCreated,
    createdAt: claim.metadata.creationTimestamp ?? "",
  }
}

async function findClaim(
  client: CyclopsClient,
  namespace: string,
  name: string,
): Promise<SdkClaim> {
  const claim = (await client.listClaims(namespace)).find(
    candidate => candidate.metadata.name === name,
  )
  if (!claim) throw new Error(`Claim ${name} was not found`)
  return claim
}

export async function listClaims(namespace: string): Promise<Claim[]> {
  if (isLocalVisualPreview()) return []
  return withClient(async client =>
    (await client.listClaims(namespace)).map(claimModel),
  )
}

export async function createClaim(namespace: string, poolName: string): Promise<Claim> {
  return withClient(async client => {
    const pool = await client.getPool(poolName)
    if (pool.metadata.namespace !== namespace) {
      throw new Error(`Pool ${poolName} belongs to namespace ${pool.metadata.namespace}`)
    }
    return claimModel(await client.createClaim(buildClaimRequest(pool)))
  })
}

export async function getClaim(namespace: string, name: string): Promise<Claim> {
  return withClient(async client => claimModel(await findClaim(client, namespace, name)))
}

export async function deleteClaim(namespace: string, name: string): Promise<void> {
  await withClient(async client => {
    await client.deleteClaim(await findClaim(client, namespace, name))
  })
}
