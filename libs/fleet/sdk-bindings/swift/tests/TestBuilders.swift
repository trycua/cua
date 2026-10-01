import Foundation

@main struct TestBuilders {
    static func main() throws {
        let service: SandboxService = try SandboxServiceBuilder()
            .name(value: "mcp")
            .targetPort(value: 3000)
            .build()
        let vm: VmTemplate = try VmTemplateBuilder()
            .containerDiskImage(value: "registry.example/vm:latest")
            .imagePullSecret(value: "registry-secret")
            .cpuCores(value: 4)
            .memory(value: "8Gi")
            .services(value: [service])
            .build()
        let template: OsGymSandboxTemplateSpec = try OsGymSandboxTemplateSpecBuilder()
            .vmTemplate(value: vm)
            .build()
        let request: CreateTemplateRequest = try CreateTemplateRequestBuilder()
            .namespace(value: "default")
            .name(value: "desktop")
            .spec(value: template)
            .build()

        precondition(service.protocol == nil)
        precondition(vm.command == nil)
        precondition(request.spec.vmTemplate.containerDiskImage == "registry.example/vm:latest")

        do {
            _ = try VmTemplateBuilder().build()
            fatalError("missing required field did not fail")
        } catch SchemaBuildError.MissingRequiredField(let recordType, let field) {
            precondition(recordType == "VmTemplate")
            precondition(field == "container_disk_image")
        }

        let legacy = SandboxTemplateRef(name: "legacy")
        precondition(legacy.name == "legacy")

        let configuration = try CyclopsTokenProviderConfigurationBuilder()
            .baseUrl(value: "https://api.example.test")
            .poolPollIntervalMs(value: 5000)
            .poolPollLimit(value: 120)
            .claimPollIntervalMs(value: 5000)
            .claimPollLimit(value: 120)
            .build()
        let userKey = try CreateUserApiKeyRequestBuilder()
            .name(value: "automation")
            .scope(value: [])
            .build()
        let autoscaling = try WarmPoolAutoscalingBuilder()
            .minPoolSize(value: 1)
            .initialPoolSize(value: 2)
            .maxPoolSize(value: 5)
            .build()

        precondition(configuration.poolPollIntervalMs == 5000)
        precondition(userKey.scope.isEmpty)
        precondition(autoscaling.maxPoolSize == 5)
        do {
            _ = try TemplateBuilder().build()
            fatalError("missing Template field did not fail")
        } catch SdkBuildError.MissingRequiredField(_, _) {
        }
        do {
            _ = try CreateClaimRequestBuilder().build()
            fatalError("missing claim pool did not fail")
        } catch SdkBuildError.MissingRequiredField(_, _) {
        }
    }
}
