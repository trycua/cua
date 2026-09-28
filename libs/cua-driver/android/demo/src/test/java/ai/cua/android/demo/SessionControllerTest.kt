package ai.cua.android.demo

import ai.cua.driver.sdk.AppTarget
import ai.cua.driver.sdk.DisplayFrame
import ai.cua.driver.sdk.DriverResult
import ai.cua.driver.sdk.SessionInfo
import ai.cua.driver.sdk.StopInfo
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SessionControllerTest {
    private class Operations : SessionOperations {
        val calls = mutableListOf<String>()
        var created = 0
        var createGate: CompletableDeferred<Unit>? = null
        var stopGate: CompletableDeferred<Unit>? = null
        var previewGate: CompletableDeferred<Unit>? = null
        var createFailure: Exception? = null
        var stopFailure: Exception? = null
        var previewGeneration = "runtime-1"
        var stopGeneration = "runtime-1"
        var stopInfo = StopInfo("stopped", "released")

        private fun session(id: String) = SessionInfo(id, 5, 1080, 1920, 30_000, null)
        private fun <T> result(data: T, generation: String = "runtime-1") =
            DriverResult("request", generation, data)

        override suspend fun create(): DriverResult<SessionInfo> {
            created++
            calls += "create:$created"
            createGate?.await()
            createFailure?.let { throw it }
            return result(session("session-$created"))
        }

        override suspend fun launch(sessionId: String): DriverResult<AppTarget> {
            calls += "launch:$sessionId"
            return result(AppTarget("target", 7, 5, "ai.cua.fixture.notes"))
        }

        override suspend fun preview(sessionId: String, targetId: String): DriverResult<DisplayFrame> {
            calls += "preview:$sessionId"
            previewGate?.await()
            return result(DisplayFrame(null, targetId, 5, 1080, 1920, 0, 0, byteArrayOf(1)),
                previewGeneration)
        }

        override suspend fun renew(sessionId: String): DriverResult<SessionInfo> {
            calls += "renew:$sessionId"
            return result(session(sessionId))
        }

        override suspend fun stop(sessionId: String): DriverResult<StopInfo> {
            calls += "stop:$sessionId"
            stopGate?.await()
            stopFailure?.let { throw it }
            return result(stopInfo, stopGeneration)
        }
    }

    private class Harness(testScope: TestScope, val operations: Operations = Operations()) {
        val states = mutableListOf<DemoSessionState>()
        var idleCount = 0
        val owner = CoroutineScope(testScope.coroutineContext + SupervisorJob(testScope.coroutineContext[kotlinx.coroutines.Job]))
        val controller = SessionController(operations, owner,
            onState = { states += it }, onIdle = { idleCount++ },
            nowMs = { testScope.testScheduler.currentTime })
        val state: DemoSessionState get() = states.last()
        fun close() = owner.cancel()
    }

    @Test fun stopDrainsHeldCreationAndReleasesExactlyOnceWithoutLaunching() = runTest {
        val h = Harness(this)
        val gate = CompletableDeferred<Unit>()
        h.operations.createGate = gate
        h.controller.start()
        runCurrent()
        h.controller.requestStop()
        h.controller.requestStop()
        runCurrent()
        assertEquals(listOf("create:1"), h.operations.calls)
        assertEquals("Stopping", h.state.status)
        gate.complete(Unit)
        runCurrent()
        assertEquals(listOf("create:1", "stop:session-1"), h.operations.calls)
        assertEquals("Stopped", h.state.status)
        assertEquals(1, h.idleCount)
        h.close()
    }

    @Test fun startDuringCleanupQueuesExactlyOneSessionWithoutGoingIdle() = runTest {
        val h = Harness(this)
        h.controller.start()
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        h.operations.stopGate = gate
        h.controller.requestStop()
        runCurrent()
        h.controller.start()
        h.controller.start()
        assertEquals(1, h.operations.created)
        assertEquals(0, h.idleCount)
        gate.complete(Unit)
        runCurrent()
        assertEquals(2, h.operations.created)
        assertTrue(h.operations.calls.indexOf("stop:session-1") < h.operations.calls.indexOf("create:2"))
        assertEquals("session-2", h.state.sessionId)
        assertEquals("Running", h.state.status)
        assertEquals(0, h.idleCount)
        h.controller.requestStop()
        runCurrent()
        assertEquals(1, h.idleCount)
        h.close()
    }

    @Test fun stopCancelsQueuedRestart() = runTest {
        val h = Harness(this)
        h.controller.start()
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        h.operations.stopGate = gate
        h.controller.requestStop()
        runCurrent()
        h.controller.start()
        h.controller.requestStop()
        gate.complete(Unit)
        runCurrent()
        assertEquals(1, h.operations.created)
        assertEquals("Stopped", h.state.status)
        assertEquals(1, h.idleCount)
        h.close()
    }

    @Test fun failedCleanupNeverPublishesStoppedOrRunsQueuedRestart() = runTest {
        val h = Harness(this)
        h.controller.start()
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        h.operations.stopGate = gate
        h.operations.stopFailure = IllegalStateException("release uncertain")
        h.controller.requestStop()
        runCurrent()
        h.controller.start()
        gate.complete(Unit)
        runCurrent()
        assertEquals(1, h.operations.created)
        assertTrue(h.state.status.startsWith("Error: Cleanup unverified:"))
        assertFalse(h.states.any { it.status == "Stopped" })
        assertEquals(1, h.idleCount)
        h.close()
    }

    @Test fun duplicateStartKeepsSessionWhilePreviewsAndLeaseRenewalsContinue() = runTest {
        val h = Harness(this)
        h.controller.start()
        runCurrent()
        h.controller.start()
        assertEquals(1, h.state.previewFrames)
        advanceTimeBy(10_000)
        runCurrent()
        assertEquals(1, h.operations.created)
        assertEquals(11, h.state.previewFrames)
        assertEquals(1, h.state.renewals)
        assertEquals(1, h.operations.calls.count { it == "renew:session-1" })
        h.controller.requestStop()
        runCurrent()
        assertEquals("Stopped", h.state.status)
        assertNull(h.state.preview)
        assertEquals(10_000, testScheduler.currentTime)
        h.close()
    }

    @Test fun runtimeChangeTriggersCleanupAndNeverSilentlyResumes() = runTest {
        val h = Harness(this)
        h.operations.previewGeneration = "runtime-2"
        h.operations.stopGeneration = "runtime-2"
        h.controller.start()
        runCurrent()
        assertTrue(h.state.status.startsWith("Error:"))
        assertTrue(h.state.status.contains("Runtime changed"))
        assertEquals(0, h.state.previewFrames)
        assertEquals(1, h.operations.calls.count { it == "stop:session-1" })
        advanceTimeBy(30_000)
        runCurrent()
        assertEquals(1, h.operations.created)
        assertFalse(h.states.any { it.status == "Stopped" })
        h.close()
    }

    @Test fun unknownCreateFailureNeverRetriesOrClaimsStopped() = runTest {
        val h = Harness(this)
        h.operations.createFailure = IllegalStateException("completion unknown")
        h.controller.start()
        runCurrent()
        advanceTimeBy(30_000)
        runCurrent()
        assertEquals(listOf("create:1"), h.operations.calls)
        assertEquals("Error: completion unknown", h.state.status)
        assertFalse(h.states.any { it.status == "Stopped" })
        assertEquals(1, h.idleCount)
        h.close()
    }

    @Test fun ownerCancellationWaitsForNonCancellableCleanupWithoutRestarting() = runTest {
        val h = Harness(this)
        h.controller.start()
        runCurrent()
        val gate = CompletableDeferred<Unit>()
        h.operations.stopGate = gate
        h.close()
        runCurrent()
        assertEquals(1, h.operations.calls.count { it == "stop:session-1" })
        assertEquals(0, h.idleCount)
        h.controller.start()
        gate.complete(Unit)
        runCurrent()
        assertTrue(h.state.status.startsWith("Error: Controller destroyed"))
        assertEquals(1, h.operations.created)
        assertEquals(1, h.idleCount)
    }

    @Test fun stopDrainsPreviewAndDoesNotPublishItsLateFrame() = runTest {
        val h = Harness(this)
        val gate = CompletableDeferred<Unit>()
        h.operations.previewGate = gate
        h.controller.start()
        runCurrent()
        h.controller.requestStop()
        runCurrent()
        assertFalse(h.operations.calls.any { it.startsWith("stop:") })
        gate.complete(Unit)
        runCurrent()
        assertEquals(0, h.state.previewFrames)
        assertEquals("Stopped", h.state.status)
        assertEquals(1, h.operations.calls.count { it == "stop:session-1" })
        h.close()
    }

    @Test fun unconfirmedStopPayloadIsAnError() = runTest {
        val h = Harness(this)
        h.operations.stopInfo = StopInfo("stopped", "pending")
        h.controller.start()
        runCurrent()
        h.controller.requestStop()
        runCurrent()
        assertTrue(h.state.status.startsWith("Error: Cleanup unverified:"))
        assertFalse(h.states.any { it.status == "Stopped" })
        h.close()
    }
}
