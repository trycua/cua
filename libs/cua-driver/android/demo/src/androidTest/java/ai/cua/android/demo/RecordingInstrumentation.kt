package ai.cua.android.demo

import ai.cua.driver.sdk.AndroidDriver
import android.app.Activity
import android.app.Instrumentation
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.json.JSONObject

/** Opt-in recording fixture: real SDK input, with synthetic fixture state as its oracle. */
class RecordingInstrumentation : Instrumentation() {
    override fun onCreate(arguments: Bundle?) {
        super.onCreate(arguments)
        start()
    }

    override fun onStart() {
        val result = Bundle()
        try {
            runBlocking {
                val driver = AndroidDriver(targetContext)
                val deadline = SystemClock.elapsedRealtime() + 60_000
                var sessionId: String? = null
                var taps = 0
                while (SystemClock.elapsedRealtime() < deadline) {
                    val owner = state("ai.cua.android.demo").optJSONObject("service")
                    if (owner?.optString("status") != "Running" || !owner.optBoolean("owner_process_alive")) {
                        if (sessionId != null) break
                        delay(100)
                        continue
                    }
                    val currentSession = owner.getString("session_id")
                    check(sessionId == null || sessionId == currentSession)
                    sessionId = currentSession
                    val fixture = state("ai.cua.fixture.notes")
                    // The provider can still hold the previous Activity's state during launch.
                    if (fixture.optInt("display_id", -1) != owner.getInt("display_id") ||
                        fixture.optJSONObject("controls")?.optJSONObject("increment")?.optInt("x", 0) == 0) {
                        delay(100)
                        continue
                    }
                    val point = fixture.getJSONObject("controls").getJSONObject("increment")
                    val frame = driver.snapshot(currentSession, owner.getString("target_id")).data
                    check(frame.displayId == fixture.getInt("display_id") && frame.displayId != 0)
                    driver.tap(currentSession, requireNotNull(frame.snapshotId), point.getInt("x"), point.getInt("y"))
                    val expected = fixture.getInt("counter") + 1
                    val receiptDeadline = SystemClock.elapsedRealtime() + 2_000
                    while (state("ai.cua.fixture.notes").getInt("counter") != expected) {
                        check(SystemClock.elapsedRealtime() < receiptDeadline) { "SDK tap had no fixture receipt" }
                        delay(50)
                    }
                    taps++
                    sendStatus(0, Bundle().apply { putInt("verified_sdk_taps", taps) })
                    delay(1_500)
                }
                check(taps >= 5) { "Recording did not exercise SDK taps" }
                result.putInt("verified_sdk_taps", taps)
                // Keep the final stopped UI visible while the screen recorder drains.
                delay(10_000)
            }
            finish(Activity.RESULT_OK, result)
        } catch (error: Exception) {
            result.putString("error", error.stackTraceToString())
            finish(Activity.RESULT_CANCELED, result)
        }
    }

    private fun state(packageName: String): JSONObject {
        return requireNotNull(targetContext.contentResolver.query(Uri.parse("content://$packageName.state"), null, null, null, null)) {
            "Fixture provider unavailable: $packageName"
        }.use {
            check(it.moveToFirst())
            JSONObject(it.getString(it.getColumnIndexOrThrow("json")))
        }
    }
}
