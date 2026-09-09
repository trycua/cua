package ai.cua.android.demo

import ai.cua.driver.sdk.DisplayFrame
import org.json.JSONArray
import org.json.JSONObject
import java.net.URI

internal val AGENT_APPS = listOf("com.darkempire78.opencalculator", "org.tasks")

internal data class AgentConfig(val endpoint: String, val token: String, val task: String, val allowedApps: List<String>) {
    companion object {
        fun parse(text: String): AgentConfig {
            val json = JSONObject(text)
            json.exactKeys(setOf("endpoint", "token", "task"), setOf("allowed_apps"))
            val endpoint = json.strictString("endpoint")
            val uri = URI(endpoint)
            require(uri.scheme == "http" && uri.host == "127.0.0.1" && uri.port == 8788 &&
                uri.path == "/decide" && uri.query == null && uri.fragment == null && uri.userInfo == null) { "Expected local model relay" }
            val token = json.strictString("token")
            require(token.length in 1..4096 && token.none { it.isWhitespace() || it.isISOControl() })
            val task = json.strictString("task")
            require(task.length <= 8192)
            val apps = if (json.has("allowed_apps")) (json.get("allowed_apps") as JSONArray).let { a ->
                (0 until a.length()).map { a.get(it) as String }
            } else AGENT_APPS
            require(apps.isNotEmpty() && apps.distinct() == apps && apps.all { it in AGENT_APPS })
            return AgentConfig(endpoint, token, task, apps)
        }
    }
}

internal data class AgentAction(val type: String, val reason: String, val packageName: String? = null,
    val x: Int = 0, val y: Int = 0, val toX: Int = 0, val toY: Int = 0, val durationMs: Int = 300) {
    fun json(): JSONObject = JSONObject().put("type", type).put("reason", reason).apply {
        when (type) {
            "launch" -> put("package", packageName)
            "tap" -> { put("x", x); put("y", y) }
            "swipe" -> { put("from_x", x); put("from_y", y); put("to_x", toX); put("to_y", toY); put("duration_ms", durationMs) }
        }
    }
    companion object {
        fun parse(text: String, requestId: String, apps: List<String>, width: Int, height: Int): AgentAction {
            val response = JSONObject(text)
            response.exactKeys(setOf("request_id", "action"))
            require(response.strictString("request_id") == requestId) { "Model response correlation mismatch" }
            val a = response.get("action") as JSONObject
            val type = a.strictString("type")
            a.exactKeys(setOf("type", "reason") + when (type) {
                "launch" -> setOf("package")
                "tap" -> setOf("x", "y")
                "swipe" -> setOf("from_x", "from_y", "to_x", "to_y", "duration_ms")
                "done", "blocked" -> emptySet()
                else -> error("Unsupported model action")
            })
            val reason = a.strictString("reason").also { require(it.length <= 2000) }
            fun coordinate(key: String, bound: Int) = a.strictInt(key).also { require(it in 0 until bound) }
            return when (type) {
                "launch" -> AgentAction(type, reason, a.strictString("package").also { require(it in apps) })
                "tap" -> AgentAction(type, reason, x = coordinate("x", width), y = coordinate("y", height))
                "swipe" -> AgentAction(type, reason, x = coordinate("from_x", width), y = coordinate("from_y", height),
                    toX = coordinate("to_x", width), toY = coordinate("to_y", height),
                    durationMs = a.strictInt("duration_ms").also { require(it in 1..1000) })
                "done", "blocked" -> AgentAction(type, reason)
                else -> error("Unsupported model action")
            }
        }
    }
}

internal fun sameAgentFrame(before: DisplayFrame, after: DisplayFrame, beforeGeneration: String,
    afterGeneration: String, pixels: (DisplayFrame) -> IntArray): Boolean {
    if (beforeGeneration != afterGeneration || before.targetId != after.targetId || before.displayId != after.displayId ||
        before.width != after.width || before.height != after.height || before.rotation != after.rotation ||
        before.width <= 0 || before.height <= 0 || after.snapshotId == null || after.frameAgeMs !in 0..5000) return false
    val expectedPixels = before.width.toLong() * before.height
    val original = pixels(before)
    val fresh = pixels(after)
    return original.size.toLong() == expectedPixels && fresh.size.toLong() == expectedPixels && original.contentEquals(fresh)
}

private fun JSONObject.exactKeys(required: Set<String>, optional: Set<String> = emptySet()) {
    val actual = keys().asSequence().toSet()
    require(actual.containsAll(required) && actual.all { it in required || it in optional }) { "Unexpected JSON fields" }
}

private fun JSONObject.strictString(key: String) = (get(key) as String).also { require(it.isNotBlank()) }
private fun JSONObject.strictInt(key: String): Int {
    val value = get(key)
    require(value is Int || value is Long)
    return (value as Number).toLong().also { require(it in Int.MIN_VALUE..Int.MAX_VALUE) }.toInt()
}
