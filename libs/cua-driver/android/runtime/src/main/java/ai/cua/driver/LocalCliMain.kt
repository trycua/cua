package ai.cua.driver

import ai.cua.driver.sdk.DriverClient
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject
import java.nio.file.Files
import java.nio.file.Paths
import java.nio.file.StandardOpenOption
import java.util.UUID
import kotlin.system.exitProcess

/**
 * Phone-local shell CLI, launched by app_process with the runtime APK on its classpath.
 * The runtime currently authorizes Android shell UID 2000; Termux UID authorization
 * is not enabled, so running this launcher directly as a Termux app is unsupported.
 */
object LocalCliMain {
    private data class Invocation(val request: JSONObject, val timeoutMs: Int, val image: String?)

    @JvmStatic fun main(args: Array<String>) {
        val id = UUID.randomUUID().toString()
        val invocation = try {
            parse(args, id)
        } catch (error: IllegalArgumentException) {
            finish(failure(id, "refused", 2, error.message ?: "Invalid command"))
        }
        val response = try {
            val result = DriverClient.exchange(invocation.request, invocation.timeoutMs)
            validate(result, id)
            if (result.getString("status") == "ok" && invocation.image != null) {
                saveImage(result, invocation.image)
            }
            result
        } catch (error: Exception) {
            failure(id, "uncertain", 4,
                "Local request completion is uncertain: ${error.message ?: error.javaClass.simpleName}; do not blindly retry input")
        }
        finish(response)
    }

    private fun finish(response: JSONObject): Nothing {
        println(response)
        exitProcess(response.getInt("exit_code"))
    }

    private fun failure(id: String, status: String, code: Int, message: String) = JSONObject()
        .put("contract_version", DriverClient.VERSION).put("request_id", id)
        .put("status", status).put("exit_code", code).put("data", JSONObject())
        .put("error", JSONObject().put("message", message))

    private fun parse(args: Array<String>, id: String): Invocation {
        val flags = linkedMapOf<String, MutableList<String>>()
        val words = mutableListOf<String>()
        var index = 0
        while (index < args.size) {
            val arg = args[index]
            if (arg.startsWith('-')) {
                val key = arg.substringBefore('=')
                val inline = if ('=' in arg) arg.substringAfter('=') else null
                require(key != "--device" && key != "--connection") { "$key is unsupported; use --local" }
                val value = if (key == "--local" || key == "--json") {
                    require(inline == null) { "$key does not take a value" }
                    "true"
                } else if (inline != null) {
                    inline
                } else {
                    index++
                    require(index < args.size && !args[index].startsWith("--")) { "missing value for $key" }
                    args[index]
                }
                require(value.isNotEmpty()) { "empty value for $key" }
                require(key == "--allow-app" || key !in flags) { "duplicate $key" }
                flags.getOrPut(key) { mutableListOf() }.add(value)
            } else {
                words.add(arg)
            }
            index++
        }
        fun take(key: String): String? = flags.remove(key)?.first()
        require(take("--local") != null) { "an explicit --local is required" }
        take("--json")
        val session = take("--session")
        val timeout = (take("--timeout-ms") ?: "10000").toIntOrNull()
        require(timeout != null && timeout in 1..10000) { "--timeout-ms must be between 1 and 10000" }
        val operation = when {
            words == listOf("doctor") -> "doctor"
            words == listOf("capabilities") -> "capabilities"
            words.size == 2 && words[0] == "session" && words[1] in listOf("create", "inspect", "renew", "stop") -> "session.${words[1]}"
            words == listOf("app", "launch") -> "app.launch"
            words.size == 3 && words.take(2) == listOf("app", "launch") -> {
                require("--package" !in flags) { "package specified twice" }
                flags["--package"] = mutableListOf(words[2])
                "app.launch"
            }
            words == listOf("snapshot") -> "snapshot"
            words == listOf("tap") -> "tap"
            words == listOf("gesture", "swipe") -> "gesture.swipe"
            else -> throw IllegalArgumentException("unsupported Android command")
        }
        require(operation in listOf("doctor", "capabilities", "session.create") || session != null) { "--session ID is required" }
        require(operation != "session.create" || session == null) { "session create cannot reuse --session" }
        val params = JSONObject()
        var image: String? = null
        fun required(key: String): String = take(key) ?: throw IllegalArgumentException("missing $key")
        when (operation) {
            "session.create" -> {
                take("--size")?.let { size ->
                    val parts = size.split('x')
                    require(parts.size == 2) { "--size must be WIDTHxHEIGHT" }
                    params.put("width", number(parts[0], "width", true))
                    params.put("height", number(parts[1], "height", true))
                }
                take("--density")?.let { params.put("density", number(it, "density", true)) }
                take("--label")?.let { params.put("label", it) }
                val apps = flags.remove("--allow-app")
                require(!apps.isNullOrEmpty()) { "session create requires --allow-app PACKAGE" }
                params.put("allowed_apps", JSONArray(apps))
            }
            "app.launch" -> params.put("package", required("--package"))
            "snapshot" -> {
                params.put("target_id", required("--target"))
                image = take("--image")
            }
            "tap", "gesture.swipe" -> {
                params.put("snapshot_id", required("--snapshot"))
                val coordinates = if (operation == "tap") listOf("x", "y")
                    else listOf("from_x", "from_y", "to_x", "to_y")
                for (field in coordinates) {
                    val flag = "--${field.replace('_', '-')}"
                    params.put(field, number(required(flag), flag, false))
                }
                if (operation == "gesture.swipe") {
                    params.put("duration_ms", number(required("--duration-ms"), "duration_ms", true))
                }
            }
        }
        require(flags.isEmpty()) { "unsupported flags for $operation: ${flags.keys.sorted().joinToString(", ")}" }
        val request = JSONObject().put("contract_version", DriverClient.VERSION)
            .put("request_id", id).put("operation", operation).put("params", params)
        if (session != null) request.put("session_id", session)
        return Invocation(request, timeout, image)
    }

    private fun number(value: String, name: String, positive: Boolean): Long {
        val parsed = value.toLongOrNull()
        require(!value.startsWith('-') && parsed != null && parsed in (if (positive) 1L else 0L)..4294967295L) { "invalid $name" }
        return parsed
    }

    private fun validate(response: JSONObject, id: String) {
        check(response.getString("contract_version") == DriverClient.VERSION &&
            response.getString("request_id") == id && response.optJSONObject("data") != null) {
            "Runtime response contract, request ID, or data mismatch"
        }
        val status = response.getString("status")
        val code = response.get("exit_code")
        check(code is Number && code.toDouble() == code.toInt().toDouble() && when (status) {
            "ok" -> code.toInt() == 0
            "refused" -> code.toInt() in 2..3
            "uncertain" -> code.toInt() == 4
            "error" -> code.toInt() == 5
            else -> false
        }) { "Invalid runtime response status or exit code" }
    }

    private fun saveImage(response: JSONObject, path: String) {
        val data = response.getJSONObject("data")
        val bytes = Base64.decode(data.getString("image_base64"), Base64.NO_WRAP)
        val signature = byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)
        check(bytes.size >= signature.size && bytes.take(signature.size).toByteArray().contentEquals(signature)) {
            "Snapshot image is not PNG"
        }
        Files.newOutputStream(Paths.get(path), StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE).use { it.write(bytes) }
        data.remove("image_base64")
        data.put("image_path", path)
    }
}
