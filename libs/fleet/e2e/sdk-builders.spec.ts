import { expect, test } from "@playwright/test"
import { mockAuth, mockNamespacesApi, mockPoolsApi } from "./fixtures/mock-api"

test.describe("SDK builders", () => {
  test.beforeEach(async ({ page }) => {
    await mockAuth(page)
    await page.route("**/api/config", route =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: false, billing: false }),
      }),
    )
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await page.goto("/pools")
  })

  test("builds client, claim, and user-key records", async ({ page }) => {
    const result = await page.evaluate(async () => {
      const { buildClientConfiguration, ensureSdkInitialized } = await import(
        "/src/auth/cyclops-client.ts"
      )
      const { buildClaimRequest } = await import("/src/fleet/claims.ts")
      const { buildUserApiKeyRequest } = await import("/src/fleet/userKeys.ts")
      await ensureSdkInitialized()
      const pool = {
        apiVersion: "osgym.cua.ai/v1alpha1",
        kind: "OSGymSandboxWarmPool",
        metadata: { namespace: "demo-pool", name: "demo-pool" },
        spec: {
          replicas: 1,
          sandboxTemplateRef: { name: "demo-pool-template" },
        },
      }
      const configuration = buildClientConfiguration()
      const claim = buildClaimRequest(pool)
      const userKey = buildUserApiKeyRequest("automation")
      return {
        baseUrl: configuration.baseUrl,
        poolPollIntervalMs: configuration.poolPollIntervalMs.toString(),
        poolPollLimit: configuration.poolPollLimit,
        claimPollIntervalMs: configuration.claimPollIntervalMs.toString(),
        claimPollLimit: configuration.claimPollLimit,
        claimPool: claim.pool.metadata.name,
        claimSpecUnset: claim.spec === undefined,
        claimNameUnset: claim.name === undefined,
        keyName: userKey.name,
        keyScope: userKey.scope,
      }
    })

    expect(result).toEqual({
      baseUrl: new URL(page.url()).origin,
      poolPollIntervalMs: "5000",
      poolPollLimit: 120,
      claimPollIntervalMs: "5000",
      claimPollLimit: 120,
      claimPool: "demo-pool",
      claimSpecUnset: true,
      claimNameUnset: true,
      keyName: "automation",
      keyScope: [],
    })
  })

  test("builds nested pool and template requests", async ({ page }) => {
    const result = await page.evaluate(async () => {
      const { ensureSdkInitialized } = await import("/src/auth/cyclops-client.ts")
      const { buildPoolRequest, buildTemplateRequest } = await import("/src/fleet/pools.ts")
      const { Firmware, ServiceProtocol } = await import("/sdk-bindings/ts-uniffi-browser/ts/index.web.ts")
      await ensureSdkInitialized()
      const values = {
        cpu: 4,
        ram: "8Gi",
        ociImage: "registry.example/cyclops:v1",
        replicas: 2,
        ttlSecondsAfterCreated: 3600,
        firmware: "efi" as const,
        services: [
          { name: "ssh", targetPort: 22, protocol: "TCP" },
          { name: "dns", targetPort: 53, protocol: "udp" },
        ],
        autoscaling: { minPoolSize: 1, initialPoolSize: 2, maxPoolSize: 5 },
      }
      const pool = buildPoolRequest("demo-pool", "demo-pool-template", values)
      const template = buildTemplateRequest("demo-pool", "demo-pool-template", values)
      return {
        pool: {
          namespace: pool.namespace,
          replicas: pool.spec.replicas,
          template: pool.spec.sandboxTemplateRef.name,
          autoscaling: pool.spec.autoscaling,
          ttlSecondsAfterCreated: pool.spec.ttlSecondsAfterCreated,
        },
        template: {
          namespace: template.namespace,
          name: template.name,
          image: template.spec.vmTemplate.containerDiskImage,
          imagePullSecretUnset:
            template.spec.vmTemplate.imagePullSecret === undefined,
          firmwareIsEfi: template.spec.vmTemplate.firmware === Firmware.Efi,
          services: template.spec.vmTemplate.services?.map(service => ({
            name: service.name,
            targetPort: service.targetPort,
            udp: service.protocol === ServiceProtocol.Udp,
          })),
        },
      }
    })

    expect(result.pool).toEqual({
      namespace: "demo-pool",
      replicas: 2,
      template: "demo-pool-template",
      autoscaling: { minPoolSize: 1, initialPoolSize: 2, maxPoolSize: 5 },
      ttlSecondsAfterCreated: 3600,
    })
    expect(result.template).toEqual({
      namespace: "demo-pool",
      name: "demo-pool-template",
      image: "registry.example/cyclops:v1",
      imagePullSecretUnset: true,
      firmwareIsEfi: true,
      services: [
        { name: "ssh", targetPort: 22, udp: false },
        { name: "dns", targetPort: 53, udp: true },
      ],
    })
  })

  test("rebuilds template services without mutation", async ({ page }) => {
    const result = await page.evaluate(async () => {
      const { ensureSdkInitialized } = await import("/src/auth/cyclops-client.ts")
      const { rebuildTemplateWithServices } = await import("/src/fleet/pools.ts")
      const {
        Firmware,
        ImagePullPolicy,
        OidcConfig,
        PreservedJson,
        RuntimeKind,
        SandboxServiceBuilder,
      } = await import("/sdk-bindings/ts-uniffi-browser/ts/index.web.ts")
      await ensureSdkInitialized()
      const originalService = new SandboxServiceBuilder()
        .name("old")
        .targetPort(8080)
        .build()
      const toleration = PreservedJson.fromJson(
        JSON.stringify({ key: "dedicated", operator: "Equal", value: "gpu" }),
      )
      const probes = PreservedJson.fromJson(
        JSON.stringify({ liveness: { path: "/health", port: 8080 } }),
      )
      const oidc = OidcConfig.new({
        credentialsSecret: "oidc-credentials",
        tokenUrl: "https://issuer.example.test/token",
        awsRoleArn: "arn:aws:iam::123456789012:role/cyclops",
        awsRegion: "us-east-1",
        refreshIntervalSeconds: 300,
      })
      const original = {
        apiVersion: "osgym.cua.ai/v1alpha1",
        kind: "OSGymSandboxTemplate",
        metadata: {
          namespace: "demo-pool",
          name: "demo-pool-template",
          labels: new Map([["owner", "cyclops"]]),
          creationTimestamp: "2026-08-07T00:00:00Z",
        },
        spec: {
          vmTemplate: {
            containerDiskImage: "registry.example/cyclops:v1",
            command: ["sleep", "infinity"],
            runtime: RuntimeKind.Gvisor,
            runtimeClassName: "kata",
            nodeSelector: new Map([["node-role.kubernetes.io/gpu", "true"]]),
            tolerations: [toleration],
            imagePullPolicy: ImagePullPolicy.IfNotPresent,
            imagePullSecret: "pull-secret",
            cpuCores: 4,
            memory: "8Gi",
            firmware: Firmware.Efi,
            probes,
            services: [originalService],
            oidc,
          },
        },
      }
      const originalVm = original.spec.vmTemplate
      const rebuilt = rebuildTemplateWithServices(original, [
        { name: "ssh", targetPort: 22, protocol: "TCP" },
      ])
      return {
        templateChanged: rebuilt !== original,
        vmChanged: rebuilt.spec.vmTemplate !== originalVm,
        originalServices: original.spec.vmTemplate.services?.map(service => ({
          name: service.name,
          targetPort: service.targetPort,
        })),
        originalNodeSelector: Array.from(
          original.spec.vmTemplate.nodeSelector?.entries() ?? [],
        ),
        originalTolerations: original.spec.vmTemplate.tolerations?.map(value =>
          value.toJson(),
        ),
        originalProbes: original.spec.vmTemplate.probes?.toJson(),
        originalOidc: original.spec.vmTemplate.oidc,
        rebuiltServices: rebuilt.spec.vmTemplate.services?.map(service => ({
          name: service.name,
          targetPort: service.targetPort,
        })),
        vm: {
          containerDiskImage: rebuilt.spec.vmTemplate.containerDiskImage,
          command: rebuilt.spec.vmTemplate.command,
          runtime: rebuilt.spec.vmTemplate.runtime,
          runtimeClassName: rebuilt.spec.vmTemplate.runtimeClassName,
          nodeSelector: Array.from(
            rebuilt.spec.vmTemplate.nodeSelector?.entries() ?? [],
          ),
          tolerations: rebuilt.spec.vmTemplate.tolerations?.map(value =>
            value.toJson(),
          ),
          imagePullPolicy: rebuilt.spec.vmTemplate.imagePullPolicy,
          imagePullSecret: rebuilt.spec.vmTemplate.imagePullSecret,
          cpuCores: rebuilt.spec.vmTemplate.cpuCores,
          memory: rebuilt.spec.vmTemplate.memory,
          firmware: rebuilt.spec.vmTemplate.firmware,
          probes: rebuilt.spec.vmTemplate.probes?.toJson(),
          oidc: rebuilt.spec.vmTemplate.oidc,
        },
        owner: rebuilt.metadata.labels?.get("owner"),
        creationTimestamp: rebuilt.metadata.creationTimestamp,
      }
    })

    expect(result).toEqual({
      templateChanged: true,
      vmChanged: true,
      originalServices: [{ name: "old", targetPort: 8080 }],
      originalNodeSelector: [["node-role.kubernetes.io/gpu", "true"]],
      originalTolerations: [
        '{"key":"dedicated","operator":"Equal","value":"gpu"}',
      ],
      originalProbes: '{"liveness":{"path":"/health","port":8080}}',
      originalOidc: {
        credentialsSecret: "oidc-credentials",
        tokenUrl: "https://issuer.example.test/token",
        awsRoleArn: "arn:aws:iam::123456789012:role/cyclops",
        awsRegion: "us-east-1",
        refreshIntervalSeconds: 300,
      },
      rebuiltServices: [{ name: "ssh", targetPort: 22 }],
      vm: {
        containerDiskImage: "registry.example/cyclops:v1",
        command: ["sleep", "infinity"],
        runtime: 2,
        runtimeClassName: "kata",
        nodeSelector: [["node-role.kubernetes.io/gpu", "true"]],
        tolerations: [
          '{"key":"dedicated","operator":"Equal","value":"gpu"}',
        ],
        imagePullPolicy: 1,
        imagePullSecret: "pull-secret",
        cpuCores: 4,
        memory: "8Gi",
        firmware: 1,
        probes: '{"liveness":{"path":"/health","port":8080}}',
        oidc: {
          credentialsSecret: "oidc-credentials",
          tokenUrl: "https://issuer.example.test/token",
          awsRoleArn: "arn:aws:iam::123456789012:role/cyclops",
          awsRegion: "us-east-1",
          refreshIntervalSeconds: 300,
        },
      },
      owner: "cyclops",
      creationTimestamp: "2026-08-07T00:00:00Z",
    })
  })

  test("rolls back once and rethrows the template failure", async ({ page }) => {
    const poolPath =
      "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/rollback-pool/osgymsandboxwarmpools"
    const templatePath =
      "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/rollback-pool/osgymsandboxtemplates"
    const deletePath = `${poolPath}/rollback-pool`
    const templateFailure = "template creation failed distinctively"
    let deleteAttempts = 0

    await page.route(poolPath, async route => {
      if (route.request().method() !== "POST") return route.continue()
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          apiVersion: "osgym.cua.ai/v1alpha1",
          kind: "OSGymSandboxWarmPool",
          metadata: { namespace: "rollback-pool", name: "rollback-pool" },
          spec: {
            replicas: 1,
            sandboxTemplateRef: { name: "rollback-pool-template" },
          },
        }),
      })
    })
    await page.route(templatePath, route =>
      route.fulfill({ status: 422, body: templateFailure }),
    )
    await page.route(deletePath, async route => {
      deleteAttempts += 1
      await route.fulfill({ status: 503, body: "rollback cleanup failed" })
    })

    const result = await page.evaluate(async () => {
      const { createPool } = await import("/src/fleet/pools.ts")
      const { SdkError } = await import("/sdk-bindings/ts-uniffi-browser/ts/index.web.ts")
      try {
        await createPool("rollback-pool", {
          cpu: 1,
          ram: "1Gi",
          ociImage: "registry.example/cyclops:v1",
          replicas: 1,
        })
      } catch (error) {
        if (SdkError.Status.instanceOf(error)) return error.inner
        return { unexpectedError: String(error) }
      }
      return { unexpectedSuccess: true }
    })

    expect(deleteAttempts).toBe(1)
    expect(result).toEqual({
      operation: "create template",
      status: 422,
      body: templateFailure,
    })
  })

})
