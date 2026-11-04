// Loopback static server for the page: this directory at /, the SDK's
// browser build (@trycua/cua/browser) at /sdk/ and its one bare import,
// @ubjs/core (plain ES modules), at /ubjs-core/ (see the import map in
// index.html). No bundler, no dependencies.
//
//   node serve.mjs [port]      -> prints the page URL
import { createReadStream, existsSync, statSync } from "node:fs"
import { createServer } from "node:http"
import { dirname, extname, join, normalize, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const here = dirname(fileURLToPath(import.meta.url))
export const sdkBrowserDir = resolve(here, "node_modules/@trycua/cua/browser")
const ubjsCoreDir = resolve(here, "node_modules/@trycua/cua/node_modules/@ubjs/core/dist/esm")
const mounts = [
  ["/sdk/", sdkBrowserDir],
  ["/ubjs-core/", ubjsCoreDir],
]

const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".wasm": "application/wasm",
  ".map": "application/json",
}

function resolvePath(urlPath) {
  const path = decodeURIComponent(urlPath.split("?")[0])
  const mount = mounts.find(([prefix]) => path.startsWith(prefix))
  const [root, rest] = mount ? [mount[1], path.slice(mount[0].length)] : [here, path.slice(1)]
  const file = normalize(join(root, rest || "index.html"))
  if (!file.startsWith(root)) return null // no path traversal
  if (!mount && file.includes("node_modules")) return null
  return file
}

/** Starts the server on 127.0.0.1; resolves to `{ server, url }`. */
export function serve(port = 0) {
  const server = createServer((req, res) => {
    const file = resolvePath(req.url ?? "/")
    if (!file || !existsSync(file) || !statSync(file).isFile()) {
      res.writeHead(404).end("not found")
      return
    }
    res.writeHead(200, {
      "content-type": types[extname(file)] ?? "application/octet-stream",
      "cache-control": "no-store",
    })
    createReadStream(file).pipe(res)
  })
  return new Promise((ok, fail) => {
    server.once("error", fail)
    server.listen(port, "127.0.0.1", () =>
      ok({ server, url: `http://127.0.0.1:${server.address().port}/` }),
    )
  })
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (!existsSync(join(sdkBrowserDir, "index.js"))) {
    console.error(`missing ${sdkBrowserDir}/index.js: build it with (cd libs/cua/typescript && npm run build:browser)`)
    process.exit(3)
  }
  const { url } = await serve(Number(process.argv[2] ?? 8765))
  console.log(`open ${url}?env=<CUA_ENV_URL>&token=<CUA_ENV_TOKEN>`)
}
