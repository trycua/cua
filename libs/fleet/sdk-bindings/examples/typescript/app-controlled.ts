#!/usr/bin/env -S node --enable-source-maps
import { randomUUID } from "node:crypto"
import { connect, type Claim, type Pool, type Sandbox, type SDK } from "../../typescript/dist/index.js"

async function runAgent(sdk: SDK, sandbox: Sandbox): Promise<unknown> {
  const deadline = Date.now() + 300_000
  const mcp = sdk.serviceFetch(sandbox, "mcp")
  while (true) {
    const response = await mcp("/mcp", {
      method: "POST",
      headers: { accept: "application/json, text/event-stream", "content-type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "cyclops-sdk-typescript-example", version: "0.1.0" } },
      }),
    })
    if (response.status >= 200 && response.status < 300) {
      return { namespace: sandbox.namespace, claim: sandbox.claim, sandbox: sandbox.sandbox, mcp_status: response.status }
    }
    const body = await response.text()
    if (![502, 503, 504].includes(response.status) || Date.now() >= deadline) {
      throw new Error(`MCP initialize failed with HTTP ${response.status}: ${body}`)
    }
    await new Promise((resolve) => setTimeout(resolve, 5_000))
  }
}


const namespace = process.env.CYCLOPS_NAMESPACE ?? `sdk-example-${randomUUID().replaceAll("-", "").slice(0, 8)}`
const sdk = await connect({
  baseUrl: process.env.CUA_BASE_URL ?? "https://run.cua.ai",
  oauth: {
    tokenUrl: process.env.CUA_TOKEN_URL ?? "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
    clientId: required("CUA_CLIENT_ID"),
    clientSecret: required("CUA_CLIENT_SECRET"),
  },
})
let pool: Pool | undefined
let claim: Claim | undefined
try {
  pool = await sdk.createPool({
    namespace,
    spec: {
      replicas: 1,
      services: [{ name: "mcp", targetPort: 3000, protocol: "TCP" }],
      template: {
        containerDiskImage: required("CUA_IMAGE"),
        imagePullSecret: required("CUA_IMAGE_PULL_SECRET"),
        cpuCores: 4,
        memory: "4Gi",
      },
    },
  })
  claim = await sdk.createClaim({ pool })
  const sandbox = await sdk.waitClaim(claim)
  const result = await runAgent(sdk, sandbox)
  console.log(JSON.stringify(result))
} finally {
  try {
    if (claim) await sdk.deleteClaim(claim)
  } finally {
    try {
      if (pool) await sdk.deletePool(pool)
    } finally {
      sdk.close()
    }
  }
}

function required(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`${name} is required`)
  return value
}
