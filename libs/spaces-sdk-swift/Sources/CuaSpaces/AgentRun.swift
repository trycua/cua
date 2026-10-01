import Foundation

/// A handle on one agent run.
///
/// Carries its own `RunID` and its own `Space`, so neither is restated (§7),
/// and every operation on a run is a method on it.
public struct AgentRun: Sendable, Identifiable {

    public let id: RunID
    public let space: Space
    public let agent: String
    /// What the harness published about how this run takes turns. `nil` for a
    /// handle recovered by id rather than returned by `startAgent` — the SDK
    /// does not invent one.
    public let turnModel: TurnModel?
    /// The metadata this run was started with (§21, §41).
    public let metadata: [String: String]
    public let notes: [String]

    init(id: RunID, space: Space, agent: String, turnModel: TurnModel?,
         metadata: [String: String], notes: [String]) {
        self.id = id
        self.space = space
        self.agent = agent
        self.turnModel = turnModel
        self.metadata = metadata
        self.notes = notes
    }

    // MARK: - State

    /// The full state of this run — same type the roster reports (§22).
    ///
    /// - Parameter tail: how many lines of output to bring back. `0` asks for
    ///   none, which is the cheap probe.
    public func status(tail: Int = 200) async throws -> RunSnapshot {
        let d = try await space.connection.object("agent_status", [
            "space": .string(space.id.rawValue),
            "run_id": .string(id.rawValue),
            "tail": .number(Double(tail)),
        ])
        var enriched = d
        if let summary = d["summary"]?.stringValue {
            enriched["summary"] = .string(RunMetadata.strip(summary))
            enriched["raw_summary"] = .string(summary)
        }
        let snapshot = RunSnapshot.decode(
            enriched, id: id, space: space.id, requestedTail: tail > 0 ? tail : nil,
            carryingReasonFrom: await space.connection.lastSnapshot(id))
        await space.connection.remember(snapshot)
        return snapshot
    }

    /// State changes as a sequence.
    ///
    /// `FRICTION.md` §2: *"There is no way to be told that an agent changed
    /// state. Every consumer writes the same loop: call `agent_status`,
    /// compare, sleep one second, repeat, with a timeout it has to invent."*
    ///
    /// The transport cannot push, so the SDK still polls — but the SDK is the
    /// only thing that does, the backoff is inside, and a consumer writes
    /// `for await`. The first element is emitted immediately; later elements
    /// only when something visibly changed. The sequence finishes on its own
    /// when the run ends, so there is no invented timeout at the call site.
    public func stateUpdates(every interval: Duration = .seconds(1),
                             tail: Int = 200,
                             finishWhenEnded: Bool = true) -> AsyncStream<RunSnapshot> {
        AsyncStream { continuation in
            let task = Task {
                var previous: RunSnapshot?
                var backoff = interval
                while !Task.isCancelled {
                    do {
                        let snapshot = try await status(tail: tail)
                        if previous.map({ snapshot.differsVisibly(from: $0) }) ?? true {
                            continuation.yield(snapshot)
                            backoff = interval
                        } else {
                            // Nothing moved; ease off, but never past 8x.
                            backoff = min(backoff * 2, interval * 8)
                        }
                        previous = snapshot
                        if finishWhenEnded, snapshot.state.hasEnded { break }
                    } catch {
                        // A failed probe is not a state: `unknown` means the
                        // probe failed, which is exactly what §9 preserves.
                        let snapshot = RunSnapshot(
                            id: id, space: space.id, agent: agent, state: .unknown,
                            reason: "\(error)", acceptsMessage: false, exitCode: nil,
                            summary: previous?.summary ?? "", outputTail: previous?.outputTail)
                        continuation.yield(snapshot)
                        previous = snapshot
                        backoff = min(backoff * 2, interval * 8)
                    }
                    try? await Task.sleep(for: backoff)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Wait for a condition, with the poll loop and its timeout inside the SDK
    /// rather than reinvented per consumer (§2).
    /// - Note: the budget is **honoured**. An earlier implementation consumed
    ///   `stateUpdates`, whose back-off grows to `interval * 8` when nothing
    ///   moves, and only checked the deadline *after* the next yield — so a
    ///   two-second budget could return at eight seconds. The deadline is now
    ///   the thing that bounds the sleep, not something consulted after it.
    @discardableResult
    public func wait(upTo limit: Duration = .seconds(120),
                     tail: Int = 200,
                     pollingEvery interval: Duration = .seconds(1),
                     until satisfied: @Sendable (RunSnapshot) -> Bool) async throws -> RunSnapshot {
        let deadline = ContinuousClock.now + limit
        var last: RunSnapshot?
        while !Task.isCancelled {
            if let snapshot = try? await status(tail: tail) {
                last = snapshot
                if satisfied(snapshot) { return snapshot }
            }
            let remaining = deadline - ContinuousClock.now
            guard remaining > .zero else { break }
            try? await Task.sleep(for: min(interval, remaining))
        }
        if let last, satisfied(last) { return last }
        throw SpacesError.timedOut(waitingFor: .runState(id, description: "in the requested state"),
                                   after: limit)
    }

    // MARK: - Messaging

    /// Speak to the run.
    ///
    /// Refusal is the default and is a **value**, not an error: §9 records
    /// refusal-over-silent-damage as a thing the SDK must keep, and §39 records
    /// that a refusal has to reach a surface a person can see, so the SDK never
    /// swallows one. `.queueUntilIdle` is the outbox §24 asks for.
    @discardableResult
    public func send(_ text: String, mode: DeliveryMode = .refuseIfBusy) async throws -> Delivery {
        switch mode {
        case .refuseIfBusy:
            return try await deliver(text, force: false, queuedFor: nil)
        case .abandonCurrentTurn:
            return try await deliver(text, force: true, queuedFor: nil)
        case let .queue(timeout, lifetime):
            let started = ContinuousClock.now
            let first = try await deliver(text, force: false, queuedFor: nil)
            if first.accepted { return first }
            // Visible and cancellable while it waits — and dying with this
            // process, which is what `lifetime` says on its face.
            let held = QueuedMessage(run: id, text: text, queuedUntil: lifetime)
            let outbox = await space.connection.outbox
            await outbox.enqueue(held)
            defer { Task { await outbox.dequeue(held) } }
            _ = try await wait(upTo: timeout, tail: 0) { $0.acceptsMessage || $0.state.hasEnded }
            if await outbox.isCancelled(held.id) {
                return Delivery(runID: id, accepted: false,
                                reason: "withdrawn from the outbox before delivery")
            }
            let elapsed = ContinuousClock.now - started
            return try await deliver(text, force: false, queuedFor: elapsed)
        }
    }

    /// Messages this process is holding for this run. See `QueueLifetime`.
    public var outbox: Outbox {
        get async { await space.connection.outbox }
    }

    private func deliver(_ text: String, force: Bool, queuedFor: Duration?) async throws -> Delivery {
        var a: [String: JSONValue] = [
            "space": .string(space.id.rawValue),
            "run_id": .string(id.rawValue),
            "text": .string(text),
        ]
        if force { a["force"] = true }
        let d = try await space.connection.object("agent_message", a)
        let delivery = Delivery(d, runID: id, queuedFor: queuedFor)
        // The turn boundary is recorded here, at the moment of delivery, so it
        // does not depend on the app having been watching the tail (§20).
        if delivery.accepted {
            await space.connection.openTurn(on: id, message: text, delivery: delivery)
        }
        return delivery
    }

    // MARK: - Events and interrupting

    /// The run's normalised events after `cursor` (`agent_events`): the same
    /// kinds for every harness. Pass `page.cursor` back as `after` to continue;
    /// `page.caughtUp` means nothing more is written yet.
    public func eventPage(after cursor: UInt64 = 0, max: Int? = nil) async throws -> RunEventPage {
        var a: [String: JSONValue] = [
            "space": .string(space.id.rawValue),
            "run_id": .string(id.rawValue),
            "cursor": .number(Double(cursor)),
        ]
        if let max { a["max"] = .number(Double(max)) }
        return RunEventPage(try await space.connection.object("agent_events", a))
    }

    /// Cancel the turn in flight (`agent_interrupt`). The session stays open
    /// for `send(_:)`. Returns whether the server says it interrupted one.
    @discardableResult
    public func interrupt() async throws -> Bool {
        let d = try await space.connection.object("agent_interrupt", [
            "space": .string(space.id.rawValue),
            "run_id": .string(id.rawValue),
        ])
        return d["interrupted"]?.boolValue ?? false
    }

    // MARK: - Ending

    /// Stop the process. The result reports a **verified** liveness probe
    /// rather than assuming the kill worked (§9).
    @discardableResult
    public func stop() async throws -> StopOutcome {
        let d = try await space.connection.object("agent_stop", [
            "space": .string(space.id.rawValue),
            "run_id": .string(id.rawValue),
        ])
        return StopOutcome(d)
    }

    /// Remove the **whole** run: the process, the run directory, the
    /// LaunchAgent plist, and the Terminal window it left open.
    ///
    /// `FRICTION.md` §8 — one call, and it says what it could not remove
    /// instead of reporting success it did not verify.
    @discardableResult
    public func delete() async throws -> RunCleanup {
        var residue: [String] = []

        var stopped = false
        do {
            let outcome = try await stop()
            stopped = outcome.stopped || outcome.alive == false
            if !stopped { residue.append("process: \(outcome.reason)") }
        } catch {
            residue.append("agent_stop: \(error)")
        }

        // Expanded once, here, so the `rm` and the verification agree about
        // what path they are talking about. A quoted `~` does not expand.
        let directory = space.guestPath(id.directory)
        let plist = space.guestPath(
            "~/Library/LaunchAgents/com.trycua.agentrun.\(id.rawValue).plist")
        var directoryRemoved = false
        var plistRemoved = false
        var terminalClosed = false

        do {
            _ = try await space.bash("rm -rf '\(directory)'")
            directoryRemoved = try await !space.fileExists(directory)
            if !directoryRemoved { residue.append(directory) }
        } catch {
            residue.append("\(directory): \(error)")
        }

        do {
            _ = try await space.bash("rm -f '\(plist)'")
            plistRemoved = try await !space.fileExists(plist)
            if !plistRemoved { residue.append(plist) }
        } catch {
            residue.append("\(plist): \(error)")
        }

        // The Terminal window exists only on Local (macOS) Spaces. Closing it
        // is best-effort — it is a window on someone's desktop, not state —
        // but "best-effort" is not a licence to *claim* it closed. §8 asks for
        // one call that says what it could not remove; a demo Space that has
        // accumulated a hundred orphaned `watch.command` windows is what
        // happens when this branch reports success it never checked.
        if space.provider == .local {
            // **Signals, not `osascript`.**
            //
            // Closing the window by telling Terminal to do it is an Automation
            // request, and that is not available: on a Space that has never
            // been asked it raises a consent dialog on the user's desktop, and
            // on one that has been asked and refused it simply exits 1 while
            // looking like it worked. Neither is something a library call may
            // do to a machine. So the window is closed by killing the process
            // that holds it — the run's own `watch-<run_id>.command` watcher,
            // which is named for the run precisely so it can be found without
            // reading window titles.
            //
            // `kill -9`, and the -9 matters: SIGTERM is a graceful quit, and a
            // graceful quit makes Terminal *save* its windows to Resume state,
            // so they come back on its next launch. Killing a run's watcher
            // outright leaves nothing to restore.
            _ = try? await space.bash(
                "pkill -9 -f 'watch-\(id.rawValue).command' 2>/dev/null; "
                // Belt and braces: the watcher identified by the log it holds
                // open, for a run started before the name carried the run id.
                + "for p in $(pgrep -f 'tail -f out.log' 2>/dev/null); do "
                + "lsof -p $p 2>/dev/null | grep -q '\(id.rawValue)/out.log' "
                + "&& kill -9 $p; done; true")
            // Verify through the Space's own window list rather than a second
            // scripting round trip: it needs no automation consent, so a run
            // deleted from a suite cannot raise a TCC prompt on the user's
            // machine (§36 is about consent dialogs being unclearable).
            if let remaining = try? await space.windows()
                .filter({ $0.title.contains(id.rawValue) }) {
                terminalClosed = remaining.isEmpty
                if !terminalClosed {
                    residue.append(contentsOf: remaining.map { "window: \($0.displayName)" })
                }
            } else {
                // The list itself failed, so nothing is known. `unknown` is not
                // `closed` — §9's rule, applied to a window instead of a state.
                terminalClosed = false
                residue.append("window: could not verify (window list unavailable)")
            }
        } else {
            // No Terminal window is created off Local, so there is nothing to
            // close and nothing to claim.
            terminalClosed = true
        }

        return RunCleanup(runID: id, processStopped: stopped,
                          directoryRemoved: directoryRemoved,
                          launchAgentRemoved: plistRemoved,
                          terminalWindowClosed: terminalClosed,
                          residue: residue)
    }

    // MARK: - Screen

    /// The window this run is working in, or `nil` when the join cannot be
    /// made (§37).
    public func window() async throws -> SpaceWindow? {
        try await space.window(for: id)
    }
}
