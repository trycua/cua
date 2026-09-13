import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import { defineCommand, type Command } from "just-bash/browser"
import {
  isHelpRequest,
  renderCommandHelp,
  type BrowserCommandHelp,
} from "./browser-command-help.js"

const CUA_DOCS_MCP_URL = "https://vk-mcp.cua.ai/mcp"

export const BROWSER_MCP_COMMAND_NAMES = [
  "query_docs_db",
  "query_docs_vectors",
  "query_code_db",
  "query_code_vectors",
] as const

export type BrowserMcpCommandName = (typeof BROWSER_MCP_COMMAND_NAMES)[number]

const MCP_COMMAND_HELP: Record<BrowserMcpCommandName, BrowserCommandHelp> = {
  query_docs_db: {
    summary: "Run a read-only SQL query against the indexed CUA documentation database.",
    usage: "query_docs_db '{\"sql\":\"SELECT ...\"}'",
    arguments: ["sql: Read-only SQL query supported by the documentation service."],
    output: "An MCP result object containing structured rows and/or text content.",
    presentation: [
      "Answer the user's question directly; do not dump the MCP wrapper object.",
      "Cite relevant documentation URLs returned by the query next to the claims they support.",
      "Use a table only when the result is naturally tabular.",
    ],
    safety: "Read-only remote query. Ordinary browser Bash still has no general network access.",
    examples: ["query_docs_db '{\"sql\":\"SELECT title, url FROM documents LIMIT 10\"}' | jq"],
  },
  query_docs_vectors: {
    summary: "Search CUA documentation semantically.",
    usage: "query_docs_vectors '{\"query\":\"text\",\"limit\":5}'",
    arguments: [
      "query: Natural-language search text.",
      "limit: Optional result limit.",
      "where: Optional service-supported filter.",
      "select: Optional fields to return.",
    ],
    output: "An MCP result object containing ranked documentation matches.",
    presentation: [
      "Synthesize the matches into a concise answer instead of listing raw embeddings or scores.",
      "Cite the returned documentation URLs next to the claims they support.",
      "Mention uncertainty when the matches do not directly answer the question.",
    ],
    safety: "Read-only remote search. Ordinary browser Bash still has no general network access.",
    examples: ["query_docs_vectors '{\"query\":\"create a warm pool\",\"limit\":5}' | jq"],
  },
  query_code_db: {
    summary: "Run a read-only SQL query against indexed versioned CUA source code.",
    usage: "query_code_db '{\"sql\":\"SELECT ...\"}'",
    arguments: ["sql: Read-only SQL query supported by the code index."],
    output: "An MCP result object containing structured code-index rows and/or text content.",
    presentation: [
      "Explain the relevant implementation rather than dumping raw rows.",
      "Cite each source as component@version:path, adding line information when returned.",
      "Use short code excerpts only when needed to explain behavior.",
    ],
    safety: "Read-only remote query. Results describe indexed code and do not execute it.",
    examples: ["query_code_db '{\"sql\":\"SELECT component, version, path FROM code_files LIMIT 10\"}' | jq"],
  },
  query_code_vectors: {
    summary: "Search indexed versioned CUA source code semantically.",
    usage: "query_code_vectors '{\"query\":\"text\",\"limit\":5}'",
    arguments: [
      "query: Natural-language code search text.",
      "limit: Optional result limit.",
      "where: Optional service-supported filter.",
      "select: Optional fields to return.",
      "component: Optional CUA component filter.",
    ],
    output: "An MCP result object containing ranked versioned code matches.",
    presentation: [
      "Summarize how the matching code works and distinguish direct evidence from inference.",
      "Cite each source as component@version:path, adding line information when returned.",
      "Do not present similarity scores unless the user asks for search diagnostics.",
    ],
    safety: "Read-only remote search. Results describe indexed code and do not execute it.",
    examples: ["query_code_vectors '{\"query\":\"browser agent bash commands\",\"limit\":5,\"component\":\"cloud\"}' | jq"],
  },
}

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
      if (isHelpRequest(args)) {
        return { stdout: renderCommandHelp(name, MCP_COMMAND_HELP[name]), stderr: "", exitCode: 0 }
      }
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
