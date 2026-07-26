/**
 * Provision a CUA sandbox and let a Pi agent operate it through both MCP servers.
 *
 * This example claims a sandbox from an existing pool configured with the
 * computer-server and cua-driver services described by CUA_SERVICES.
 *
 * From cyclops-cs/js-sdk:
 *   npm run build
 *   npm install --no-save --package-lock=false \
 *     @earendil-works/pi-coding-agent @modelcontextprotocol/sdk typebox
 *   CUA_CLIENT_ID=ukey-... CUA_CLIENT_SECRET=... CUA_POOL=my-pool \
 *     node examples/pi-agent.mjs
 *
 * Pi requires Node.js 22.19 or newer and a configured model/API key. Use a
 * per-user Cyclops key because Kubernetes pool and claim operations require the
 * user's Kubernetes identity.
 */

import { randomUUID } from "node:crypto"

import {
  createAgentSession,
  defineTool,
  SessionManager,
} from "@earendil-works/pi-coding-agent"
// The MCP SDK calls this class Client; aliasing it makes the session role clear.
import { Client as ClientSession } from "@modelcontextprotocol/sdk/client"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import { Type } from "typebox"

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
const DEFAULT_TASK =
  "Open Firefox and navigate to https://news.ycombinator.com. " +
  "Take a screenshot, then click on the top story."

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
      body = error.error
        ? JSON.stringify(error.error)
        : await error.clone().text()
    } catch {
      // Status information remains useful when the response body is unavailable.
    }
    return `${error.status} ${error.statusText}${body ? `: ${body}` : ""}`
  }
  return error instanceof Error ? error.message : String(error)
}

async function createClaim(client, pool, claimName) {
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
    name: `cyclops-pi-${service.name}`,
    version: "1.0.0",
  })

  await session.connect(transport)
  const { tools } = await session.listTools()
  info(`${service.name}: connected to ${url} (${tools.length} tools)`)

  return { service, session, tools }
}

function safeToolName(name) {
  return name.replace(/[^a-zA-Z0-9_-]/g, "_")
}

function toPiContent(content = []) {
  return content.map((item) => {
    if (item.type === "text") {
      return { type: "text", text: item.text }
    }
    if (item.type === "image") {
      return { type: "image", data: item.data, mimeType: item.mimeType }
    }
    return { type: "text", text: JSON.stringify(item) }
  })
}

function registerPiTools(connections) {
  const names = new Set()

  return connections.flatMap(({ service, session, tools }) =>
    tools.map((tool) => {
      const name = `${safeToolName(service.name)}__${safeToolName(tool.name)}`
      if (names.has(name)) throw new Error(`Duplicate Pi tool name: ${name}`)
      names.add(name)

      return defineTool({
        name,
        label: `${service.name}: ${tool.name}`,
        description:
          tool.description ??
          `${tool.name} from the ${service.name} MCP server`,
        parameters: Type.Unsafe(
          tool.inputSchema ?? { type: "object", properties: {} },
        ),
        execute: async (_toolCallId, parameters, signal) => {
          try {
            const result = await session.callTool(
              { name: tool.name, arguments: parameters },
              undefined,
              signal ? { signal } : undefined,
            )
            return {
              content: toPiContent(result.content),
              details: {
                server: service.name,
                tool: tool.name,
                structuredContent: result.structuredContent,
              },
              isError: result.isError === true,
            }
          } catch (error) {
            return {
              content: [
                {
                  type: "text",
                  text:
                    `${service.name}/${tool.name} failed: ` +
                    `${await describeError(error)}`,
                },
              ],
              details: { server: service.name, tool: tool.name },
              isError: true,
            }
          }
        },
      })
    }),
  )
}

async function takeInitialScreenshot(connections) {
  const computerServer = connections.find(
    ({ service }) => service.name === "computer-server",
  )
  const screenshotTool = computerServer?.tools.find(
    (tool) => tool.name === "screenshot",
  )
  if (!computerServer || !screenshotTool) {
    throw new Error("computer-server did not advertise a screenshot tool")
  }

  const result = await computerServer.session.callTool({
    name: screenshotTool.name,
    arguments: {},
  })
  if (result.isError) {
    throw new Error(
      `Initial screenshot failed: ${JSON.stringify(result.content)}`,
    )
  }

  const images = toPiContent(result.content).filter(
    (item) => item.type === "image",
  )
  info(`initial screenshot captured (${images.length} image attachment)`)
  return images
}

function finalAssistantText(session) {
  const message = session.state.messages.findLast(
    (candidate) => candidate.role === "assistant",
  )
  return (
    message?.content
      .filter((part) => part.type === "text")
      .map((part) => part.text)
      .join("") ?? ""
  )
}

async function main() {
  const pool = requiredEnv("CUA_POOL")
  const baseUrl = process.env.CUA_BASE_URL ?? DEFAULT_BASE_URL
  const claimName =
    process.env.CUA_CLAIM_NAME ?? `pi-agent-${randomUUID().slice(0, 8)}`
  const bindTimeoutMs = envNumber(
    "CUA_BIND_TIMEOUT_MS",
    DEFAULT_BIND_TIMEOUT_MS,
  )
  const readyTimeoutMs = envNumber(
    "CUA_READY_TIMEOUT_MS",
    DEFAULT_READY_TIMEOUT_MS,
  )
  const task = process.argv.slice(2).join(" ") || DEFAULT_TASK

  let client
  let claimCreated = false
  let piSession
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

    section("3. Connect to MCP Servers")
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

    section("4. Register Tools with Pi")
    const customTools = registerPiTools(mcpConnections)
    info(`registered ${customTools.length} namespaced MCP tools`)
    for (const tool of customTools) info(tool.name)

    section("5. Run the Agent Task")
    const initialImages = await takeInitialScreenshot(mcpConnections)
    info(`task: ${task}`)

    const createdSession = await createAgentSession({
      tools: customTools.map((tool) => tool.name),
      customTools,
      sessionManager: SessionManager.inMemory(),
    })
    piSession = createdSession.session
    piSession.subscribe((event) => {
      if (event.type === "tool_execution_start") {
        info(`Pi calling ${event.toolName}`)
      }
    })

    await piSession.prompt(
      "You operate one CUA desktop through two MCP servers. Tool names are " +
        "namespaced by server (for example computer-server__screenshot and " +
        "cua-driver__click). Inspect the screenshot, take one action at a " +
        `time, and verify the result after each action.\n\nTask: ${task}`,
      { images: initialImages },
    )

    const result = finalAssistantText(piSession)
    if (!result) throw new Error("Pi completed without a text result")
    console.log(`\nAgent result:\n${result}`)
  } finally {
    section("6. Cleanup")

    if (piSession) {
      piSession.dispose()
      info("Pi session disposed")
    }

    for (const connection of mcpConnections.reverse()) {
      try {
        await connection.session.close()
        info(`${connection.service.name} MCP session closed`)
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
  console.error(`\npi-agent failed: ${await describeError(error)}`)
  process.exitCode = 1
}
