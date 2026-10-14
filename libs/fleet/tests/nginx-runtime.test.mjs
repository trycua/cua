import test from "node:test"
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises"
import http from "node:http"
import net from "node:net"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const nginxConf = process.env.NGINX_CONF
  ? path.resolve(process.env.NGINX_CONF)
  : path.resolve(__dirname, "../nginx.conf")

test("nginx exposes Fleet analytics and permits large OAuth callback headers", async () => {
  const source = await readFile(nginxConf, "utf8")
  assert.match(
    source,
    /location ~ \^\/api\/\([^\n]*analytics[^\n]*\)\(\/\|\$\)/,
    "/api/analytics must be proxied to the backend",
  )

  const oauthLocation = source.match(/location \/oauth2\/ \{([\s\S]*?)\n    \}/)?.[1]
  assert.ok(oauthLocation, "missing /oauth2/ location")
  assert.match(oauthLocation, /proxy_buffer_size\s+32k;/)
  assert.match(oauthLocation, /proxy_buffers\s+8\s+32k;/)
})

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject)
      resolve(server.address().port)
    })
  })
}

function close(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()))
  })
}

async function unusedPort() {
  const server = net.createServer()
  const port = await listen(server)
  await close(server)
  return port
}

function request({ port, payload, contentLength }) {
  return new Promise((resolve, reject) => {
    const headers = { Authorization: "Bearer test-token" }
    if (contentLength) headers["Content-Length"] = payload.length

    const req = http.request(
      {
        host: "127.0.0.1",
        port,
        path: "/api/svc/test/echo/upload",
        method: "POST",
        headers,
      },
      (res) => {
        const chunks = []
        res.on("data", (chunk) => chunks.push(chunk))
        res.on("end", () => {
          resolve({ status: res.statusCode, body: Buffer.concat(chunks) })
        })
      },
    )
    req.on("error", reject)

    if (contentLength) {
      req.end(payload)
      return
    }

    const midpoint = Math.floor(payload.length / 2)
    req.write(payload.subarray(0, midpoint))
    req.end(payload.subarray(midpoint))
  })
}

test("/api/svc accepts large bodies after oauth2 auth", async (t) => {
  const nginxBin = process.env.NGINX_BIN ?? "nginx"
  const version = spawnSync(nginxBin, ["-v"], { encoding: "utf8" })
  if (version.error?.code === "ENOENT" && !process.env.NGINX_BIN) {
    t.skip("set NGINX_BIN to run the nginx integration test")
    return
  }
  assert.equal(version.status, 0, version.stderr || version.error?.message)

  const authServer = http.createServer((_req, res) => {
    res.writeHead(202, { "X-Auth-Request-User": "test-user" })
    res.end()
  })
  const backendServer = http.createServer((req, res) => {
    let bytes = 0
    const hash = createHash("sha256")
    req.on("data", (chunk) => {
      bytes += chunk.length
      hash.update(chunk)
    })
    req.on("end", () => {
      const body = JSON.stringify({ bytes, sha256: hash.digest("hex") })
      res.writeHead(200, {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(body),
      })
      res.end(body)
    })
  })
  const authPort = await listen(authServer)
  const backendPort = await listen(backendServer)
  const nginxPort = await unusedPort()
  const tempDir = await mkdtemp(path.join(os.tmpdir(), "cyclops-nginx-test-"))
  const clientBodyTemp = path.join(tempDir, "client-body")
  await mkdir(clientBodyTemp)

  let stderr = ""
  let nginx
  try {
    const source = await readFile(nginxConf, "utf8")
    const rendered = source
      .replace("listen 80;", `listen 127.0.0.1:${nginxPort};`)
      .replace(
        /\$\{CYCLOPS_CS_BACKEND\}/g,
        `http://127.0.0.1:${backendPort}`,
      )
      .replace(
        /\$\{CYCLOPS_CS_OAUTH2_PROXY\}/g,
        `http://127.0.0.1:${authPort}`,
      )
      .replace(/\$\{CYCLOPS_CS_CONFIG_JSON\}/g, "{}")
      .replace(
        "root /usr/share/nginx/html;",
        `root ${JSON.stringify(tempDir)};`,
      )
    const serverConf = path.join(tempDir, "server.conf")
    const mainConf = path.join(tempDir, "nginx.conf")
    await writeFile(serverConf, rendered)
    await writeFile(
      mainConf,
      `worker_processes 1;\nerror_log stderr notice;\npid ${tempDir}/nginx.pid;\nevents { worker_connections 128; }\nhttp {\n  access_log off;\n  client_body_temp_path ${clientBodyTemp};\n  include ${serverConf};\n}\n`,
    )

    nginx = spawn(nginxBin, ["-c", mainConf, "-g", "daemon off;"], {
      stdio: ["ignore", "ignore", "pipe"],
    })
    nginx.stderr.setEncoding("utf8")
    nginx.stderr.on("data", (chunk) => {
      stderr += chunk
    })

    await new Promise((resolve, reject) => {
      const deadline = Date.now() + 5000
      const probe = () => {
        const socket = net.connect(nginxPort, "127.0.0.1")
        socket.once("connect", () => {
          socket.destroy()
          resolve()
        })
        socket.once("error", (error) => {
          socket.destroy()
          if (nginx.exitCode !== null || Date.now() >= deadline) {
            reject(new Error(`nginx failed to start: ${error.message}\n${stderr}`))
          } else {
            setTimeout(probe, 25)
          }
        })
      }
      probe()
    })

    for (const sample of [
      { name: "content-length", size: 1_048_577, contentLength: true },
      { name: "chunked", size: 1_572_864, contentLength: false },
    ]) {
      await t.test(sample.name, async () => {
        const payload = Buffer.alloc(sample.size, 0xa5)
        const response = await request({
          port: nginxPort,
          payload,
          contentLength: sample.contentLength,
        })
        assert.equal(response.status, 200, stderr)
        assert.deepEqual(JSON.parse(response.body), {
          bytes: payload.length,
          sha256: createHash("sha256").update(payload).digest("hex"),
        })
      })
    }
  } finally {
    if (nginx?.exitCode === null) {
      nginx.kill("SIGTERM")
      await new Promise((resolve) => nginx.once("exit", resolve))
    }
    await Promise.allSettled([close(authServer), close(backendServer)])
    await rm(tempDir, { recursive: true, force: true })
  }
})
