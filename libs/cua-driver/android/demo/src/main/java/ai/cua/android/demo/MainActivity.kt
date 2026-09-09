package ai.cua.android.demo

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.graphics.BitmapFactory
import android.os.Bundle
import android.os.IBinder
import android.text.Editable
import android.text.TextWatcher
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.widget.Button
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

class MainActivity : Activity() {
    private lateinit var humanEditor: EditText
    private lateinit var status: TextView
    private lateinit var preview: ImageView
    private val uiScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var observation: Job? = null
    private var bound = false
    private var sessionState = DemoSessionState(ownerGeneration = "unbound")
    private val activityGeneration = UUID.randomUUID().toString()
    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            val service = (binder as SessionService.LocalBinder).service
            observation?.cancel()
            observation = uiScope.launch { service.state.collect { render(it) } }
        }
        override fun onServiceDisconnected(name: ComponentName) {
            observation?.cancel()
            status.text = "Controller disconnected"
            record("controller_disconnected")
        }
    }
    private val events = ArrayDeque<JSONObject>()
    private var previewFrames = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        humanEditor = EditText(this).apply {
            id = R.id.human_editor
            hint = "Human input (synthetic text only)"
            setSingleLine(true)
            setText(savedInstanceState?.getString("text") ?: "")
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
                override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) { record("text") }
                override fun afterTextChanged(s: Editable?) {}
            })
            setOnFocusChangeListener { _, _ -> record("editor_focus") }
        }
        status = TextView(this).apply { id = R.id.status; text = "Stopped" }
        preview = ImageView(this).apply { id = R.id.preview; contentDescription = "Read-only agent display preview"; scaleType = ImageView.ScaleType.FIT_CENTER }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setOnApplyWindowInsetsListener { view, insets ->
                val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
                view.setPadding(24 + bars.left, 24 + bars.top, 24 + bars.right, 24 + bars.bottom)
                insets
            }
            addView(TextView(this@MainActivity).apply { text = "Cua Android Demo"; textSize = 24f })
            addView(humanEditor)
            addView(status)
            addView(Button(this@MainActivity).apply {
                id = R.id.start; text = "Start"
                setOnClickListener { startForegroundService(Intent(this@MainActivity, SessionService::class.java).setAction(SessionService.ACTION_START)) }
            })
            if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) addView(Button(this@MainActivity).apply {
                id = R.id.run_agent; text = "Run agent"
                setOnClickListener { runAgent() }
            })
            addView(Button(this@MainActivity).apply {
                id = R.id.stop
                text = "Stop"
                setOnClickListener { startService(Intent(this@MainActivity, SessionService::class.java).setAction(SessionService.ACTION_STOP)) }
            })
            addView(preview, LinearLayout.LayoutParams(-1, 0, 1f))
        })
        window.insetsController?.setSystemBarsAppearance(
            WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
            WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS)
        if (checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        record("created")
        window.decorView.post { record("layout") }
        if (intent.action == SessionService.ACTION_AGENT) runAgent()
    }

    private suspend fun render(value: DemoSessionState) {
        val bitmap = value.preview?.let { frame ->
            withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(frame.png, 0, frame.png.size) }
        }
        sessionState = value
        status.text = if (value.mode == "agent") "${value.status} | ${value.phase} | step ${value.agentSteps}/40\nModel: ${value.modelStatus} | renewals ${value.renewals}\n${value.currentPackage ?: ""}\n${value.lastReason.take(180)}" else value.status
        if (value.mode == "agent" && value.preview != null && value.phase == "done") status.append("\n${value.previewLabel}")
        preview.contentDescription = value.previewLabel
        preview.setImageBitmap(bitmap)
        previewFrames = value.previewFrames
        record("controller_state")
    }

    override fun onStart() {
        super.onStart()
        bound = bindService(Intent(this, SessionService::class.java), connection, BIND_AUTO_CREATE)
    }

    override fun onStop() {
        observation?.cancel()
        if (bound) { unbindService(connection); bound = false }
        super.onStop()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.action == SessionService.ACTION_AGENT) runAgent()
        // Debug fixture hook exercises real Activity recreation without restarting the owner.
        if (intent.action == "ai.cua.android.demo.RECREATE" && applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) recreate()
    }

    private fun runAgent() {
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0)
            startForegroundService(Intent(this, SessionService::class.java).setAction(SessionService.ACTION_AGENT))
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("text", humanEditor.text.toString())
        super.onSaveInstanceState(outState)
    }

    override fun onDestroy() {
        uiScope.cancel()
        super.onDestroy()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        record("window_focus")
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        val handled = super.dispatchTouchEvent(event)
        record("touch", JSONObject().put("action", event.actionMasked).put("receiver_display_id", window.decorView.display?.displayId ?: -1).put("handled", handled))
        return handled
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val handled = super.dispatchKeyEvent(event)
        record("key", JSONObject().put("action", event.action).put("key_code", event.keyCode).put("receiver_display_id", window.decorView.display?.displayId ?: -1).put("handled", handled))
        return handled
    }

    private fun record(kind: String, details: JSONObject = JSONObject()) {
        if (!::humanEditor.isInitialized || !::status.isInitialized) return
        events.addLast(details.put("kind", kind).put("time_ms", android.os.SystemClock.uptimeMillis()).put("display_id", window.decorView.display?.displayId ?: -1))
        while (events.size > 128) events.removeFirst()
        val state = JSONObject().put("package", packageName).put("display_id", window.decorView.display?.displayId ?: -1)
            .put("text", humanEditor.text.toString()).put("status", status.text.toString())
            .put("window_focus", hasWindowFocus()).put("editor_focus", humanEditor.hasFocus()).put("events", JSONArray(events.toList()))
            .put("preview_frames", previewFrames)
            .put("activity_generation", activityGeneration).put("controller", sessionState.evidence())
        val controls = JSONObject()
        for ((name, id) in listOf("editor" to R.id.human_editor, "start" to R.id.start, "stop" to R.id.stop, "run_agent" to R.id.run_agent)) {
            val view = findViewById<android.view.View>(id) ?: continue
            val xy = IntArray(2); view.getLocationOnScreen(xy)
            controls.put(name, JSONObject().put("x", xy[0] + view.width / 2).put("y", xy[1] + view.height / 2))
        }
        state.put("controls", controls)
        synchronized(StateProvider::class.java) { java.io.File(filesDir, "state.json").writeText(state.toString()) }
    }
}
