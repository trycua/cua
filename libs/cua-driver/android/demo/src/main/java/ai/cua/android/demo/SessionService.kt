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

/** The user-started foreground service owns the lease; Activities only observe it. */
class SessionService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val mutableState = MutableStateFlow(DemoSessionState())
    val state: StateFlow<DemoSessionState> = mutableState
    private lateinit var controller: SessionController
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
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
        })
        publish(state.value)
    }

    override fun onBind(intent: Intent): IBinder = binder

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> {
                startForeground(NOTIFICATION_ID, notification())
                controller.start()
            }
            ACTION_STOP -> controller.requestStop()
            else -> if (state.value.status == "Stopped") stopSelf(startId)
        }
        // Never silently recreate a lost controller or adopt an old session after process death.
        return START_NOT_STICKY
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
        mutableState.value = value
        synchronized(StateProvider::class.java) { File(filesDir, "service-state.json").writeText(value.evidence().toString()) }
    }

    override fun onDestroy() {
        controller.requestStop()
        scope.cancel()
        super.onDestroy()
    }

    companion object {
        const val ACTION_START = "ai.cua.android.demo.START"
        const val ACTION_STOP = "ai.cua.android.demo.STOP"
        private const val CHANNEL = "cua_session"
        private const val NOTIFICATION_ID = 1
    }
}
