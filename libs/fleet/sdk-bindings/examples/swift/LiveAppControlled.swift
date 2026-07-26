import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

final class UrlSessionHttpClient: HttpClient, @unchecked Sendable {
    func execute(request: HttpRequest) async throws -> HttpResponse {
        guard let url = URL(string: request.url) else {
            throw HttpError.Transport(reason: "invalid URL: \(request.url)")
        }
        var native = URLRequest(url: url, timeoutInterval: 60)
        native.httpMethod = request.method
        request.headers.forEach { native.setValue($0.value, forHTTPHeaderField: $0.name) }
        native.httpBody = request.body
        do {
            let (body, response) = try await URLSession.shared.data(for: native)
            guard let http = response as? HTTPURLResponse else {
                throw HttpError.Transport(reason: "non-HTTP response")
            }
            let headers = http.allHeaderFields.compactMap { key, value -> HttpHeader? in
                guard let name = key as? String else { return nil }
                return HttpHeader(name: name, value: String(describing: value))
            }
            return HttpResponse(status: UInt16(http.statusCode), headers: headers, body: body)
        } catch let error as HttpError {
            throw error
        } catch {
            throw HttpError.Transport(reason: error.localizedDescription)
        }
    }
}

private func required(_ name: String) -> String {
    guard let value = ProcessInfo.processInfo.environment[name], !value.isEmpty else {
        fatalError("\(name) is required")
    }
    return value
}

private func poolSpec() -> PoolSpec {
    PoolSpec(
        replicas: 1,
        template: PoolTemplate(
            runtime: nil, runtimeClassName: nil, nodeSelector: nil, tolerations: nil, command: nil,
            containerDiskImage: required("CUA_IMAGE"), imagePullSecret: required("CUA_IMAGE_PULL_SECRET"),
            cpuCores: 4, memory: "4Gi", firmware: nil, probes: nil, oidc: nil
        ),
        autoscaling: nil,
        services: [SandboxService(name: "mcp", targetPort: 3000, protocol: nil)]
    )
}

private func initializeMcp(client: CyclopsClient, sandbox: Sandbox) async throws -> UInt16 {
    let body = Data(#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cyclops-uniffi-swift","version":"0.1.0"}}}"#.utf8)
    let clock = ContinuousClock()
    let deadline = clock.now.advanced(by: .seconds(300))
    while true {
        let response = try await client.serviceRequest(
            sandbox: sandbox,
            service: "mcp",
            path: "/mcp",
            request: HttpRequest(
                method: "POST",
                url: "https://ignored.invalid/mcp",
                headers: [
                    HttpHeader(name: "accept", value: "application/json, text/event-stream"),
                    HttpHeader(name: "content-type", value: "application/json"),
                ],
                body: body
            )
        )
        if (200...299).contains(Int(response.status)) { return response.status }
        if [502, 503, 504].contains(Int(response.status)) && clock.now < deadline {
            try await Task.sleep(for: .seconds(5))
            continue
        }
        throw HttpError.Transport(reason: "MCP initialize failed with HTTP \(response.status)")
    }
}

private func cleanup(client: CyclopsClient, claim: Claim?, pool: Pool?) async {
    do {
        if let claim { try await client.deleteClaim(claim: claim) }
    } catch {
        FileHandle.standardError.write(Data("claim cleanup failed: \(error)\n".utf8))
    }
    do {
        if let pool { try await client.deletePool(pool: pool) }
    } catch {
        FileHandle.standardError.write(Data("pool cleanup failed: \(error)\n".utf8))
    }
}

@main struct LiveAppControlled {
    static func main() async throws {
        let namespace = required("CYCLOPS_NAMESPACE")
        let credentials = CyclopsCredentials(clientId: required("CUA_CLIENT_ID"), clientSecret: required("CUA_CLIENT_SECRET"))
        let client = try CyclopsClient.connect(
            configuration: CyclopsConfiguration(baseUrl: required("CUA_BASE_URL"), tokenUrl: required("CUA_TOKEN_URL"), credentials: credentials, poolPollIntervalMs: 5_000, poolPollLimit: 120, claimPollIntervalMs: 5_000, claimPollLimit: 120),
            httpClient: UrlSessionHttpClient()
        )
        var createdPool: Pool?
        var createdClaim: Claim?
        do {
            let pool = try await client.createPool(request: CreatePoolRequest(namespace: namespace, spec: poolSpec()))
            createdPool = pool
            let claim = try await client.createClaim(request: CreateClaimRequest(pool: pool, spec: nil))
            createdClaim = claim
            let sandbox = try await client.waitClaim(claim: claim)
            let status = try await initializeMcp(client: client, sandbox: sandbox)
            print(#"{"namespace":"\#(namespace)","pool":"\#(pool.metadata.name)","claim":"\#(claim.metadata.name)","sandbox":"\#(sandbox.name)","mcp_status":\#(status)}"#)
        } catch {
            await cleanup(client: client, claim: createdClaim, pool: createdPool)
            throw error
        }
        await cleanup(client: client, claim: createdClaim, pool: createdPool)
    }
}
