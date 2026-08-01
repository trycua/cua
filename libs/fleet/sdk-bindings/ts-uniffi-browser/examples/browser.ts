import {
  CyclopsClient,
  uniffiInitAsync,
  type Claim,
  type Pool,
} from "../ts/index.web"
import type { PoolSpec } from "../ts/cyclops_sdk_schema"

declare global {
  interface Window {
    __CYCLOPS_BROWSER_CONFIG__?: BrowserRuntimeConfig
  }
}

type BrowserRuntimeConfig = {
  accessToken: string
  baseUrl: string
  namespace: string
  image: string
  imagePullSecret?: string
}

const serviceName = "mcp"
const servicePath = "/health"
const output = document.querySelector<HTMLPreElement>("#output")!
const runButton = document.querySelector<HTMLButtonElement>("#run")!
const sdkReady = document.querySelector<HTMLElement>("[data-testid=sdk-ready]")!
const lifecycleStatus = document.querySelector<HTMLElement>("[data-testid=lifecycle-status]")!

const sdkInitialization = uniffiInitAsync().then(
  () => {
    sdkReady.textContent = "ready"
  },
  (error) => {
    sdkReady.textContent = "failed"
    log("SDK initialization failed:", error instanceof Error ? error.message : String(error))
    throw error
  },
)

function log(message: string, value?: unknown): void {
  output.textContent += `\n${message}${value === undefined ? "" : ` ${JSON.stringify(value, null, 2)}`}`
}

function describeError(error: unknown): Record<string, unknown> {
  if (error instanceof Error) {
    return {
      type: error.constructor.name,
      name: error.name,
      message: error.message,
      stack: error.stack,
      cause: error.cause instanceof Error ? { name: error.cause.name, message: error.cause.message, stack: error.cause.stack } : error.cause,
    }
  }

  return { type: typeof error, value: String(error) }
}

function runtimeConfig(): BrowserRuntimeConfig {
  const config = window.__CYCLOPS_BROWSER_CONFIG__
  if (!config) {
    throw new Error("browser runtime configuration is unavailable")
  }

  for (const [name, value] of Object.entries(config)) {
    if (!value?.trim()) {
      throw new Error(`browser runtime configuration is missing ${name}`)
    }
  }
  return config
}

function makePoolSpec(image: string, imagePullSecret?: string): PoolSpec {
  return {
    replicas: 1,
    template: {
      containerDiskImage: image,
      cpuCores: 4,
      memory: "4Gi",
      imagePullSecret,
    },
    services: [{ name: serviceName, targetPort: 3000 }],
  }
}

async function runLifecycle(): Promise<void> {
  await sdkInitialization
  const config = runtimeConfig()
  log("[auth] Supplying runner access token to SDK client.")
  const client = CyclopsClient.connectBrowserWithAccessToken(
    {
      baseUrl: config.baseUrl,
      poolPollIntervalMs: 5_000n,
      poolPollLimit: 120,
      claimPollIntervalMs: 5_000n,
      claimPollLimit: 120,
    },
    config.accessToken,
  )
  log("[auth] SDK client connected.")

  let pool: Pool | undefined
  let claim: Claim | undefined
  try {
    log("[1/5] Creating pool...")
    pool = await client.createPool({
      namespace: config.namespace,
      spec: makePoolSpec(config.image, config.imagePullSecret),
    })

    log("[2/5] Creating claim...")
    claim = await client.createClaim({ pool })

    log("[3/5] Waiting for claim to bind a sandbox...")
    const sandbox = await client.waitClaim(claim)

    log("[4/5] Calling the sandbox service...")
    const response = await client.serviceRequest(sandbox, serviceName, servicePath, {
      method: "GET",
      url: `https://ignored.invalid${servicePath}`,
      headers: [],
    })
    log("Service response:", {
      status: response.status,
      body: new TextDecoder().decode(response.body),
    })
    lifecycleStatus.textContent = "completed"
    log("[5/5] Lifecycle completed; cleanup will now run.")
  } finally {
    if (claim) {
      log("[cleanup] Deleting claim...")
      await client.deleteClaim(claim)
    }
    if (pool) {
      log("[cleanup] Deleting pool...")
      await client.deletePool(pool)
    }
  }
}

runButton.addEventListener("click", async () => {
  output.textContent = "Running..."
  lifecycleStatus.textContent = "running"
  runButton.disabled = true
  try {
    await runLifecycle()
  } catch (error) {
    lifecycleStatus.textContent = "failed"
    log("Lifecycle failed:", describeError(error))
  } finally {
    runButton.disabled = false
  }
})
