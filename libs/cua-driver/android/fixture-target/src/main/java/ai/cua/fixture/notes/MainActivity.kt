package ai.cua.fixture.notes

import android.app.Activity
import android.os.Bundle
import android.text.Editable
import android.text.TextWatcher
import android.view.KeyEvent
import android.view.MotionEvent
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import org.json.JSONArray
import org.json.JSONObject

class MainActivity : Activity() {
    private lateinit var editor: EditText
    private lateinit var counter: TextView
    private var count = 0
    private val events = ArrayDeque<JSONObject>()
    private var authorizationProbe = "pending"

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        count = savedInstanceState?.getInt("count") ?: 0
        editor = EditText(this).apply {
            id = R.id.editor
            hint = "Synthetic notes only"
            setSingleLine(true)
            setText(savedInstanceState?.getString("text") ?: "")
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
                override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) { record("text") }
                override fun afterTextChanged(s: Editable?) {}
            })
            setOnFocusChangeListener { _, _ -> record("editor_focus") }
        }
        counter = TextView(this).apply { id = R.id.counter; text = "Count: $count"; textSize = 24f }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 48, 24, 24)
            addView(TextView(this@MainActivity).apply { text = "Synthetic Notes Fixture"; textSize = 24f })
            addView(editor)
            addView(Button(this@MainActivity).apply {
                id = R.id.increment
                text = "Increment"
                setOnClickListener { count++; counter.text = "Count: $count"; record("increment") }
            })
            addView(counter)
        })
        record("created")
        window.decorView.post { record("layout") }
        kotlin.concurrent.thread(name = "unauthorized-fixture-probe") {
            val result = try {
                val response = ai.cua.driver.sdk.DriverClient(this).call("doctor")
                if (response.optString("status") == "refused" &&
                    response.optJSONObject("error")?.optString("reason") == "caller_not_authorized") "denied" else "unexpected_response"
            } catch (_: Exception) { "transport_error" }
            runOnUiThread { authorizationProbe = result; record("authorization_probe") }
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putInt("count", count)
        outState.putString("text", editor.text.toString())
        super.onSaveInstanceState(outState)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        record("window_focus")
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        val result = super.dispatchTouchEvent(event)
        record("touch", JSONObject().put("action", event.actionMasked).put("x", event.x).put("y", event.y).put("receiver_display_id", window.decorView.display?.displayId ?: -1).put("handled", result))
        return result
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val result = super.dispatchKeyEvent(event)
        record("key", JSONObject().put("action", event.action).put("key_code", event.keyCode).put("receiver_display_id", window.decorView.display?.displayId ?: -1).put("handled", result))
        return result
    }

    private fun record(kind: String, details: JSONObject = JSONObject()) {
        if (!::editor.isInitialized) return
        details.put("kind", kind).put("display_id", window.decorView.display?.displayId ?: -1).put("time_ms", android.os.SystemClock.uptimeMillis())
        events.addLast(details)
        while (events.size > 128) events.removeFirst()
        val state = JSONObject().put("package", packageName).put("display_id", window.decorView.display?.displayId ?: -1)
            .put("authorization_probe", authorizationProbe)
            .put("text", editor.text.toString()).put("counter", count)
            .put("window_focus", hasWindowFocus()).put("editor_focus", editor.hasFocus())
            .put("events", JSONArray(events.toList()))
        val controls = JSONObject()
        for ((name, id) in listOf("editor" to R.id.editor, "increment" to R.id.increment)) {
            val view = findViewById<android.view.View>(id) ?: continue
            val xy = IntArray(2); view.getLocationOnScreen(xy)
            controls.put(name, JSONObject().put("x", xy[0] + view.width / 2).put("y", xy[1] + view.height / 2))
        }
        state.put("controls", controls)
        synchronized(StateProvider::class.java) { java.io.File(filesDir, "state.json").writeText(state.toString()) }
    }
}
