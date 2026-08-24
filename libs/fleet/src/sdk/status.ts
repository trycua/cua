export interface WarmPoolObservedStatus {
  replicas?: number
  readyReplicas?: number
}

export function deriveWarmPoolPhase(
  specReplicas: number,
  status: WarmPoolObservedStatus | undefined,
): "Ready" | "Provisioning" {
  if (!status) return "Provisioning"
  const expected = status.replicas ?? specReplicas
  return (status.readyReplicas ?? 0) >= expected ? "Ready" : "Provisioning"
}

const tombstones = new Set<string>()

export function tombstonePool(name: string): void {
  tombstones.add(name)
}

export function reconcileTombstones(present: Iterable<string>): void {
  const names = new Set(present)
  for (const tombstone of tombstones) {
    if (!names.has(tombstone)) tombstones.delete(tombstone)
  }
}

export function isTombstoned(name: string): boolean {
  return tombstones.has(name)
}

export type PoolStatus =
  | { kind: "healthy"; label: "Healthy" }
  | { kind: "provisioning"; label: "Provisioning" }
  | { kind: "terminating"; label: "Terminating" }
  | { kind: "unknown"; label: "Unknown" }

export function derivePoolStatus(pool: { name: string; phase: string }): PoolStatus {
  if (isTombstoned(pool.name)) {
    return { kind: "terminating", label: "Terminating" }
  }
  const phase = (pool.phase ?? "").toLowerCase()
  if (phase === "ready") return { kind: "healthy", label: "Healthy" }
  if (phase === "provisioning") {
    return { kind: "provisioning", label: "Provisioning" }
  }
  return { kind: "unknown", label: "Unknown" }
}
