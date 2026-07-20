import Foundation

actor ScriptedHttpClient: HttpClient {
    private var expected: [(String, String, UInt16, String)] = [
        ("POST", "https://keycloak.invalid/token", 200, #"{"access_token":"offline-token","expires_in":3600}"#),
        ("POST", "https://cyclops.invalid/api/namespaces", 201, "{}"),
        ("POST", "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools", 201, #"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"replicas":1,"template":{"containerDiskImage":"registry.example/desktop:offline"},"services":[{"name":"mcp","targetPort":8080}]},"status":null}"#),
        ("POST", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims", 201, #"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":{"phase":"Bound","sandbox":{"name":"offline-sandbox"}}}"#),
        ("GET", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", 200, #"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":{"phase":"Bound","sandbox":{"name":"offline-sandbox"}}}"#),
        ("GET", "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default", 200, #"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"replicas":1,"template":{"containerDiskImage":"registry.example/desktop:offline"},"services":[{"name":"mcp","targetPort":8080}]},"status":null}"#),
        ("POST", "https://cyclops.invalid/api/svc/default/offline-sandbox-mcp/mcp", 202, "offline service accepted"),
        ("DELETE", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", 204, ""),
        ("DELETE", "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default", 204, ""),
        ("DELETE", "https://cyclops.invalid/api/namespaces/default", 204, "")]
    private var requests = [HttpRequest]()
    func execute(request: HttpRequest) async throws -> HttpResponse { let item = expected.removeFirst(); precondition(request.method == item.0 && request.url == item.1); requests.append(request); return HttpResponse(status: item.2, headers: [], body: Data(item.3.utf8)) }
    func assertExhausted() { precondition(expected.isEmpty) }
    func methods() -> [String] { requests.map(\.method) }
}

@main struct AppControlled {
    static func main() async throws {
        let vmTemplate = VmTemplate(containerDiskImage: "registry.example/desktop:offline", command: nil, runtime: nil, runtimeClassName: nil, nodeSelector: nil, tolerations: nil, imagePullPolicy: nil, imagePullSecret: nil, cpuCores: nil, memory: nil, firmware: nil, probes: nil, services: nil, oidc: nil)
        let spec = PoolSpec(replicas: 1, template: PoolTemplate(runtime: nil, runtimeClassName: nil, nodeSelector: nil, tolerations: nil, command: nil, containerDiskImage: vmTemplate.containerDiskImage, imagePullSecret: nil, cpuCores: nil, memory: nil, firmware: nil, probes: nil, oidc: nil), autoscaling: nil, services: [SandboxService(name: "mcp", targetPort: 8080, protocol: nil)])
        let transport = ScriptedHttpClient()
        let credentials = CyclopsCredentials(clientId: "client-id", clientSecret: "client-secret")
        let client = try CyclopsClient.connect(configuration: CyclopsConfiguration(baseUrl: "https://cyclops.invalid", tokenUrl: "https://keycloak.invalid/token", credentials: credentials, poolPollIntervalMs: 1, poolPollLimit: 1, claimPollIntervalMs: 1, claimPollLimit: 2), httpClient: transport)
        let pool = try await client.createPool(request: CreatePoolRequest(namespace: "default", spec: spec))
        let claim = try await client.createClaim(request: CreateClaimRequest(pool: pool, spec: ClaimSpec(sandboxTemplateRef: SandboxTemplateRef(name: pool.metadata.name), warmpool: nil, bindDeadline: nil, lifecycle: nil)))
        let sandbox = try await client.waitClaim(claim: claim)
        let service = try await client.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: HttpRequest(method: "POST", url: "https://ignored.invalid/mcp", headers: [], body: Data("{\"offline\":true}".utf8)))
        try await client.deleteClaim(claim: claim)
        try await client.deletePool(pool: pool)
        await transport.assertExhausted()
        let methods = await transport.methods()
        precondition(methods == ["POST", "POST", "POST", "POST", "GET", "GET", "POST", "DELETE", "DELETE", "DELETE"])
        print("pool=\(pool.metadata.name) claim=\(claim.metadata.name) sandbox=\(sandbox.name) service_status=\(service.status)")
    }
}
