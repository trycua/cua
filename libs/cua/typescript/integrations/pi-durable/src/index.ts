// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `@trycua/pi-durable`: Cua Spaces as the execution environment of Pi Durable.
 *
 * ```ts
 * import { embedded, SandboxCreateOptions } from "@trycua/cua"
 * import { createRegistry, Harness } from "@earendil-works/pi-durable"
 * import { CodingTools } from "@earendil-works/pi-durable/tools"
 * import { CuaSandboxSpaces, durableSpaces, WorkspaceSnapshots } from "@trycua/pi-durable"
 *
 * const spaces = new CuaSandboxSpaces({
 *   sandboxes: embedded().sandboxes(),
 *   createOptions: (name) => SandboxCreateOptions.create({ image: "linux", name, kind: "container" }),
 * })
 * const durable = durableSpaces({
 *   spaces,
 *   snapshots: new WorkspaceSnapshots({ spaces, root: "/home/cua/work" }),
 *   cwd: "/home/cua/work",
 * })
 * const registry = createRegistry()
 * registry.install(CodingTools)
 * registry.install(durable.extension) // wraps bash, write and edit
 * const harness = await Harness.open(storage, { models, registry, env: durable.env }, context)
 * ```
 */

export {
  type DurableSpaces,
  type DurableSpacesOptions,
  durableSpaces,
  ImageDoc,
  type RecoveryEvent,
  SpaceDoc,
} from "./durable.js"
export {
  FencedFileSystem,
  FencedSqliteDatabase,
  type Fence,
  type FencedSqlite,
  HaltError,
  haltOf,
  isStaleEpochError,
  openFencedSqlite,
  openFencedSqliteStorage,
  StaleEpochError,
  staleEpochOf,
} from "./fence.js"
export {
  DEFAULT_TRANSACTIONAL_LIMITS,
  type ImageFile,
  type ImageState,
  TransactionalLimitExceeded,
  type TransactionalLimits,
  TransactionalUnsupported,
} from "./transactional.js"
export type { CheckpointRecord, SpaceState } from "./durable.js"
export { SpacesExecutionEnv, type SpacesExecutionEnvOptions } from "./env.js"
export {
  type CheckpointStore,
  checkpointLabel,
  type CuaSandboxesLike,
  CuaSandboxSpaces,
  type CuaSandboxSpacesOptions,
  type CuaCheckpointsLike,
  DirectoryCheckpointStore,
  SandboxSnapshots,
  type SnapshotProvider,
  type SpaceProvider,
  WorkspaceSnapshots,
  type WorkspaceSnapshotsOptions,
} from "./providers.js"
export {
  advanceEpoch,
  type ExecRecord,
  getExecRecord,
  type SpacesdCommandLike,
  type SpacesdLike,
  type SpacesdProcessLike,
} from "./spacesd.js"
export {
  type DriverContext,
  type DriverRun,
  type DurableSessionOptions,
  EXIT_CRASHED,
  EXIT_DONE,
  EXIT_FENCED,
  EXIT_PERMANENT,
  PermanentFailure,
  readSessionEnv,
  runDurableSession,
  startHeartbeat,
  type SessionInfo,
} from "./driver.js"
