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
