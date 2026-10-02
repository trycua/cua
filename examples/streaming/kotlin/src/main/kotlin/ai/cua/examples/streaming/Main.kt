// The shared streaming scenario (examples/streaming/SCENARIO.md) in Kotlin,
// headless only, on the generated UniFFI binding (`ai.cua.sdk`).
package ai.cua.examples.streaming

import ai.cua.sdk.Cua
import ai.cua.sdk.CuaConfig
import ai.cua.sdk.DecodedFrameSink
import ai.cua.sdk.DecodedVideoFrame
import ai.cua.sdk.SpacesdClient
import ai.cua.sdk.MediaEvent
import ai.cua.sdk.MediaOpenOptions
import ai.cua.sdk.MediaSession
import ai.cua.sdk.PcmAudio
import ai.cua.sdk.PcmSink
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import java.io.BufferedWriter
import java.io.File
import java.io.FileWriter
import java.lang.management.ManagementFactory
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.file.Files
import kotlin.system.exitProcess

// ---------------------------------------------------------------- inputs

data class Config(
    val url: String,
    val token: String,
    val seconds: Double,
    val outDir: String,
    val benchJsonl: String?,
    val benchTarget: String?,
    val benchSeconds: Double,
    val benchAudio: Boolean,
) {
    companion object {
        fun fromEnv(): Config {
            fun v(k: String) = System.getenv(k)?.takeIf { it.isNotEmpty() }
            val token = v("CUA_ENV_TOKEN") ?: error("CUA_ENV_TOKEN is required")
            val seconds = v("CUA_STREAM_SECONDS")?.toDoubleOrNull() ?: 5.0
            if (v("CUA_HEADLESS") == "0") System.err.println("note: the Kotlin example is headless only")
            return Config(
                url = v("CUA_ENV_URL") ?: "http://127.0.0.1:33211",
                token = token,
                seconds = seconds,
                outDir = v("CUA_OUT_DIR") ?: "./out",
                benchJsonl = v("CUA_BENCH_JSONL"),
                benchTarget = v("CUA_BENCH_TARGET"),
                benchSeconds = v("CUA_BENCH_SECONDS")?.toDoubleOrNull() ?: seconds,
                benchAudio = v("CUA_BENCH_AUDIO") != "0",
            )
        }
    }
}

// ---------------------------------------------------------------- helpers

fun unixNanos(): Long {
    val i = java.time.Instant.now()
    return i.epochSecond * 1_000_000_000L + i.nano
}

fun mono(): Double = System.nanoTime() / 1e9

fun fnv1a64(data: ByteArray): String {
    var h = -0x340d631b7bdddcdbL // 0xcbf29ce484222325
    for (b in data) {
        h = h xor (b.toLong() and 0xff)
        h *= 0x100000001b3L
    }
    return java.lang.Long.toUnsignedString(h, 16).padStart(16, '0')
}

fun JsonElement?.obj(k: String): JsonObject? = (this as? JsonObject)?.let { it[k] ?: it[camel(k)] } as? JsonObject
fun JsonElement?.prim(k: String): JsonPrimitive? = (this as? JsonObject)?.let { it[k] ?: it[camel(k)] } as? JsonPrimitive
fun JsonElement?.str(k: String): String? = prim(k)?.contentOrNull
fun JsonElement?.num(k: String): Double? = prim(k)?.let { it.doubleOrNull ?: it.contentOrNull?.toDoubleOrNull() }
fun JsonElement?.bool(k: String): Boolean? = prim(k)?.booleanOrNull
fun camel(snake: String): String {
    val p = snake.split('_')
    return p.first() + p.drop(1).joinToString("") { it.replaceFirstChar(Char::uppercase) }
}

fun parse(s: String): JsonElement? = runCatching { Json.parseToJsonElement(s) }.getOrNull()

fun round1(v: Double?): JsonElement =
    if (v == null || !v.isFinite()) JsonNull else JsonPrimitive(Math.round(v * 10) / 10.0)

fun writeWav(path: File, sampleRate: Int, channels: Int, pcm: ShortArray, n: Int) {
    val bb = ByteBuffer.allocate(44 + n * 2).order(ByteOrder.LITTLE_ENDIAN)
    bb.put("RIFF".toByteArray()).putInt(36 + n * 2).put("WAVE".toByteArray())
    bb.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(channels.toShort())
    bb.putInt(sampleRate).putInt(sampleRate * channels * 2).putShort((channels * 2).toShort()).putShort(16)
    bb.put("data".toByteArray()).putInt(n * 2)
    for (i in 0 until n) bb.putShort(pcm[i])
    path.parentFile?.mkdirs()
    path.writeBytes(bb.array())
}

/** SCENARIO.md "Bench timecode": full unix ms or null. */
fun decodeTimecode(
    f: DecodedVideoFrame, originX: Double, originY: Double, scale: Double, clientUnixNs: Long,
): Long? {
    val w = f.width.toInt(); val h = f.height.toInt(); val stride = f.stride.toInt(); val d = f.data
    val bits = IntArray(48)
    for (i in 0 until 48) {
        var sum = 0
        for (dy in 0 until 4) for (dx in 0 until 4) {
            val x = (originX + (16 * i + 6 + dx) * scale).toInt()
            val y = (originY + (6 + dy) * scale).toInt()
            if (x < 0 || y < 0 || x >= w || y >= h) return null
            val o = y * stride + x * 4
            if (o + 2 >= d.size) return null
            sum += (d[o].toInt() and 0xff) + (d[o + 1].toInt() and 0xff) + (d[o + 2].toInt() and 0xff)
        }
        bits[i] = if (sum / 48 > 128) 1 else 0
    }
    if (bits.slice(0..3) != listOf(1, 0, 1, 0) || bits.slice(44..47) != listOf(0, 1, 0, 1)) return null
    var value = 0L
    for (i in 4 until 36) value = (value shl 1) or bits[i].toLong()
    var check = 0
    for (i in 36 until 44) check = (check shl 1) or bits[i]
    val x = ((value ushr 24) xor (value ushr 16) xor (value ushr 8) xor value).toInt() and 0xff
    if (x != check) return null
    val now = clientUnixNs / 1_000_000
    val window = 1L shl 32
    val base = now - (now and (window - 1)) + value
    return listOf(base - window, base, base + window).minByOrNull { kotlin.math.abs(it - now) }
}

/** Thread-safe buffered JSONL appender. */
class JsonlWriter(path: String) {
    private val out: BufferedWriter
    init {
        File(path).absoluteFile.parentFile?.mkdirs()
        out = BufferedWriter(FileWriter(path, true), 64 * 1024)
    }
    @Synchronized fun line(s: String) { out.write(s); out.write("\n") }
    @Synchronized fun close() { out.close() }
}

// ---------------------------------------------------------------- recorder

data class TimecodeLocator(val originX: Double, val originY: Double, val contentWidth: Double)

/**
 * Frames, PCM and events of one media session. The SDK calls it on its
 * delivery thread; state is guarded by `this`.
 */
class Recorder(
    private val bench: JsonlWriter? = null,
    private val timecode: TimecodeLocator? = null,
) : DecodedFrameSink, PcmSink {
    private val start = mono()
    @Volatile var frames = 0L; private set
    var firstAt: Double? = null; private set
    var lastAt: Double? = null; private set
    @Volatile var last: DecodedVideoFrame? = null; private set
    var pcmFrames = 0L; private set
    var tcDecoded = 0L; private set
    var track: Int? = null; private set
    var sampleRate = 48_000; private set
    var channels = 2; private set
    private var pcm = ShortArray(48_000 * 2 * 10)
    private var pcmLen = 0
    private val maxPcm = 48_000 * 2 * 600
    private val events = ArrayList<MediaEvent>()
    var decodeErrors = 0; private set

    override fun onDecodedFrame(frame: DecodedVideoFrame) {
        val now = mono()
        val ns = unixNanos()
        val tc = if (bench != null && timecode != null) {
            val s = if (timecode.contentWidth > 0) frame.width.toDouble() / timecode.contentWidth else 1.0
            decodeTimecode(frame, timecode.originX * s, timecode.originY * s, s, ns)
        } else null
        synchronized(this) {
            frames++
            if (firstAt == null) firstAt = now
            lastAt = now
            last = frame
            if (tc != null) tcDecoded++
        }
        // Encoded size / keyframe flag are not on the decoded callback: null.
        bench?.line(
            "{\"t\":\"frame\",\"unix_ns\":$ns,\"seq\":${frame.sequence},\"bytes\":null,\"key\":null," +
                "\"cap_us\":${frame.captureTimestampUs},\"w\":${frame.width},\"h\":${frame.height},\"tc_ms\":${tc ?: "null"}}",
        )
    }

    override fun onEvent(event: MediaEvent) {
        synchronized(this) {
            if (events.size < 4096) events.add(event)
            if (event.kind == "decode_error") decodeErrors++
        }
    }

    override fun onPcm(audio: PcmAudio) {
        val ns = unixNanos()
        val ch = maxOf(1, audio.channels.toInt())
        synchronized(this) {
            if (track == null) {
                track = audio.trackId.toInt(); sampleRate = audio.sampleRate.toInt(); channels = ch
            }
            if (audio.trackId.toInt() == track) {
                pcmFrames++
                val n = audio.samples.size
                if (pcmLen + n <= maxPcm) {
                    if (pcmLen + n > pcm.size) pcm = pcm.copyOf(maxOf(pcm.size * 2, pcmLen + n))
                    for ((i, s) in audio.samples.withIndex()) pcm[pcmLen + i] = s
                    pcmLen += n
                }
            }
        }
        bench?.line("{\"t\":\"audio\",\"unix_ns\":$ns,\"pts_us\":${audio.ptsUs},\"bytes\":null,\"samples\":${audio.samples.size / ch}}")
    }

    @Synchronized fun eventsFrom(i: Int): List<MediaEvent> = if (i < events.size) events.subList(i, events.size).toList() else emptyList()
    @Synchronized fun eventCount() = events.size
    @Synchronized fun firstFrameMs(): Double? = firstAt?.let { (it - start) * 1000 }
    @Synchronized fun fps(): Double? {
        val a = firstAt ?: return null; val b = lastAt ?: return null
        return if (frames > 1 && b > a) (frames - 1) / (b - a) else null
    }
    @Synchronized fun pixel(x: Int, y: Int): IntArray? {
        val f = last ?: return null
        if (x < 0 || y < 0 || x >= f.width.toInt() || y >= f.height.toInt()) return null
        val o = y * f.stride.toInt() + x * 4
        return intArrayOf(f.data[o + 2].toInt() and 0xff, f.data[o + 1].toInt() and 0xff, f.data[o].toInt() and 0xff)
    }
    fun saveWav(path: File): Boolean = synchronized(this) {
        if (track == null) return false
        writeWav(path, sampleRate, channels, pcm, pcmLen)
        true
    }
}

// ---------------------------------------------------------------- scenario

data class Target(
    val kind: String, val id: String, val title: String,
    val x: Double, val y: Double, val width: Double, val height: Double,
    val available: Boolean, val primary: Boolean,
)

class Scenario(private val cfg: Config, private val env: SpacesdClient) {

    suspend fun listTargets(): List<Target> {
        val out = env.callJson("StreamService/ListTargets", """{"include_windows":true}""")
        val items = (parse(out) as? JsonObject)?.get("targets") as? JsonArray
            ?: error("unexpected ListTargets response: ${out.take(300)}")
        return items.mapNotNull { t ->
            val available = t.bool("available") ?: false
            t.obj("display")?.let { d ->
                val b = d.obj("bounds"); val n = d.obj("native_size")
                return@mapNotNull Target(
                    "display", d.str("id") ?: "", d.str("name") ?: "", b.num("x") ?: 0.0, b.num("y") ?: 0.0,
                    n.num("width") ?: b.num("width") ?: 0.0, n.num("height") ?: b.num("height") ?: 0.0,
                    available, d.bool("primary") ?: false,
                )
            }
            t.obj("window")?.let { w ->
                val b = w.obj("bounds")
                return@mapNotNull Target(
                    "window", w.obj("ref").str("id") ?: "", w.str("title") ?: "", b.num("x") ?: 0.0, b.num("y") ?: 0.0,
                    b.num("width") ?: 0.0, b.num("height") ?: 0.0, available, false,
                )
            }
            null
        }
    }

    private suspend fun waitForWindow(title: String, attempts: Int = 40): Pair<Target, List<Target>> {
        repeat(attempts) {
            val all = listTargets()
            all.firstOrNull { it.kind == "window" && it.title == title && it.width > 0 }?.let { return it to all }
            delay(250)
        }
        error("window \"$title\" not listed after $attempts attempts")
    }

    private fun options(t: Target, audio: Boolean, interactive: Boolean = false): MediaOpenOptions {
        val policy = if (interactive) ""","policy":"SESSION_POLICY_ALLOW_ACTIVATION"""" else ""
        return MediaOpenOptions(
            display = if (t.kind == "display") t.id else null,
            windowHandle = if (t.kind == "window") t.id else null,
            maxFps = 30u, maxDimension = 0u, audio = audio, disableVideo = false,
            requestJson = """{"codecs":["MEDIA_CODEC_H264"]$policy}""",
        )
    }

    private suspend fun stream(
        t: Target, name: String, interactive: Boolean = false,
        during: (suspend (MediaSession, Recorder) -> Unit)? = null,
    ): Pair<JsonObject, Long> {
        val rec = Recorder()
        val t0 = mono()
        val session = env.openMediaDecodedWithAudio(options(t, true, interactive), rec, rec)
        println("$name: session ${session.sessionId()} codec ${session.codec()}")
        during?.invoke(session, rec)
        val remaining = cfg.seconds - (mono() - t0)
        if (remaining > 0) delay((remaining * 1000).toLong())
        val stats = session.stats()
        runCatching { session.closeAsync() }
        val wav = File(cfg.outDir, "$name.wav")
        val wrote = runCatching { rec.saveWav(wav) }.getOrDefault(false)
        val last = rec.last
        val summary = buildJsonObject {
            put("frames", rec.frames)
            put("keyframes", JsonNull) // not on the decoded callback
            put("bytes", JsonNull) // not on the decoded callback
            put("first_frame_ms", round1(rec.firstFrameMs()))
            put("fps", round1(rec.fps()))
            put("audio_packets", stats.audioPackets.toLong())
            put("pcm_frames", rec.pcmFrames)
            put("frames_dropped", stats.framesDropped.toLong())
            put("last_hash", last?.let { fnv1a64(it.data) })
            put("last_size", last?.let { "${it.width}x${it.height}" })
            put("hash_of", "decoded_bgra")
            put("wav", if (wrote) wav.path else null)
            put("decode_errors", rec.decodeErrors)
        }
        println("$name: $summary")
        return summary to rec.frames
    }

    private suspend fun pressLines(): Int {
        val out = runCatching { env.sh("cat /tmp/cua-fixtures/grid.jsonl 2>/dev/null || true", 10_000u) }.getOrNull()
            ?: return 0
        return String(out.stdout).lineSequence().count { line ->
            val o = parse(line) as? JsonObject ?: return@count false
            val cell = (o["cell"] as? JsonArray)?.map { (it as? JsonPrimitive)?.intOrNull }
            o.str("type") == "button_press" && cell == listOf(2, 3)
        }
    }

    private suspend fun click(window: Target, session: MediaSession, rec: Recorder): JsonObject {
        var n = 0
        while (rec.frames == 0L && n++ < 100) delay(50)
        delay(500)
        val frame = rec.last ?: return buildJsonObject { put("sent", false); put("error", "no frame before click") }
        val scale = if (window.width > 0) frame.width.toDouble() / window.width else 1.0
        val px = Math.round(200 * scale).toInt(); val py = Math.round(280 * scale).toInt()
        val before = pressLines()
        val sessionId = rec.eventsFrom(0).firstNotNullOfOrNull { e ->
            if (e.kind == "session_opened") parse(e.json).obj("payload").str("session_id") else null
        } ?: session.sessionId()
        val actionId = "kotlin-click-${unixNanos()}"
        val action = buildJsonObject {
            put("type", "action")
            putJsonObject("payload") {
                put("action_id", actionId); put("session_id", sessionId); put("tool", "click")
                putJsonObject("arguments") { put("x", px); put("y", py) }
                putJsonObject("basis") {
                    put("kind", "pixel"); put("geometry_epoch", frame.geometryEpoch.toLong())
                    put("frame_sequence", frame.sequence.toLong())
                }
            }
        }
        val mark = rec.eventCount()
        var sent = false
        var via: String? = null
        var delivered = false
        var actionError: JsonElement = JsonNull
        try {
            session.sendControl(action.toString())
            sent = true; via = "action"
            loop@ for (i in 0 until 60) {
                for (e in rec.eventsFrom(mark)) {
                    if (e.kind != "action_result") continue
                    val p = parse(e.json).obj("payload")
                    if (p.str("action_id") == actionId) {
                        delivered = p.bool("delivered") ?: false
                        actionError = p?.get("error") ?: JsonNull
                        break@loop
                    }
                }
                delay(50)
            }
        } catch (e: Exception) {
            actionError = JsonPrimitive(e.toString())
        }
        suspend fun waitLogged(tries: Int): Boolean {
            repeat(tries) { if (pressLines() > before) return true; delay(250) }
            return false
        }
        var logged = if (delivered) waitLogged(8) else false
        var envReport: JsonElement = JsonNull
        if (!logged) {
            // Fallback: env pointer API, screen coordinates, foreground
            // delivery (auto picks X11 XSendEvent, which GTK3 ignores).
            val req = """{"target":{"delivery":"DELIVERY_FOREGROUND"},"click":{"position":{"x":${window.x + 200},"y":${window.y + 280}},"button":"MOUSE_BUTTON_LEFT","count":1}}"""
            runCatching { env.pointerJson(req) }.onSuccess {
                sent = true; via = "env"; envReport = (parse(it) as? JsonObject)?.get("report") ?: JsonNull
                logged = waitLogged(12)
            }
        }
        delay(400)
        val pix = rec.pixel(px, py)
        return buildJsonObject {
            put("sent", sent); put("via", via); put("logged", logged)
            put("pixel_ok", pix != null && kotlin.math.abs(pix[0] - 72) <= 24 && kotlin.math.abs(pix[1] - 153) <= 24 && kotlin.math.abs(pix[2] - 128) <= 24)
            pix?.let { p -> putJsonArray("pixel") { p.forEach { add(JsonPrimitive(it)) } } }
            putJsonArray("at") { add(JsonPrimitive(px)); add(JsonPrimitive(py)) }
            put("action_delivered", delivered); put("action_error", actionError); put("env_report", envReport)
        }
    }

    suspend fun run(): Int {
        println("health: ${env.health()}")
        val started = env.sh("cua-fixtures start grid", 30_000u)
        println("fixture: exit ${started.exit.code} ${String(started.stdout).trim()}")
        val (grid, targets) = waitForWindow("CUA Fixture Grid")
        for (t in targets) println("target ${t.kind} ${t.id} \"${t.title}\" ${t.width.toInt()}x${t.height.toInt()}+${t.x.toInt()}+${t.y.toInt()} available=${t.available}")
        val display = targets.firstOrNull { it.kind == "display" && it.primary }
            ?: targets.firstOrNull { it.kind == "display" } ?: error("no display target")
        val (desktop, desktopFrames) = stream(display, "desktop")
        var click: JsonObject = buildJsonObject {}
        val (window, windowFrames) = stream(grid, "window", interactive = true) { s, r -> click = click(grid, s, r) }
        val summary = buildJsonObject {
            put("example", "kotlin"); put("desktop", desktop); put("window", window); put("click", click)
        }
        println("SUMMARY $summary")
        val logged = click.bool("logged") ?: false
        return if (desktopFrames > 0 && windowFrames > 0 && logged) 0 else 1
    }

    suspend fun runBench(path: String): Int {
        val spec = cfg.benchTarget ?: "display:"
        val targets = listTargets()
        val target: Target
        var locator: TimecodeLocator? = null
        if (spec.startsWith("window:")) {
            val title = spec.removePrefix("window:")
            target = targets.firstOrNull { it.kind == "window" && it.title == title } ?: error("bench target $spec not listed")
            locator = TimecodeLocator(0.0, 0.0, target.width)
        } else {
            val id = spec.removePrefix("display:")
            target = targets.firstOrNull { it.kind == "display" && (if (id.isEmpty()) it.primary else it.id == id) }
                ?: targets.firstOrNull { it.kind == "display" } ?: error("bench target $spec not listed")
            targets.firstOrNull { it.kind == "window" && it.title == "CUA Bench Timecode" }?.let { w ->
                locator = TimecodeLocator(w.x - target.x, w.y - target.y, target.width)
            }
        }
        val out = JsonlWriter(path)
        val rec = Recorder(out, locator)
        out.line("{\"t\":\"open\",\"unix_ns\":${unixNanos()}}")
        val session = if (cfg.benchAudio) env.openMediaDecodedWithAudio(options(target, true), rec, rec)
        else env.openMediaDecoded(options(target, false), rec)
        delay((cfg.benchSeconds * 1000).toLong())
        runCatching { session.closeAsync() }
        // getrusage equivalent: process CPU time (user+sys) from the JVM.
        val os = ManagementFactory.getOperatingSystemMXBean() as? com.sun.management.OperatingSystemMXBean
        val cpu = (os?.processCpuTime ?: 0L) / 1e9
        val thr = ManagementFactory.getThreadMXBean()
        val user = thr.allThreadIds.sumOf { maxOf(0L, thr.getThreadUserTime(it)) } / 1e9
        out.line("{\"t\":\"end\",\"unix_ns\":${unixNanos()},\"cpu_user_s\":${"%.3f".format(user)},\"cpu_sys_s\":${"%.3f".format(maxOf(0.0, cpu - user))}}")
        out.close()
        println("SUMMARY " + buildJsonObject {
            put("example", "kotlin")
            putJsonObject("bench") { put("target", spec); put("frames", rec.frames); put("tc_decoded", rec.tcDecoded); put("jsonl", path) }
        })
        return if (rec.frames > 0) 0 else 1
    }
}

fun main() {
    val cfg = Config.fromEnv()
    val code = runBlocking {
        try {
            // Throwaway SDK state: no host state is read or written.
            val tmp = Files.createTempDirectory("cua-streaming-kotlin").toString()
            val cua = Cua.embedded(
                CuaConfig(stateDir = "$tmp/state", fleetFromEnv = false, spacesHome = "$tmp/home", teleportHome = "$tmp/home"),
            )
            val env = cua.spacesd(cfg.url, cfg.token)
            val s = Scenario(cfg, env)
            cfg.benchJsonl?.let { s.runBench(it) } ?: s.run()
        } catch (e: Exception) {
            System.err.println("error: $e")
            1
        }
    }
    exitProcess(code)
}
