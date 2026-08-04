import ai.cua.cyclops.sdk.*
import ai.cua.cyclops.sdk.schema.*
import kotlinx.coroutines.runBlocking
import java.util.Collections

private data class Expected(val method: String, val url: String, val headers: List<Pair<String, String>>, val body: ByteArray?, val status: UShort, val response: String)
private class ScriptedHttpClient : HttpClient {
    private val lock = Any()
    private val expected = ArrayDeque(listOf(
        Expected("POST", "https://keycloak.invalid/token", listOf("accept" to "application/json", "content-type" to "application/x-www-form-urlencoded", "authorization" to "Basic Y2xpZW50LWlkOmNsaWVudC1zZWNyZXQ="), "grant_type=client_credentials".encodeToByteArray(), 200u, "{\"access_token\":\"offline-token\",\"expires_in\":3600}"),
        Expected("POST", "https://cyclops.invalid/api/namespaces", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), "{\"name\":\"default\"}".encodeToByteArray(), 201u, "{}"),
        Expected("POST", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxwarmpools", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxWarmPool\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"replicas\":1,\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":null}".encodeToByteArray(), 201u, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxWarmPool\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"replicas\":1,\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":null}"),
        Expected("POST", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxClaim\",\"metadata\":{\"namespace\":\"default\",\"name\":\"claim-1\",\"labels\":null},\"spec\":{\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":null}".encodeToByteArray(), 201u, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxClaim\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":{\"phase\":\"Bound\",\"sandbox\":{\"name\":\"offline-sandbox\"}}}"),
        Expected("GET", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), null, 200u, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxClaim\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"sandboxTemplateRef\":{\"name\":\"default\"}},\"status\":{\"phase\":\"Bound\",\"sandbox\":{\"name\":\"offline-sandbox\"}}}"),
        Expected("GET", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxtemplates/default", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), null, 200u, "{\"apiVersion\":\"osgym.cua.ai/v1alpha1\",\"kind\":\"OSGymSandboxTemplate\",\"metadata\":{\"namespace\":\"default\",\"name\":\"default\",\"labels\":null},\"spec\":{\"vmTemplate\":{\"containerDiskImage\":\"registry.example/desktop:offline\",\"services\":[{\"name\":\"mcp\",\"targetPort\":8080}]}}}"),
        Expected("POST", "https://cyclops.invalid/api/svc/default/offline-sandbox-mcp/mcp", listOf("authorization" to "Bearer offline-token"), "{\"offline\":true}".encodeToByteArray(), 202u, "offline service accepted"),
        Expected("DELETE", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxclaims/default", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), null, 204u, ""),
        Expected("DELETE", "https://cyclops.invalid/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/default/osgymsandboxwarmpools/default", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), null, 204u, ""),
        Expected("DELETE", "https://cyclops.invalid/api/namespaces/default", listOf("accept" to "application/json", "content-type" to "application/json", "authorization" to "Bearer offline-token"), null, 204u, "")
    ))
    override suspend fun execute(request: HttpRequest): HttpResponse = synchronized(lock) {
        val item = expected.removeFirst()
        check(request.method == item.method && request.url == item.url)
        check(request.headers.map { it.name to it.value } == item.headers)
        check((request.body == null && item.body == null) || (request.body != null && item.body != null && request.body.contentEquals(item.body)))
        HttpResponse(item.status, emptyList(), item.response.encodeToByteArray())
    }
    fun assertExhausted() = check(expected.isEmpty())
}

fun main() = runBlocking {
    val spec = OsGymSandboxWarmPoolSpec(1u, SandboxTemplateRef("default"), null)
    val transport = ScriptedHttpClient()
    val credentials = CyclopsCredentials("client-id", "client-secret")
    val client = CyclopsClient.connect(CyclopsConfiguration("https://cyclops.invalid", "https://keycloak.invalid/token", credentials, 1uL, 1u, 1uL, 2u), transport)
    val pool = client.createPool(CreatePoolRequest("default", spec))
    val claim = client.createClaim(CreateClaimRequest(pool, ClaimSpec(SandboxTemplateRef(pool.metadata.name), null, null, null)))
    val sandbox = client.waitClaim(claim)
    val service = client.serviceRequest(sandbox, "mcp", "/mcp", HttpRequest("POST", "https://ignored.invalid/mcp", emptyList(), "{\"offline\":true}".encodeToByteArray()))
    client.deleteClaim(claim)
    client.deletePool(pool)
    check(pool.metadata.name == "default" && claim.metadata.name == "default" && sandbox.name == "offline-sandbox")
    check(service.status == 202.toUShort())
    transport.assertExhausted()
    println("pool=${pool.metadata.name} claim=${claim.metadata.name} sandbox=${sandbox.name} service_status=${service.status}")
}
