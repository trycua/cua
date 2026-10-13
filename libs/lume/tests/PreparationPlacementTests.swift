import CryptoKit
import Foundation
import Testing

@testable import lume

private final class PlacementFixtureProtocol: URLProtocol, @unchecked Sendable {
    struct Plan: @unchecked Sendable {
        let status: Int
        let body: Data
        let contentLength: Bool
        let delay: TimeInterval
    }

    nonisolated(unsafe) static var plan = Plan(status: 200, body: Data(), contentLength: true, delay: 0)
    private var stopped = false

    override class func canInit(with request: URLRequest) -> Bool { request.url?.host == "placement.invalid" }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let plan = Self.plan
        guard let url = request.url else { return }
        let fields = plan.contentLength ? ["Content-Length": String(plan.body.count)] : [:]
        let response = HTTPURLResponse(url: url, statusCode: plan.status, httpVersion: "HTTP/1.1", headerFields: fields)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        let send: @Sendable () -> Void = { [weak self] in
            guard let self, !self.stopped else { return }
            self.client?.urlProtocol(self, didLoad: plan.body)
            self.client?.urlProtocolDidFinishLoading(self)
        }
        if plan.delay == 0 { send() } else { DispatchQueue.global().asyncAfter(deadline: .now() + plan.delay, execute: send) }
    }

    override func stopLoading() { stopped = true }
}

private func placementFixtureConfiguration() -> URLSessionConfiguration {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [PlacementFixtureProtocol.self]
    return configuration
}

private func placementDigest(_ data: Data) -> String {
    "sha256:" + SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

private func placementTemp(_ name: String) throws -> URL {
    let url = FileManager.default.temporaryDirectory.appendingPathComponent("lume-placement-\(name)-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    return url
}

@Suite(.serialized)
struct PreparationPlacementTests {
    @Test func ownedRootAndSixWriterRootsArePrivateAndContained() throws {
        let base = try placementTemp("roots")
        defer { try? FileManager.default.removeItem(at: base) }
        let root = try OwnedScopeWorkRoot(scopeID: "scope-1", root: base.appendingPathComponent("work"))
        let operation = try root.operationRoot("operation-1")
        let layout = try StagingLayout(operationRoot: operation)
        let roots = [layout.wholeVM, layout.legacyDownload, layout.ociTemporary, layout.cacheReassembly, layout.gunzipRaw, layout.finalAssembly]
        #expect(roots.count == 6)
        #expect(roots.allSatisfy { $0.path.hasPrefix(root.root.path + "/operations/") })
        #expect((try FileManager.default.attributesOfItem(atPath: layout.wholeVM.path)[.posixPermissions] as? NSNumber)?.intValue == 0o700)
        #expect(throws: PreparationPlacementError.invalidOperationIdentifier) { try root.operationRoot("../escape") }
    }

    @Test func gcsRefusesOwnedWorkRootBeforeRegistryConstruction() throws {
        let base = try placementTemp("gcs")
        defer { try? FileManager.default.removeItem(at: base) }
        let config = RegistryConfig(type: .gcs, ghcr: .defaultConfig, gcs: GCSConfig(apiUrl: "https://api.invalid", apiKey: "key"))
        #expect(throws: PreparationPlacementError.unsupportedGCSWorkRoot) {
            try RegistryFactory.createRegistry(config: config, preparationWorkRoot: base.path, preparationScopeID: "scope", preparationOperationID: "op")
        }
    }

    @Test func streamingCommitEnforcesDigestAndBodyLimitAndPreservesDestination() async throws {
        let base = try placementTemp("stream")
        defer { try? FileManager.default.removeItem(at: base) }
        let body = Data(repeating: 0x42, count: 64)
        let destination = base.appendingPathComponent("destination")
        let prior = Data("verified".utf8)
        try prior.write(to: destination)
        PlacementFixtureProtocol.plan = .init(status: 200, body: body, contentLength: false, delay: 0)
        let input = StreamingLayerRequest(
            request: URLRequest(url: URL(string: "http://placement.invalid/layer")!),
            operationRoot: base.appendingPathComponent("operation"),
            destination: destination,
            expectedDigest: placementDigest(body),
            admission: BodyAdmissionPolicy(maximumBytes: 128),
            onBytes: { _ in })
        #expect(try await StreamingLayerWriter.run(input, configuration: placementFixtureConfiguration()) == 64)
        #expect(try Data(contentsOf: destination) == body)

        PlacementFixtureProtocol.plan = .init(status: 404, body: Data("not found".utf8), contentLength: true, delay: 0)
        let rejected = StreamingLayerRequest(
            request: input.request,
            operationRoot: base.appendingPathComponent("rejected"),
            destination: destination,
            admission: BodyAdmissionPolicy(maximumBytes: 128),
            onBytes: { _ in })
        do {
            _ = try await StreamingLayerWriter.run(rejected, configuration: placementFixtureConfiguration())
            Issue.record("expected status rejection")
        } catch let error as PreparationPlacementError {
            #expect(error == .responseRejected(404))
        }
        #expect(try Data(contentsOf: destination) == body)

        PlacementFixtureProtocol.plan = .init(status: 200, body: Data(repeating: 0x21, count: 129), contentLength: true, delay: 0)
        let limited = StreamingLayerRequest(
            request: input.request,
            operationRoot: base.appendingPathComponent("limited"),
            destination: destination,
            expectedDigest: nil,
            admission: BodyAdmissionPolicy(maximumBytes: 32),
            onBytes: { _ in })
        do {
            _ = try await StreamingLayerWriter.run(limited, configuration: placementFixtureConfiguration())
            Issue.record("expected content-length rejection")
        } catch let error as PreparationPlacementError {
            #expect(error == .contentLengthExceeded(129))
        }
        #expect(try Data(contentsOf: destination) == body)

        PlacementFixtureProtocol.plan = .init(status: 200, body: Data("bad".utf8), contentLength: true, delay: 0)
        let mismatch = StreamingLayerRequest(
            request: input.request,
            operationRoot: base.appendingPathComponent("mismatch"),
            destination: destination,
            expectedDigest: "sha256:\(String(repeating: "0", count: 64))",
            admission: BodyAdmissionPolicy(maximumBytes: 32),
            onBytes: { _ in })
        do {
            _ = try await StreamingLayerWriter.run(mismatch, configuration: placementFixtureConfiguration())
            Issue.record("expected digest rejection")
        } catch {
            #expect(error is PreparationPlacementError)
        }
        #expect(try Data(contentsOf: destination) == body)
    }

    @Test func streamingCancellationCleansPartAndPreservesDestination() async throws {
        let base = try placementTemp("cancel")
        defer { try? FileManager.default.removeItem(at: base) }
        let destination = base.appendingPathComponent("destination")
        let prior = Data("verified".utf8)
        try prior.write(to: destination)
        PlacementFixtureProtocol.plan = .init(status: 200, body: Data(repeating: 0x11, count: 10), contentLength: true, delay: 0.2)
        let input = StreamingLayerRequest(
            request: URLRequest(url: URL(string: "http://placement.invalid/layer")!),
            operationRoot: base.appendingPathComponent("operation"),
            destination: destination,
            admission: BodyAdmissionPolicy(maximumBytes: 128),
            onBytes: { _ in })
        let task = Task { try await StreamingLayerWriter.run(input, configuration: placementFixtureConfiguration()) }
        task.cancel()
        do {
            _ = try await task.value
            Issue.record("expected cancellation")
        } catch {
            #expect(true)
        }
        #expect(try Data(contentsOf: destination) == prior)
        #expect((try? FileManager.default.contentsOfDirectory(at: input.operationRoot, includingPropertiesForKeys: nil).filter { $0.pathExtension == "part" }.isEmpty) ?? true)
    }
}
