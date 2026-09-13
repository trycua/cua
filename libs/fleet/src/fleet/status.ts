import {
  removedPoolDisplayStatus,
  terminatingPoolDisplayStatus,
  unknownPoolDisplayStatus,
  type PoolDisplayStatus,
} from "../../sdk-bindings/ts-uniffi-browser/ts/index.web"
import type { PoolSummary } from "./models"

export function historicalPoolStatus(
  poolId: string,
  currentPools: PoolSummary[] | null,
): PoolDisplayStatus {
  if (!currentPools) return unknownPoolDisplayStatus()
  return currentPools.find(
    pool => `${pool.namespace}:${pool.name}` === poolId,
  )?.status ?? removedPoolDisplayStatus()
}

const tombstones = new Set<string>()

export function tombstonePool(name: string): void {
  tombstones.add(name)
}

export function applyPoolTombstones(pools: PoolSummary[]): PoolSummary[] {
  const names = new Set(pools.map(pool => pool.name))
  for (const tombstone of tombstones) {
    if (!names.has(tombstone)) tombstones.delete(tombstone)
  }
  return pools.map(pool =>
    tombstones.has(pool.name)
      ? { ...pool, status: terminatingPoolDisplayStatus() }
      : pool,
  )
}
