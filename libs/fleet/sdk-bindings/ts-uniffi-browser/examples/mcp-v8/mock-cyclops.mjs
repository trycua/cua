// Static file + mock Cyclops backend for the mcp-v8 browser-SDK experiment.
// Serves the esbuild bundle and .wasm with correct MIME types, and answers
// the pool-collection list route with an empty Kubernetes-style list.
import { createServer } from "node:http"
import { readFile } from "node:fs/promises"
import { join } from "node:path"

const DIST = process.argv[2]
const PORT = Number(process.argv[3] ?? 8788)

const MIME = {
  ".js": "text/javascript",
  ".wasm": "application/wasm",
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${PORT}`)
  console.log(`[mock] ${req.method} ${url.pathname} auth=${req.headers.authorization ?? "-"}`)

  // Mock Cyclops list endpoints (Kubernetes-style resource lists).
  const poolList = url.pathname.match(
    /^\/api\/k8s\/apis\/osgym\.cua\.ai\/v1alpha1\/namespaces\/([^/]+)\/osgymsandboxwarmpools$/,
  )
  if (poolList && req.method === "GET") {
    if (req.headers.authorization !== "Bearer test-token") {
      res.writeHead(401, { "content-type": "application/json" })
      res.end(JSON.stringify({ message: "missing or wrong bearer token" }))
      return
    }
    res.writeHead(200, { "content-type": "application/json" })
    res.end(
      JSON.stringify({
        items: [
          {
            apiVersion: "osgym.cua.ai/v1alpha1",
            kind: "OsGymSandboxWarmPool",
            metadata: { name: "demo-pool", namespace: poolList[1] },
            spec: { replicas: 2, sandboxTemplateRef: { name: "demo-template" } },
          },
        ],
      }),
    )
    return
  }

  // Static files from DIST.
  const name = url.pathname.replace(/^\/+/, "")
  if (name === "fleet-sdk-web.js" || name === "index_bg.wasm") {
    try {
      const body = await readFile(join(DIST, name))
      const ext = name.slice(name.lastIndexOf("."))
      res.writeHead(200, { "content-type": MIME[ext] ?? "application/octet-stream" })
      res.end(body)
      return
    } catch (err) {
      res.writeHead(500)
      res.end(String(err))
      return
    }
  }

  res.writeHead(404, { "content-type": "application/json" })
  res.end(JSON.stringify({ message: `no route for ${req.method} ${url.pathname}` }))
})

server.listen(PORT, "127.0.0.1", () => {
  console.log(`[mock] listening on http://127.0.0.1:${PORT}`)
})
