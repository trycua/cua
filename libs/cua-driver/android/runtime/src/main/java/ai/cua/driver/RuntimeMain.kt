package ai.cua.driver

import ai.cua.driver.sdk.DriverClient
import android.app.ActivityManager
import android.app.ActivityOptions
import android.content.Context
import android.content.AttributionSource
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.hardware.input.InputManager
import android.media.ImageReader
import android.net.LocalServerSocket
import android.net.LocalSocket
import android.os.Handler
import android.os.Binder
import android.os.IBinder
import android.os.Bundle
import android.os.Parcel
import android.os.ParcelFileDescriptor
import android.os.Parcelable
import android.os.Looper
import android.os.Process
import android.os.SystemClock
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import android.system.ErrnoException
import android.util.Base64
import android.view.InputDevice
import android.view.InputEvent
import android.view.MotionEvent
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.Semaphore
import kotlin.concurrent.thread

/** Explicit shell-authorized development runtime. It is not an Android app service. */
object RuntimeMain {
    @JvmStatic fun main(args: Array<String>) {
        check(Process.myUid() == 2000) { "Launch through authorized adb shell" }
        check(android.os.Build.VERSION.SDK_INT >= 37) { "This experiment requires Android 17/API 37" }
        Looper.prepareMainLooper()
        val activityThread = Class.forName("android.app.ActivityThread")
        val instance = activityThread.getMethod("systemMain").invoke(null)
        val system = activityThread.getMethod("getSystemContext").invoke(instance) as Context
        val context = system.createPackageContext("com.android.shell", 0)
        val runtime = Runtime(context)
        val server = LocalServerSocket(DriverClient.SOCKET)
        val slots = Semaphore(4)
        val binder = object : Binder() {
            override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
                if (code == INTERFACE_TRANSACTION) { reply?.writeString("ai.cua.driver.v0"); return true }
                if (code != FIRST_CALL_TRANSACTION || reply == null) return super.onTransact(code, data, reply, flags)
                data.enforceInterface("ai.cua.driver.v0")
                val uid = Binder.getCallingUid()
                val payload = data.readString() ?: throw IllegalArgumentException("Missing request")
                require(payload.toByteArray(Charsets.UTF_8).size <= 65536)
                check(slots.tryAcquire()) { "Runtime busy" }
                val pipe = ParcelFileDescriptor.createPipe()
                thread(name = "cua-binder-request") {
                    try {
                        pipe[1].use {
                            val bytes = (runtime.request(uid, payload).toString() + "\n").toByteArray(Charsets.UTF_8)
                            writePipe(it, bytes)
                        }
                    } catch (_: Exception) {
                        System.err.println("Binder response delivery failed; retained mutation outcome may be uncertain to caller")
                    } finally { slots.release() }
                }
                reply.writeNoException()
                reply.writeTypedObject(pipe[0], Parcelable.PARCELABLE_WRITE_RETURN_VALUE)
                pipe[0].close()
                return true
            }
        }
        registerBinder(binder)
        thread(name = "cua-accept") {
            while (true) {
                val socket = server.accept()
                if (!slots.tryAcquire()) { socket.close(); continue }
                thread(name = "cua-request") {
                    try { runtime.serve(socket) } finally { socket.close(); slots.release() }
                }
            }
        }
        Executors.newSingleThreadScheduledExecutor().scheduleWithFixedDelay(
            { runtime.expire() }, 1, 1, java.util.concurrent.TimeUnit.SECONDS)
        System.err.println("Cua Android runtime ready; contract=${DriverClient.VERSION}")
        Looper.loop()
    }

    private val registrationToken = Binder()
    private fun writePipe(pipe: ParcelFileDescriptor, bytes: ByteArray) {
        val fd = pipe.fileDescriptor
        Os.fcntlInt(fd, OsConstants.F_SETFL, Os.fcntlInt(fd, OsConstants.F_GETFL, 0) or OsConstants.O_NONBLOCK)
        val poll = StructPollfd().apply { this.fd = fd; events = OsConstants.POLLOUT.toShort() }
        val deadline = SystemClock.elapsedRealtime() + 10000
        var offset = 0
        while (offset < bytes.size) {
            val remaining = deadline - SystemClock.elapsedRealtime()
            check(remaining > 0) { "Response deadline expired" }
            try {
                poll.revents = 0
                check(Os.poll(arrayOf(poll), remaining.toInt()) > 0 &&
                    poll.revents.toInt() and (OsConstants.POLLERR or OsConstants.POLLNVAL or OsConstants.POLLHUP) == 0)
                offset += Os.write(fd, bytes, offset, minOf(8192, bytes.size - offset))
            } catch (error: ErrnoException) {
                if (error.errno != OsConstants.EINTR && error.errno != OsConstants.EAGAIN) throw error
            }
        }
    }

    private fun registerBinder(binder: IBinder) {
        val manager = Class.forName("android.app.ActivityManager").getMethod("getService").invoke(null)
        val api = Class.forName("android.app.IActivityManager")
        val authority = "ai.cua.driver.runtime.bridge"
        val holder = api.getMethod("getContentProviderExternal", String::class.java, Int::class.javaPrimitiveType,
            IBinder::class.java, String::class.java).invoke(manager, authority, 0, registrationToken, "cua-driver")
        check(holder != null) { "Runtime APK provider must be installed first" }
        val provider = holder.javaClass.getField("provider").get(holder)
        val source = AttributionSource.Builder(2000).setPackageName("com.android.shell").build()
        Class.forName("android.content.IContentProvider").getMethod("call", AttributionSource::class.java,
            String::class.java, String::class.java, String::class.java, Bundle::class.java)
            .invoke(provider, source, authority, "register", null, Bundle().apply { putBinder("binder", binder) })
    }
}

private data class Session(
    val id: String, val owner: Int, val display: VirtualDisplay, var reader: ImageReader,
    val width: Int, val height: Int, val allowed: Set<String>, var expires: Long,
    var target: String? = null, var packageName: String? = null, var taskId: Int? = null,
    var snapshot: String? = null, var observed: Long = 0, var snapshotFrameTime: Long = 0,
    var bitmap: Bitmap? = null, var frameTime: Long = 0, var launchUncertain: Boolean = false,
    var targetGeneration: Long = 0, var captureAfter: Long = 0, var captureMethod: String = "image_reader",
    val label: String? = null,
) {
    val tasks = SessionTasks(allowed, display.display.displayId)
}

private class Runtime(private val context: Context) {
    private val generation = UUID.randomUUID().toString()
    private val displays = context.getSystemService(DisplayManager::class.java)
    private val activity = context.getSystemService(ActivityManager::class.java)
    private val input = context.getSystemService(InputManager::class.java)
    private var session: Session? = null
    private val stopped = LinkedHashMap<String, Int>()
    private val pendingCleanup = mutableSetOf<Int>()
    private val pendingTaskCleanup = mutableSetOf<Int>()
    private val dedup = LinkedHashMap<String, Pair<String, JSONObject>>()

    fun serve(socket: LocalSocket) {
        socket.soTimeout = 10000
        val response = request(socket.peerCredentials.uid, DriverClient.readLineBounded(socket.inputStream, 65536))
        socket.outputStream.write((response.toString() + "\n").toByteArray(Charsets.UTF_8))
        socket.outputStream.flush()
    }

    fun request(uid: Int, payload: String): JSONObject {
        var id = "invalid"
        var admitted = false
        val response = try {
            val request = try { JSONObject(payload) }
                catch (_: org.json.JSONException) { throw Refusal("invalid_json", 2) }
            id = request.optString("request_id", "invalid")
            if (!authorized(uid)) throw Refusal("caller_not_authorized")
            validate(request, setOf("contract_version", "request_id", "operation", "session_id", "params"))
            validateRequest(request)
            if (request.getString("contract_version") != DriverClient.VERSION) throw Refusal("incompatible_contract", 2)
            if (id.codePointCount(0, id.length) !in 1..128) throw Refusal("invalid_request_id", 2)
            val body = request.toString()
            synchronized(this) {
                expire()
                val key = "$uid:$id"
                val previous = dedup[key]
                if (previous != null) {
                    if (previous.first != body) throw Refusal("request_id_conflict", 2)
                    previous.second
                } else {
                    admitted = true
                    val result = try {
                        val data = dispatch(request, uid)
                        envelope(id, "ok", 0).put("data", data).also {
                            if (data.has("action")) it.put("action", data.remove("action"))
                        }
                    } catch (error: Refusal) {
                        envelope(id, "refused", error.code).put("error", JSONObject().put("reason", error.reason))
                    } catch (error: Exception) {
                        System.err.println("Runtime operation failed: ${error.javaClass.simpleName}")
                        envelope(id, "uncertain", 4).put("error", JSONObject().put("reason", "runtime_failure")
                            .put("exception", error.javaClass.simpleName))
                    }
                    if (request.getString("operation") in setOf("session.create", "session.renew", "session.stop", "app.launch", "tap", "gesture.swipe")) {
                        dedup[key] = body to result
                        while (dedup.size > 128) dedup.remove(dedup.keys.first())
                    }
                    result
                }
            }
        } catch (error: Refusal) {
            envelope(id, "refused", error.code).put("error", JSONObject().put("reason", error.reason))
        } catch (error: Exception) {
            System.err.println("Runtime operation failed: ${error.javaClass.simpleName}")
            envelope(id, if (admitted) "uncertain" else "error", if (admitted) 4 else 5)
                .put("error", JSONObject().put("reason", "runtime_failure").put("exception", error.javaClass.simpleName))
        }
        return response
    }

    private fun authorized(uid: Int): Boolean {
        if (uid == 2000) return true
        val pm = context.packageManager
        return try {
            val demo = pm.getApplicationInfo("ai.cua.android.demo", 0)
            uid == demo.uid && pm.checkSignatures("ai.cua.android.demo", "ai.cua.driver.runtime") == PackageManager.SIGNATURE_MATCH
        } catch (_: PackageManager.NameNotFoundException) { false }
    }

    private fun envelope(id: String, status: String, exit: Int) = JSONObject()
        .put("contract_version", DriverClient.VERSION).put("request_id", id)
        .put("runtime_generation", generation).put("status", status).put("exit_code", exit).put("data", JSONObject())

    private fun validate(obj: JSONObject, keys: Set<String>) {
        if (obj.keys().asSequence().any { it !in keys }) throw Refusal("unknown_field", 2)
    }

    private fun validateRequest(r: JSONObject) {
        for (key in listOf("contract_version", "request_id", "operation")) {
            if (r.opt(key) !is String) throw Refusal("invalid_$key", 2)
        }
        if (r.has("session_id")) {
            val sid = r.opt("session_id") as? String ?: throw Refusal("invalid_session_id", 2)
            if (sid.codePointCount(0, sid.length) !in 1..128) throw Refusal("invalid_session_id", 2)
        }
        val p = r.opt("params") as? JSONObject ?: throw Refusal("invalid_params", 2)
        val operation = r.getString("operation")
        val keys = when (operation) {
            "doctor", "capabilities", "session.inspect", "session.renew", "session.stop" -> emptySet()
            "session.create" -> setOf("width", "height", "density", "allowed_apps", "label")
            "app.launch" -> setOf("package")
            "snapshot", "preview" -> setOf("target_id")
            "tap" -> setOf("snapshot_id", "x", "y")
            "gesture.swipe" -> setOf("snapshot_id", "from_x", "from_y", "to_x", "to_y", "duration_ms")
            else -> throw Refusal("operation_unsupported", 2)
        }
        validate(p, keys)
        if (operation == "session.create" && r.has("session_id")) throw Refusal("unexpected_session_id", 2)
        if (operation !in setOf("doctor", "capabilities", "session.create") && !r.has("session_id")) throw Refusal("missing_session_id", 2)
        val strings = when (operation) {
            "app.launch" -> listOf("package")
            "snapshot", "preview" -> listOf("target_id")
            "tap", "gesture.swipe" -> listOf("snapshot_id")
            else -> emptyList()
        }
        for (key in strings) {
            val value = p.opt(key) as? String ?: throw Refusal("invalid_$key", 2)
            if (key != "package" && value.codePointCount(0, value.length) !in 1..128) throw Refusal("invalid_$key", 2)
        }
        if (p.has("label")) {
            val label = p.opt("label") as? String ?: throw Refusal("invalid_label", 2)
            if (label.codePointCount(0, label.length) > 128) throw Refusal("invalid_label", 2)
        }
        val numbers = when (operation) {
            "tap" -> listOf("x", "y")
            "gesture.swipe" -> listOf("from_x", "from_y", "to_x", "to_y", "duration_ms")
            else -> emptyList()
        }
        for (key in numbers) {
            val number = p.opt(key) as? Number ?: throw Refusal("invalid_$key", 2)
            if (!number.toDouble().isFinite() || number.toDouble() < 0) throw Refusal("invalid_$key", 2)
        }
        for (key in listOf("width", "height", "density", "duration_ms")) {
            if (p.has(key) && p.opt(key) !is Int) throw Refusal("invalid_$key", 2)
        }
        if (operation == "session.create") {
            val apps = p.opt("allowed_apps") as? JSONArray ?: throw Refusal("invalid_allowlist", 2)
            if (apps.length() !in 1..8) throw Refusal("invalid_allowlist", 2)
            for (i in 0 until apps.length()) {
                val pkg = apps.opt(i) as? String ?: throw Refusal("invalid_package", 2)
                if (!pkg.matches(Regex("[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+"))) throw Refusal("invalid_package", 2)
            }
            if (p.optInt("width", 1080) !in 320..1920 || p.optInt("height", 1920) !in 320..2400 ||
                p.optInt("density", 320) !in 120..640) throw Refusal("invalid_geometry", 2)
        }
        if (operation == "app.launch" && !p.getString("package").matches(Regex("[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+"))) throw Refusal("invalid_package", 2)
        if (operation == "gesture.swipe" && p.getInt("duration_ms") !in 1..1000) throw Refusal("invalid_duration", 2)
    }

    private fun active(request: JSONObject, uid: Int): Session {
        val s = session ?: throw Refusal("session_not_active")
        if (request.optString("session_id") != s.id) throw Refusal("stale_session")
        if (s.owner != uid) throw Refusal("session_owner_mismatch")
        return s
    }

    private fun sessionInfo(s: Session) = JSONObject().put("session_id", s.id)
        .put("state", "active").put("display_id", s.display.display.displayId)
        .put("display_generation", s.id).put("width", s.width).put("height", s.height)
        .put("lease_remaining_ms", (s.expires - SystemClock.elapsedRealtime()).coerceAtLeast(0))
        .put("lease_owner", "caller").put("target_id", s.target ?: JSONObject.NULL)
        .put("target_generation", s.targetGeneration).put("package", s.packageName ?: JSONObject.NULL)
        .put("task_id", s.taskId ?: JSONObject.NULL).put("owned_task_count", s.tasks.size)
        .put("label", s.label ?: JSONObject.NULL)

    private fun action(transport: String, effect: String = "unverifiable", detail: String) = JSONObject()
        .put("action_id", UUID.randomUUID().toString())
        .put("requested_delivery", "background").put("actual_delivery", "unknown")
        .put("transport", transport).put("effect", effect)
        .put("evidence", JSONArray().put(JSONObject().put("kind", "native_api_result").put("detail", detail)))

    private fun capabilities(): JSONObject {
        val entries = JSONArray()
        for (name in listOf("display.virtual", "app.launch.virtual", "capture.display", "touch.background", "gesture.swipe", "gesture.cancel", "preview.read_only")) {
            entries.put(JSONObject().put("capability", name).put("support", "conditional")
                .put("authorization", "granted").put("readiness", "ready")
                .put("conditions", JSONArray(listOf("authorized_shell_helper", "validated_session_target")))
                .put("qualification", JSONObject().put("status", "unverified")))
        }
        for (name in listOf("accessibility.display", "text.semantic", "text.raw_keys", "key.background", "human.concurrent_typing")) {
            entries.put(JSONObject().put("capability", name).put("support", "unsupported")
                .put("authorization", "unknown").put("readiness", "blocked").put("reason", "not_qualified_in_initial_slice"))
        }
        return JSONObject().put("capabilities", entries)
    }

    private fun dispatch(request: JSONObject, uid: Int): JSONObject {
        val op = request.getString("operation")
        val p = request.getJSONObject("params")
        return when (op) {
            "doctor" -> {
                validate(p, emptySet())
                JSONObject().put("backend", "android").put("runtime_uid", Process.myUid())
                    .put("runtime_pid", Process.myPid())
                    .put("android_release", android.os.Build.VERSION.RELEASE)
                    .put("api_level", android.os.Build.VERSION.SDK_INT).put("runtime_generation", generation)
                    .put("dedup_retained_mutations", 128).put("max_response_bytes", DriverClient.MAX_BYTES)
                    .put("capabilities", capabilities().getJSONArray("capabilities"))
            }
            "capabilities" -> { validate(p, emptySet()); capabilities() }
            "session.create" -> create(p, uid)
            "session.inspect" -> { validate(p, emptySet()); sessionInfo(active(request, uid)) }
            "session.renew" -> {
                validate(p, emptySet()); val s = active(request, uid)
                s.expires = SystemClock.elapsedRealtime() + 60000; sessionInfo(s)
            }
            "session.stop" -> {
                validate(p, emptySet())
                val sid = request.optString("session_id")
                if (stopped[sid] == uid) JSONObject().put("state", "stopped").put("cleanup", "released")
                else { val s = active(request, uid); stop(s); JSONObject().put("state", "stopped").put("cleanup", "released") }
            }
            "app.launch" -> launch(active(request, uid), p)
            "snapshot", "preview" -> snapshot(active(request, uid), p, op == "snapshot")
            "tap", "gesture.swipe" -> gesture(active(request, uid), p, op == "gesture.swipe")
            else -> throw Refusal("operation_unsupported")
        }
    }

    private fun create(p: JSONObject, uid: Int): JSONObject {
        validate(p, setOf("width", "height", "density", "allowed_apps", "label"))
        if (session != null) throw Refusal("device_session_busy")
        pendingCleanup.removeAll { displays.getDisplay(it) == null }
        if (pendingCleanup.isNotEmpty()) throw Refusal("previous_display_cleanup_pending")
        if (pendingTaskCleanup.isNotEmpty()) {
            val live = inventory().map { it.id }.toSet()
            pendingTaskCleanup.retainAll(live)
            if (pendingTaskCleanup.isNotEmpty()) throw Refusal("previous_task_cleanup_pending")
        }
        val width = p.optInt("width", 1080); val height = p.optInt("height", 1920)
        val density = p.optInt("density", 320)
        if (width !in 320..1920 || height !in 320..2400 || density !in 120..640) throw Refusal("invalid_geometry", 2)
        val allowed = p.getJSONArray("allowed_apps")
        if (allowed.length() !in 1..8) throw Refusal("invalid_allowlist", 2)
        val packages = (0 until allowed.length()).map { allowed.getString(it) }.toSet()
        if (packages.any { !it.matches(Regex("[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+")) }) throw Refusal("invalid_package", 2)
        val reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 3)
        try {
            // AOSP DisplayManager flags are hidden SDK constants; resolve by name and fail closed.
            fun flag(name: String) = DisplayManager::class.java.getField("VIRTUAL_DISPLAY_FLAG_$name").getInt(null)
            val flags = DisplayManager.VIRTUAL_DISPLAY_FLAG_PUBLIC or DisplayManager.VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY or
                flag("DESTROY_CONTENT_ON_REMOVAL") or flag("TRUSTED") or flag("OWN_FOCUS") or flag("STEAL_TOP_FOCUS_DISABLED")
            val display = displays.createVirtualDisplay("Cua agent", width, height, density, reader.surface, flags,
                null, Handler(Looper.getMainLooper())) ?: throw Refusal("display_creation_failed")
            val s = Session(UUID.randomUUID().toString(), uid, display, reader, width, height, packages,
                SystemClock.elapsedRealtime() + 60000, label = p.optString("label").takeIf { it.isNotEmpty() })
            session = s
            return sessionInfo(s).put("flags", flags)
        } catch (error: Exception) { reader.close(); throw error }
    }

    private fun inventory(): List<TaskPlacement> {
        val tasks = activity.getRunningTasks(100)
        if (tasks.size >= 100) throw Refusal("task_inventory_incomplete")
        return tasks.map { TaskPlacement(it.taskId, it.javaClass.getField("displayId").getInt(it),
            it.baseActivity?.packageName, it.topActivity?.packageName) }
    }

    private fun task(s: Session): TaskPlacement {
        val displayId = s.display.display.displayId
        val tasks = inventory()
        val task = tasks.firstOrNull {
            it.display == displayId
        } ?: throw Refusal("target_not_observable")
        if (task.topPackage != s.packageName || task.basePackage != s.packageName ||
            task.id != s.taskId || task.id !in s.tasks.ids ||
            tasks.any { it.display == displayId && it.id !in s.tasks.ids }) {
            throw Refusal("target_placement_changed")
        }
        return task
    }

    private fun launch(s: Session, p: JSONObject): JSONObject {
        validate(p, setOf("package"))
        val pkg = p.getString("package")
        if (pkg !in s.allowed) throw Refusal("app_not_allowed")
        if (s.launchUncertain) throw Refusal("previous_launch_uncertain_stop_required")
        val intent = context.packageManager.getLaunchIntentForPackage(pkg) ?: throw Refusal("app_not_launchable")
        val component = intent.component ?: throw Refusal("app_component_unavailable")
        val before = inventory()
        val existing = s.tasks.prepare(pkg, before)
        s.snapshot = null; s.target = null; s.packageName = pkg; s.taskId = null
        s.bitmap?.recycle(); s.bitmap = null; s.frameTime = 0
        s.targetGeneration++
        s.launchUncertain = true
        if (existing != null) {
            // The system Context's ActivityManager supplies package=android, which is not
            // owned by shell UID 2000. Attribute this Binder call to the actual caller.
            // No display ID or task-reparenting option: only raise the verified owned task.
            val manager = Class.forName("android.app.ActivityTaskManager").getMethod("getService").invoke(null)
            Class.forName("android.app.IActivityTaskManager").getMethod("moveTaskToFront",
                Class.forName("android.app.IApplicationThread"), String::class.java,
                Int::class.javaPrimitiveType, Int::class.javaPrimitiveType, Bundle::class.java)
                .invoke(manager, null, "com.android.shell", existing, 0, null)
        } else {
            // NEW_TASK | MULTIPLE_TASK requests a new task, never reuse of a main-display task.
            val launch = ProcessBuilder("/system/bin/am", "start", "--display", s.display.display.displayId.toString(),
                "-n", component.flattenToString(), "-f", "0x18000000").redirectErrorStream(true).start()
            if (!launch.waitFor(3, java.util.concurrent.TimeUnit.SECONDS)) {
                launch.destroyForcibly(); throw IllegalStateException("Activity launch timed out")
            }
            val launchOutput = launch.inputStream.bufferedReader().readText()
            check(launch.exitValue() == 0 && !launchOutput.contains("Error:")) { "Activity launch failed" }
        }
        var actual: TaskPlacement? = null
        for (attempt in 0..19) {
            val top = inventory().firstOrNull { it.display == s.display.display.displayId }
            if (top != null && top.basePackage == pkg && top.topPackage == pkg &&
                (existing == null || top.id == existing)) { actual = top; break }
            Thread.sleep(100)
        }
        val observed = actual ?: throw IllegalStateException("Launch placement unverified after dispatch")
        if (existing == null) {
            try { s.tasks.admit(pkg, before.map { it.id }.toSet(), observed) }
            catch (error: Refusal) { throw IllegalStateException("Launch task ownership unverified after dispatch", error) }
        }
        s.taskId = observed.id
        try { task(s) }
        catch (error: Refusal) { throw IllegalStateException("Launch placement changed after dispatch", error) }
        reconnectCapture(s)
        s.target = UUID.randomUUID().toString(); s.launchUncertain = false
        return JSONObject().put("target_id", s.target).put("task_id", s.taskId)
            .put("target_generation", s.targetGeneration).put("owned_task_count", s.tasks.size)
            .put("display_id", s.display.display.displayId).put("package", pkg)
            .put("action", action(if (existing == null) "android_activity_manager_shell" else "android_activity_manager",
                detail = "Owned task placement read back from task service"))
    }

    private fun reconnectCapture(s: Session) {
        s.snapshot = null
        val replacement = ImageReader.newInstance(s.width, s.height, PixelFormat.RGBA_8888, 3)
        val old = s.reader
        try {
            // A new output queue requests real composition even when the app's buffers are static.
            // Reusing the same surface could coalesce into a no-op during display traversal.
            s.captureAfter = System.nanoTime()
            s.display.surface = replacement.surface
            s.reader = replacement
            s.bitmap?.recycle(); s.bitmap = null; s.frameTime = 0
            s.captureMethod = "surface_replacement"
        } catch (error: Exception) { replacement.close(); throw error }
        old.close()
    }

    private fun frame(s: Session): Bitmap {
        var image = s.reader.acquireLatestImage()
        if (image != null && image.timestamp < s.captureAfter) { image.close(); image = null }
        if (image == null && s.bitmap == null) {
            for (attempt in 0..19) {
                Thread.sleep(50); image = s.reader.acquireLatestImage()
                if (image != null && image.timestamp < s.captureAfter) { image.close(); image = null }
                if (image != null) break
            }
        }
        image?.use {
            val plane = it.planes[0]
            val padded = Bitmap.createBitmap(plane.rowStride / plane.pixelStride, s.height, Bitmap.Config.ARGB_8888)
            padded.copyPixelsFromBuffer(plane.buffer)
            val cropped = Bitmap.createBitmap(padded, 0, 0, s.width, s.height)
            s.bitmap?.recycle(); s.bitmap = cropped
            if (cropped !== padded) padded.recycle()
            s.frameTime = it.timestamp
        }
        return s.bitmap ?: throw Refusal("capture_not_ready")
    }

    private fun snapshot(s: Session, p: JSONObject, actionable: Boolean): JSONObject {
        validate(p, setOf("target_id"))
        if (p.getString("target_id") != s.target) throw Refusal("stale_target")
        task(s)
        var image = frame(s)
        if (actionable && (s.frameTime <= 0 || (System.nanoTime() - s.frameTime) / 1_000_000 !in 0..5000)) {
            reconnectCapture(s)
            image = frame(s)
        }
        task(s)
        val age = (System.nanoTime() - s.frameTime) / 1_000_000
        if (actionable && (s.frameTime <= 0 || age !in 0..5000)) throw Refusal("frame_stale")
        val bitmap = if (actionable) image else Bitmap.createScaledBitmap(image, 270, 270 * s.height / s.width, true)
        val out = ByteArrayOutputStream(); bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
        if (bitmap !== image) bitmap.recycle()
        if (actionable) {
            s.snapshot = UUID.randomUUID().toString(); s.observed = SystemClock.elapsedRealtime()
            s.snapshotFrameTime = s.frameTime
        }
        return JSONObject().put("snapshot_id", if (actionable) s.snapshot else JSONObject.NULL)
            .put("target_id", s.target).put("display_id", s.display.display.displayId)
            .put("target_generation", s.targetGeneration).put("capture_method", s.captureMethod)
            .put("width", if (actionable) s.width else 270).put("height", if (actionable) s.height else 270 * s.height / s.width)
            .put("rotation", s.display.display.rotation).put("frame_age_ms", age)
            .put("frame_time_source", "producer_monotonic_ns")
            .put("image_base64", Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP))
            .put("tree", JSONObject.NULL).put("tree_status", "unsupported")
    }

    private fun gesture(s: Session, p: JSONObject, swipe: Boolean): JSONObject {
        validate(p, if (swipe) setOf("snapshot_id", "from_x", "from_y", "to_x", "to_y", "duration_ms") else setOf("snapshot_id", "x", "y"))
        if (s.snapshot == null || p.getString("snapshot_id") != s.snapshot || SystemClock.elapsedRealtime() - s.observed > 5000) throw Refusal("stale_snapshot")
        if (s.snapshotFrameTime <= 0 || (System.nanoTime() - s.snapshotFrameTime) / 1_000_000 !in 0..5000) throw Refusal("frame_stale")
        if (s.display.display.rotation != 0) throw Refusal("rotation_not_supported")
        task(s)
        val x = p.getDouble(if (swipe) "from_x" else "x").toFloat()
        val y = p.getDouble(if (swipe) "from_y" else "y").toFloat()
        val endX = if (swipe) p.getDouble("to_x").toFloat() else x
        val endY = if (swipe) p.getDouble("to_y").toFloat() else y
        if (listOf(x, endX).any { !it.isFinite() || it < 0 || it >= s.width } || listOf(y, endY).any { !it.isFinite() || it < 0 || it >= s.height }) throw Refusal("point_out_of_bounds", 2)
        val duration = if (swipe) p.getInt("duration_ms") else 40
        if (duration !in 1..1000) throw Refusal("invalid_duration", 2)
        s.snapshot = null
        val downTime = SystemClock.uptimeMillis()
        fun inject(action: Int, px: Float, py: Float) {
            val event = MotionEvent.obtain(downTime, SystemClock.uptimeMillis(), action, px, py, 0)
            try {
                event.source = InputDevice.SOURCE_TOUCHSCREEN
                InputEvent::class.java.getMethod("setDisplayId", Int::class.javaPrimitiveType).invoke(event, s.display.display.displayId)
                val accepted = InputManager::class.java.getMethod("injectInputEvent", InputEvent::class.java, Int::class.javaPrimitiveType).invoke(input, event, 0) as Boolean
                check(accepted) { "Input not accepted" }
            } finally { event.recycle() }
        }
        var released = false
        var pressed = false
        try {
            inject(MotionEvent.ACTION_DOWN, x, y)
            pressed = true
            val steps = if (swipe) (duration / 16).coerceAtLeast(1) else 1
            for (i in 1..steps) {
                Thread.sleep((duration / steps).toLong())
                check(SystemClock.elapsedRealtime() < s.expires) { "Session lease expired during input" }
                task(s)
                if (swipe) inject(MotionEvent.ACTION_MOVE, x + (endX - x) * i / steps, y + (endY - y) * i / steps)
            }
            inject(MotionEvent.ACTION_UP, endX, endY); released = true
        } catch (error: Exception) {
            throw IllegalStateException("Input interrupted after admission", error)
        } finally {
            if (pressed && !released) {
                try { inject(MotionEvent.ACTION_CANCEL, endX, endY) }
                catch (_: Exception) { System.err.println("Gesture cancellation unverified") }
            }
        }
        return JSONObject().put("action", action("android_input_manager", detail = "Display-targeted events accepted; application effect requires independent verification"))
    }

    @Synchronized fun expire() {
        val s = session ?: return
        if (SystemClock.elapsedRealtime() >= s.expires) {
            try { stop(s) } catch (_: Exception) { System.err.println("Session cleanup uncertain") }
        }
    }

    private fun stop(s: Session) {
        s.snapshot = null
        val displayId = s.display.display.displayId
        pendingCleanup.add(displayId)
        pendingTaskCleanup.addAll(s.tasks.ids)
        session = null
        try { s.display.release() }
        finally { s.reader.close(); s.bitmap?.recycle() }
        for (attempt in 0..9) {
            if (displays.getDisplay(displayId) == null) break
            Thread.sleep(50)
        }
        if (displays.getDisplay(displayId) != null) throw IllegalStateException("Display release not observed")
        pendingCleanup.remove(displayId)
        for (attempt in 0..9) {
            pendingTaskCleanup.retainAll(inventory().map { it.id }.toSet())
            if (pendingTaskCleanup.isEmpty()) break
            Thread.sleep(50)
        }
        if (pendingTaskCleanup.isNotEmpty()) throw IllegalStateException("Owned task cleanup not observed")
        stopped[s.id] = s.owner
        while (stopped.size > 128) stopped.remove(stopped.keys.first())
        session = null
    }
}
