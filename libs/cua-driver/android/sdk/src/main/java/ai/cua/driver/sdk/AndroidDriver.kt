package ai.cua.driver.sdk

import android.content.Context
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.util.Base64
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

data class DriverResult<T>(val requestId: String, val runtimeGeneration: String, val data: T,
    val action: ActionEvidence? = null)
data class SessionOptions(val allowedApps: List<String>, val width: Int = 1080,
    val height: Int = 1920, val density: Int = 320, val label: String? = null)
data class SessionInfo(val sessionId: String, val displayId: Int, val width: Int, val height: Int,
    val leaseRemainingMs: Long, val targetId: String?)
data class AppTarget(val targetId: String, val taskId: Int, val displayId: Int, val packageName: String)
data class DisplayFrame(val snapshotId: String?, val targetId: String, val displayId: Int,
    val width: Int, val height: Int, val rotation: Int, val frameAgeMs: Long, val png: ByteArray)
data class StopInfo(val state: String, val cleanup: String)
data class Capability(val capability: String, val support: String, val authorization: String,
    val readiness: String, val conditions: List<String>?, val qualificationStatus: String?, val reason: String?)
data class DoctorInfo(val backend: String, val runtimeUid: Int, val runtimePid: Int,
    val androidRelease: String, val apiLevel: Int, val runtimeGeneration: String,
    val dedupRetainedMutations: Int, val maxResponseBytes: Int, val capabilities: List<Capability>)
data class NativeEvidence(val kind: String, val detail: String)
/** Transport acceptance does not establish delivery or an application effect. */
data class ActionEvidence(val actionId: String, val requestedDelivery: String, val actualDelivery: String,
    val transport: String, val effect: String, val evidence: List<NativeEvidence>)

class DriverRefusedException(val requestId: String, val runtimeGeneration: String,
    val exitCode: Int, val reason: String) : Exception("Request $requestId refused: $reason")
class DriverUncertainException(val requestId: String, val runtimeGeneration: String?,
    val reason: String, cause: Throwable? = null) :
    Exception("Request $requestId completion is uncertain ($reason); do not blindly retry", cause)
class DriverRuntimeException(val requestId: String, val runtimeGeneration: String,
    val reason: String) : Exception("Request $requestId failed before admission: $reason")
class DriverRequestCancelledException(val requestId: String, val mayHaveBeenDispatched: Boolean,
    cause: CancellationException) : CancellationException(
        "Request $requestId cancelled; mayHaveBeenDispatched=$mayHaveBeenDispatched. Cancellation does not retract input."
    ) { init { initCause(cause) } }

internal fun interface DriverTransport {
    fun call(operation: String, sessionId: String?, params: JSONObject, requestId: String): JSONObject
}

/** Coroutine facade over the experimental runtime. Requests are never automatically retried. */
class AndroidDriver internal constructor(private val transport: DriverTransport) {
    constructor(context: Context) : this(DriverClient(context.applicationContext).let { client ->
        DriverTransport { operation, sessionId, params, requestId ->
            client.call(operation, sessionId, params, requestId)
        }
    })

    suspend fun doctor(requestId: String = newId()): DriverResult<DoctorInfo> =
        request("doctor", requestId = requestId) { d ->
            DoctorInfo(d.string("backend").also { require(it == "android") }, d.int("runtime_uid"),
                d.int("runtime_pid"), d.string("android_release"), d.int("api_level"),
                d.string("runtime_generation"), d.int("dedup_retained_mutations"),
                d.int("max_response_bytes"), parseCapabilities(d))
        }.also { requireCorrelated(it.requestId, it.runtimeGeneration == it.data.runtimeGeneration) }

    suspend fun capabilities(requestId: String = newId()): DriverResult<List<Capability>> =
        request("capabilities", requestId = requestId, parse = ::parseCapabilities)

    suspend fun createSession(options: SessionOptions, requestId: String = newId()): DriverResult<SessionInfo> {
        require(options.width in 320..1920 && options.height in 320..2400 && options.density in 120..640)
        require(options.allowedApps.size in 1..8)
        options.allowedApps.forEach(::validatePackage)
        options.label?.let { require(it.codePointCount(0, it.length) <= 128) }
        val params = JSONObject().put("allowed_apps", JSONArray(options.allowedApps))
            .put("width", options.width).put("height", options.height).put("density", options.density)
        options.label?.let { params.put("label", it) }
        return request("session.create", params = params, requestId = requestId, parse = ::parseSession)
    }

    suspend fun inspectSession(sessionId: String, requestId: String = newId()): DriverResult<SessionInfo> =
        sessionRequest("session.inspect", sessionId, requestId)
    suspend fun renewSession(sessionId: String, requestId: String = newId()): DriverResult<SessionInfo> =
        sessionRequest("session.renew", sessionId, requestId)
    private suspend fun sessionRequest(operation: String, sessionId: String, requestId: String) =
        request(operation, sessionId, requestId = requestId, parse = ::parseSession).also {
            requireCorrelated(requestId, it.data.sessionId == sessionId)
        }

    suspend fun stopSession(sessionId: String, requestId: String = newId()): DriverResult<StopInfo> =
        request("session.stop", sessionId, requestId = requestId) { d ->
            StopInfo(d.string("state").also { require(it == "stopped") },
                d.string("cleanup").also { require(it == "released") })
        }

    suspend fun launchApp(sessionId: String, packageName: String, requestId: String = newId()): DriverResult<AppTarget> {
        validatePackage(packageName)
        return request("app.launch", sessionId, JSONObject().put("package", packageName), requestId) { d ->
            AppTarget(d.string("target_id"), d.int("task_id"), d.int("display_id"),
                d.string("package").also { require(it == packageName) })
        }
    }

    suspend fun snapshot(sessionId: String, targetId: String, requestId: String = newId()): DriverResult<DisplayFrame> =
        frame("snapshot", sessionId, targetId, requestId)
    suspend fun preview(sessionId: String, targetId: String, requestId: String = newId()): DriverResult<DisplayFrame> =
        frame("preview", sessionId, targetId, requestId)
    private suspend fun frame(operation: String, sessionId: String, targetId: String, requestId: String): DriverResult<DisplayFrame> {
        validateId(targetId)
        return request(operation, sessionId, JSONObject().put("target_id", targetId), requestId) { d ->
            val snapshotId = d.nullableString("snapshot_id")
            require((operation == "snapshot") == (snapshotId != null))
            require(d.string("target_id") == targetId)
            val png = Base64.getDecoder().decode(d.string("image_base64"))
            require(png.size >= 8 && png.copyOfRange(0, 8).contentEquals(PNG_SIGNATURE))
            val age = d.signedLong("frame_age_ms")
            if (operation == "snapshot") require(age in 0..5000)
            DisplayFrame(snapshotId, targetId, d.int("display_id"), d.int("width", 1),
                d.int("height", 1), d.int("rotation").also { require(it <= 3) },
                age, png)
        }
    }

    suspend fun tap(sessionId: String, snapshotId: String, x: Int, y: Int,
        requestId: String = newId()): DriverResult<Unit> {
        validateId(snapshotId); validatePoint(x, y)
        return request("tap", sessionId, JSONObject().put("snapshot_id", snapshotId).put("x", x).put("y", y), requestId) { Unit }
    }

    suspend fun swipe(sessionId: String, snapshotId: String, fromX: Int, fromY: Int, toX: Int, toY: Int,
        durationMs: Int = 300, requestId: String = newId()): DriverResult<Unit> {
        validateId(snapshotId); validatePoint(fromX, fromY); validatePoint(toX, toY)
        require(durationMs in 1..1000)
        return request("gesture.swipe", sessionId, JSONObject().put("snapshot_id", snapshotId)
            .put("from_x", fromX).put("from_y", fromY).put("to_x", toX).put("to_y", toY)
            .put("duration_ms", durationMs), requestId) { Unit }
    }

    private suspend fun <T> request(operation: String, sessionId: String? = null,
        params: JSONObject = JSONObject(), requestId: String, parse: (JSONObject) -> T): DriverResult<T> {
        validateId(requestId); sessionId?.let(::validateId)
        val dispatched = AtomicBoolean(false)
        try {
            return withContext(Dispatchers.IO) {
                ensureActive()
                dispatched.set(true)
                val response = transport.call(operation, sessionId, params, requestId)
                decode(response, operation, requestId, parse)
            }
        } catch (error: CancellationException) {
            throw DriverRequestCancelledException(requestId, dispatched.get(), error)
        } catch (error: DriverRefusedException) { throw error
        } catch (error: DriverRuntimeException) { throw error
        } catch (error: DriverUncertainException) { throw error
        } catch (error: Exception) {
            throw DriverUncertainException(requestId, null, "transport_or_invalid_response", error)
        }
    }

    private fun <T> decode(r: JSONObject, operation: String, requestId: String,
        parse: (JSONObject) -> T): DriverResult<T> {
        require(r.string("contract_version") == DriverClient.VERSION)
        require(r.string("request_id") == requestId)
        val generation = r.string("runtime_generation")
        val status = r.string("status")
        val code = r.int("exit_code")
        val data = r.obj("data")
        when (status) {
            "refused" -> {
                require(code == 2 || code == 3)
                throw DriverRefusedException(requestId, generation, code, r.obj("error").string("reason"))
            }
            "uncertain" -> {
                require(code == 4)
                throw DriverUncertainException(requestId, generation, r.obj("error").string("reason"))
            }
            "error" -> {
                require(code == 5)
                throw DriverRuntimeException(requestId, generation, r.obj("error").string("reason"))
            }
            "ok" -> require(code == 0 && !r.has("error"))
            else -> error("Unknown response status")
        }
        val needsAction = operation in setOf("app.launch", "tap", "gesture.swipe")
        require(r.has("action") == needsAction)
        if (operation == "tap" || operation == "gesture.swipe") require(data.length() == 0)
        val action = if (needsAction) parseAction(r.obj("action")) else null
        return DriverResult(requestId, generation, parse(data), action)
    }

    private companion object {
        val PNG_SIGNATURE = byteArrayOf(-119, 80, 78, 71, 13, 10, 26, 10)
        fun newId() = UUID.randomUUID().toString()
        fun validateId(value: String) { require(value.codePointCount(0, value.length) in 1..128) }
        fun validatePackage(value: String) { require(value.matches(Regex("[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+"))) }
        // The runtime additionally checks coordinates against the actual session geometry and snapshot.
        fun validatePoint(x: Int, y: Int) { require(x in 0 until 1920 && y in 0 until 2400) }
        fun requireCorrelated(requestId: String, valid: Boolean) {
            if (!valid) throw DriverUncertainException(requestId, null, "response_correlation_mismatch")
        }
        fun parseSession(d: JSONObject): SessionInfo {
            require(d.string("state") == "active" && d.string("lease_owner") == "caller")
            val id = d.string("session_id")
            require(d.string("display_generation") == id)
            return SessionInfo(id, d.int("display_id"), d.int("width", 320).also { require(it <= 1920) },
                d.int("height", 320).also { require(it <= 2400) }, d.long("lease_remaining_ms"), d.nullableString("target_id"))
        }
        fun parseCapabilities(d: JSONObject): List<Capability> = d.array("capabilities").objects().map { c ->
            Capability(c.string("capability"), c.string("support"), c.string("authorization"), c.string("readiness"),
                if (c.has("conditions")) c.array("conditions").let { a -> (0 until a.length()).map { a.get(it) as String } } else null,
                if (c.has("qualification")) c.obj("qualification").string("status") else null,
                if (c.has("reason")) c.string("reason") else null)
        }
        fun parseAction(a: JSONObject) = ActionEvidence(a.string("action_id"), a.string("requested_delivery"),
            a.string("actual_delivery"), a.string("transport"), a.string("effect"),
            a.array("evidence").objects().map { NativeEvidence(it.string("kind"), it.string("detail")) })
        fun JSONObject.string(key: String): String = (get(key) as String).also { require(it.isNotEmpty()) }
        fun JSONObject.nullableString(key: String): String? = if (get(key) === JSONObject.NULL) null else string(key)
        fun JSONObject.obj(key: String) = get(key) as JSONObject
        fun JSONObject.array(key: String) = get(key) as JSONArray
        fun JSONObject.signedLong(key: String): Long {
            val value = get(key)
            require(value is Int || value is Long)
            return (value as Number).toLong()
        }
        fun JSONObject.long(key: String): Long = signedLong(key).also { require(it >= 0) }
        fun JSONObject.int(key: String, minimum: Int = 0): Int = long(key).also { require(it in minimum.toLong()..Int.MAX_VALUE) }.toInt()
        fun JSONArray.objects(): List<JSONObject> = (0 until length()).map { get(it) as JSONObject }
    }
}
