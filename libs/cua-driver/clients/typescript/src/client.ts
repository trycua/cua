import { GeneratedClient } from "./generated.js"
import { normalizeToolResult, type ToolResult } from "./result.js"
import { StdioMcpTransport, type Transport } from "./transport.js"

export class CuaDriverClient extends GeneratedClient {
  constructor(private readonly transport: Transport) {
    super()
  }

  static stdio(command: readonly string[] = ["cua-driver", "mcp"]): CuaDriverClient {
    return new CuaDriverClient(new StdioMcpTransport(command))
  }

  async callTool(
    name: string,
    arguments_: Record<string, unknown> = {},
  ): Promise<ToolResult> {
    const result = await this.transport.request("tools/call", {
      name,
      arguments: arguments_,
    })
    return normalizeToolResult(result)
  }

  listTools(): Promise<Record<string, unknown>> {
    return this.transport.request("tools/list", {})
  }

  close(): Promise<void> {
    return this.transport.close()
  }

}
