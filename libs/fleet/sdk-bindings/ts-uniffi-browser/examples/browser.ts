import {
  CreateClaimRequestBuilder,
  CreatePoolRequestBuilder,
  CreateTemplateRequestBuilder,
  CreateUserApiKeyRequestBuilder,
  CyclopsClient,
  CyclopsTokenProviderConfigurationBuilder,
  OsGymSandboxTemplateSpecBuilder,
  OsGymSandboxWarmPoolSpecBuilder,
  SandboxServiceBuilder,
  SandboxTemplateRefBuilder,
  SchemaBuildError,
  SdkBuildError,
  TemplateBuilder,
  VmTemplateBuilder,
  WarmPoolAutoscalingBuilder,
  uniffiInitAsync,
  type Claim,
  type OsGymSandboxTemplateSpec,
  type OsGymSandboxWarmPoolSpec,
  type Pool,
  type Template,
  type VmTemplateBuilderLike,
} from "../ts/index.web"

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
const builderReady = document.querySelector<HTMLElement>("[data-testid=builder-ready]")!
const lifecycleStatus = document.querySelector<HTMLElement>("[data-testid=lifecycle-status]")!

function verifyBuilderContract(): void {
  const configuration = new CyclopsTokenProviderConfigurationBuilder()
    .baseUrl("https://api.example.test")
    .poolPollIntervalMs(5_000n)
    .poolPollLimit(120)
    .claimPollIntervalMs(5_000n)
    .claimPollLimit(120)
    .build()
  const userKey = new CreateUserApiKeyRequestBuilder()
    .name("automation")
    .scope([])
    .build()
  const autoscaling = new WarmPoolAutoscalingBuilder()
    .minPoolSize(1)
    .initialPoolSize(2)
    .maxPoolSize(5)
    .build()
  const vm = new VmTemplateBuilder()
    .containerDiskImage("image")
    .build()
  const templateSpec = new OsGymSandboxTemplateSpecBuilder()
    .vmTemplate(vm)
    .build()
  const metadata = {
    namespace: "default",
    name: "template",
  }
  const template = new TemplateBuilder()
    .apiVersion("osgym.cua.ai/v1alpha1")
    .kind("OSGymSandboxTemplate")
    .metadata(metadata)
    .spec(templateSpec)
    .build()
  const templateRef = new SandboxTemplateRefBuilder()
    .name("template")
    .build()
  const poolSpec = new OsGymSandboxWarmPoolSpecBuilder()
    .replicas(1)
    .sandboxTemplateRef(templateRef)
    .build()
  const pool = {
    apiVersion: "osgym.cua.ai/v1alpha1",
    kind: "OSGymSandboxWarmPool",
    metadata: { namespace: "default", name: "default" },
    spec: poolSpec,
  }
  const claim = new CreateClaimRequestBuilder().pool(pool).build()
  const service = new SandboxServiceBuilder()
    .name("mcp")
    .targetPort(3000)
    .build()
  const poolRequest = new CreatePoolRequestBuilder()
    .namespace("default")
    .spec(poolSpec)
    .build()
  const templateRequest = new CreateTemplateRequestBuilder()
    .namespace("default")
    .name("template")
    .spec(templateSpec)
    .build()

  try {
    new CreateClaimRequestBuilder().build()
    throw new Error("incomplete SDK builder unexpectedly succeeded")
  } catch (error) {
    if (
      !SdkBuildError.MissingRequiredField.instanceOf(error) ||
      error.inner.recordType !== "CreateClaimRequest" ||
      error.inner.field !== "pool"
    ) {
      throw error
    }
  }

  try {
    new VmTemplateBuilder().build()
    throw new Error("incomplete schema builder unexpectedly succeeded")
  } catch (error) {
    if (
      !SchemaBuildError.MissingRequiredField.instanceOf(error) ||
      error.inner.recordType !== "VmTemplate" ||
      error.inner.field !== "container_disk_image"
    ) {
      throw error
    }
  }

  if (
    configuration.poolPollLimit !== 120 ||
    userKey.scope.length !== 0 ||
    autoscaling.maxPoolSize !== 5 ||
    template.metadata.name !== "template" ||
    claim.name !== undefined ||
    service.targetPort !== 3000 ||
    poolRequest.namespace !== "default" ||
    templateRequest.name !== "template"
  ) {
    throw new Error("generated browser builder contract returned unexpected records")
  }
}

const sdkInitialization = uniffiInitAsync().then(
  () => {
    sdkReady.textContent = "ready"
    try {
      verifyBuilderContract()
      builderReady.textContent = "ready"
    } catch (error) {
      builderReady.textContent = "failed"
      throw error
    }
  },
  (error) => {
    sdkReady.textContent = "failed"
    builderReady.textContent = "failed"
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

function makeTemplateSpec(
  image: string,
  imagePullSecret?: string,
): OsGymSandboxTemplateSpec {
  const service = new SandboxServiceBuilder()
    .name(serviceName)
    .targetPort(3000)
    .build()
  let vmBuilder: VmTemplateBuilderLike = new VmTemplateBuilder()
    .containerDiskImage(image)
    .cpuCores(4)
    .memory("4Gi")
    .services([service])
  if (imagePullSecret) vmBuilder = vmBuilder.imagePullSecret(imagePullSecret)
  return new OsGymSandboxTemplateSpecBuilder()
    .vmTemplate(vmBuilder.build())
    .build()
}

function makePoolSpec(namespace: string): OsGymSandboxWarmPoolSpec {
  const reference = new SandboxTemplateRefBuilder()
    .name(`${namespace}-template`)
    .build()
  return new OsGymSandboxWarmPoolSpecBuilder()
    .replicas(1)
    .sandboxTemplateRef(reference)
    .build()
}

async function runLifecycle(): Promise<void> {
  await sdkInitialization
  const config = runtimeConfig()
  log("[auth] Supplying runner access token to SDK client.")
  const clientConfiguration = new CyclopsTokenProviderConfigurationBuilder()
    .baseUrl(config.baseUrl)
    .poolPollIntervalMs(5_000n)
    .poolPollLimit(120)
    .claimPollIntervalMs(5_000n)
    .claimPollLimit(120)
    .build()
  const client = CyclopsClient.connectBrowserWithAccessToken(
    clientConfiguration,
    config.accessToken,
  )
  log("[auth] SDK client connected.")

  let pool: Pool | undefined
  let template: Template | undefined
  let claim: Claim | undefined
  try {
    log("[1/5] Creating pool and template...")
    const poolSpec = makePoolSpec(config.namespace)
    pool = await client.createPool(
      new CreatePoolRequestBuilder()
        .namespace(config.namespace)
        .spec(poolSpec)
        .build(),
    )
    const templateSpec = makeTemplateSpec(config.image, config.imagePullSecret)
    template = await client.createTemplate(
      new CreateTemplateRequestBuilder()
        .namespace(config.namespace)
        .name(`${config.namespace}-template`)
        .spec(templateSpec)
        .build(),
    )

    log("[2/5] Creating claim...")
    claim = await client.createClaim(
      new CreateClaimRequestBuilder().pool(pool).build(),
    )

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
    if (template) {
      log("[cleanup] Deleting template...")
      await client.deleteTemplate(template)
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
