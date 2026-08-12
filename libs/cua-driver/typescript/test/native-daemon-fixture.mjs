import net from "node:net"

const socketPath = process.argv[2]
if (!socketPath) throw new Error("missing socket path")

let completedCalls = 0
const server = net.createServer((connection) => {
  let buffer = ""
  connection.setEncoding("utf8")
  connection.on("data", (chunk) => {
    buffer += chunk
    const newline = buffer.indexOf("\n")
    if (newline < 0) return
    const request = JSON.parse(buffer.slice(0, newline))
    if (request.method === "metadata") {
      connection.end(
        `${JSON.stringify({
          ok: true,
          result: {
            driver_version: "0.12.6",
            contract_version: "0.7.0",
            tools_list_schema_version: "1",
            capability_version: "1",
            mcp_protocol_version: "2025-06-18",
            pid: process.pid,
            embedded: false,
            host_bundle_id: null,
          },
        })}\n`,
      )
      return
    }
    process.send?.({ request })
    const structuredContent =
      request.name === "verify_state"
        ? {
            status: "satisfied",
            stable: true,
            elapsed_ms: 12,
            samples: 2,
            predicates: [],
          }
        : {
            effect: "unverifiable",
            route: "global_input",
            delivery: { mode: "not_applicable" },
          }
    connection.end(
      `${JSON.stringify({
        ok: true,
        result: {
          content: [
            { type: "text", text: "node ffi" },
            { type: "image", mimeType: "image/png", data: "cG5n" },
          ],
          structuredContent,
          isError: false,
        },
      })}\n`,
    )
    completedCalls += 1
    if (completedCalls === 2) server.close()
  })
})

server.listen(socketPath, () => process.send?.({ ready: true }))
