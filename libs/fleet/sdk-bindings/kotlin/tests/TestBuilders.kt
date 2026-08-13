import ai.cua.cyclops.sdk.CreateClaimRequestBuilder
import ai.cua.cyclops.sdk.CreatePoolRequestBuilder
import ai.cua.cyclops.sdk.CreateTemplateRequest
import ai.cua.cyclops.sdk.CreateTemplateRequestBuilder
import ai.cua.cyclops.sdk.CreateUserApiKeyRequestBuilder
import ai.cua.cyclops.sdk.CyclopsTokenProviderConfigurationBuilder
import ai.cua.cyclops.sdk.SdkBuildException
import ai.cua.cyclops.sdk.TemplateBuilder
import ai.cua.cyclops.sdk.schema.OsGymSandboxTemplateSpec
import ai.cua.cyclops.sdk.schema.OsGymSandboxTemplateSpecBuilder
import ai.cua.cyclops.sdk.schema.SandboxService
import ai.cua.cyclops.sdk.schema.SandboxServiceBuilder
import ai.cua.cyclops.sdk.schema.SandboxTemplateRef
import ai.cua.cyclops.sdk.schema.SchemaBuildException
import ai.cua.cyclops.sdk.schema.VmTemplate
import ai.cua.cyclops.sdk.schema.VmTemplateBuilder
import ai.cua.cyclops.sdk.schema.WarmPoolAutoscalingBuilder

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

    val configuration = CyclopsTokenProviderConfigurationBuilder()
        .baseUrl("https://api.example.test")
        .poolPollIntervalMs(5000uL)
        .poolPollLimit(120u)
        .claimPollIntervalMs(5000uL)
        .claimPollLimit(120u)
        .build()
    val userKey = CreateUserApiKeyRequestBuilder()
        .name("automation")
        .scope(emptyList())
        .build()
    val autoscaling = WarmPoolAutoscalingBuilder()
        .minPoolSize(1u)
        .initialPoolSize(2u)
        .maxPoolSize(5u)
        .build()

    check(configuration.poolPollIntervalMs == 5000uL)
    check(userKey.scope.isEmpty())
    check(autoscaling.maxPoolSize == 5u)
    check(runCatching { TemplateBuilder().build() }.exceptionOrNull() is SdkBuildException.MissingRequiredField)
    check(runCatching { CreateClaimRequestBuilder().build() }.exceptionOrNull() is SdkBuildException.MissingRequiredField)
}
