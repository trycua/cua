package ai.cua.android.demo

import ai.cua.driver.sdk.*
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import java.util.UUID

internal interface AgentOperations {
    suspend fun create(): DriverResult<SessionInfo>
    suspend fun launch(sessionId: String, packageName: String): DriverResult<AppTarget>
    suspend fun snapshot(sessionId: String, targetId: String): DriverResult<DisplayFrame>
    suspend fun preview(sessionId: String, targetId: String): DriverResult<DisplayFrame>
    suspend fun renew(sessionId: String): DriverResult<SessionInfo>
    suspend fun act(sessionId: String, snapshotId: String, action: AgentAction): DriverResult<Unit>
    suspend fun stop(sessionId: String): DriverResult<StopInfo>
}
internal fun interface AgentModel {
    suspend fun decide(requestId: String, frame: DisplayFrame, currentPackage: String, history: List<AgentAction>): AgentAction
}

/** Serializes runtime ownership changes with the preview check immediately before dispatch. */
internal class AgentRuntimeGate {
    private val lock = Mutex()
    var target: AppTarget? = null
        private set

    suspend fun <T> drain(call: suspend () -> T): T = withContext(NonCancellable) { lock.withLock { call() } }

    suspend fun launch(call: suspend () -> AppTarget): AppTarget = drain {
        target = null
        call().also { target = it }
    }

    suspend fun <T> preview(expected: AppTarget, call: suspend () -> T): T? = drain {
        if (target != expected) null else call()
    }
}

/** Runtime calls drain before cancellation is observed; inference remains cancellable. */
internal class AgentController(private val operations: AgentOperations, private val model: AgentModel,
    private val apps: List<String>, private val scope: CoroutineScope,
    private val pixels: (DisplayFrame) -> IntArray, private val onState: (DemoSessionState) -> Unit,
    private val onIdle: () -> Unit, private val nowMs: () -> Long = { System.nanoTime() / 1_000_000 },
    private val maxSteps: Int = 40, private val maxDurationMs: Long = 600_000) {
    private var job: Job? = null
    private var state = DemoSessionState(mode = "agent")
    fun start() {
        if (job != null || !scope.isActive) return
        state = DemoSessionState(mode = "agent", status = "Starting", phase = "creating", modelStatus = "idle")
        onState(state)
        job = scope.launch(start = CoroutineStart.LAZY) { run(); job = null; onIdle() }
        job!!.start()
    }
    fun requestStop() { job?.cancel(); if (job != null) publish("Stopping", "stopping") }
    private fun publish(status: String = state.status, phase: String = state.phase) {
        state = state.copy(status = status, phase = phase); onState(state)
    }
    private suspend fun run() {
        var session: String? = null
        var generation: String? = null
        var failure: String? = null
        var completed = false
        var blocked = false
        var finalFrame: DisplayFrame? = null
        fun <T> checked(result: DriverResult<T>): T {
            check(result.runtimeGeneration == generation) { "Runtime generation changed" }
            return result.data
        }
        val runtime = AgentRuntimeGate()
        suspend fun <T> drain(call: suspend () -> T): T = runtime.drain(call)
        try {
            // Save the created ID inside the non-cancellable region, before returning to a cancelled owner.
            drain {
                val result = operations.create()
                session = result.data.sessionId; generation = result.runtimeGeneration
                state = state.copy(sessionId = session, runtimeGeneration = generation, displayId = result.data.displayId)
            }
            currentCoroutineContext().ensureActive()
            val id = requireNotNull(session)
            withTimeout(maxDurationMs) {
                coroutineScope {
                    launch {
                        while (isActive) {
                            delay(8_000)
                            drain { checked(operations.renew(id)) }
                            ensureActive()
                            state = state.copy(renewals = state.renewals + 1); publish()
                        }
                    }
                    val previewJob = launch {
                        while (isActive) {
                            delay(1_000)
                            val current = runtime.target ?: continue
                            val frame = runtime.preview(current) { checked(operations.preview(id, current.targetId)) }
                            ensureActive()
                            if (frame != null && runtime.target == current) { state = state.copy(preview = frame, previewFrames = state.previewFrames + 1); publish() }
                        }
                    }
                    suspend fun launchApp(pkg: String) {
                        val launched = runtime.launch {
                            checked(operations.launch(id, pkg)).also {
                                check(it.packageName in apps && it.displayId == state.displayId)
                            }
                        }
                        ensureActive()
                        state = state.copy(targetId = launched.targetId, taskId = launched.taskId, currentPackage = launched.packageName)
                    }
                    launchApp(apps.first())
                    delay(1_000)
                    val history = mutableListOf<AgentAction>()
                    val started = nowMs()
                    for (step in 1..maxSteps) {
                        ensureActive()
                        check(nowMs() - started < maxDurationMs) { "Agent time limit reached" }
                        val current = requireNotNull(runtime.target)
                        state = state.copy(agentSteps = step, modelStatus = "idle"); publish("Running", "snapshot")
                        suspend fun capture(): DisplayFrame {
                            val frame = drain { checked(operations.snapshot(id, current.targetId)) }
                            ensureActive()
                            check(runtime.target == current && frame.targetId == current.targetId && frame.displayId == current.displayId) {
                                "Snapshot target changed"
                            }
                            return frame
                        }
                        val before = capture()
                        state = state.copy(preview = before, previewFrames = state.previewFrames + 1, modelStatus = "running")
                        publish(phase = "inference")
                        val action = model.decide(UUID.randomUUID().toString(), before, current.packageName, history.toList())
                        ensureActive()
                        state = state.copy(lastReason = action.reason, modelStatus = "responded"); publish(phase = "verify frame")
                        var matchedFrame: DisplayFrame? = null
                        // Observe up to one full caret phase without masking pixels or relaxing freshness.
                        // Every admitted capture drains; the gaps remain cancellable and allow lease renewal.
                        for (attempt in 0..4) {
                            if (attempt > 0) delay(150)
                            val candidate = capture()
                            check(candidate.width == before.width && candidate.height == before.height && candidate.rotation == before.rotation) {
                                "Snapshot geometry changed"
                            }
                            if (sameAgentFrame(before, candidate, requireNotNull(generation), requireNotNull(generation), pixels)) {
                                matchedFrame = candidate
                                break
                            }
                        }
                        val after = matchedFrame
                        if (after == null) {
                            state = state.copy(lastReason = "Display changed during inference; requesting a new decision")
                            publish(phase = "replanning")
                            continue
                        }
                        history += action
                        publish(phase = "acting")
                        when (action.type) {
                            "launch" -> launchApp(requireNotNull(action.packageName))
                            "tap", "swipe" -> drain { checked(operations.act(id, requireNotNull(after.snapshotId), action)) }
                            "done" -> { completed = true; finalFrame = after; break }
                            "blocked" -> { blocked = true; break }
                            else -> error("Unsupported action")
                        }
                        ensureActive()
                        delay(650)
                    }
                    previewJob.cancelAndJoin()
                    coroutineContext.cancelChildren()
                    if (!completed && !blocked) error("Agent step limit reached")
                }
            }
        } catch (error: TimeoutCancellationException) {
            failure = "Agent time limit reached"
        } catch (_: CancellationException) {
            state = state.copy(modelStatus = "cancelled")
        } catch (error: Exception) {
            // Do not expose network/config payloads or retry an uncertain input.
            failure = "Agent halted: ${error.javaClass.simpleName}"
        } finally {
            publish("Stopping", "cleanup")
            if (session != null) {
                try {
                    drain {
                        val stopped = checked(operations.stop(requireNotNull(session)))
                        check(stopped.state == "stopped" && stopped.cleanup == "released")
                    }
                } catch (_: Exception) { failure = "Cleanup unverified" }
            }
            state = state.copy(status = failure?.let { "Error: $it" } ?: if (blocked) "Blocked" else "Stopped",
                phase = if (failure != null) "stopped" else if (blocked) "blocked" else if (completed) "done" else "stopped",
                modelStatus = if (failure != null) "failed" else if (blocked) "blocked" else if (completed) "done" else state.modelStatus,
                preview = if (completed && failure == null) finalFrame else null,
                previewLabel = if (completed && failure == null) "Last frame · workspace closed" else "Live preview")
            onState(state)
        }
    }
}
