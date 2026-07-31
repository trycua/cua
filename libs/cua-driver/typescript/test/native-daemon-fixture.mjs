import net from "node:net"

const socketPath = process.argv[2]
if (!socketPath) throw new Error("missing socket path")

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
            contract_version: "0.3.0",
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
    connection.end(
      `${JSON.stringify({
        ok: true,
        result: {
          content: [
            { type: "text", text: "node ffi" },
            { type: "image", mimeType: "image/png", data: "cG5n" },
          ],
          structuredContent: { verified: true },
          isError: false,
        },
      })}\n`,
    )
    server.close()
  })
})

server.listen(socketPath, () => process.send?.({ ready: true }))
