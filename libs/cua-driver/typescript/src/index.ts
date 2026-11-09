/**
 * Rust-backed SDK for Cua Driver client applications.
 *
 * Agents should configure `cua-driver mcp` through their runtime's existing
 * MCP client instead of importing a language MCP facade.
 */
import {
  CuaDriver as NativeCuaDriver,
  type CuaDriverLike,
  type DriverOptions,
  SdkClientKind,
} from "./native/cua_driver_sdk.js"
import { createDriver } from "./daemon.js"

/**
 * Options for `CuaDriver.create()`. `embedded: true` forces the in-process
 * runtime on macOS, where `create()` otherwise prefers the CuaDriver.app
 * daemon so the agent cursor overlay and the app's permissions work.
 */
export type CreateOptions = DriverOptions & { embedded?: boolean }

const CuaDriver = NativeCuaDriver as typeof NativeCuaDriver & {
  create(options?: CreateOptions): CuaDriverLike
}
type CuaDriver = NativeCuaDriver

// The same native library backs Python and TypeScript. The package root tags
// both the canonical same-process constructor and the temporary daemon
// compatibility constructor with the importing runtime.
NativeCuaDriver.create = (options?: CreateOptions) => {
  const { embedded, ...driverOptions } = options ?? {}
  return createDriver(
    () => NativeCuaDriver.connectWithClientKind(undefined, SdkClientKind.Typescript),
    () =>
      NativeCuaDriver.createWithClientKind(
        options === undefined ? undefined : (driverOptions as DriverOptions),
        SdkClientKind.Typescript,
      ),
    embedded === true,
    (driverOptions as Partial<DriverOptions>).claudeCodeCompatibility === true,
  )
}
NativeCuaDriver.createConfigured = (options) =>
  NativeCuaDriver.createConfiguredWithClientKind(options, SdkClientKind.Typescript)
NativeCuaDriver.createConfiguredWithAuthorizationHost = (options, host) =>
  NativeCuaDriver.createConfiguredWithAuthorizationHostAndClientKind(
    options,
    host,
    SdkClientKind.Typescript,
  )
NativeCuaDriver.createConfiguredWithActivityObserver = (options, observer) =>
  NativeCuaDriver.createConfiguredWithActivityObserverAndClientKind(
    options,
    observer,
    SdkClientKind.Typescript,
  )
NativeCuaDriver.createConfiguredWithHostIntegrations = (options, host, observer) =>
  NativeCuaDriver.createConfiguredWithHostIntegrationsAndClientKind(
    options,
    host,
    observer,
    SdkClientKind.Typescript,
  )
NativeCuaDriver.createPrivateWorker = (options) =>
  NativeCuaDriver.createPrivateWorkerWithClientKind(options, SdkClientKind.Typescript)
NativeCuaDriver.connect = (socketPath: string | undefined) =>
  NativeCuaDriver.connectWithClientKind(socketPath, SdkClientKind.Typescript)

export * from "./native/index.js"
export { CuaDriver }
export { default } from "./native/index.js"
