import ai.cua.cyclops.sdk.*
import ai.cua.cyclops.sdk.schema.*
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.runBlocking

private data class Expected(
    val method: String,
    val url: String,
    val headers: List<Pair<String, String>>,
    val body: ByteArray?,
    val status: UShort,
    val response: ByteArray,
)

private val jsonHeaders = listOf(
    "accept" to "application/json",
    "content-type" to "application/json",
    "authorization" to "Bearer offline-token",
)

private fun textExpected(
    method: String,
    url: String,
    headers: List<Pair<String, String>>,
    body: ByteArray?,
    status: UShort,
    response: String,
) = Expected(method, url, headers, body, status, response.encodeToByteArray())

private fun tokenExpected() = textExpected(
    "POST",
    "https://keycloak.invalid/token",
    listOf("accept" to "application/json", "content-type" to "application/x-www-form-urlencoded"),
    "grant_type=client_credentials&client_id=client-id&client_secret=client-secret".encodeToByteArray(),
    200u,
    "{\"access_token\":\"offline-token\",\"expires_in\":3600}",
)

private fun serviceExpected(body: ByteArray?, response: ByteArray) = Expected(
    "POST",
    "https://cyclops.invalid/api/svc/default/offline-sandbox-mcp/mcp",
    listOf("authorization" to "Bearer offline-token"),
    body,
    202u,
    response,
)

private fun lifecycleQueue() = listOf(
    tokenExpected(),
    textExpected("POST", "https://cyclops.invalid/api/namespaces", jsonHeaders, "{\"name\":\"default\"}".encodeToByteArray(), 201u, "{}"),
    textExpected("POST", "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools", jsonHeaders, "{\"apiVersion\":\"cua.ai/v1\",\"kind\":\"OSGymWorkspacePool\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"replicas\":1,\"template\":{\"containerDiskImage\":\"registry.example/desktop:offline\"},\"services\":[{\"name\":\"mcp\",\"targetPort\":8080}]},\"status\":null}".encodeToByteArray(), 201u, "{\"apiVersion\":\"cua.ai/v1\",\"kind\":\"OSGymWorkspacePool\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"replicas\":1,\"template\":{\"containerDiskImage\":\"registry.example/desktop:offline\"},\"services\":[{\"name\":\"mcp\",\"targetPort\":8080}]},\"status\":null}"),
    textExpected("POST", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims", jsonHeaders, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxClaim\",\"metadata\":{\"namespace\":\"default\",\"name\":\"claim-1\",\"labels\":null},\"spec\":{\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":null}".encodeToByteArray(), 201u, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxClaim\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":{\"phase\":\"Bound\",\"sandbox\":{\"name\":\"offline-sandbox\"}}}"),
    textExpected("GET", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", jsonHeaders, null, 200u, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxClaim\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":{\"phase\":\"Bound\",\"sandbox\":{\"name\":\"offline-sandbox\"}}}"),
    textExpected("GET", "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default", jsonHeaders, null, 200u, "{\"apiVersion\":\"cua.ai/v1\",\"kind\":\"OSGymWorkspacePool\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"replicas\":1,\"template\":{\"containerDiskImage\":\"registry.example/desktop:offline\"},\"services\":[{\"name\":\"mcp\",\"targetPort\":8080}]},\"status\":null}"),
    serviceExpected("{\"offline\":true}".encodeToByteArray(), "offline service accepted".encodeToByteArray()),
    textExpected("DELETE", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", jsonHeaders, null, 204u, ""),
    textExpected("DELETE", "https://cyclops.invalid/api/k8s/apis/cua.ai/v1/namespaces/default/osgymworkspacepools/default", jsonHeaders, null, 204u, ""),
    textExpected("DELETE", "https://cyclops.invalid/api/namespaces/default", jsonHeaders, null, 204u, ""),
)

private class ScriptedHttpClient(expected: List<Expected>) : HttpClient {
    private val lock = Any()
    private val expected = ArrayDeque(expected)

    override suspend fun execute(request: HttpRequest): HttpResponse = synchronized(lock) {
        val item = expected.removeFirstOrNull() ?: error("unexpected request")
        check(request.method == item.method && request.url == item.url)
        check(request.headers.map { it.name to it.value } == item.headers)
        check((request.body == null && item.body == null) || (request.body != null && item.body != null && request.body.contentEquals(item.body)))
        HttpResponse(item.status, emptyList(), item.response)
    }

    fun assertExhausted() = synchronized(lock) { check(expected.isEmpty()) }
}

private class FailingHttpClient : HttpClient {
    override suspend fun execute(request: HttpRequest): HttpResponse {
        throw HttpException.Transport(reason = "scripted callback failure")
    }
}

private fun configuration() = CyclopsConfiguration(
    "https://cyclops.invalid",
    "https://keycloak.invalid/token",
    CyclopsCredentials("client-id", "client-secret"),
    1uL,
    1u,
    1uL,
    2u,
)

private fun serviceRequest(body: ByteArray?) = HttpRequest("POST", "https://ignored.invalid/mcp", emptyList(), body)
private val sandbox = Sandbox("default", "default", "offline-sandbox", listOf("mcp"))

fun main() = runBlocking {
    val vmTemplate = VmTemplate("registry.example/desktop:offline", null, null, null, null, null, null, null, null, null, null, null, null, null)
    val spec = PoolSpec(1u, PoolTemplate(null, null, null, null, null, vmTemplate.containerDiskImage, null, null, null, null, null, null), null, listOf(SandboxService("mcp", 8080u, null)))
    val transport = ScriptedHttpClient(lifecycleQueue())
    val client = CyclopsClient.connect(configuration(), transport)
    val pool = client.createPool(CreatePoolRequest("default", spec))
    val claim = client.createClaim(CreateClaimRequest(pool, ClaimSpec(SandboxTemplateRef(pool.metadata.name), null, null, null)))
    val createdSandbox = client.waitClaim(claim)
    val service = client.serviceRequest(createdSandbox, "mcp", "/mcp", serviceRequest("{\"offline\":true}".encodeToByteArray()))
    client.deleteClaim(claim)
    client.deletePool(pool)
    check(pool.metadata.name == "default" && claim.metadata.name == "default" && createdSandbox.name == "offline-sandbox")
    check(service.status == 202.toUShort())
    transport.assertExhausted()

    val failingClient = CyclopsClient.connect(configuration(), FailingHttpClient())
    try {
        failingClient.serviceRequest(sandbox, "mcp", "/mcp", serviceRequest(null))
        error("expected generated SdkException.Transport")
    } catch (error: SdkException.Transport) {
        check(error.reason == "scripted callback failure")
    }

    val bodyTransport = ScriptedHttpClient(listOf(
        tokenExpected(),
        serviceExpected(null, byteArrayOf()),
        serviceExpected(ByteArray(0), byteArrayOf(0, 0x7f, 0xff.toByte())),
    ))
    val bodyClient = CyclopsClient.connect(configuration(), bodyTransport)
    check(bodyClient.serviceRequest(sandbox, "mcp", "/mcp", serviceRequest(null)).body.isEmpty())
    check(bodyClient.serviceRequest(sandbox, "mcp", "/mcp", serviceRequest(ByteArray(0))).body.contentEquals(byteArrayOf(0, 0x7f, 0xff.toByte())))
    bodyTransport.assertExhausted()

    val concurrentTransport = ScriptedHttpClient(listOf(
        tokenExpected(),
        serviceExpected(null, "warm".encodeToByteArray()),
        serviceExpected("parallel".encodeToByteArray(), "first".encodeToByteArray()),
        serviceExpected("parallel".encodeToByteArray(), "second".encodeToByteArray()),
    ))
    val concurrentClient = CyclopsClient.connect(configuration(), concurrentTransport)
    concurrentClient.serviceRequest(sandbox, "mcp", "/mcp", serviceRequest(null))
    val responses = coroutineScope {
        listOf(
            async { concurrentClient.serviceRequest(sandbox, "mcp", "/mcp", serviceRequest("parallel".encodeToByteArray())) },
            async { concurrentClient.serviceRequest(sandbox, "mcp", "/mcp", serviceRequest("parallel".encodeToByteArray())) },
        ).awaitAll()
    }
    check(responses.map { it.body.decodeToString() }.toSet() == setOf("first", "second"))
    concurrentTransport.assertExhausted()
    println("pool=${pool.metadata.name} claim=${claim.metadata.name} sandbox=${createdSandbox.name} service_status=${service.status}")
}
