import {
  CyclopsClient,
  CyclopsCredentials,
  type Claim,
  type CreateClaimRequest,
  type CreatePoolRequest,
  type HttpClient,
  type HttpRequest,
  type HttpResponse,
  type Pool,
  type PoolSpec,
} from "../index.ts";

const serviceName = "mcp";
const servicePath = "/health";

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

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing required environment variable: ${name}`);
  return value;
}

function poolSpec(image: string, imagePullSecret?: string): PoolSpec {
  return {
    replicas: 1,
    template: {
      // The live example requests the same resources as the working SDK examples.
      containerDiskImage: image,
      imagePullSecret,
      cpuCores: 4,
      memory: "4Gi",
    },
    services: [{ name: serviceName, targetPort: 3000 }],
  };
}

async function main(): Promise<void> {
  const namespace = requiredEnv("CYCLOPS_NAMESPACE");
  const image = requiredEnv("CYCLOPS_IMAGE");
  const client = CyclopsClient.connect({
    baseUrl: requiredEnv("CYCLOPS_BASE_URL"),
    tokenUrl: requiredEnv("CYCLOPS_TOKEN_URL"),
    credentials: new CyclopsCredentials(requiredEnv("CYCLOPS_CLIENT_ID"), requiredEnv("CYCLOPS_CLIENT_SECRET")),
    poolPollIntervalMs: 5000n,
    poolPollLimit: 100,
    claimPollIntervalMs: 5000n,
    claimPollLimit: 120,
  }, new FetchHttpClient());

  let pool: Pool | undefined;
  let claim: Claim | undefined;
  try {
    console.log("[1/5] Creating pool...");
    const createPool: CreatePoolRequest = { namespace, spec: poolSpec(image, process.env.CYCLOPS_IMAGE_PULL_SECRET) };
    pool = await client.createPool(createPool);
    console.log(pool);

    console.log("[2/5] Creating claim...");
    const createClaim: CreateClaimRequest = { pool };
    claim = await client.createClaim(createClaim);
    console.log(claim);

    console.log("[3/5] Waiting for claim to bind a sandbox...");
    const sandbox = await client.waitClaim(claim);
    console.log(sandbox);

    console.log("[4/5] Calling the sandbox service...");
    const response = await client.serviceRequest(sandbox, serviceName, servicePath, {
      method: "GET", url: `https://ignored.invalid${servicePath}`, headers: [],
    });
    console.log({ status: response.status, body: new TextDecoder().decode(response.body) });
    console.log("[5/5] Lifecycle completed; cleanup will now run.");
  } finally {
    if (claim) {
      console.log("[cleanup] Deleting claim...");
      await client.deleteClaim(claim);
    }
    if (pool) {
      console.log("[cleanup] Deleting pool...");
      await client.deletePool(pool);
    }
  }
}

main().catch((error: unknown) => {
  console.error("Lifecycle failed:", error);
  process.exitCode = 1;
});
