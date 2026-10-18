/**
 * MCP servers inside a sandbox, with the official TypeScript SDK
 * (`@modelcontextprotocol/client`, an optional peer dependency).
 *
 * cua does not implement MCP: `sandbox.mcpConfig(service)` returns the
 * endpoint URL and the headers its route needs (nothing locally; the Fleet
 * gateway bearer and claim in the cloud; the daemon bearer through
 * `cua daemon`), and `connectMcp` hands that to the SDK's streamable-HTTP
 * transport, so every protocol revision and content block works unchanged.
 *
 * ```ts
 * const client = await connectMcp(await sb.mcpConfig("mcp", undefined))
 * const result = await client.callTool({ name: "add", arguments: { a: 2, b: 3 } })
 * ```
 */

/** Where an MCP endpoint is: `Sandbox.mcpConfig()` returns one. */
export interface McpEndpointConfig {
  url: string
  headers: Array<{ name: string; value: string }> | Record<string, string>
}

/** The headers as a plain object. */
export function mcpHeaders(config: McpEndpointConfig): Record<string, string> {
  if (Array.isArray(config.headers)) {
    return Object.fromEntries(config.headers.map((h) => [h.name, h.value]))
  }
  return { ...config.headers }
}

/**
 * Connects the official SDK's `Client` to `config` over streamable HTTP.
 * Fleet bearers are short-lived: fetch a fresh config per connection.
 */
export async function connectMcp(
  config: McpEndpointConfig,
  clientInfo: { name: string; version: string } = { name: "cua", version: "0" },
): Promise<any> {
  let sdk: any
  try {
    sdk = await import("@modelcontextprotocol/client" as string)
  } catch (error) {
    throw new Error(
      "connectMcp needs the official MCP SDK: npm install @modelcontextprotocol/client",
      { cause: error },
    )
  }
  const transport = new sdk.StreamableHTTPClientTransport(new URL(config.url), {
    requestInit: { headers: mcpHeaders(config) },
  })
  const client = new sdk.Client(clientInfo)
  await client.connect(transport)
  return client
}
