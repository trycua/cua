package ai.cua.android.demo

import ai.cua.driver.sdk.AppTarget
import ai.cua.driver.sdk.DisplayFrame
import ai.cua.driver.sdk.DriverResult
import ai.cua.driver.sdk.SessionInfo
import ai.cua.driver.sdk.StopInfo
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

data class DemoSessionState(
    val ownerGeneration: String = UUID.randomUUID().toString(),
    val status: String = "Stopped",
    val sessionId: String? = null,
    val runtimeGeneration: String? = null,
    val displayId: Int? = null,
    val targetId: String? = null,
    val taskId: Int? = null,
    val preview: DisplayFrame? = null,
    val previewFrames: Int = 0,
    val renewals: Int = 0,
    val mode: String = "fixture",
    val phase: String = "idle",
    val lastReason: String = "",
    val agentSteps: Int = 0,
    val modelStatus: String = "idle",
    val currentPackage: String? = null,
    val previewLabel: String = "Live preview",
    val finalFrameFile: String? = null,
)

internal interface SessionOperations {
    suspend fun create(): DriverResult<SessionInfo>
    suspend fun launch(sessionId: String): DriverResult<AppTarget>
    suspend fun preview(sessionId: String, targetId: String): DriverResult<DisplayFrame>
    suspend fun renew(sessionId: String): DriverResult<SessionInfo>
    suspend fun stop(sessionId: String): DriverResult<StopInfo>
}

/** Calls and callbacks are confined to the owner's coroutine dispatcher. */
internal class SessionController(
    private val operations: SessionOperations,
    private val scope: CoroutineScope,
    initialState: DemoSessionState = DemoSessionState(),
    private val onState: (DemoSessionState) -> Unit,
    private val onIdle: () -> Unit,
    private val nowMs: () -> Long = { System.nanoTime() / 1_000_000 },
) {
    private var state = initialState
    private var job: Job? = null
    private var stopping = false
    private var restartRequested = false
    private var stopSignal = CompletableDeferred<Unit>()

    fun start() {
        if (job != null) {
            if (stopping) restartRequested = true
            return
        }
        if (!scope.isActive) return
        beginSession()
        // Store ownership before an immediate dispatcher can run the coroutine to completion.
        job = scope.launch(start = CoroutineStart.LAZY) {
            try {
                do {
                    val successfulCleanup = runSession()
                    val restart = successfulCleanup && restartRequested && isActive
                    restartRequested = false
                    if (!restart) break
                    beginSession()
                } while (true)
            } finally {
                job = null
                restartRequested = false
                onIdle()
            }
        }
        job!!.start()
    }

    fun requestStop() {
        restartRequested = false
        if (job == null) {
            onIdle()
            return
        }
        stopping = true
        publish(state.copy(status = "Stopping"))
        stopSignal.complete(Unit)
    }

    private fun beginSession() {
        stopping = false
        stopSignal = CompletableDeferred()
        publish(DemoSessionState(ownerGeneration = state.ownerGeneration, status = "Starting"))
    }

    private suspend fun runSession(): Boolean {
        var sessionId: String? = null
        var runtimeGeneration: String? = null
        var failure: String? = null
        fun <T> sameRuntime(result: DriverResult<T>): T {
            check(result.runtimeGeneration == runtimeGeneration) {
                "Runtime changed; session is no longer usable"
            }
            return result.data
        }
        try {
            // Explicit Stop drains creation so a returned session can always be released.
            val created = operations.create()
            sessionId = created.data.sessionId
            runtimeGeneration = created.runtimeGeneration
            publish(state.copy(sessionId = sessionId, runtimeGeneration = runtimeGeneration,
                displayId = created.data.displayId))
            if (!stopping) {
                val target = sameRuntime(operations.launch(sessionId))
                publish(state.copy(targetId = target.targetId, taskId = target.taskId,
                    status = if (stopping) "Stopping" else "Running"))
                var lastRenew = nowMs()
                while (!stopping) {
                    if (nowMs() - lastRenew >= 10_000) {
                        sameRuntime(operations.renew(sessionId))
                        lastRenew = nowMs()
                        publish(state.copy(renewals = state.renewals + 1))
                    }
                    if (stopping) break
                    val frame = sameRuntime(operations.preview(sessionId, target.targetId))
                    if (!stopping) {
                        publish(state.copy(preview = frame, previewFrames = state.previewFrames + 1))
                        withTimeoutOrNull(1_000) { stopSignal.await() }
                    }
                }
            }
        } catch (error: CancellationException) {
            failure = "Controller destroyed; cleanup requested"
        } catch (error: Exception) {
            failure = error.message ?: error.javaClass.simpleName
        } finally {
            stopping = true
            publish(state.copy(status = "Stopping"))
            if (sessionId != null) {
                try {
                    withContext(NonCancellable) {
                        val stopped = sameRuntime(operations.stop(sessionId))
                        check(stopped.state == "stopped" && stopped.cleanup == "released") {
                            "Session release was not confirmed"
                        }
                    }
                } catch (error: Exception) {
                    failure = "Cleanup unverified: ${error.message ?: error.javaClass.simpleName}"
                }
            }
            publish(state.copy(status = failure?.let { "Error: $it" } ?: "Stopped", preview = null))
        }
        return failure == null
    }

    private fun publish(value: DemoSessionState) {
        state = value
        onState(value)
    }
}
