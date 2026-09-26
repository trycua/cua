package ai.cua.fixture.notes

import android.app.Activity
import android.os.Bundle
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import org.json.JSONObject

/**
 * Exported, non-launcher synthetic screen for explicit Activity selection. It reports its own
 * class, task and display so the harness can check which component the driver started.
 */
open class DetailActivity : Activity() {
    private var taps = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        taps = savedInstanceState?.getInt("taps") ?: 0
        val label = TextView(this).apply { text = "Taps: $taps"; textSize = 24f }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(24, 48, 24, 24)
            addView(TextView(this@DetailActivity).apply { text = "Synthetic Detail Screen"; textSize = 24f })
            addView(Button(this@DetailActivity).apply {
                id = R.id.detail_tap
                text = "Tap target"
                setOnClickListener { taps++; label.text = "Taps: $taps"; record() }
            })
            addView(label)
        })
        window.decorView.post { record() }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putInt("taps", taps)
        super.onSaveInstanceState(outState)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        record()
    }

    private fun record() {
        val button = findViewById<View>(R.id.detail_tap) ?: return
        val xy = IntArray(2); button.getLocationOnScreen(xy)
        val state = JSONObject().put("package", packageName).put("activity", javaClass.name)
            .put("display_id", window.decorView.display?.displayId ?: -1).put("task_id", taskId)
            .put("taps", taps).put("window_focus", hasWindowFocus())
            .put("controls", JSONObject().put("tap", JSONObject()
                .put("x", xy[0] + button.width / 2).put("y", xy[1] + button.height / 2)))
        synchronized(StateProvider::class.java) { java.io.File(filesDir, "detail.json").writeText(state.toString()) }
    }
}

/** Refusal fixtures: declared `exported="false"` and `enabled="false"` in the manifest. */
class PrivateActivity : DetailActivity()
class DisabledActivity : DetailActivity()
