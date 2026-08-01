import Foundation

private struct Expected: @unchecked Sendable {
    let method: String
    let url: String
    let headers: [HttpHeader]
    let body: Data?
    let status: UInt16
    let response: Data
}

private let jsonHeaders = [
    HttpHeader(name: "accept", value: "application/json"),
    HttpHeader(name: "content-type", value: "application/json"),
    HttpHeader(name: "authorization", value: "Bearer offline-token"),
]

private func tokenExpected() -> Expected {
    Expected(
        method: "POST",
        url: "https://keycloak.invalid/token",
        headers: [
            HttpHeader(name: "accept", value: "application/json"),
            HttpHeader(name: "content-type", value: "application/x-www-form-urlencoded"),
        ],
        body: Data("grant_type=client_credentials&client_id=client-id&client_secret=client-secret".utf8),
        status: 200,
        response: Data(#"{"access_token":"offline-token","expires_in":3600}"#.utf8)
    )
}

private func serviceExpected(body: Data?, response: Data) -> Expected {
    Expected(
        method: "POST",
        url: "https://cyclops.invalid/api/svc/default/offline-sandbox-mcp/mcp",
        headers: [HttpHeader(name: "authorization", value: "Bearer offline-token")],
        body: body,
        status: 202,
        response: response
    )
}

private func lifecycleQueue() -> [Expected] {
    [
        tokenExpected(),
        Expected(method: "POST", url: "https://cyclops.invalid/api/namespaces", headers: jsonHeaders, body: Data(#"{"name":"default"}"#.utf8), status: 201, response: Data("{}".utf8)),
        Expected(method: "POST", url: "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools", headers: jsonHeaders, body: Data(#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"replicas":1,"template":{"containerDiskImage":"registry.example/desktop:offline"},"services":[{"name":"mcp","targetPort":8080}]},"status":null}"#.utf8), status: 201, response: Data(#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"replicas":1,"template":{"containerDiskImage":"registry.example/desktop:offline"},"services":[{"name":"mcp","targetPort":8080}]},"status":null}"#.utf8)),
        Expected(method: "POST", url: "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims", headers: jsonHeaders, body: Data(#"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"claim-1","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":null}"#.utf8), status: 201, response: Data(#"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":{"phase":"Bound","sandbox":{"name":"offline-sandbox"}}}"#.utf8)),
        Expected(method: "GET", url: "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", headers: jsonHeaders, body: nil, status: 200, response: Data(#"{"apiVersion":"osgym.cua.ai/v1alpha1","kind":"OSGymSandboxClaim","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"sandboxTemplateRef":{"name":"default"}},"status":{"phase":"Bound","sandbox":{"name":"offline-sandbox"}}}"#.utf8)),
        Expected(method: "GET", url: "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default", headers: jsonHeaders, body: nil, status: 200, response: Data(#"{"apiVersion":"cua.ai/v1","kind":"OSGymWorkspacePool","metadata":{"namespace":"default","name":"default","labels":null},"spec":{"replicas":1,"template":{"containerDiskImage":"registry.example/desktop:offline"},"services":[{"name":"mcp","targetPort":8080}]},"status":null}"#.utf8)),
        serviceExpected(body: Data(#"{"offline":true}"#.utf8), response: Data("offline service accepted".utf8)),
        Expected(method: "DELETE", url: "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", headers: jsonHeaders, body: nil, status: 204, response: Data()),
        Expected(method: "DELETE", url: "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default", headers: jsonHeaders, body: nil, status: 204, response: Data()),
        Expected(method: "DELETE", url: "https://cyclops.invalid/api/namespaces/default", headers: jsonHeaders, body: nil, status: 204, response: Data()),
    ]
}

actor ScriptedHttpClient: HttpClient {
    private var expected: [Expected]
    private var requests: [HttpRequest] = []

    fileprivate init(expected: [Expected]) {
        self.expected = expected
    }

    func execute(request: HttpRequest) async throws -> HttpResponse {
        guard !expected.isEmpty else { preconditionFailure("unexpected request") }
        let item = expected.removeFirst()
        precondition(request.method == item.method && request.url == item.url)
        precondition(request.headers == item.headers)
        precondition(request.body == item.body)
        requests.append(request)
        return HttpResponse(status: item.status, headers: [], body: item.response)
    }

    func assertExhausted() { precondition(expected.isEmpty) }
    func recordedRequestCount() -> Int { requests.count }
}

final class FailingHttpClient: HttpClient, @unchecked Sendable {
    func execute(request: HttpRequest) async throws -> HttpResponse {
        throw HttpError.Transport(reason: "scripted callback failure")
    }
}

private func configuration() -> CyclopsConfiguration {
    CyclopsConfiguration(baseUrl: "https://cyclops.invalid", tokenUrl: "https://keycloak.invalid/token", credentials: CyclopsCredentials(clientId: "client-id", clientSecret: "client-secret"), poolPollIntervalMs: 1, poolPollLimit: 1, claimPollIntervalMs: 1, claimPollLimit: 2)
}

private let sandbox = Sandbox(namespace: "default", claim: "default", name: "offline-sandbox", services: ["mcp"])
private func serviceRequest(_ body: Data?) -> HttpRequest {
    HttpRequest(method: "POST", url: "https://ignored.invalid/mcp", headers: [], body: body)
}

@main struct AppControlled {
    static func main() async throws {
        let vmTemplate = VmTemplate(containerDiskImage: "registry.example/desktop:offline", command: nil, runtime: nil, runtimeClassName: nil, nodeSelector: nil, tolerations: nil, imagePullPolicy: nil, imagePullSecret: nil, cpuCores: nil, memory: nil, firmware: nil, probes: nil, services: nil, oidc: nil)
        let spec = PoolSpec(replicas: 1, template: PoolTemplate(runtime: nil, runtimeClassName: nil, nodeSelector: nil, tolerations: nil, command: nil, containerDiskImage: vmTemplate.containerDiskImage, imagePullSecret: nil, cpuCores: nil, memory: nil, firmware: nil, probes: nil, oidc: nil), autoscaling: nil, services: [SandboxService(name: "mcp", targetPort: 8080, protocol: nil)])
        let transport = ScriptedHttpClient(expected: lifecycleQueue())
        let client = try CyclopsClient.connect(configuration: configuration(), httpClient: transport)
        let pool = try await client.createPool(request: CreatePoolRequest(namespace: "default", spec: spec))
        let claim = try await client.createClaim(request: CreateClaimRequest(pool: pool, spec: ClaimSpec(sandboxTemplateRef: SandboxTemplateRef(name: pool.metadata.name), warmpool: nil, bindDeadline: nil, lifecycle: nil)))
        let createdSandbox = try await client.waitClaim(claim: claim)
        let service = try await client.serviceRequest(sandbox: createdSandbox, service: "mcp", path: "/mcp", request: serviceRequest(Data(#"{"offline":true}"#.utf8)))
        try await client.deleteClaim(claim: claim)
        try await client.deletePool(pool: pool)
        await transport.assertExhausted()
        precondition(service.status == 202)

        let failingClient = try CyclopsClient.connect(configuration: configuration(), httpClient: FailingHttpClient())
        do {
            _ = try await failingClient.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: serviceRequest(nil))
            preconditionFailure("expected generated SdkError.Transport")
        } catch SdkError.Transport(let reason) {
            precondition(reason == "scripted callback failure")
        }

        let bodyTransport = ScriptedHttpClient(expected: [
            tokenExpected(),
            serviceExpected(body: nil, response: Data()),
            serviceExpected(body: Data(), response: Data([0, 0x7f, 0xff])),
        ])
        let bodyClient = try CyclopsClient.connect(configuration: configuration(), httpClient: bodyTransport)
        let emptyBodyResponse = try await bodyClient.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: serviceRequest(nil))
        precondition(emptyBodyResponse.body.isEmpty)
        let binaryBodyResponse = try await bodyClient.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: serviceRequest(Data()))
        precondition(binaryBodyResponse.body == Data([0, 0x7f, 0xff]))
        await bodyTransport.assertExhausted()

        let concurrentTransport = ScriptedHttpClient(expected: [
            tokenExpected(),
            serviceExpected(body: nil, response: Data("warm".utf8)),
            serviceExpected(body: Data("parallel".utf8), response: Data("first".utf8)),
            serviceExpected(body: Data("parallel".utf8), response: Data("second".utf8)),
        ])
        let concurrentClient = try CyclopsClient.connect(configuration: configuration(), httpClient: concurrentTransport)
        _ = try await concurrentClient.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: serviceRequest(nil))
        async let first = concurrentClient.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: serviceRequest(Data("parallel".utf8)))
        async let second = concurrentClient.serviceRequest(sandbox: sandbox, service: "mcp", path: "/mcp", request: serviceRequest(Data("parallel".utf8)))
        let concurrentResponses = try await (first, second)
        precondition(Set([concurrentResponses.0.body, concurrentResponses.1.body]) == Set([Data("first".utf8), Data("second".utf8)]))
        await concurrentTransport.assertExhausted()
        let recordedRequestCount = await concurrentTransport.recordedRequestCount()
        precondition(recordedRequestCount == 4)
        print("pool=\(pool.metadata.name) claim=\(claim.metadata.name) sandbox=\(createdSandbox.name) service_status=\(service.status)")
    }
}
