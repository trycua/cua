package ai.cua.android.demo

import ai.cua.driver.sdk.DisplayFrame
import kotlinx.coroutines.suspendCancellableCoroutine
import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.util.Base64
import kotlin.concurrent.thread
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

internal class LocalAgentModel(private val config: AgentConfig) : AgentModel {
    override suspend fun decide(requestId: String, frame: DisplayFrame, currentPackage: String,
        history: List<AgentAction>): AgentAction = suspendCancellableCoroutine { continuation ->
        val connection = URL(config.endpoint).openConnection() as HttpURLConnection
        continuation.invokeOnCancellation { connection.disconnect() }
        thread(name = "cua-model-request", isDaemon = true) {
            try {
                if (!continuation.isActive) return@thread
                val body = JSONObject().put("request_id", requestId).put("task", config.task)
                    .put("allowed_apps", JSONArray(config.allowedApps)).put("current_package", currentPackage)
                    .put("width", frame.width).put("height", frame.height)
                    .put("image_base64", Base64.getEncoder().encodeToString(frame.png))
                    .put("history", JSONArray(history.map { JSONObject().put("action", it.json()).put("reason", it.reason) }))
                    .toString().toByteArray(Charsets.UTF_8)
                connection.apply {
                    requestMethod = "POST"; doOutput = true; instanceFollowRedirects = false
                    connectTimeout = 10_000; readTimeout = 120_000
                    setRequestProperty("Authorization", "Bearer ${config.token}")
                    setRequestProperty("Content-Type", "application/json")
                    setFixedLengthStreamingMode(body.size)
                }
                connection.outputStream.use { it.write(body) }
                check(connection.responseCode == 200) { "Model relay failed" }
                val bytes = connection.inputStream.use { it.readNBytes(65537) }
                require(bytes.size <= 65536) { "Model response too large" }
                val action = AgentAction.parse(bytes.toString(Charsets.UTF_8), requestId, config.allowedApps, frame.width, frame.height)
                continuation.resume(action)
            } catch (error: Exception) {
                if (continuation.isActive) continuation.resumeWithException(error)
            } finally { connection.disconnect() }
        }
    }
}
