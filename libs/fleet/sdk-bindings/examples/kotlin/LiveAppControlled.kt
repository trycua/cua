import ai.cua.cyclops.sdk.*
import ai.cua.cyclops.sdk.schema.*
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import java.net.URI
import java.net.http.HttpClient as JdkHttpClient
import java.net.http.HttpRequest as JdkHttpRequest
import java.net.http.HttpResponse as JdkHttpResponse
import java.time.Duration

private fun required(name: String) = System.getenv(name)?.takeIf { it.isNotBlank() }
    ?: error("$name is required")

private class JavaHttpClient : HttpClient {
    private val client = JdkHttpClient.newBuilder().connectTimeout(Duration.ofSeconds(30)).build()

    override suspend fun execute(request: HttpRequest): HttpResponse {
        val builder = JdkHttpRequest.newBuilder(URI.create(request.url)).timeout(Duration.ofSeconds(60))
        request.headers.forEach { builder.header(it.name, it.value) }
        val publisher = request.body?.let(JdkHttpRequest.BodyPublishers::ofByteArray)
            ?: JdkHttpRequest.BodyPublishers.noBody()
        builder.method(request.method, publisher)
        val response = client.send(builder.build(), JdkHttpResponse.BodyHandlers.ofByteArray())
        return HttpResponse(
            response.statusCode().toUShort(),
            response.headers().map().flatMap { (name, values) -> values.map { HttpHeader(name, it) } },
            response.body(),
        )
    }
}

private fun poolSpec() = PoolSpec(
    1u,
    PoolTemplate(
        null, null, null, null, null,
        required("CUA_IMAGE"),
        required("CUA_IMAGE_PULL_SECRET"),
        4u,
        "4Gi",
        null, null, null,
    ),
    null,
    listOf(SandboxService("mcp", 3000u, null)),
)

private suspend fun initializeMcp(client: CyclopsClient, sandbox: Sandbox): UShort {
    val body = """{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cyclops-uniffi-kotlin","version":"0.1.0"}}}""".encodeToByteArray()
    val deadline = System.nanoTime() + Duration.ofMinutes(5).toNanos()
    while (true) {
        val response = client.serviceRequest(
            sandbox,
            "mcp",
            "/mcp",
            HttpRequest(
                "POST",
                "https://ignored.invalid/mcp",
                listOf(
                    HttpHeader("accept", "application/json, text/event-stream"),
                    HttpHeader("content-type", "application/json"),
                ),
                body,
            ),
        )
        if (response.status.toInt() in 200..299) return response.status
        if (response.status.toInt() in setOf(502, 503, 504) && System.nanoTime() < deadline) {
            delay(5_000)
            continue
        }
        error("MCP initialize failed with HTTP ${response.status}: ${response.body.decodeToString()}")
    }
}

fun main() = runBlocking {
    val namespace = required("CYCLOPS_NAMESPACE")
    val client = CyclopsClient.connect(
        CyclopsConfiguration(
            required("CUA_BASE_URL"),
            required("CUA_TOKEN_URL"),
            CyclopsCredentials(required("CUA_CLIENT_ID"), required("CUA_CLIENT_SECRET")),
            5_000uL, 120u, 5_000uL, 120u,
        ),
        JavaHttpClient(),
    )
    var pool: Pool? = null
    var claim: Claim? = null
    try {
        pool = client.createPool(CreatePoolRequest(namespace, poolSpec()))
        claim = client.createClaim(CreateClaimRequest(pool, null))
        val sandbox = client.waitClaim(claim)
        val status = initializeMcp(client, sandbox)
        println("namespace=$namespace pool=${pool.metadata.name} claim=${claim.metadata.name} sandbox=${sandbox.name} mcp_status=$status")
    } finally {
        claim?.let { client.deleteClaim(it) }
        pool?.let { client.deletePool(it) }
        client.close()
    }
}
