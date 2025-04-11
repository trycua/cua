import ArgumentParser
import Foundation
import Virtualization

@MainActor
extension Server {
    // MARK: - VM Management Handlers

    func handleListVMs() async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            let vms = try vmController.list()
            return try .json(vms)
        } catch {
            return .badRequest(message: error.localizedDescription)
        }
    }

    func handleGetVM(name: String) async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            let vm = try vmController.get(name: name)
            return try .json(vm.details)
        } catch {
            return .badRequest(message: error.localizedDescription)
        }
    }

    func handleCreateVM(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(CreateVMRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        do {
            let sizes = try request.parse()
            let vmController = LumeController()
            try await vmController.create(
                name: request.name,
                os: request.os,
                diskSize: sizes.diskSize,
                cpuCount: request.cpu,
                memorySize: sizes.memory,
                display: request.display,
                ipsw: request.ipsw
            )

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "VM created successfully", "name": request.name,
                ])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleDeleteVM(name: String) async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            try await vmController.delete(name: name)
            return HTTPResponse(
                statusCode: .ok, headers: ["Content-Type": "application/json"], body: Data())
        } catch {
            return HTTPResponse(
                statusCode: .badRequest, headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription)))
        }
    }

    func handleCloneVM(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(CloneRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        do {
            let vmController = LumeController()
            try vmController.clone(name: request.name, newName: request.newName)

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "VM cloned successfully",
                    "source": request.name,
                    "destination": request.newName,
                ])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    // MARK: - VM Operation Handlers

    func handleSetVM(name: String, body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(SetVMRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        do {
            let vmController = LumeController()
            let sizes = try request.parse()
            try vmController.updateSettings(
                name: name,
                cpu: request.cpu,
                memory: sizes.memory,
                diskSize: sizes.diskSize,
                display: sizes.display?.string
            )

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "VM settings updated successfully"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleStopVM(name: String) async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            try await vmController.stopVM(name: name)
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "VM stopped successfully"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleRunVM(name: String, body: Data?) async throws -> HTTPResponse {
        let request =
            body.flatMap { try? JSONDecoder().decode(RunVMRequest.self, from: $0) }
            ?? RunVMRequest(noDisplay: nil, sharedDirectories: nil, recoveryMode: nil)

        do {
            let dirs = try request.parse()

            // Start VM in background
            startVM(
                name: name,
                noDisplay: request.noDisplay ?? false,
                sharedDirectories: dirs,
                recoveryMode: request.recoveryMode ?? false
            )

            // Return response immediately
            return HTTPResponse(
                statusCode: .accepted,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "VM start initiated",
                    "name": name,
                    "status": "pending",
                ])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    // MARK: - Image Management Handlers

    func handleIPSW() async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            let url = try await vmController.getLatestIPSWURL()
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["url": url.absoluteString])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handlePull(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(PullRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        do {
            let vmController = LumeController()
            try await vmController.pullImage(
                image: request.image,
                name: request.name,
                registry: request.registry,
                organization: request.organization
            )
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "Image pulled successfully"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handlePruneImages() async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            try await vmController.pruneImages()
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "Successfully removed cached images"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleGetImages(_ request: HTTPRequest) async throws -> HTTPResponse {
        let pathAndQuery = request.path.split(separator: "?", maxSplits: 1)
        let queryParams =
            pathAndQuery.count > 1
            ? pathAndQuery[1]
                .split(separator: "&")
                .reduce(into: [String: String]()) { dict, param in
                    let parts = param.split(separator: "=", maxSplits: 1)
                    if parts.count == 2 {
                        dict[String(parts[0])] = String(parts[1])
                    }
                } : [:]

        let organization = queryParams["organization"] ?? "trycua"

        do {
            let vmController = LumeController()
            let imageList = try await vmController.getImages(organization: organization)

            // Create a response format that matches the CLI output
            let response = imageList.local.map {
                [
                    "repository": $0.repository,
                    "imageId": $0.imageId,
                ]
            }

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(response)
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    // MARK: - Private Helper Methods

    nonisolated private func startVM(
        name: String,
        noDisplay: Bool,
        sharedDirectories: [SharedDirectory] = [],
        recoveryMode: Bool = false
    ) {
        Task.detached { @MainActor @Sendable in
            Logger.info("Starting VM in background", metadata: ["name": name])
            do {
                let vmController = LumeController()
                try await vmController.runVM(
                    name: name,
                    noDisplay: noDisplay,
                    sharedDirectories: sharedDirectories,
                    recoveryMode: recoveryMode
                )
                Logger.info("VM started successfully in background", metadata: ["name": name])
            } catch {
                Logger.error(
                    "Failed to start VM in background",
                    metadata: [
                        "name": name,
                        "error": error.localizedDescription,
                    ])
            }
        }
    }
}
