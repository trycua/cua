import ArgumentParser
import Foundation
import Virtualization

@MainActor
extension Server {
    // MARK: - VM Management Handlers

    func handleListVMs(storage: String? = nil) async throws -> HTTPResponse {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMList)

        do {
            let vmController = LumeController()
            let vms = try vmController.list(storage: storage)
            return try .json(vms)
        } catch {
            print(
                "ERROR: Failed to list VMs: \(error.localizedDescription), storage=\(String(describing: storage))"
            )
            return .badRequest(message: error.localizedDescription)
        }
    }

    func handleGetVM(name: String, storage: String? = nil) async throws -> HTTPResponse {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMGet)

        // Check if an async pull is in progress for this VM name
        let pullProgress = await PullProgressTracker.shared.getPullProgress(for: name)
        let pullError = await PullProgressTracker.shared.getError(for: name)

        if let errorMsg = pullError {
            // Pull failed — surface the error
            return .badRequest(message: "Pull failed for '\(name)': \(errorMsg)")
        }

        if let progress = pullProgress {
            // Pull in progress — return a synthetic "pulling" status without hitting disk
            return try Self.pullingResponse(name: name, progress: progress)
        }

        do {
            let vmController = LumeController()
            // Use getDetails() for consistent status including provisioning state
            let details = try vmController.getDetails(name: name, storage: storage)
            return try HTTPResponse.json(details)
        } catch {
            return .badRequest(message: error.localizedDescription)
        }
    }

    /// The synthetic `GET /lume/vms/:name` body while an async pull runs:
    /// `downloadProgress` is the percent (0 to 100) older clients read;
    /// `downloadedBytes`, `totalBytes` and `bytesPerSecond` give byte detail.
    nonisolated static func pullingResponse(name: String, progress: PullProgress) throws
        -> HTTPResponse
    {
        let responseBody: [String: AnyEncodable] = [
            "name": AnyEncodable(name),
            "status": AnyEncodable("pulling"),
            "downloadProgress": AnyEncodable(progress.percent),
            "downloadedBytes": AnyEncodable(progress.downloadedBytes),
            "totalBytes": AnyEncodable(progress.totalBytes),
            "bytesPerSecond": AnyEncodable(progress.bytesPerSecond),
        ]
        return try HTTPResponse(
            statusCode: .ok,
            headers: ["Content-Type": "application/json"],
            body: JSONEncoder().encode(responseBody)
        )
    }

    /// `POST /lume/pull/cancel` with `{"name": "<vm>"}`: cancels the async pull
    /// started by `/lume/pull/start` for that name and waits (up to
    /// `pullCancelTimeout` seconds) until it stopped and cleaned up. 200 once it
    /// ended, 404 when no async pull runs for the name, 500 if it did not stop
    /// in time.
    func handlePullCancel(_ body: Data?, timeout: TimeInterval = 30) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(PullCancelRequest.self, from: body),
            !request.name.isEmpty
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        let outcome = await PullProgressTracker.shared.cancel(name: request.name, timeout: timeout)
        switch outcome {
        case .notFound:
            return HTTPResponse(
                statusCode: .notFound,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(
                    APIError(message: "no pull in progress for \(request.name)"))
            )
        case .cancelled:
            Logger.info("Async pull cancelled", metadata: ["name": request.name])
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "Pull cancelled",
                    "name": request.name,
                ])
            )
        case .timedOut:
            return HTTPResponse(
                statusCode: .internalServerError,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(
                    APIError(
                        message:
                            "the pull for \(request.name) did not stop within \(Int(timeout)) seconds"
                    ))
            )
        }
    }

    func handlePullStart(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(PullRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        let imageName = request.image.split(separator: ":").first.map(String.init) ?? request.image
        // Telemetry: the catalog id or `custom`, never the reference.
        TelemetryClient.shared.record(event: TelemetryEvent.apiPull, properties: [
            "image": TelemetryClient.imageId(
                request.image, registry: request.registry, organization: request.organization)
        ])

        let vmName = request.name ?? imageName
        let tracker = PullProgressTracker.shared

        // A second start for a name that is already pulling joins that pull
        // instead of racing a duplicate download into the same VM directory.
        if await tracker.activeCancellableToken(for: vmName) != nil {
            return HTTPResponse(
                statusCode: .accepted,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": AnyEncodable("Pull already in progress"),
                    "name": AnyEncodable(vmName),
                    "image": AnyEncodable(request.image),
                ])
            )
        }

        let token = await tracker.begin(name: vmName, cancellable: true)
        let pullStartedAt = Date()

        let task = Task.detached { @MainActor @Sendable in
            let outcome: PullProgressTracker.Outcome
            do {
                let vmController = LumeController()
                try await vmController.pullImage(
                    image: request.image,
                    name: request.name,
                    registry: request.registry,
                    organization: request.organization,
                    storage: request.storage,
                    progressHandler: { progress in
                        Task { await tracker.setProgress(progress, for: vmName, token: token) }
                    }
                )
                outcome = .completed
                Logger.info("Async pull completed", metadata: ["name": vmName])
            } catch {
                if Task.isCancelled || error is CancellationError {
                    outcome = .cancelled
                    Logger.info("Async pull stopped by cancel", metadata: ["name": vmName])
                } else {
                    outcome = .failed(error.localizedDescription)
                    Logger.error(
                        "Async pull failed",
                        metadata: ["name": vmName, "error": error.localizedDescription])
                }
            }
            let succeeded: Bool
            if case .completed = outcome { succeeded = true } else { succeeded = false }
            TelemetryClient.shared.recordOperationCompleted(
                operation: "pull_start",
                transport: .http,
                success: succeeded,
                errorClass: succeeded ? .none : .operationError,
                elapsed: Date().timeIntervalSince(pullStartedAt)
            )
            // Last step: a waiting cancel returns once this ran, after cleanup.
            await tracker.finish(name: vmName, token: token, outcome: outcome)
        }
        await tracker.attach(task, name: vmName, token: token)

        return HTTPResponse(
            statusCode: .accepted,
            headers: ["Content-Type": "application/json"],
            body: try JSONEncoder().encode([
                "message": AnyEncodable("Pull started"),
                "name": AnyEncodable(vmName),
                "image": AnyEncodable(request.image),
            ])
        )
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

        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMCreate, properties: [
            "os_type": request.os.lowercased(),
            "cpu": request.cpu,
            "memory": request.memory,
            "disk_size": request.diskSize
        ])

        do {
            let sizes = try request.parse()
            let vmController = LumeController()

            // Load unattended config if specified
            var unattendedConfig: UnattendedConfig? = nil
            if let unattendedArg = request.unattended {
                unattendedConfig = try UnattendedConfig.load(from: unattendedArg)
            }

            let networkMode = try request.parseNetworkMode()

            // Use async create - returns immediately while VM is provisioned in background
            try vmController.createAsync(
                name: request.name,
                os: request.os,
                diskSize: sizes.diskSize,
                cpuCount: request.cpu,
                memorySize: sizes.memory,
                display: request.display,
                ipsw: request.ipsw,
                storage: request.storage,
                unattendedConfig: unattendedConfig,
                networkMode: networkMode
            )

            // Return 202 Accepted - VM creation is in progress
            return HTTPResponse(
                statusCode: .accepted,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "VM creation started",
                    "name": request.name,
                    "status": "provisioning",
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

    func handleDeleteVM(name: String, storage: String? = nil) async throws -> HTTPResponse {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMDelete)

        do {
            let vmController = LumeController()
            try await vmController.delete(name: name, storage: storage)
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

        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMClone)

        do {
            let vmController = LumeController()
            try vmController.clone(
                name: request.name,
                newName: request.newName,
                sourceLocation: request.sourceLocation,
                destLocation: request.destLocation
            )

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

        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMUpdate)

        do {
            let vmController = LumeController()
            let sizes = try request.parse()
            try vmController.updateSettings(
                name: name,
                cpu: request.cpu,
                memory: sizes.memory,
                diskSize: sizes.diskSize,
                display: sizes.display?.string,
                storage: request.storage,
                noBackup: request.noBackup ?? false,
                keepBackup: request.keepBackup ?? false,
                dryRun: request.dryRun ?? false
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

    func handleStopVM(
        name: String,
        storage: String? = nil,
        force: Bool = false,
        timeout: TimeInterval = VM.defaultStopTimeout
    ) async throws -> HTTPResponse {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiVMStop)

        Logger.info(
            "Stopping VM",
            metadata: [
                "name": name, "storage": String(describing: storage), "force": "\(force)",
            ])

        do {
            Logger.info("Creating VM controller", metadata: ["name": name])
            let vmController = LumeController()

            Logger.info("Calling stopVM on controller", metadata: ["name": name])
            try await vmController.stopVM(
                name: name, storage: storage, force: force, timeout: timeout)

            Logger.info(
                "VM stopped, waiting 5 seconds for locks to clear", metadata: ["name": name])

            // Add a delay to ensure locks are fully released before returning
            for i in 1...5 {
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                Logger.info("Lock clearing delay", metadata: ["name": name, "seconds": "\(i)/5"])
            }

            // Verify the VM is really in a stopped state
            Logger.info("Verifying VM is stopped", metadata: ["name": name])
            let vm = try? vmController.get(name: name, storage: storage)
            if let vm = vm, vm.details.status == "running" {
                Logger.info(
                    "VM still reports as running despite stop operation",
                    metadata: ["name": name, "severity": "warning"])
            } else {
                Logger.info(
                    "Verification complete: VM is in stopped state", metadata: ["name": name])
            }

            Logger.info("Returning successful response", metadata: ["name": name])
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "VM stopped successfully"])
            )
        } catch {
            Logger.error(
                "Failed to stop VM",
                metadata: [
                    "name": name,
                    "error": error.localizedDescription,
                    "storage": String(describing: storage),
                ])
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleSetupVM(name: String, body: Data?) async throws -> HTTPResponse {
        Logger.info("Setting up VM", metadata: ["name": name])

        guard let body = body else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Request body is required"))
            )
        }

        do {
            let request = try JSONDecoder().decode(SetupVMRequest.self, from: body)

            // Load config from path or parse YAML directly
            let config: UnattendedConfig
            if let configPath = request.configPath {
                config = try UnattendedConfig.load(from: configPath)
            } else if let configYaml = request.configYaml {
                config = try UnattendedConfig.parse(yaml: configYaml)
            } else {
                return HTTPResponse(
                    statusCode: .badRequest,
                    headers: ["Content-Type": "application/json"],
                    body: try JSONEncoder().encode(APIError(message: "Either configPath or configYaml is required"))
                )
            }

            // Run setup in background task since it can take a long time
            Task {
                do {
                    let vmController = LumeController()
                    try await vmController.setup(
                        name: name,
                        config: config,
                        storage: request.storage,
                        vncPort: request.vncPort ?? 0,
                        noDisplay: request.noDisplay ?? true,
                        debug: request.debug ?? false,
                        debugDir: request.debugDir
                    )
                    Logger.info("Unattended setup completed", metadata: ["name": name])
                } catch {
                    Logger.error("Unattended setup failed", metadata: [
                        "name": name,
                        "error": error.localizedDescription
                    ])
                }
            }

            return HTTPResponse(
                statusCode: .accepted,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "Setup started", "name": name])
            )
        } catch {
            Logger.error("Failed to start setup", metadata: [
                "name": name,
                "error": error.localizedDescription
            ])
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleRunVM(name: String, body: Data?) async throws -> HTTPResponse {
        Logger.info("Running VM", metadata: ["name": name])

        // Log the raw body data if available
        if let body = body, let bodyString = String(data: body, encoding: .utf8) {
            Logger.info("Run VM raw request body", metadata: ["name": name, "body": bodyString])
        } else {
            Logger.info("No request body or could not decode as string", metadata: ["name": name])
        }

        do {
            Logger.info("Creating VM controller and parsing request", metadata: ["name": name])
            let request: RunVMRequest
            if let body {
                do {
                    request = try JSONDecoder().decode(RunVMRequest.self, from: body)
                } catch {
                    throw ValidationError("Invalid run request body")
                }
            } else {
                request = RunVMRequest(
                    noDisplay: nil, sharedDirectories: nil, recoveryMode: nil, storage: nil,
                    diskPath: nil, nvramPath: nil, network: nil, clipboard: nil, vnc: nil)
            }

            // Record telemetry
            TelemetryClient.shared.record(event: TelemetryEvent.apiVMRun, properties: [
                "headless": request.noDisplay ?? false
            ])

            Logger.info(
                "Parsed request",
                metadata: [
                    "name": name,
                    "noDisplay": String(describing: request.noDisplay),
                    "sharedDirectories": "\(request.sharedDirectories?.count ?? 0)",
                    "storage": String(describing: request.storage),
                ])

            Logger.info("Parsing shared directories", metadata: ["name": name])
            let dirs = try request.parse()
            Logger.info(
                "Successfully parsed shared directories",
                metadata: ["name": name, "count": "\(dirs.count)"])

            let networkMode = try request.parseNetworkMode()
            let vncPolicy = try request.validatedVNCPolicy(noDisplayDefault: false)
            let noDisplay = request.noDisplay ?? false

            // Start VM in background
            Logger.info("Starting VM in background", metadata: ["name": name])
            startVM(
                name: name,
                noDisplay: noDisplay,
                sharedDirectories: dirs,
                recoveryMode: request.recoveryMode ?? false,
                storage: request.storage,
                diskPath: request.diskPath.map { Path($0) },
                nvramPath: request.nvramPath.map { Path($0) },
                networkMode: networkMode,
                clipboard: request.clipboard ?? false,
                vncPolicy: vncPolicy
            )
            Logger.info("VM start initiated in background", metadata: ["name": name])

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
            Logger.error(
                "Failed to run VM",
                metadata: [
                    "name": name,
                    "error": error.localizedDescription,
                ])
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

        // Telemetry: the catalog id or `custom`, never the reference.
        TelemetryClient.shared.record(event: TelemetryEvent.apiPull, properties: [
            "image": TelemetryClient.imageId(
                request.image, registry: request.registry, organization: request.organization)
        ])

        let vmName = request.name ?? (request.image.split(separator: ":").first.map(String.init) ?? request.image)
        let token = await PullProgressTracker.shared.begin(name: vmName, cancellable: false)
        do {
            let vmController = LumeController()
            try await vmController.pullImage(
                image: request.image,
                name: request.name,
                registry: request.registry,
                organization: request.organization,
                storage: request.storage,
                progressHandler: { progress in
                    Task {
                        await PullProgressTracker.shared.setProgress(
                            progress, for: vmName, token: token)
                    }
                }
            )
            await PullProgressTracker.shared.finish(name: vmName, token: token, outcome: .completed)

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "Image pulled successfully",
                    "image": request.image,
                    "name": request.name ?? "default",
                ])
            )
        } catch {
            await PullProgressTracker.shared.finish(
                name: vmName, token: token, outcome: .failed(error.localizedDescription))
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

    func handlePush(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(PushRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiPush)
        let pushStartedAt = Date()

        // Trigger push asynchronously, return Accepted immediately
        Task.detached { @MainActor @Sendable in
            do {
                let vmController = LumeController()
                try await vmController.pushImage(
                    name: request.name,
                    imageName: request.imageName,
                    tags: request.tags,
                    registry: request.registry,
                    organization: request.organization,
                    storage: request.storage,
                    chunkSizeMb: request.chunkSizeMb,
                    verbose: false,  // Verbose typically handled by server logs
                    dryRun: false,  // Default API behavior is likely non-dry-run
                    reassemble: false,  // Default API behavior is likely non-reassemble
                    singleLayer: request.singleLayer
                )
                print(
                    "Background push completed successfully for image: \(request.imageName):\(request.tags.joined(separator: ","))"
                )
                TelemetryClient.shared.recordOperationCompleted(
                    operation: "push",
                    transport: .http,
                    success: true,
                    errorClass: .none,
                    elapsed: Date().timeIntervalSince(pushStartedAt)
                )
            } catch {
                print(
                    "Background push failed for image: \(request.imageName):\(request.tags.joined(separator: ",")) - Error: \(error.localizedDescription)"
                )
                TelemetryClient.shared.recordOperationCompleted(
                    operation: "push",
                    transport: .http,
                    success: false,
                    errorClass: .operationError,
                    elapsed: Date().timeIntervalSince(pushStartedAt)
                )
            }
        }

        return HTTPResponse(
            statusCode: .accepted,
            headers: ["Content-Type": "application/json"],
            body: try JSONEncoder().encode([
                "message": AnyEncodable("Push initiated in background"),
                "name": AnyEncodable(request.name),
                "imageName": AnyEncodable(request.imageName),
                "tags": AnyEncodable(request.tags),
            ])
        )
    }

    func handleGetImages(_ request: HTTPRequest) async throws -> HTTPResponse {
        // Record telemetry
        TelemetryClient.shared.record(event: TelemetryEvent.apiImages)

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

    // MARK: - Config Management Handlers

    func handleGetConfig() async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            let settings = vmController.getSettings()
            return try .json(settings)
        } catch {
            return .badRequest(message: error.localizedDescription)
        }
    }

    struct ConfigRequest: Codable {
        let homeDirectory: String?
        let cacheDirectory: String?
        let cachingEnabled: Bool?
    }

    func handleUpdateConfig(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(ConfigRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        do {
            let vmController = LumeController()

            if let homeDir = request.homeDirectory {
                try vmController.setHomeDirectory(homeDir)
            }

            if let cacheDir = request.cacheDirectory {
                try vmController.setCacheDirectory(path: cacheDir)
            }

            if let cachingEnabled = request.cachingEnabled {
                try vmController.setCachingEnabled(cachingEnabled)
            }

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "Configuration updated successfully"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleGetLocations() async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            let locations = vmController.getLocations()
            return try .json(locations)
        } catch {
            return .badRequest(message: error.localizedDescription)
        }
    }

    struct LocationRequest: Codable {
        let name: String
        let path: String
    }

    func handleAddLocation(_ body: Data?) async throws -> HTTPResponse {
        guard let body = body,
            let request = try? JSONDecoder().decode(LocationRequest.self, from: body)
        else {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: "Invalid request body"))
            )
        }

        do {
            let vmController = LumeController()
            try vmController.addLocation(name: request.name, path: request.path)

            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode([
                    "message": "Location added successfully",
                    "name": request.name,
                    "path": request.path,
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

    func handleRemoveLocation(_ name: String) async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            try vmController.removeLocation(name: name)
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "Location removed successfully"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    func handleSetDefaultLocation(_ name: String) async throws -> HTTPResponse {
        do {
            let vmController = LumeController()
            try vmController.setDefaultLocation(name: name)
            return HTTPResponse(
                statusCode: .ok,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(["message": "Default location set successfully"])
            )
        } catch {
            return HTTPResponse(
                statusCode: .badRequest,
                headers: ["Content-Type": "application/json"],
                body: try JSONEncoder().encode(APIError(message: error.localizedDescription))
            )
        }
    }

    // MARK: - Log Handlers

    func handleGetLogs(type: String?, lines: Int?) async throws -> HTTPResponse {
        do {
            let logType = type?.lowercased() ?? "all"
            let infoPath = "/tmp/lume_daemon.log"
            let errorPath = "/tmp/lume_daemon.error.log"

            let fileManager = FileManager.default
            var response: [String: String] = [:]

            // Function to read log files
            func readLogFile(path: String) -> String? {
                guard fileManager.fileExists(atPath: path) else {
                    return nil
                }

                do {
                    let content = try String(contentsOfFile: path, encoding: .utf8)

                    // If lines parameter is provided, return only the specified number of lines from the end
                    if let lineCount = lines {
                        let allLines = content.components(separatedBy: .newlines)
                        let startIndex = max(0, allLines.count - lineCount)
                        let lastLines = Array(allLines[startIndex...])
                        return lastLines.joined(separator: "\n")
                    }

                    return content
                } catch {
                    return "Error reading log file: \(error.localizedDescription)"
                }
            }

            // Get logs based on requested type
            if logType == "info" || logType == "all" {
                response["info"] = readLogFile(path: infoPath) ?? "Info log file not found"
            }

            if logType == "error" || logType == "all" {
                response["error"] = readLogFile(path: errorPath) ?? "Error log file not found"
            }

            return try .json(response)
        } catch {
            return .badRequest(message: error.localizedDescription)
        }
    }

    // MARK: - Host Status Handler

    /// Response structure for host status endpoint
    struct HostStatusResponse: Codable {
        let status: String
        let vmCount: Int
        let maxVMs: Int
        let availableSlots: Int
        let version: String

        enum CodingKeys: String, CodingKey {
            case status
            case vmCount = "vm_count"
            case maxVMs = "max_vms"
            case availableSlots = "available_slots"
            case version
        }
    }

    /// Handle GET /lume/host/status - Report host capacity and health for orchestrator
    func handleGetHostStatus() async throws -> HTTPResponse {
        do {
            let vmController = LumeController()

            // Get all VMs across all storage locations
            let vms = try vmController.list(storage: nil)

            // Count running VMs (Apple policy: max 2 VMs per host)
            let runningVMs = vms.filter { $0.status == "running" }
            let maxVMs = 2  // Apple Virtualization Framework limit

            let response = HostStatusResponse(
                status: "healthy",
                vmCount: runningVMs.count,
                maxVMs: maxVMs,
                availableSlots: max(0, maxVMs - runningVMs.count),
                version: "1.0.0"  // Could be derived from build info
            )

            return try .json(response)
        } catch {
            Logger.error("Failed to get host status", metadata: ["error": error.localizedDescription])
            return .badRequest(message: error.localizedDescription)
        }
    }

    // MARK: - Private Helper Methods

    nonisolated private func startVM(
        name: String,
        noDisplay: Bool,
        sharedDirectories: [SharedDirectory] = [],
        recoveryMode: Bool = false,
        storage: String? = nil,
        diskPath: Path? = nil,
        nvramPath: Path? = nil,
        networkMode: NetworkMode? = nil,
        clipboard: Bool = false,
        vncPolicy: VNCPolicy = .enabled
    ) {
        Logger.info(
            "Starting VM in detached task",
            metadata: [
                "name": name,
                "noDisplay": "\(noDisplay)",
                "recoveryMode": "\(recoveryMode)",
                "storage": String(describing: storage),
                "networkMode": networkMode?.description ?? "vm-config",
                "vncPolicy": vncPolicy.rawValue,
            ])

        Task.detached { @MainActor @Sendable in
            Logger.info("Background task started for VM", metadata: ["name": name])
            do {
                Logger.info("Creating VM controller in background task", metadata: ["name": name])
                let vmController = LumeController()

                Logger.info(
                    "Calling runVM on controller",
                    metadata: [
                        "name": name,
                        "noDisplay": "\(noDisplay)",
                    ])
                try await vmController.runVM(
                    name: name,
                    noDisplay: noDisplay,
                    sharedDirectories: sharedDirectories,
                    recoveryMode: recoveryMode,
                    storage: storage,
                    diskPath: diskPath,
                    nvramPath: nvramPath,
                    networkMode: networkMode,
                    clipboard: clipboard,
                    vncPolicy: vncPolicy,
                    telemetryTransport: .http
                )
                Logger.info("VM started successfully in background task", metadata: ["name": name])
            } catch {
                Logger.error(
                    "Failed to start VM in background task",
                    metadata: [
                        "name": name,
                        "error": error.localizedDescription,
                    ])
            }
        }
        Logger.info("Background task dispatched for VM", metadata: ["name": name])
    }
}
