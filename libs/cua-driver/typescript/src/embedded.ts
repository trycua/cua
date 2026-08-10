/**
 * Embedded-host exports backed by the same Rust/UniFFI implementation as the
 * package root. This subpath is organizational only; it contains no independent
 * process, socket, authorization, or restart logic.
 */
export {
  EmbeddedCuaDriverHost,
  EmbeddedDriverConnection,
  EmbeddedDriverError,
  EmbeddedDriverExit,
  EmbeddedDriverHostOptions,
  EmbeddedDriverHostState,
  EmbeddedEnvironmentVariable,
  EmbeddedMcpConfiguration,
  EmbeddedPermissionMode,
} from "./native/cua_driver_sdk.js"
