import { createInterface } from "node:readline"

let initialized = false
const lines = createInterface({ input: process.stdin })

lines.on("line", line => {
  const request = JSON.parse(line)
  if (request.method === "notifications/initialized") {
    initialized = true
    return
  }
  let result
  if (request.method === "initialize") {
    result = {
      protocolVersion: request.params.protocolVersion,
      capabilities: {},
      serverInfo: { name: "fixture", version: "1" },
    }
  } else if (!initialized) {
    respond(request.id, undefined, { code: -32002, message: "not initialized" })
    return
  } else if (request.method === "test/hang") {
    return
  } else if (request.method === "tools/list") {
    result = { tools: [{ name: "get_desktop_state" }] }
  } else if (request.method === "tools/call") {
    result = {
      content: [{ type: "text", text: "fixture result" }],
      structuredContent: {
        name: request.params.name,
        arguments: request.params.arguments ?? {},
      },
    }
  } else {
    respond(request.id, undefined, { code: -32601, message: `unknown method: ${request.method}` })
    return
  }
  respond(request.id, result)
})

function respond(id, result, error) {
  const response = { jsonrpc: "2.0", id }
  if (error) response.error = error
  else response.result = result
  process.stdout.write(`${JSON.stringify(response)}\n`)
}
