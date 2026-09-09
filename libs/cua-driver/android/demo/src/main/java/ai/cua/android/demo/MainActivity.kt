package ai.cua.android.demo

import android.app.Activity
import android.graphics.BitmapFactory
import android.os.Bundle
import android.text.Editable
import android.text.TextWatcher
import android.util.Base64
import android.view.KeyEvent
import android.view.MotionEvent
import android.widget.Button
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import ai.cua.driver.sdk.DriverClient
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private lateinit var humanEditor: EditText
    private lateinit var status: TextView
    private lateinit var preview: ImageView
    private val worker = Executors.newSingleThreadExecutor()
    @Volatile private var running = false
    @Volatile private var busy = false
    @Volatile private var destroyed = false
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
            setPadding(24, 48, 24, 24)
            addView(TextView(this@MainActivity).apply { text = "Cua Android Demo"; textSize = 24f })
            addView(humanEditor)
            addView(status)
            addView(Button(this@MainActivity).apply { id = R.id.start; text = "Start"; setOnClickListener { startSession() } })
            addView(Button(this@MainActivity).apply {
                id = R.id.stop
                text = "Stop"
                setOnClickListener { running = false; showStatus(if (busy) "Stopping" else "Stopped") }
            })
            addView(preview, LinearLayout.LayoutParams(-1, 0, 1f))
        })
        record("created")
        window.decorView.post { record("layout") }
    }

    private fun startSession() {
        if (busy || destroyed) return
        busy = true
        running = true
        showStatus("Starting")
        worker.execute {
            val client = DriverClient(this@MainActivity)
            var sessionId: String? = null
            try {
                sessionId = data(client.call("session.create", params = JSONObject()
                    .put("width", 1080).put("height", 1920).put("density", 320)
                    .put("allowed_apps", JSONArray().put("ai.cua.fixture.notes")))).getString("session_id")
                val targetId = data(client.call("app.launch", sessionId, JSONObject().put("package", "ai.cua.fixture.notes"))).getString("target_id")
                showStatus("Running")
                var lastRenew = android.os.SystemClock.uptimeMillis()
                while (running && !destroyed) {
                    val snapshot = data(client.call("preview", sessionId, JSONObject().put("target_id", targetId)))
                    val bytes = Base64.decode(snapshot.getString("image_base64"), Base64.DEFAULT)
                    val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
                        ?: error("Preview image could not be decoded")
                    runOnUiThread {
                        if (!destroyed) { preview.setImageBitmap(bitmap); previewFrames++; record("preview") }
                    }
                    if (android.os.SystemClock.uptimeMillis() - lastRenew >= 10_000) {
                        data(client.call("session.renew", sessionId))
                        lastRenew = android.os.SystemClock.uptimeMillis()
                    }
                    Thread.sleep(1000)
                }
                showStatus("Stopping")
            } catch (error: Exception) {
                showStatus("Error: ${error.message ?: error.javaClass.simpleName}")
            } finally {
                if (sessionId != null) {
                    try { data(client.call("session.stop", sessionId)); showStatus("Stopped") }
                    catch (error: Exception) { showStatus("Stop failed: ${error.message}") }
                }
                running = false
                busy = false
            }
        }
    }

    private fun data(response: JSONObject): JSONObject {
        check(response.optString("status") == "ok") { response.optJSONObject("error")?.optString("message") ?: "Runtime request failed" }
        return response.getJSONObject("data")
    }

    private fun showStatus(message: String) = runOnUiThread {
        if (!destroyed) { status.text = message; record("status") }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("text", humanEditor.text.toString())
        super.onSaveInstanceState(outState)
    }

    override fun onDestroy() {
        destroyed = true
        running = false
        worker.shutdown()
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
        val controls = JSONObject()
        for ((name, id) in listOf("editor" to R.id.human_editor, "start" to R.id.start, "stop" to R.id.stop)) {
            val view = findViewById<android.view.View>(id) ?: continue
            val xy = IntArray(2); view.getLocationOnScreen(xy)
            controls.put(name, JSONObject().put("x", xy[0] + view.width / 2).put("y", xy[1] + view.height / 2))
        }
        state.put("controls", controls)
        synchronized(StateProvider::class.java) { java.io.File(filesDir, "state.json").writeText(state.toString()) }
    }
}
