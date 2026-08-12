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
    }
}
