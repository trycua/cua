import {
  CyclopsClient,
  CyclopsCredentials,
  type Claim,
  type HttpClient,
  type HttpRequest,
  type HttpResponse,
  type Pool,
} from "../ts/cyclops_sdk";
import type { ClaimSpec, PoolSpec } from "../ts/cyclops_sdk_schema";

const serviceName = "mcp";
const servicePath = "/health";
const output = document.querySelector<HTMLPreElement>("#output")!;
const runButton = document.querySelector<HTMLButtonElement>("#run")!;
const form = document.querySelector<HTMLFormElement>("#config")!;

class FetchHttpClient implements HttpClient {
  async execute(request: HttpRequest): Promise<HttpResponse> {
    const response = await fetch(request.url, {
      method: request.method,
      headers: request.headers.map(({ name, value }) => [name, value]),
      body: request.body,
    });
    return {
      status: response.status,
      headers: [...response.headers].map(([name, value]) => ({ name, value })),
      body: await response.arrayBuffer(),
    };
  }
}

function log(message: string, value?: unknown): void {
  output.textContent += `\n${message}${value === undefined ? "" : ` ${JSON.stringify(value, null, 2)}`}`;
}

function values(): Record<string, string> {
  const data = new FormData(form);
  return Object.fromEntries([...data.entries()].map(([key, value]) => [key, String(value).trim()]));
}

function makePoolSpec(image: string, imagePullSecret: string): PoolSpec {
  return {
    replicas: 1,
    template: {
      // `containerDiskImage` is the only required template field. Production
      // pools should also set resource and scheduling requirements.
      containerDiskImage: image,
      imagePullSecret: imagePullSecret || undefined,
    },
    services: [{ name: serviceName, targetPort: 3000 }],
  };
}

async function runLifecycle(): Promise<void> {
  const config = values();
  const client = CyclopsClient.connect({
    baseUrl: config.baseUrl,
    tokenUrl: config.tokenUrl,
    credentials: new CyclopsCredentials(config.clientId, config.clientSecret),
    poolPollIntervalMs: 5000n,
    poolPollLimit: 100,
    claimPollIntervalMs: 2000n,
    claimPollLimit: 100,
  }, new FetchHttpClient());

  let pool: Pool | undefined;
  let claim: Claim | undefined;
  try {
    log("[1/8] Listing existing pools...");
    log("Pools:", await client.listPools(config.namespace));

    log("[2/8] Creating pool...");
    pool = await client.createPool({ namespace: config.namespace, spec: makePoolSpec(config.image, config.imagePullSecret) });
    log("Pool:", pool);

    log("[3/8] Listing pools after creation...");
    log("Pools:", await client.listPools(config.namespace));

    log("[4/8] Creating claim...");
    const spec: ClaimSpec = { sandboxTemplateRef: { name: pool.metadata.name } };
    claim = await client.createClaim({ pool, spec });
    log("Claim:", claim);

    log("[5/8] Listing claims...");
    log("Claims:", await client.listClaims(config.namespace));

    log("[6/8] Waiting for claim to bind a sandbox...");
    const sandbox = await client.waitClaim(claim);
    log("Sandbox:", sandbox);

    log("[7/8] Calling the sandbox service...");
    const response = await client.serviceRequest(sandbox, serviceName, servicePath, {
      method: "GET", url: `https://ignored.invalid${servicePath}`, headers: [],
    });
    log("Service response:", { status: response.status, body: new TextDecoder().decode(response.body) });
    log("[8/8] Lifecycle completed; cleanup will now run.");
  } finally {
    if (claim) {
      log("[cleanup] Deleting claim...");
      await client.deleteClaim(claim);
    }
    if (pool) {
      log("[cleanup] Deleting pool...");
      await client.deletePool(pool);
    }
  }
}

runButton.addEventListener("click", async () => {
  if (!form.reportValidity()) return;
  output.textContent = "Running...";
  runButton.disabled = true;
  try {
    await runLifecycle();
  } catch (error) {
    log("Lifecycle failed:", error instanceof Error ? error.message : String(error));
  } finally {
    runButton.disabled = false;
  }
});
