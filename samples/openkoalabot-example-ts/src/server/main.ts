#!/usr/bin/env node
/**
 * OpenKoalaBots (TypeScript): the local server + web UI.
 *
 *   node dist/server/main.js [--port 4780] [--daemon] [--home DIR] [--teleport-home DIR]
 *                            [--model-url URL [--model NAME] [--key-var VAR]]
 *
 * By default the Spaces runtime is embedded in this process with its own
 * registry under `~/.openkoalabot-example-ts` (never `~/.cua`); `--daemon` uses a
 * running `cua daemon` instead. Session teleport (`/api/teleport`) works
 * through the Cua Spaces daemon (`--daemon`), and only after the page shows
 * you the manifest and you approve it; the embedded runtime answers that
 * teleport ships with Cua Spaces. `--teleport-home DIR` points teleport at
 * a directory instead of the real host (a generated profile, for demos).
 * "Teleport an app…" ships with Cua Spaces (source-available); this
 * sample says so instead of offering it.
 *
 * Routines (`<home>/routines.json`) fire on their schedule while the server
 * runs. `--model-url` points every Bot at a custom model endpoint, with the
 * key forwarded from this process's `--key-var` (default ANTHROPIC_API_KEY).
 */
import { connect, embedded } from "@trycua/cua"
import { SpaceCreateOptions, SpaceStreamOptions, type SpaceLike } from "@trycua/cua/spaces"
import { existsSync, mkdirSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import type { SpacesPort } from "../core/app.js"
import { startServer } from "./app.js"

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`)
  return i >= 0 ? process.argv[i + 1] : undefined
}

const home = resolve(arg("home") ?? join(homedir(), ".openkoalabot-example-ts"))
// #region docs:ts-open
const cua = process.argv.includes("--daemon")
  ? connect()
  : (mkdirSync(home, { recursive: true }),
    embedded({
      stateDir: join(home, "sandboxes"),
      spacesHome: join(home, "spaces"),
      teleportHome: arg("teleport-home"),
      fleetFromEnv: true,
    }))
const spaces = cua.spaces()
// #endregion docs:ts-open
const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../web/dist")

const running = await startServer(
  {
    spaces: spaces as unknown as SpacesPort,
    cloud: Boolean(process.env.CUA_CLIENT_ID || process.env.FLEETS_TOKEN || process.env.CUA_TOKEN),
    // The New Space wizard: one SDK call per plan (src/core/plan.ts).
    create: async (call) => {
      // #region docs:ts-create
      const c = await spaces.create(
        SpaceCreateOptions.create({
          on: call.on,
          image: call.image,
          kind: call.kind,
          runtime: call.runtime,
          name: call.name,
          cpus: call.cpus,
          memoryMb: call.memoryMb === undefined ? undefined : BigInt(call.memoryMb),
          wait: call.wait,
          spacesd: call.spacesd,
        }),
      )
      if (!c.space) throw new Error(`Space ${c.pendingId ?? ""} is not ready yet`)
      return c.space
      // #endregion docs:ts-create
    },
    stream: (space) => {
      const s = space as unknown as SpaceLike
      return {
        openStream: (o) =>
          s.openStream(SpaceStreamOptions.create({ maxFps: o.maxFps, maxDimension: o.maxDimension, codecs: ["h264"], ...(o.windowId ? { windowId: o.windowId } : {}) })),
        closeStream: (id) => s.closeStream(id),
        windows: async () => {
          // `windows()` lists only streamable windows.
          const rows = await s.windows(undefined)
          // Every row's icon in one SDK call (its one icon cache; misses in one guest round trip).
          const icons = await s
            .appIcons(rows.map((w) => ({ appName: w.appName, appId: w.appId, pid: w.pid })))
            .catch(() => rows.map(() => undefined))
          return rows.map((w, i) => ({
            windowId: w.windowId,
            app: w.appName,
            title: w.title,
            width: w.bounds[2] ?? 0,
            height: w.bounds[3] ?? 0,
            ...iconField(icons[i]),
          }))
        },
      }
    },
  },
  {
    port: Number(arg("port") ?? 4780),
    webRoot: existsSync(webRoot) ? webRoot : undefined,
    dataDir: home,
    ...(arg("model-url")
      ? { endpoint: { baseUrl: arg("model-url"), model: arg("model"), envFromHost: [arg("key-var") ?? "ANTHROPIC_API_KEY"] } }
      : {}),
  },
)
/** The row's `icon`: the Space's own icon file as a `data:` URL, or nothing. */
function iconField(icon: { bytes: ArrayBuffer; contentType: string } | undefined): { icon?: string } {
  if (!icon || icon.bytes.byteLength === 0) return {}
  return { icon: `data:${icon.contentType};base64,${Buffer.from(new Uint8Array(icon.bytes)).toString("base64")}` }
}

console.log(`OpenKoalaBots on ${running.url}/#token=${running.token}`)
if (!existsSync(webRoot)) console.log("(web UI not built: npm run build:web, or npm run dev:web for Vite)")
const stop = () => running.close().then(() => process.exit(0))
process.on("SIGINT", stop)
process.on("SIGTERM", stop)
