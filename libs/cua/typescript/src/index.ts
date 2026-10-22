/**
 * The cua SDK for Node: a generated binding over the Rust `cua-sdk` crate.
 *
 * ```ts
 * import { embedded, SandboxCreateOptions, http } from "@trycua/cua"
 * const cua = embedded()
 * const sb = await cua.sandboxes().create(SandboxCreateOptions.create({
 *   on: "local",                                // or "cloud"; kind / runtime: "auto" by default
 *   image: "python:3.12-slim",
 *   command: ["python", "-m", "my_mcp", "--port", "8765"],
 *   services: new Map([["mcp", 8765]]),
 *   waitFor: [http("mcp", "/health")],
 * }))
 * const r = await sb.service("mcp").request("POST", "/mcp", body, undefined, headers)
 * const url = await sb.service("mcp").url()           // usable from this machine
 * const share = await sb.publicUrl("mcp", 3600, undefined)  // shareable, expires
 * ```
 *
 * The same objects work against a running `cua daemon` (`connect()`).
 * Browser builds use `@trycua/cua/browser` (env over gRPC-Web).
 */
import { existsSync } from "node:fs"
import { createRequire } from "node:module"
import { dirname, join } from "node:path"
import {
  Container,
  Cua,
  CuaConfig,
  CuaError,
  CuaError_Tags,
  type CuaLike,
  type ErrorReport,
  Sandbox,
  type SandboxCreateOptions,
  type SandboxLike,
  Sandboxes,
  type SpaceCreateProgress,
  ReadinessProbe,
  RegistrySecret,
  type ResolvedImage,
  ambiguousSandboxCandidates,
  canonicalImageTier,
  errorDocUrl,
  omarchyImage,
  resolveImage,
  cuaSdkVersion,
  errorReport,
  telemetrySetSurface,
} from "./native/index.js"

export * from "./native/index.js"
export { connectMcp, mcpHeaders, type McpEndpointConfig } from "./mcp.js"

// Usage telemetry (anonymous, content-free; https://cua.ai/docs/cua-sdk/concepts/telemetry)
// attributes events to this binding. It sends nothing by itself; off with
// DO_NOT_TRACK=1, CUA_TELEMETRY=0 or `telemetrySetEnabled(false)`.
try {
  telemetrySetSurface("sdk_typescript", cuaSdkVersion())
} catch {
  // Telemetry never fails an import.
}

/**
 * The qualified refs (`local:box`, `cloud:box`) a `CuaError.AmbiguousSandbox`
 * lists: a bare sandbox name that matches sandboxes in more than one
 * location. Empty for any other error.
 */
export function ambiguousCandidates(error: unknown): string[] {
  if (!CuaError.AmbiguousSandbox.instanceOf(error)) return []
  return ambiguousSandboxCandidates(String((error as Error).message ?? error))
}

/**
 * The link to a `CuaError`'s entry (cause and fix) on the errors
 * reference; `undefined` for any other value. Every `CuaError` also carries
 * it as `docUrl`.
 */
export function cuaErrorDocUrl(error: unknown): string | undefined {
  if (!CuaError.instanceOf(error)) return undefined
  return errorDocUrl(String((error as { tag?: unknown }).tag ?? ""))
}

// `docUrl` on every CuaError variant (a getter on each variant class).
for (const tag of Object.values(CuaError_Tags)) {
  const variant = (CuaError as unknown as Record<string, { prototype: object } | undefined>)[tag]
  if (variant && !Object.prototype.hasOwnProperty.call(variant.prototype, "docUrl")) {
    Object.defineProperty(variant.prototype, "docUrl", {
      get(this: { tag?: unknown }) {
        return errorDocUrl(String(this.tag ?? ""))
      },
      configurable: true,
    })
  }
}

// `code`, `hint` and `report` on every CuaError variant, and a message Node
// prints: an uncaught error shows `CuaError.NotFound: sandbox nope` and its
// fix, not `NotFound [Error]`.
const REPORTS = new WeakMap<object, ErrorReport>()
function reportOf(error: { tag?: unknown; message?: unknown }): ErrorReport {
  let r = REPORTS.get(error)
  if (!r) {
    const tag = String(error.tag ?? "")
    const raw = String(error.message ?? "").replace(new RegExp(`^CuaError\\.${tag}: `), "")
    r = errorReport(tag, raw)
    REPORTS.set(error, r)
  }
  return r
}
for (const tag of Object.values(CuaError_Tags)) {
  const variant = (CuaError as unknown as Record<string, { prototype: object } | undefined>)[tag]
  if (!variant) continue
  const define = (key: PropertyKey, value: PropertyDescriptor) => {
    if (!Object.prototype.hasOwnProperty.call(variant.prototype, key)) {
      Object.defineProperty(variant.prototype, key, { configurable: true, ...value })
    }
  }
  define("report", { get(this: never) { return reportOf(this) } })
  define("code", { get(this: never) { return reportOf(this).code } })
  define("hint", { get(this: never) { return reportOf(this).hint } })
  define(Symbol.for("nodejs.util.inspect.custom"), {
    value(this: Error & { tag?: unknown }) {
      const r = reportOf(this)
      return `${this.message}${r.hint ? `\n  fix: ${r.hint}` : ""}\n${String(this.stack ?? "")
        .split("\n")
        .filter((l) => l.trim().startsWith("at "))
        .slice(0, 6)
        .join("\n")}`
    },
  })
}

/** What `Sandboxes.create` takes beside the options. */
export interface CreateExtra {
  /** Called with each step of the create (`phase`, `detail`, `fraction`, `bytesDone` ...). */
  onProgress?: (progress: SpaceCreateProgress) => void
}

declare module "./native/cua_sdk.js" {
  interface SandboxesLike {
    /** Creates a sandbox and tells `extra.onProgress` each step. */
    create(
      options: SandboxCreateOptions,
      extra: CreateExtra,
      asyncOpts_?: { signal: AbortSignal },
    ): Promise<SandboxLike>
    /** Deletes a sandbox by ref or unique name (same as `delete_`). */
    delete(name: string, asyncOpts_?: { signal: AbortSignal }): Promise<void>
  }
  interface SandboxLike {
    /** Deletes this sandbox (same as `delete_`). */
    delete(asyncOpts_?: { signal: AbortSignal }): Promise<void>
  }
}

/** The error's stable code (`not_found`), or `undefined` for any other value. */
export function cuaErrorCode(error: unknown): string | undefined {
  return CuaError.instanceOf(error) ? (error as unknown as { code: string }).code : undefined
}

/** The error's next step, one sentence. */
export function cuaErrorHint(error: unknown): string | undefined {
  return CuaError.instanceOf(error)
    ? (error as unknown as { hint?: string }).hint
    : undefined
}

/**
 * `delete` for `Sandboxes` and `Sandbox` (the generated name is `delete_`,
 * which a reserved word forced), and `onProgress` on `Sandboxes.create`.
 */
function installSandboxExtras(): void {
  /* eslint-disable @typescript-eslint/no-explicit-any */
  const sandboxes = Sandboxes.prototype as any
  const sandbox = Sandbox.prototype as any
  sandboxes.delete ??= function (this: any, ...a: unknown[]) {
    return this.delete_(...a)
  }
  sandbox.delete ??= function (this: any, ...a: unknown[]) {
    return this.delete_(...a)
  }
  const plain = sandboxes.create
  sandboxes.create = function (this: any, options: unknown, extra?: CreateExtra) {
    if (!extra?.onProgress) return plain.call(this, options, extra)
    const onProgress = extra.onProgress
    return this.createWithProgress(options, {
      onProgress(p: SpaceCreateProgress) {
        try {
          onProgress(p)
        } catch {
          // A progress callback never fails a create.
        }
      },
    })
  }
}
installSandboxExtras()

/**
 * Points `CUA_BIN` at the CLI of the matching `@trycua/cua-<platform>`
 * package, so the SDK can start the cua daemon (which serves local public
 * URLs) on demand. An explicit `CUA_BIN` wins.
 */
function defaultCuaBin(): void {
  if (process.env.CUA_BIN) return
  const arch = process.arch
  const triple =
    process.platform === "linux"
      ? `linux-${arch}-gnu`
      : process.platform === "win32"
        ? `win32-${arch}-msvc`
        : `${process.platform}-${arch}`
  try {
    const require = createRequire(import.meta.url)
    const exe = process.platform === "win32" ? "cua.exe" : "cua"
    const bin = join(dirname(require.resolve(`@trycua/cua-${triple}/package.json`)), exe)
    if (existsSync(bin)) process.env.CUA_BIN = bin
  } catch {
    // No platform package: the SDK looks for `cua` on PATH.
  }
}

/**
 * An SDK runtime in this process (no I/O until the first call). Pass
 * `accountFromSession: true` to use the `cua auth login` session for the
 * relay account the runtime lists "My machines" with.
 */
export function embedded(config: Partial<CuaConfig> = {}): CuaLike {
  defaultCuaBin()
  return Cua.embedded(CuaConfig.create(config))
}

/** A client of a running `cua daemon` (socket path or loopback URL). */
export function connect(address?: string, token?: string): CuaLike {
  return Cua.connect(address, token)
}

/** An image tier: `slim`, `full` (the default), or on macOS `xcode` / `xcode-<X.Y>`. */
export type ImageTier = "slim" | "full" | "xcode" | `xcode-${string}`

/** Options for `Image.linux()` / `windows()` / `macos()`. */
export interface ImageOptions {
  version?: string
  tier?: ImageTier
}

function tierImage(os: string, version?: string | ImageOptions, tier?: ImageTier): string {
  const o = typeof version === "object" ? version : { version, tier }
  return canonicalImageTier(os, o.version, o.tier)
}

/**
 * Canonical images: `Image.linux()` is `ghcr.io/trycua/linux:24.04`
 * (`CUA_IMAGE_LINUX` overrides it), `Image.windows()`
 * `ghcr.io/trycua/windows:2022`, `Image.macos()` `ghcr.io/trycua/macos:26`.
 * These are the full tier (dev tooling); `{ tier: "slim" }` is the minimal
 * image CI runs and, on macOS, `{ tier: "xcode" }` adds a pinned Xcode.
 * `Image.omarchy()` is `ghcr.io/trycua/omarchy:edge`, an amd64 VM. Images CI
 * has not published yet throw `ImageNotPublished`; pass the reference to
 * `fromRegistry` to use one anyway.
 * `Image.resolve(ref, backend)` is the one resolver: the digest-pinned
 * variant a backend runs (rootfs, the `-disk` containerDisk, Lume).
 */
export const Image = {
  linux: (version?: string | ImageOptions, tier?: ImageTier): string =>
    tierImage("linux", version, tier),
  windows: (version?: string | ImageOptions, tier?: ImageTier): string =>
    tierImage("windows", version, tier),
  macos: (version?: string | ImageOptions, tier?: ImageTier): string =>
    tierImage("macos", version, tier),
  /** Omarchy (Arch Linux, Hyprland) with cua-spacesd: an amd64 VM. */
  omarchy: (channel?: string): string => omarchyImage(channel),
  // Literal: `ubuntu:24.04` is docker.io/library/ubuntu:24.04.
  fromRegistry: (reference: string): string => reference,
  resolve: (reference: string, backend = "local", arch?: string): ResolvedImage =>
    resolveImage(reference, backend, arch),
}

/** Ready once a TCP connect to the declared `service` succeeds. */
export function tcp(service: string): ReadinessProbe {
  return ReadinessProbe.create({ service })
}

/** Ready once `GET path` on the declared `service` returns 2xx. */
export function http(service: string, path = "/"): ReadinessProbe {
  return ReadinessProbe.create({ service, httpPath: path })
}

/**
 * A sidecar container, addressed by name on every runtime: the sandbox
 * reaches it at its `name` (and on `localhost` where they share a network
 * namespace), it reaches the sandbox at `main`, and `services` may name its
 * ports. Local containers (with `runtime: "runc"`) and every cloud sandbox;
 * local VM sandboxes refuse sidecars. With sidecars the service names
 * `main`, `sidecars` and `sc` are reserved.
 *
 * ```ts
 * SandboxCreateOptions.create({ image: "python:3.12-slim",
 *   sidecars: [sidecar("redis:7-alpine", { ports: [6379] })],
 *   services: new Map([["db", 6379]]) })
 * ```
 */
export function sidecar(
  image: string,
  opts: { command?: string[]; env?: Record<string, string>; ports?: number[]; name?: string } = {},
): Container {
  return Container.create({
    image,
    env: new Map(Object.entries(opts.env ?? {})),
    ports: opts.ports ?? [],
    ...(opts.command ? { command: opts.command } : {}),
    ...(opts.name ? { name: opts.name } : {}),
  })
}

/**
 * Credentials for a private registry image (`registrySecret` on
 * `SandboxCreateOptions`). Locally they authenticate the pull; in the cloud
 * the SDK stores them as the sandbox's registry pull secret. Never logged.
 */
export const registrySecret = {
  /** A user name and password (or token). */
  basic: (username: string, password: string, registry?: string): RegistrySecret =>
    new RegistrySecret.Basic({ username, password, ...(registry ? { registry } : {}) }),
  /** Read from environment variables at create time. */
  fromEnv: (
    usernameVar = "CUA_REGISTRY_USERNAME",
    passwordVar = "CUA_REGISTRY_PASSWORD",
    registry?: string,
  ): RegistrySecret =>
    new RegistrySecret.FromEnv({ usernameVar, passwordVar, ...(registry ? { registry } : {}) }),
  /** A private Amazon ECR image: a login token from the AWS CLI. */
  awsEcr: (region?: string): RegistrySecret =>
    new RegistrySecret.AwsEcr(region ? { region } : {}),
}
