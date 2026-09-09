package ai.cua.driver.sdk

import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class AndroidDriverTest {
    private fun envelope(id: String = "req", data: JSONObject = JSONObject()) = JSONObject()
        .put("contract_version", DriverClient.VERSION).put("request_id", id)
        .put("runtime_generation", "generation").put("status", "ok").put("exit_code", 0).put("data", data)
    private fun action() = JSONObject().put("action_id", "action").put("requested_delivery", "background")
        .put("actual_delivery", "unknown").put("transport", "android_input_manager").put("effect", "unverifiable")
        .put("evidence", JSONArray().put(JSONObject().put("kind", "native_api_result").put("detail", "accepted")))
    private fun session() = JSONObject().put("session_id", "session").put("state", "active")
        .put("display_id", 2).put("display_generation", "session").put("width", 1080).put("height", 1920)
        .put("lease_remaining_ms", 60000L).put("lease_owner", "caller").put("target_id", JSONObject.NULL)
    private fun driver(response: JSONObject) = AndroidDriver(DriverTransport { _, _, _, _ -> response })
    private suspend inline fun <reified T : Throwable> failure(block: () -> Unit): T {
        try { block() } catch (error: Throwable) {
            if (error is T) return error
            throw AssertionError("Expected ${T::class.java}, got $error", error)
        }
        throw AssertionError("Expected ${T::class.java}")
    }

    @Test fun sessionWireAndRequestCorrelation() = runBlocking {
        var calls = 0
        val sdk = AndroidDriver(DriverTransport { op, sid, p, id ->
            calls++
            assertEquals("session.create", op); assertNull(sid); assertEquals("req", id)
            assertEquals(1080, p.getInt("width")); assertEquals(320, p.getInt("density"))
            assertEquals("com.example.app", p.getJSONArray("allowed_apps").getString(0))
            assertFalse(p.has("label"))
            envelope(id, session())
        })
        val result = sdk.createSession(SessionOptions(listOf("com.example.app")), "req")
        assertEquals("req", result.requestId); assertEquals("generation", result.runtimeGeneration)
        assertEquals(60000L, result.data.leaseRemainingMs); assertNull(result.data.targetId)
        assertEquals(1, calls)
    }

    @Test fun invalidEnvelopesAreUncertain() = runBlocking {
        val variants = listOf(
            envelope().put("contract_version", "other"), envelope("other"),
            envelope().put("runtime_generation", JSONObject.NULL), envelope().put("status", "success"),
            envelope().put("exit_code", "0"), envelope().put("exit_code", 0.0),
            envelope().put("exit_code", 3), envelope().put("data", JSONArray()),
            envelope().put("status", "refused").put("exit_code", 4),
            envelope().put("status", "uncertain").put("exit_code", 3),
            envelope().put("error", JSONObject()), envelope().put("action", JSONObject.NULL)
        )
        for (response in variants) {
            assertEquals("req", failure<DriverUncertainException> { driver(response).capabilities("req") }.requestId)
        }
    }

    @Test fun refusalsAndUncertainOutcomesPreserveIds() = runBlocking {
        val refused = envelope().put("status", "refused").put("exit_code", 3)
            .put("error", JSONObject().put("reason", "stale_snapshot"))
        val error = failure<DriverRefusedException> { driver(refused).tap("session", "snapshot", 10, 20, "req") }
        assertEquals("req", error.requestId); assertEquals("generation", error.runtimeGeneration)
        assertEquals("stale_snapshot", error.reason)
        val uncertain = envelope().put("status", "uncertain").put("exit_code", 4)
            .put("error", JSONObject().put("reason", "runtime_failure"))
        val unknown = failure<DriverUncertainException> { driver(uncertain).tap("session", "snapshot", 10, 20, "req") }
        assertEquals("req", unknown.requestId); assertEquals("generation", unknown.runtimeGeneration)
    }

    @Test fun actionEvidenceDoesNotUpgradeAcceptanceToEffect() = runBlocking {
        val result = driver(envelope().put("action", action())).tap("session", "snapshot", 10, 20, "req")
        assertEquals(Unit, result.data)
        assertEquals("unknown", result.action!!.actualDelivery)
        assertEquals("unverifiable", result.action.effect)
        assertEquals("accepted", result.action.evidence.single().detail)
        failure<DriverUncertainException> { driver(envelope()).tap("session", "snapshot", 10, 20, "req") }
        Unit
    }

    @Test fun typedFieldsRejectCoercionAndWrongSession() = runBlocking {
        for (data in listOf(session().put("width", "1080"), session().put("width", 1080.5),
            session().put("lease_remaining_ms", -1), session().put("target_id", 42),
            session().put("state", "stopped"), session().put("display_generation", "other"))) {
            failure<DriverUncertainException> { driver(envelope(data = data)).inspectSession("session", "req") }
        }
        failure<DriverUncertainException> { driver(envelope(data = session())).inspectSession("different", "req") }
        Unit
    }

    @Test fun invalidInputsNeverReachTransport() = runBlocking {
        var calls = 0
        val sdk = AndroidDriver(DriverTransport { _, _, _, _ -> calls++; envelope() })
        for (options in listOf(SessionOptions(emptyList()), SessionOptions(List(9) { "com.example.app" }),
            SessionOptions(listOf("bad")), SessionOptions(listOf("com.example.app"), width = 319),
            SessionOptions(listOf("com.example.app"), height = 2401),
            SessionOptions(listOf("com.example.app"), density = 119),
            SessionOptions(listOf("com.example.app"), label = "x".repeat(129)))) {
            failure<IllegalArgumentException> { sdk.createSession(options, "req") }
        }
        failure<IllegalArgumentException> { sdk.tap("session", "snapshot", -1, 0, "req") }
        failure<IllegalArgumentException> { sdk.tap("session", "snapshot", 1920, 0, "req") }
        failure<IllegalArgumentException> { sdk.swipe("session", "snapshot", 0, 0, 0, 2400, 300, "req") }
        failure<IllegalArgumentException> { sdk.swipe("session", "snapshot", 0, 0, 1, 1, 1001, "req") }
        failure<IllegalArgumentException> { sdk.inspectSession("", "req") }
        failure<IllegalArgumentException> { sdk.capabilities("x".repeat(129)) }
        assertEquals(0, calls)
    }

    @Test fun frameAndPreviewHaveDistinctSnapshotSemantics() = runBlocking {
        val bytes = byteArrayOf(-119, 80, 78, 71, 13, 10, 26, 10, 0)
        val frame = JSONObject().put("snapshot_id", "snapshot").put("target_id", "target")
            .put("display_id", 2).put("width", 1080).put("height", 1920).put("rotation", 0)
            .put("frame_age_ms", 50).put("image_base64", java.util.Base64.getEncoder().encodeToString(bytes))
        val result = driver(envelope(data = frame)).snapshot("session", "target", "req")
        assertArrayEquals(bytes, result.data.png)
        assertEquals("snapshot", result.data.snapshotId)
        failure<DriverUncertainException> { driver(envelope(data = frame)).preview("session", "target", "req") }
        frame.put("snapshot_id", JSONObject.NULL).put("width", 270).put("height", 480)
        assertNull(driver(envelope(data = frame)).preview("session", "target", "req").data.snapshotId)
        failure<DriverUncertainException> { driver(envelope(data = frame)).snapshot("session", "target", "req") }
        frame.put("image_base64", "not png")
        failure<DriverUncertainException> { driver(envelope(data = frame)).preview("session", "target", "req") }
        Unit
    }

    @Test fun capabilitiesPreserveUnverifiedAndUnsupportedStates() = runBlocking {
        val caps = JSONArray().put(JSONObject().put("capability", "touch.background").put("support", "conditional")
            .put("authorization", "granted").put("readiness", "ready")
            .put("conditions", JSONArray().put("validated_session_target"))
            .put("qualification", JSONObject().put("status", "unverified")))
            .put(JSONObject().put("capability", "text.semantic").put("support", "unsupported")
                .put("authorization", "unknown").put("readiness", "blocked").put("reason", "not_qualified_in_initial_slice"))
        val result = driver(envelope(data = JSONObject().put("capabilities", caps))).capabilities("req")
        assertEquals("unverified", result.data[0].qualificationStatus)
        assertEquals("unsupported", result.data[1].support)
        assertNull(result.data[1].qualificationStatus)
    }

    @Test fun transportFailureNeverRetries() = runBlocking {
        var calls = 0
        val sdk = AndroidDriver(DriverTransport { _, _, _, _ -> calls++; throw java.io.IOException("response lost") })
        val error = failure<DriverUncertainException> { sdk.stopSession("session", "req") }
        assertEquals("req", error.requestId)
        assertEquals(1, calls)
    }

    @Test fun cancellationAfterDispatchPreservesIdentityWithoutRetry() = runBlocking {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val observed = AtomicReference<DriverRequestCancelledException>()
        var calls = 0
        val sdk = AndroidDriver(DriverTransport { _, _, _, _ ->
            calls++; entered.countDown()
            check(release.await(5, TimeUnit.SECONDS))
            envelope().put("action", action())
        })
        val job = async(kotlinx.coroutines.Dispatchers.Default) {
            try { sdk.tap("session", "snapshot", 10, 20, "req") }
            catch (error: DriverRequestCancelledException) { observed.set(error); throw error }
        }
        try {
            assertTrue(entered.await(5, TimeUnit.SECONDS))
            job.cancel()
        } finally { release.countDown() }
        job.cancelAndJoin()
        assertEquals("req", observed.get().requestId)
        assertTrue(observed.get().mayHaveBeenDispatched)
        assertEquals(1, calls)
    }
}
