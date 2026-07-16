/**
 * Provision a CUA sandbox and operate it with the official Claude Agent SDK.
 *
 * This example claims a sandbox from an existing pool configured with the
 * computer-server and cua-driver services described by CUA_SERVICES. It
 * preflights both Streamable HTTP MCP endpoints, discovers their tools, then
 * gives only those remote MCP tools to Claude.
 *
 * From cyclops-cs/js-sdk:
 *   npm run build
 *   npm install --no-save --package-lock=false \
 *     @anthropic-ai/claude-agent-sdk @modelcontextprotocol/sdk
 *   CUA_CLIENT_ID=ukey-... CUA_CLIENT_SECRET=... CUA_POOL=my-pool \
 *     ANTHROPIC_API_KEY=... node examples/claude-agent-sdk.mjs
 *
 * Optional: CUA_BASE_URL, CUA_TOKEN_URL, CUA_CLAIM_NAME, CLAUDE_MODEL,
 * CLAUDE_MAX_TURNS, CLAUDE_MAX_BUDGET_USD, CUA_BIND_TIMEOUT_MS, and
 * CUA_READY_TIMEOUT_MS.
 *
 * Use a per-user Cyclops key because Kubernetes claim operations require the
 * user's Kubernetes identity. The Claude Agent SDK requires Node.js 18+.
 */

import { randomUUID } from "node:crypto"

import { query } from "@anthropic-ai/claude-agent-sdk"
// The MCP SDK calls this class Client; aliasing it makes the session role clear.
import { Client as ClientSession } from "@modelcontextprotocol/sdk/client"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"

import { ContentType, CyclopsClient, DEFAULT_BASE_URL } from "../dist/index.js"

const CUA_SERVICES = [
  {
    name: "computer-server",
    service_suffix: "server",
    guest_port: 8000,
    mcp_path: "/mcp",
    health_path: "/status",
  },
  {
    name: "cua-driver",
    service_suffix: "mcp",
    guest_port: 3000,
    mcp_path: "/mcp",
    health_path: "/healthz",
  },
]

const CLAIM_GROUP = "osgym.cua.ai"
const CLAIM_VERSION = "v1alpha1"
const CLAIM_PLURAL = "osgymsandboxclaims"
const POLL_INTERVAL_MS = 2_000
const DEFAULT_BIND_TIMEOUT_MS = 10 * 60_000
const DEFAULT_READY_TIMEOUT_MS = 5 * 60_000
const DEFAULT_MAX_TURNS = 20
const DEFAULT_MAX_BUDGET_USD = 5
const DEFAULT_TASK =
  "Open Firefox and navigate to https://example.com. Take a screenshot and " +
  "report the page title. Do not click links, enter data, download files, or " +
  "change browser or system settings."

function section(title) {
  console.log(`\n=== ${title} ===`)
}

function info(message) {
  console.log(`  ${message}`)
}

function requiredEnv(name) {
  const value = process.env[name]
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`)
  }
  return value
}

function envNumber(name, fallback) {
  const raw = process.env[name]
  if (!raw) return fallback
  const value = Number(raw)
  if (!Number.isFinite(value) || value <= 0) {
    throw new Error(`${name} must be a positive number`)
  }
  return value
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}

function trimSlashes(path) {
  return path.replace(/^\/+|\/+$/g, "")
}

function claimsPath(pool, claimName) {
  const collection = `apis/${CLAIM_GROUP}/${CLAIM_VERSION}/namespaces/${pool}/${CLAIM_PLURAL}`
  return claimName ? `${collection}/${claimName}` : collection
}

function serviceName(sandbox, service) {
  return `${sandbox}-${service.service_suffix}`
}

function serviceUrl(baseUrl, pool, sandbox, service, path) {
  const proxyPath = `/api/svc/${pool}/${serviceName(sandbox, service)}/${trimSlashes(path)}`
  return new URL(proxyPath, `${baseUrl.replace(/\/$/, "")}/`).toString()
}

async function describeError(error) {
  if (error instanceof Response) {
    let body = ""
    try {
      const parsedError = "error" in error ? error.error : undefined
      body = parsedError
        ? JSON.stringify(parsedError)
        : await error.clone().text()
    } catch {
      // Status information remains useful when the response body is unavailable.
    }
    return `${error.status} ${error.statusText}${body ? `: ${body}` : ""}`
  }
  return error instanceof Error ? error.message : String(error)
}

async function createClaim(client, pool, claimName) {
  // The generated GET helper does not model claim creation, but the generated
  // client's public request method supports the same authenticated proxy route.
  await client.request({
    path: `/api/k8s/${claimsPath(pool)}`,
    method: "POST",
    secure: true,
    type: ContentType.Json,
    format: "json",
    body: {
      apiVersion: `${CLAIM_GROUP}/${CLAIM_VERSION}`,
      kind: "OSGymSandboxClaim",
      metadata: { name: claimName },
      spec: {
        sandboxTemplateRef: { name: `${pool}-template` },
        bindDeadline: 600,
      },
    },
  })
}

async function waitForBoundSandbox(client, pool, claimName, timeoutMs) {
  const deadline = Date.now() + timeoutMs
  let previousPhase

  while (Date.now() < deadline) {
    const response = await client.k8S.getK8S(claimsPath(pool, claimName), {
      format: "json",
    })
    const status = response.data.status ?? {}
    const phase = status.phase ?? "Pending"
    const sandbox = status.sandbox?.name

    if (phase !== previousPhase) {
      info(`claim ${claimName}: ${phase}`)
      previousPhase = phase
    }
    if (phase === "Bound" && sandbox) return sandbox
    if (phase === "Failed") {
      throw new Error(`Claim ${claimName} failed: ${JSON.stringify(status)}`)
    }
    await sleep(POLL_INTERVAL_MS)
  }

  throw new Error(`Timed out waiting for claim ${claimName} to bind`)
}

async function waitForService(client, pool, sandbox, service, timeoutMs) {
  const deadline = Date.now() + timeoutMs
  const name = serviceName(sandbox, service)
  const path = trimSlashes(service.health_path)
  let lastError = "not ready"

  while (Date.now() < deadline) {
    try {
      await client.svc.getSvc(pool, name, path, { format: "text" })
      info(
        `${service.name} ready at ${name} ` +
          `(guest port ${service.guest_port})`,
      )
      return
    } catch (error) {
      lastError = await describeError(error)
      await sleep(POLL_INTERVAL_MS)
    }
  }

  throw new Error(`${service.name} did not become ready: ${lastError}`)
}

async function releaseClaim(client, pool, claimName) {
  await client.request({
    path: `/api/k8s/${claimsPath(pool, claimName)}`,
    method: "DELETE",
    secure: true,
  })
}

async function connectMcpServer({
  baseUrl,
  pool,
  sandbox,
  service,
  authHeaders,
}) {
  const url = serviceUrl(baseUrl, pool, sandbox, service, service.mcp_path)
  const transport = new StreamableHTTPClientTransport(new URL(url), {
    requestInit: { headers: authHeaders },
  })
  const session = new ClientSession({
    name: `cyclops-claude-${service.name}`,
    version: "1.0.0",
  })

  await session.connect(transport)
  const { tools } = await session.listTools()
  info(`${service.name}: connected to ${url} (${tools.length} tools)`)

  return { service, session, tools, url }
}

function normalizedServerName(name) {
  return name.replace(/[^a-zA-Z0-9_-]/g, "_")
}

function claudeToolName(serverName, toolName) {
  return `mcp__${normalizedServerName(serverName)}__${toolName}`
}

function buildClaudeMcpConfig(connections, authHeaders) {
  return Object.fromEntries(
    connections.map(({ service, tools, url }) => [
      service.name,
      {
        type: "http",
        url,
        headers: authHeaders,
        tools: tools.map((tool) => ({
          name: tool.name,
          permission_policy: "always_allow",
        })),
        timeout: 120_000,
        alwaysLoad: true,
      },
    ]),
  )
}

function discoveredClaudeTools(connections) {
  return connections.flatMap(({ service, tools }) =>
    tools.map((tool) => claudeToolName(service.name, tool.name)),
  )
}

function logAssistantMessage(message) {
  for (const block of message.message.content) {
    if (block.type === "text") {
      for (const line of block.text.trim().split("\n")) info(`Claude: ${line}`)
    } else if (block.type === "tool_use") {
      info(`Claude calling ${block.name}`)
    }
  }
}

async function main() {
  const pool = requiredEnv("CUA_POOL")
  const baseUrl = process.env.CUA_BASE_URL ?? DEFAULT_BASE_URL
  const claimName =
    process.env.CUA_CLAIM_NAME ?? `claude-agent-${randomUUID().slice(0, 8)}`
  const bindTimeoutMs = envNumber(
    "CUA_BIND_TIMEOUT_MS",
    DEFAULT_BIND_TIMEOUT_MS,
  )
  const readyTimeoutMs = envNumber(
    "CUA_READY_TIMEOUT_MS",
    DEFAULT_READY_TIMEOUT_MS,
  )
  const maxTurns = envNumber("CLAUDE_MAX_TURNS", DEFAULT_MAX_TURNS)
  const maxBudgetUsd = envNumber(
    "CLAUDE_MAX_BUDGET_USD",
    DEFAULT_MAX_BUDGET_USD,
  )
  const task = process.argv.slice(2).join(" ") || DEFAULT_TASK

  requiredEnv("ANTHROPIC_API_KEY")

  let client
  let claimCreated = false
  let claudeQuery
  const mcpConnections = []

  try {
    section("1. CUA Authentication")
    client = await CyclopsClient.fromKey({
      clientId: requiredEnv("CUA_CLIENT_ID"),
      clientSecret: requiredEnv("CUA_CLIENT_SECRET"),
      baseUrl,
      tokenUrl: process.env.CUA_TOKEN_URL,
    })
    info("authenticated with Cyclops client credentials")

    section("2. Provision a CUA Sandbox")
    info(`pool: ${pool}`)
    for (const service of CUA_SERVICES) {
      info(
        `${service.name}: ${service.service_suffix} -> guest port ` +
          `${service.guest_port}, MCP ${service.mcp_path}`,
      )
    }
    await createClaim(client, pool, claimName)
    claimCreated = true
    info(`claim created: ${claimName}`)
    const sandbox = await waitForBoundSandbox(
      client,
      pool,
      claimName,
      bindTimeoutMs,
    )
    info(`sandbox bound: ${sandbox}`)

    for (const service of CUA_SERVICES) {
      await waitForService(client, pool, sandbox, service, readyTimeoutMs)
    }

    const authHeaders = {
      Authorization: `Bearer ${await client.tokenProvider.getToken()}`,
    }

    section("3. Connect and Discover MCP Tools")
    for (const service of CUA_SERVICES) {
      mcpConnections.push(
        await connectMcpServer({
          baseUrl,
          pool,
          sandbox,
          service,
          authHeaders,
        }),
      )
    }

    const mcpServers = buildClaudeMcpConfig(mcpConnections, authHeaders)
    const allowedTools = discoveredClaudeTools(mcpConnections)
    info(`discovered ${allowedTools.length} Claude MCP tools`)
    for (const tool of allowedTools) info(tool)

    section("4. Run the Claude Agent Task")
    info(`task: ${task}`)

    claudeQuery = query({
      prompt: task,
      options: {
        allowedTools,
        tools: [],
        mcpServers,
        strictMcpConfig: true,
        settingSources: [],
        persistSession: false,
        maxTurns,
        maxBudgetUsd,
        model: process.env.CLAUDE_MODEL,
        systemPrompt:
          "You operate one CUA desktop through two remote MCP servers: " +
          "computer-server and cua-driver. Use only the provided MCP tools. " +
          "Inspect the desktop before acting, take one action at a time, and " +
          "verify each result. Do not enter credentials, submit forms, make " +
          "purchases, download files, or change system settings unless the " +
          "user explicitly asks.",
        stderr: (data) => console.error(`[claude-agent-sdk] ${data.trimEnd()}`),
      },
    })

    let finalResult
    for await (const message of claudeQuery) {
      if (message.type === "assistant") {
        logAssistantMessage(message)
      } else if (message.type === "result") {
        if (message.subtype === "success") {
          finalResult = message.result
          info(
            `completed in ${message.num_turns} turns ` +
              `($${message.total_cost_usd.toFixed(4)})`,
          )
        } else {
          throw new Error(
            `Claude Agent SDK stopped with ${message.subtype}: ` +
              `${message.errors.join("; ")}`,
          )
        }
      }
    }

    if (!finalResult) {
      throw new Error("Claude Agent SDK completed without a result message")
    }
    console.log(`\nAgent result:\n${finalResult}`)
  } finally {
    section("5. Cleanup")

    if (claudeQuery) {
      claudeQuery.close()
      info("Claude Agent SDK query closed")
    }

    for (const connection of mcpConnections.reverse()) {
      try {
        await connection.session.close()
        info(`${connection.service.name} MCP preflight session closed`)
      } catch (error) {
        info(
          `${connection.service.name} MCP cleanup failed: ` +
            `${await describeError(error)}`,
        )
      }
    }

    if (client && claimCreated) {
      try {
        await releaseClaim(client, pool, claimName)
        info(`claim released: ${claimName}`)
      } catch (error) {
        info(`claim cleanup failed: ${await describeError(error)}`)
      }
    }
  }
}

try {
  await main()
} catch (error) {
  console.error(
    `\nclaude-agent-sdk example failed: ${await describeError(error)}`,
  )
  process.exitCode = 1
}
