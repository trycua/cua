package ai.cua.android.demo

import ai.cua.driver.sdk.AndroidDriver
import ai.cua.driver.sdk.SessionOptions
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.os.Binder
import android.os.IBinder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONObject
import java.io.File
import java.util.UUID

internal object DemoProcess { val generation: String = UUID.randomUUID().toString() }

internal fun DemoSessionState.evidence(): JSONObject = JSONObject()
    .put("process_generation", DemoProcess.generation)
    .put("owner_generation", ownerGeneration).put("status", status)
    .put("session_id", sessionId ?: JSONObject.NULL)
    .put("runtime_generation", runtimeGeneration ?: JSONObject.NULL)
    .put("display_id", displayId ?: JSONObject.NULL)
    .put("target_id", targetId ?: JSONObject.NULL).put("task_id", taskId ?: JSONObject.NULL)
    .put("preview_frames", previewFrames).put("renewals", renewals)
    .put("mode", mode).put("phase", phase).put("last_reason", lastReason)
    .put("agent_steps", agentSteps).put("model_status", modelStatus)
    .put("current_package", currentPackage ?: JSONObject.NULL)
    .put("preview_label", previewLabel).put("final_frame_file", finalFrameFile ?: JSONObject.NULL)

/** The user-started foreground service owns the lease; Activities only observe it. */
class SessionService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mutableState = MutableStateFlow(DemoSessionState())
    val state: StateFlow<DemoSessionState> = mutableState
    private lateinit var controller: SessionController
    private var agentController: AgentController? = null
    private var activeMode: String? = null
    private val binder = LocalBinder()

    inner class LocalBinder : Binder() { val service: SessionService get() = this@SessionService }

    override fun onCreate() {
        super.onCreate()
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Local Android session", NotificationManager.IMPORTANCE_LOW))
        val driver = AndroidDriver(applicationContext)
        val operations = object : SessionOperations {
            override suspend fun create() = driver.createSession(SessionOptions(allowedApps = listOf("ai.cua.fixture.notes")))
            override suspend fun launch(sessionId: String) = driver.launchApp(sessionId, "ai.cua.fixture.notes")
            override suspend fun preview(sessionId: String, targetId: String) = driver.preview(sessionId, targetId)
            override suspend fun renew(sessionId: String) = driver.renewSession(sessionId)
            override suspend fun stop(sessionId: String) = driver.stopSession(sessionId)
        }
        controller = SessionController(operations, scope, initialState = state.value, onState = ::publish, onIdle = {
            activeMode = null
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        })
        publish(state.value)
    }

    override fun onBind(intent: Intent): IBinder = binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                if (activeMode != "agent") {
                    activeMode = "fixture"
                    startForeground(NOTIFICATION_ID, notification())
                    controller.start()
                }
            }
            ACTION_AGENT -> startAgent()
            ACTION_STOP -> if (activeMode == "agent") agentController?.requestStop() else controller.requestStop()
            else -> if (state.value.status == "Stopped") stopSelf(startId)
        }
        // Never silently recreate a lost controller or adopt an old session after process death.
        return START_NOT_STICKY
    }

    private fun startAgent() {
        if (activeMode != null) return
        if (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE == 0) {
            stopSelf(); return
        }
        startForeground(NOTIFICATION_ID, notification())
        val config = try {
            val file = File(filesDir, "agent-config.json")
            require(file.length() in 1..32768)
            AgentConfig.parse(file.readText())
        } catch (_: Exception) {
            publish(DemoSessionState(mode = "agent", status = "Error: Invalid private agent configuration"))
            stopForeground(STOP_FOREGROUND_REMOVE); stopSelf(); return
        }
        val driver = AndroidDriver(applicationContext)
        val ops = object : AgentOperations {
            override suspend fun create() = driver.createSession(SessionOptions(config.allowedApps, 540, 960, 160, "Visual model agent"))
            override suspend fun launch(sessionId: String, packageName: String) = driver.launchApp(sessionId, packageName)
            override suspend fun snapshot(sessionId: String, targetId: String) = driver.snapshot(sessionId, targetId)
            override suspend fun preview(sessionId: String, targetId: String) = driver.preview(sessionId, targetId)
            override suspend fun renew(sessionId: String) = driver.renewSession(sessionId)
            override suspend fun stop(sessionId: String) = driver.stopSession(sessionId)
            override suspend fun act(sessionId: String, snapshotId: String, action: AgentAction) = when (action.type) {
                "tap" -> driver.tap(sessionId, snapshotId, action.x, action.y)
                "swipe" -> driver.swipe(sessionId, snapshotId, action.x, action.y, action.toX, action.toY, action.durationMs)
                else -> error("Unsupported input")
            }
        }
        activeMode = "agent"
        agentController = AgentController(ops, LocalAgentModel(config), config.allowedApps, scope,
            pixels = { frame ->
                val bitmap = requireNotNull(android.graphics.BitmapFactory.decodeByteArray(frame.png, 0, frame.png.size))
                try {
                    require(bitmap.width == frame.width && bitmap.height == frame.height)
                    IntArray(bitmap.width * bitmap.height).also { bitmap.getPixels(it, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height) }
                } finally { bitmap.recycle() }
            }, onState = ::publish, onIdle = {
                activeMode = null
                stopForeground(STOP_FOREGROUND_REMOVE); stopSelf()
            })
        agentController!!.start()
    }

    private fun notification(): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop = PendingIntent.getService(this, 1, Intent(this, SessionService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return Notification.Builder(this, CHANNEL).setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle("Cua Android session").setContentText("Virtual display active")
            .setContentIntent(open).setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build()).build()
    }

    private fun publish(value: DemoSessionState) {
        val published = if (value.mode == "agent" && value.phase == "done" && value.preview != null) {
            try {
                File(filesDir, "agent-final.png").writeBytes(value.preview.png)
                value.copy(finalFrameFile = "agent-final.png")
            } catch (_: Exception) {
                value.copy(lastReason = "${value.lastReason} (Final PNG could not be saved)")
            }
        } else value
        mutableState.value = published
        synchronized(StateProvider::class.java) { File(filesDir, "service-state.json").writeText(published.evidence().toString()) }
    }

    override fun onDestroy() {
        if (activeMode == "agent") agentController?.requestStop() else controller.requestStop()
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        const val ACTION_START = "ai.cua.android.demo.START"
        const val ACTION_AGENT = "ai.cua.android.demo.RUN_AGENT"
        const val ACTION_STOP = "ai.cua.android.demo.STOP"
        private const val CHANNEL = "cua_session"
        private const val NOTIFICATION_ID = 1
    }
}
