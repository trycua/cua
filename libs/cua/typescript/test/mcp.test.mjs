// The generic MCP client of the Node binding (`mcpConnectUrl`,
// `Sandbox.mcp`) against a tiny in-test streamable-HTTP MCP server (JSON
// replies, session ids). The protocol itself is tested in Rust.
import assert from "node:assert/strict"
import { createServer } from "node:http"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"

let server
let base
const sessions = new Set()

function reply(res, status, body, headers = {}) {
  res.writeHead(status, { "content-type": "application/json", ...headers })
  res.end(body === undefined ? "" : JSON.stringify(body))
}

before(async () => {
  server = createServer((req, res) => {
    let raw = ""
    req.on("data", (c) => (raw += c))
    req.on("end", () => {
      if (req.url !== "/mcp") return reply(res, 404, { error: "no route" })
      if (req.method === "DELETE") {
        sessions.delete(req.headers["mcp-session-id"])
        return reply(res, 200)
      }
      if (req.method !== "POST") return reply(res, 405, { error: "no standalone stream" })
      const m = JSON.parse(raw)
      if (m.method === "initialize") {
        const sid = `ts-${sessions.size + 1}`
        sessions.add(sid)
        return reply(
          res,
          200,
          {
            jsonrpc: "2.0",
            id: m.id,
            result: {
              protocolVersion: m.params.protocolVersion === "2025-11-25" ? "2025-11-25" : "2025-06-18",
              capabilities: { tools: {} },
              serverInfo: { name: "ts-fake", version: "1" },
            },
          },
          { "mcp-session-id": sid },
        )
      }
      if (!sessions.has(req.headers["mcp-session-id"])) return reply(res, 404, { error: "session" })
      if (m.id === undefined) return reply(res, 202)
      if (m.method === "tools/list") {
        return reply(res, 200, {
          jsonrpc: "2.0",
          id: m.id,
          result: { tools: [{ name: "add", description: "Add", inputSchema: { type: "object" } }] },
        })
      }
      if (m.method === "tools/call") {
        const { a, b } = m.params.arguments
        return reply(res, 200, {
          jsonrpc: "2.0",
          id: m.id,
          result: {
            content: [
              { type: "text", text: String(a + b) },
              { type: "image", data: PNG, mimeType: "image/png" },
            ],
            structuredContent: { sum: a + b },
          },
        })
      }
      return reply(res, 200, { jsonrpc: "2.0", id: m.id, error: { code: -32601, message: "nope" } })
    })
  })
  await new Promise((r) => server.listen(0, "127.0.0.1", r))
  base = `http://127.0.0.1:${server.address().port}`
})

after(() => server?.close())

const PNG = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.alloc(50_000, 7)]).toString("base64")

async function exerciseNative(mcp) {
  const tools = JSON.parse(await mcp.listTools())
  assert.deepEqual(
    tools.map((t) => t.name),
    ["add"],
  )
  const r = JSON.parse(await mcp.callTool("add", JSON.stringify({ a: 2, b: 3 })))
  assert.equal(r.content[0].text, "5")
  assert.deepEqual(r.content[1], { type: "image", data: PNG, mimeType: "image/png" })
  assert.deepEqual(r.structuredContent, { sum: 5 })
  await mcp.close()
}

test("mcpConnectUrl (rmcp) keeps content blocks verbatim", async () => {
  await exerciseNative(await cua.mcpConnectUrl(`${base}/mcp`, undefined))
})

test("Sandbox.mcp and the official SDK over Sandbox.mcpConfig", async () => {
  const c = cua.embedded()
  const sb = await c.sandboxes().connectUrl(base, undefined, "ts-mcp")
  await exerciseNative(await sb.mcp("env", undefined))
  const config = await sb.mcpConfig("env", undefined)
  assert.equal(config.url, `${base}/mcp`)
  const client = await cua.connectMcp(config)
  const r = await client.callTool({ name: "add", arguments: { a: 2, b: 3 } })
  assert.equal(r.content[1].type, "image")
  assert.equal(r.content[1].data, PNG)
  assert.deepEqual(r.structuredContent, { sum: 5 })
  await client.close()
  await sb.delete_()
})

test("live: the reference everything server with the official SDK", async (t) => {
  const url = process.env.CUA_E2E_MCP_EVERYTHING_URL
  if (!url) return t.skip("set CUA_E2E_MCP_EVERYTHING_URL")
  const client = await cua.connectMcp({ url, headers: {} })
  const r = await client.callTool({ name: "get-tiny-image", arguments: {} })
  const img = r.content.find((c) => c.type === "image")
  assert.equal(img.mimeType, "image/png")
  assert.equal(Buffer.from(img.data, "base64").subarray(0, 4).toString("latin1"), "\x89PNG")
  const native = await cua.mcpConnectUrl(url, undefined)
  const n = JSON.parse(await native.callTool("get-tiny-image", "{}"))
  assert.deepEqual(n.content.find((c) => c.type === "image"), img)
  await native.close()
  await client.close()
})
