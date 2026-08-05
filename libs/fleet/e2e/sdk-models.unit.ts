import { deriveWarmPoolPhase } from "../src/sdk/status.js"

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, got ${String(actual)}`)
  }
}

assertEqual(
  deriveWarmPoolPhase(0, { replicas: 0, readyReplicas: 0 }),
  "Ready",
  "a reconciled zero-sized pool is healthy",
)
assertEqual(
  deriveWarmPoolPhase(0, undefined),
  "Provisioning",
  "a pool without observed status is still provisioning",
)
assertEqual(
  deriveWarmPoolPhase(2, { replicas: 2, readyReplicas: 1 }),
  "Provisioning",
  "a partially ready pool is provisioning",
)
assertEqual(
  deriveWarmPoolPhase(2, { replicas: 2, readyReplicas: 2 }),
  "Ready",
  "a fully ready pool is healthy",
)
