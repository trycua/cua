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
            contract_version: "0.8.0",
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
    let structuredContent =
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
    let isError = false
    if (request.name === "list_apps") structuredContent = { apps: [{ pid: 42, name: "Editor", running: true, active: false }] }
    if (request.name === "list_windows") structuredContent = { windows: [{ pid: 42, window_id: 123, app_name: "Editor", title: "Document", bounds: { x: 0, y: 0, width: 800, height: 600 }, is_on_screen: true, z_index: null }] }
    if (request.name === "get_window_state") structuredContent = { pid: 42, window_id: 123, snapshot_id: "snapshot-1", screenshot_width: 800, screenshot_height: 600, elements: [{ element_index: 0, role: "button", depth: 0, element_token: "fresh-token", label: "Save" }] }
    if (request.name === "click" && (request.args.element_token === "stale-token" || request.args.target?.window_id === 124)) {
      isError = true
      structuredContent = { code: request.args.element_token === "stale-token" ? "stale_element_token" : "element_target_mismatch" }
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
          isError,
        },
      })}\n`,
    )
    completedCalls += 1
    if (completedCalls === 8) server.close()
  })
})

server.listen(socketPath, () => process.send?.({ ready: true }))
