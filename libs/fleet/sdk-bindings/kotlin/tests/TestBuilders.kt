import ai.cua.cyclops.sdk.CreatePoolRequestBuilder
import ai.cua.cyclops.sdk.CreateTemplateRequest
import ai.cua.cyclops.sdk.CreateTemplateRequestBuilder
import ai.cua.cyclops.sdk.SdkBuildException
import ai.cua.cyclops.sdk.schema.OsGymSandboxTemplateSpec
import ai.cua.cyclops.sdk.schema.OsGymSandboxTemplateSpecBuilder
import ai.cua.cyclops.sdk.schema.SandboxService
import ai.cua.cyclops.sdk.schema.SandboxServiceBuilder
import ai.cua.cyclops.sdk.schema.SandboxTemplateRef
import ai.cua.cyclops.sdk.schema.SchemaBuildException
import ai.cua.cyclops.sdk.schema.VmTemplate
import ai.cua.cyclops.sdk.schema.VmTemplateBuilder

fun main() {
    val service: SandboxService = SandboxServiceBuilder()
        .name("mcp")
        .targetPort(3000u)
        .build()
    val vm: VmTemplate = VmTemplateBuilder()
        .containerDiskImage("registry.example/vm:latest")
        .imagePullSecret("registry-secret")
        .cpuCores(4u)
        .memory("8Gi")
        .services(listOf(service))
        .build()
    val templateSpec: OsGymSandboxTemplateSpec = OsGymSandboxTemplateSpecBuilder()
        .vmTemplate(vm)
        .build()
    val request: CreateTemplateRequest = CreateTemplateRequestBuilder()
        .namespace("default")
        .name("desktop")
        .spec(templateSpec)
        .build()

    check(service.protocol == null)
    check(vm.command == null)
    check(vm.imagePullSecret == "registry-secret")
    check(vm.cpuCores == 4u)
    check(vm.memory == "8Gi")
    check(request.spec.vmTemplate.containerDiskImage == "registry.example/vm:latest")

    val schemaError = runCatching { VmTemplateBuilder().build() }.exceptionOrNull()
    check(schemaError is SchemaBuildException.MissingRequiredField)
    check(schemaError.recordType == "VmTemplate")
    check(schemaError.field == "container_disk_image")

    val sdkError = runCatching { CreatePoolRequestBuilder().build() }.exceptionOrNull()
    check(sdkError is SdkBuildException.MissingRequiredField)
    check(sdkError.recordType == "CreatePoolRequest")
    check(sdkError.field == "namespace")

    val legacy = SandboxTemplateRef(name = "legacy")
    check(legacy::class == SandboxTemplateRef::class)
    check(legacy.name == "legacy")
}
