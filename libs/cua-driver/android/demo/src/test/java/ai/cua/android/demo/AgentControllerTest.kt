package ai.cua.android.demo

import ai.cua.driver.sdk.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import org.junit.Assert.*
import org.junit.Test
import org.json.JSONObject

@OptIn(ExperimentalCoroutinesApi::class)
class AgentControllerTest {
    private fun frame(id: String = "snap", pixel: Byte = 1) = DisplayFrame(id, "target", 5, 540, 960, 0, 0, byteArrayOf(pixel))
    private class Ops : AgentOperations {
        val calls = mutableListOf<String>()
        var createGate: CompletableDeferred<Unit>? = null
        var actGate: CompletableDeferred<Unit>? = null
        var changed = false
        var pixelAt: ((Int) -> Byte)? = null
        var transformFrame: (Int, DisplayFrame) -> DisplayFrame = { _, value -> value }
        var changedGenerationAt: Int? = null
        var created = 0
        var uncertain = false
        var snapshots = 0
        fun <T> result(data: T) = DriverResult("request", "generation", data)
        override suspend fun create(): DriverResult<SessionInfo> {
            calls += "create"; created++; createGate?.await()
            return result(SessionInfo("session-$created", 5, 540, 960, 30000, null))
        }
        override suspend fun launch(sessionId: String, packageName: String): DriverResult<AppTarget> {
            calls += "launch:$packageName"; return result(AppTarget("target", 1, 5, packageName))
        }
        override suspend fun snapshot(sessionId: String, targetId: String): DriverResult<DisplayFrame> {
            snapshots++
            val frame = DisplayFrame("snap-$snapshots", targetId, 5, 540, 960, 0, 0,
                byteArrayOf(pixelAt?.invoke(snapshots) ?: if (changed) snapshots.toByte() else 1))
            return DriverResult("request", if (changedGenerationAt?.let { snapshots >= it } == true) "other-generation" else "generation",
                transformFrame(snapshots, frame))
        }
        override suspend fun preview(sessionId: String, targetId: String) = result(DisplayFrame(null, targetId, 5, 540, 960, 0, 0, byteArrayOf(1)))
        override suspend fun renew(sessionId: String): DriverResult<SessionInfo> {
            calls += "renew"; return result(SessionInfo(sessionId, 5, 540, 960, 30000, "target"))
        }
        override suspend fun act(sessionId: String, snapshotId: String, action: AgentAction): DriverResult<Unit> {
            calls += "act:$snapshotId"; actGate?.await()
            if (uncertain) throw DriverUncertainException("request", "generation", "unknown")
            return result(Unit)
        }
        override suspend fun stop(sessionId: String): DriverResult<StopInfo> {
            calls += "stop"; return result(StopInfo("stopped", "released"))
        }
    }
    private class Harness(test: TestScope, model: AgentModel, val ops: Ops = Ops(), steps: Int = 40) {
        val states = mutableListOf<DemoSessionState>()
        val scope = CoroutineScope(test.coroutineContext + SupervisorJob(test.coroutineContext[Job]))
        var idle = false
        var idleCount = 0
        val controller = AgentController(ops, model, AGENT_APPS, scope,
            { IntArray(it.width * it.height) { _ -> it.png[0].toInt() } }, { states += it }, { idle = true; idleCount++ },
            { test.testScheduler.currentTime }, maxSteps = steps)
        val state get() = states.last()
    }
    @Test fun parserRejectsUncorrelatedUnallowlistedAndInvalidInput() {
        fun parse(action: String, id: String = "r") = AgentAction.parse("""{"request_id":"$id","action":$action}""", "r", AGENT_APPS, 540, 960)
        assertEquals("tap", parse("""{"type":"tap","x":1,"y":2,"reason":"visible"}""").type)
        for (action in listOf(
            """{"type":"tap","x":540,"y":2,"reason":"bad"}""",
            """{"type":"tap","x":1.0,"y":2,"reason":"bad"}""",
            """{"type":"launch","package":"com.other.app","reason":"bad"}""",
            """{"type":"swipe","from_x":0,"from_y":0,"to_x":2,"to_y":2,"duration_ms":1001,"reason":"bad"}""",
            """{"type":"shell","reason":"bad"}""")) {
            assertThrows(Exception::class.java) { parse(action) }
        }
        assertThrows(Exception::class.java) { parse("""{"type":"done","reason":"ok"}""", "other") }
    }
    @Test fun configOnlyAdmitsPrivateLoopbackAndKnownApps() {
        val base = JSONObject().put("endpoint", "http://127.0.0.1:8788/decide").put("token", "synthetic").put("task", "Do a synthetic task")
        assertEquals(AGENT_APPS, AgentConfig.parse(base.toString()).allowedApps)
        assertThrows(Exception::class.java) { AgentConfig.parse(base.put("endpoint", "http://example.com/decide").toString()) }
    }
    @Test fun freshGateComparesDecodedPixelsAndIdentityAndRejectsStaleFrames() {
        val a = frame("old")
        fun same(b: DisplayFrame, gen: String = "g") = sameAgentFrame(a, b, "g", gen) { IntArray(it.width * it.height) { _ -> it.png[0].toInt() } }
        assertTrue(same(frame("new")))
        assertFalse(same(frame("new", 2)))
        assertFalse(same(frame("new").copy(frameAgeMs = 5001)))
        assertFalse(same(frame("new").copy(targetId = "other")))
        assertFalse(same(frame("new").copy(displayId = 7)))
        assertFalse(same(frame("new").copy(rotation = 1)))
        assertFalse(same(frame("new").copy(width = 600)))
        assertFalse(same(frame("new"), "other"))
    }
    @Test fun unknownConfigResponseAndActionFieldsAreRejected() {
        val config = JSONObject().put("endpoint", "http://127.0.0.1:8788/decide").put("token", "synthetic").put("task", "Synthetic task")
        assertThrows(Exception::class.java) { AgentConfig.parse(config.put("extra", true).toString()) }
        val validActions = listOf(
            AgentAction("done", "finished"), AgentAction("blocked", "Cannot continue"), AgentAction("tap", "visible", x = 1, y = 2),
            AgentAction("launch", "switch", AGENT_APPS.first()), AgentAction("swipe", "scroll", x = 1, y = 2, toX = 3, toY = 4))
        for (action in validActions) {
            val response = JSONObject().put("request_id", "r").put("action", action.json())
            assertEquals(action, AgentAction.parse(response.toString(), "r", AGENT_APPS, 540, 960))
            assertThrows(Exception::class.java) {
                AgentAction.parse(response.put("extra", true).toString(), "r", AGENT_APPS, 540, 960)
            }
            response.remove("extra")
            response.put("action", action.json().put("extra", true))
            assertThrows(Exception::class.java) { AgentAction.parse(response.toString(), "r", AGENT_APPS, 540, 960) }
        }
    }
    @Test fun emptyOrPartialDecodedPixelsCannotPassFreshFrameGate() {
        assertFalse(sameAgentFrame(frame("old"), frame("new"), "g", "g") { intArrayOf() })
        assertFalse(sameAgentFrame(frame("old"), frame("new"), "g", "g") { intArrayOf(1) })
    }
    @Test fun completedRunRetainsVerifiedFinalFrameWithClosedWorkspaceLabel() = runTest {
        val h = Harness(this, AgentModel { _, _, _, _ -> AgentAction("done", "Visible result confirmed") })
        h.controller.start(); advanceTimeBy(1_001); runCurrent()
        assertTrue(h.idle)
        assertEquals("Stopped", h.state.status)
        assertEquals("done", h.state.phase)
        assertEquals("done", h.state.modelStatus)
        assertEquals("snap-2", h.state.preview?.snapshotId)
        assertEquals("Last frame · workspace closed", h.state.previewLabel)
        assertEquals(1, h.ops.calls.count { it == "stop" })
        h.scope.cancel()
    }
    @Test fun blockedDecisionReleasesWorkspaceWithoutReportingCompletionOrSavingFinalImage() = runTest {
        val h = Harness(this, AgentModel { _, _, _, _ -> AgentAction("blocked", "Required control is unavailable") })
        h.controller.start(); advanceTimeBy(1_001); runCurrent()
        assertTrue(h.idle)
        assertEquals("Blocked", h.state.status)
        assertEquals("blocked", h.state.phase)
        assertEquals("blocked", h.state.modelStatus)
        assertEquals("Required control is unavailable", h.state.lastReason)
        assertNull(h.state.preview)
        assertNull(h.state.finalFrameFile)
        assertFalse(h.states.any { it.phase == "done" || it.modelStatus == "done" })
        assertEquals(1, h.ops.calls.count { it == "stop" })
        h.scope.cancel()
    }
    @Test fun stopDuringCreateDrainsAndReleasesWithoutLaunch() = runTest {
        val ops = Ops().apply { createGate = CompletableDeferred() }
        val h = Harness(this, AgentModel { _, _, _, _ -> error("unexpected inference") }, ops)
        h.controller.start(); runCurrent(); h.controller.requestStop(); runCurrent()
        assertEquals(listOf("create"), ops.calls)
        ops.createGate!!.complete(Unit); runCurrent()
        assertEquals(listOf("create", "stop"), ops.calls)
        assertTrue(h.idle); h.scope.cancel()
    }
    @Test fun renewsAndPreviewsDuringSlowInferenceAndStopCancelsItPromptly() = runTest {
        var cancelled = false
        val h = Harness(this, AgentModel { _, _, _, _ ->
            try { awaitCancellation() } finally { cancelled = true }
        })
        h.controller.start(); runCurrent(); advanceTimeBy(24_001); runCurrent()
        assertEquals(3, h.state.renewals)
        assertTrue(h.state.previewFrames >= 20)
        h.controller.requestStop(); runCurrent()
        assertTrue(cancelled); assertEquals(1, h.ops.calls.count { it == "stop" })
        assertEquals("Stopped", h.state.status); assertTrue(h.idle); h.scope.cancel()
    }
    @Test fun unchangedSlowFrameUsesNewSnapshotAndUncertainActionIsNeverRetried() = runTest {
        val ops = Ops().apply { uncertain = true }
        val h = Harness(this, AgentModel { _, _, _, _ -> delay(12_000); AgentAction("tap", "visible", x = 2, y = 3) }, ops)
        h.controller.start(); advanceTimeBy(13_001); runCurrent()
        assertEquals(listOf("act:snap-2"), ops.calls.filter { it.startsWith("act:") })
        assertTrue(h.state.status.startsWith("Error:")); assertTrue(h.idle); h.scope.cancel()
    }
    @Test fun changedFramesReplanAndConsumeStepBudgetWithoutActing() = runTest {
        var decisions = 0
        val ops = Ops().apply { changed = true }
        val h = Harness(this, AgentModel { _, _, _, _ -> decisions++; AgentAction("tap", "visible", x = 2, y = 3) }, ops, 3)
        h.controller.start(); advanceTimeBy(2_801); runCurrent()
        assertEquals(18, ops.snapshots)
        assertEquals(3, decisions); assertFalse(ops.calls.any { it.startsWith("act:") })
        assertTrue(h.state.status.startsWith("Error:")); assertTrue(h.idle); h.scope.cancel()
    }
    @Test fun laterExactMatchUsesLastFreshHandleWithoutInputDuringWait() = runTest {
        val ops = Ops().apply {
            pixelAt = { if (it in 2..5) 2 else 1 }
            uncertain = true
        }
        var decisions = 0
        val h = Harness(this, AgentModel { _, _, _, _ -> decisions++; AgentAction("tap", "visible", x = 2, y = 3) }, ops)
        h.controller.start(); advanceTimeBy(1_599); runCurrent()
        assertEquals(5, ops.snapshots)
        assertFalse(ops.calls.any { it.startsWith("act:") })
        advanceTimeBy(2); runCurrent()
        assertEquals(listOf("act:snap-6"), ops.calls.filter { it.startsWith("act:") })
        assertEquals(1, decisions)
        assertTrue(h.idle)
        h.scope.cancel()
    }
    @Test fun stopDuringExactMatchWaitCancelsBeforeAnotherCaptureOrInput() = runTest {
        val ops = Ops().apply { changed = true }
        val h = Harness(this, AgentModel { _, _, _, _ -> AgentAction("tap", "visible", x = 2, y = 3) }, ops)
        h.controller.start(); advanceTimeBy(1_001); runCurrent()
        assertEquals(2, ops.snapshots)
        h.controller.requestStop(); advanceTimeBy(1_000); runCurrent()
        assertEquals(2, ops.snapshots)
        assertFalse(ops.calls.any { it.startsWith("act:") })
        assertEquals(1, ops.calls.count { it == "stop" })
        assertTrue(h.idle)
        h.scope.cancel()
    }
    @Test fun identityChangesDuringPollingNeverAdmitInputEvenIfPixelsCouldMatchLater() = runTest {
        val transforms: List<(DisplayFrame) -> DisplayFrame> = listOf(
            { it.copy(targetId = "other") }, { it.copy(displayId = 9) },
            { it.copy(width = 541) }, { it.copy(height = 961) }, { it.copy(rotation = 1) })
        for (transform in transforms) {
            val ops = Ops().apply {
                pixelAt = { if (it == 2) 2 else 1 }
                transformFrame = { index, value -> if (index == 3) transform(value) else value }
            }
            val h = Harness(this, AgentModel { _, _, _, _ -> AgentAction("tap", "visible", x = 2, y = 3) }, ops)
            h.controller.start(); advanceTimeBy(1_151); runCurrent()
            assertEquals(3, ops.snapshots)
            assertFalse(ops.calls.any { it.startsWith("act:") })
            assertTrue(h.state.status.startsWith("Error:")); assertTrue(h.idle)
            h.scope.cancel()
        }
        val ops = Ops().apply { pixelAt = { if (it == 2) 2 else 1 }; changedGenerationAt = 3 }
        val h = Harness(this, AgentModel { _, _, _, _ -> AgentAction("tap", "visible", x = 2, y = 3) }, ops)
        h.controller.start(); advanceTimeBy(1_151); runCurrent()
        assertEquals(3, ops.snapshots)
        assertFalse(ops.calls.any { it.startsWith("act:") })
        assertTrue(h.state.status.startsWith("Error:")); assertTrue(h.idle)
        h.scope.cancel()
    }
    @Test fun startAfterInferenceFailureCreatesNewSessionAndReturnsIdleAgain() = runTest {
        var decisions = 0
        val h = Harness(this, AgentModel { _, _, _, _ ->
            if (++decisions == 1) error("Synthetic model error")
            AgentAction("done", "Visible result confirmed")
        })
        h.controller.start(); advanceTimeBy(1_001); runCurrent()
        assertEquals(1, h.idleCount)
        assertEquals("session-1", h.state.sessionId)
        assertTrue(h.state.status.startsWith("Error:"))
        h.idle = false
        h.controller.start(); advanceTimeBy(1_001); runCurrent()
        assertEquals(2, h.idleCount)
        assertEquals(2, h.ops.created)
        assertEquals("session-2", h.state.sessionId)
        assertEquals("done", h.state.phase)
        assertEquals(2, h.ops.calls.count { it == "stop" })
        h.scope.cancel()
    }
    @Test fun queuedPreviewRechecksTargetAfterQueuedSwitchInvalidatesOldTarget() = runTest {
        val gate = AgentRuntimeGate()
        val old = AppTarget("old-target", 1, 5, AGENT_APPS.first())
        val fresh = AppTarget("new-target", 2, 5, AGENT_APPS.last())
        gate.launch { old }
        val heldMutation = CompletableDeferred<Unit>()
        val mutation = launch { gate.drain { heldMutation.await() } }
        runCurrent()
        val switching = launch { gate.launch { fresh } }
        runCurrent()
        // The switch is queued behind a held mutation. The preview observes old ownership
        // before queuing behind that switch, reproducing the stale-target dispatch window.
        val observed = requireNotNull(gate.target)
        assertEquals(old, observed)
        var issuedPreviews = 0
        val preview = async { gate.preview(observed) { issuedPreviews++; error("Old target must not reach runtime") } }
        runCurrent()
        heldMutation.complete(Unit)
        mutation.join(); switching.join()
        assertNull(preview.await())
        assertEquals(0, issuedPreviews)
        assertEquals(fresh, gate.target)
        assertEquals("fresh-preview", gate.preview(fresh) { issuedPreviews++; "fresh-preview" })
        assertEquals(1, issuedPreviews)
        val runtimeError = IllegalStateException("Real runtime error must propagate")
        try {
            gate.preview(fresh) { throw runtimeError }
            fail("Runtime errors must propagate")
        } catch (actual: IllegalStateException) {
            assertEquals(runtimeError.message, actual.message)
        }
    }
    @Test fun stopDuringInputDrainsBeforeRelease() = runTest {
        val ops = Ops().apply { actGate = CompletableDeferred() }
        val h = Harness(this, AgentModel { _, _, _, _ -> AgentAction("tap", "visible", x = 2, y = 3) }, ops)
        h.controller.start(); advanceTimeBy(1_001); runCurrent(); h.controller.requestStop(); runCurrent()
        assertFalse("stop" in ops.calls)
        ops.actGate!!.complete(Unit); runCurrent()
        assertEquals("stop", ops.calls.last()); assertTrue(h.idle); h.scope.cancel()
    }
    @Test fun totalTimeoutStopsInferenceAndReleasesSession() = runTest {
        val h = Harness(this, AgentModel { _, _, _, _ -> awaitCancellation() })
        h.controller.start(); advanceTimeBy(600_001); runCurrent()
        assertEquals("Error: Agent time limit reached", h.state.status)
        assertEquals(1, h.ops.calls.count { it == "stop" }); assertTrue(h.idle); h.scope.cancel()
    }
}
