import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import { defineCommand, type Command } from "just-bash/browser"

const CUA_DOCS_MCP_URL = "https://vk-mcp.cua.ai/mcp"

export const BROWSER_MCP_COMMAND_NAMES = [
  "query_docs_db",
  "query_docs_vectors",
  "query_code_db",
  "query_code_vectors",
] as const

export type BrowserMcpCommandName = (typeof BROWSER_MCP_COMMAND_NAMES)[number]

export interface McpToolResult {
  content?: Array<{ type: string; text?: string; [key: string]: unknown }>
  structuredContent?: Record<string, unknown>
  isError?: boolean
  [key: string]: unknown
}

export interface McpClient {
  callTool(params: { name: BrowserMcpCommandName; arguments: Record<string, unknown> }): Promise<McpToolResult>
}

export type McpToolCaller = (name: BrowserMcpCommandName, arguments_: Record<string, unknown>) => Promise<McpToolResult>

type McpConnector = () => Promise<McpClient>

function decodeStdin(stdin: unknown): string {
  const bytes = Uint8Array.from(stdin as string, character => character.charCodeAt(0))
  return new TextDecoder().decode(bytes)
}

function parseArguments(args: string[], stdin: string): Record<string, unknown> {
  if (args.length > 1) throw new Error("expected one JSON object argument")
  const input = args[0] ?? stdin.trim()
  if (!input) return {}

  let parsed: unknown
  try {
    parsed = JSON.parse(input)
  } catch {
    throw new Error("arguments must be a JSON object")
  }
  if (parsed === null || Array.isArray(parsed) || typeof parsed !== "object") {
    throw new Error("arguments must be a JSON object")
  }
  return parsed as Record<string, unknown>
}

function errorMessage(error: unknown): string {
  return error instanceof Error && error.message ? error.message : "MCP command failed"
}

function toolErrorMessage(result: McpToolResult): string {
  const text = result.content?.find(item => item.type === "text" && typeof item.text === "string")?.text
  return text || "MCP tool failed"
}

async function connectCuaDocsMcp(): Promise<McpClient> {
  const client = new Client({ name: "cyclops-cs", version: "0.0.1" })
  const transport = new StreamableHTTPClientTransport(new URL(CUA_DOCS_MCP_URL), {
    requestInit: { credentials: "include" },
  })
  await client.connect(transport)
  return client
}

export function createBrowserMcpToolCaller(connect: McpConnector = connectCuaDocsMcp): McpToolCaller {
  let clientPromise: Promise<McpClient> | undefined

  return async (name, arguments_) => {
    if (!clientPromise) {
      clientPromise = connect().catch(error => {
        clientPromise = undefined
        throw error
      })
    }
    const client = await clientPromise
    return client.callTool({ name, arguments: arguments_ })
  }
}

export function createBrowserMcpCommands(callTool: McpToolCaller = createBrowserMcpToolCaller()): Command[] {
  return BROWSER_MCP_COMMAND_NAMES.map(name =>
    defineCommand(name, async (args, context) => {
      try {
        const arguments_ = parseArguments(args, decodeStdin(context.stdin))
        const result = await callTool(name, arguments_)
        if (result.isError) throw new Error(toolErrorMessage(result))
        return { stdout: `${JSON.stringify(result)}\n`, stderr: "", exitCode: 0 }
      } catch (error) {
        return { stdout: "", stderr: `${errorMessage(error)}\n`, exitCode: 1 }
      }
    }),
  )
}
