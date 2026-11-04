import Foundation

/// One tick of the roster.
public struct RosterUpdate: Sendable {
    /// Every run in the Space, newest state first seen this tick.
    public let runs: [RunSnapshot]
    /// Which runs changed visibly since the previous tick. Empty on the first
    /// tick is impossible — the first tick reports everything as changed, so a
    /// consumer never has to special-case start-up.
    public let changed: Set<RunID>
    /// What this tick cost, so the claim "one poll per tick" is checkable
    /// rather than asserted. `FRICTION.md` §25.
    public let roundTrips: Int

    /// Which runs were refreshed **by rotation** this tick rather than because
    /// they are watched. A UI can then show freshness honestly instead of
    /// implying every row is live.
    public let rotated: Set<RunID>

    public init(runs: [RunSnapshot], changed: Set<RunID>, roundTrips: Int,
                rotated: Set<RunID> = []) {
        self.runs = runs
        self.changed = changed
        self.roundTrips = roundTrips
        self.rotated = rotated
    }

    public subscript(id: RunID) -> RunSnapshot? { runs.first { $0.id == id } }
}

/// How a roster refreshes itself.
///
/// A watch-set alone could not say *"the open thread is live and every other
/// agent is eventually fresh"*, so every app kept its own round-robin loop.
/// `watched` and `rotateUnwatched` together say it: "all thirty live" is
/// `watched: everything`, and "one live, the rest rotating" is one watched id
/// and `rotateUnwatched: 1`. Same expression, two values.
public struct RosterPolicy: Sendable, Hashable {
    /// How often to tick.
    public var interval: Duration = .seconds(3)
    /// Runs refreshed in full on every tick.
    public var watched: Set<RunID> = []
    /// How many *unwatched* runs to refresh per tick, cycling. `0` refreshes
    /// none of them, so unwatched rows carry only what `agent_list` publishes.
    public var rotateUnwatched: Int = 1
    /// How much output to pull for a refreshed run.
    public var tail: Int = 400
    /// Runs this SDK did not start are reported but **not adopted** unless
    /// asked. An operator view wants `true`; a chat sidebar does not, and a
    /// sidebar full of threads the user never opened is what happens when it
    /// defaults the other way.
    public var includesForeignRuns: Bool = false

    public init(interval: Duration = .seconds(3), watched: Set<RunID> = [],
                rotateUnwatched: Int = 1, tail: Int = 400,
                includesForeignRuns: Bool = false) {
        self.interval = interval
        self.watched = watched
        self.rotateUnwatched = rotateUnwatched
        self.tail = tail
        self.includesForeignRuns = includesForeignRuns
    }

    public static let `default` = RosterPolicy()

    /// One thread open, everything else rotating.
    public static func focused(_ id: RunID) -> RosterPolicy {
        RosterPolicy(watched: [id])
    }

    /// Every run refreshed in full, every tick. Costs one round trip per run.
    public static func allLive(_ ids: Set<RunID>) -> RosterPolicy {
        RosterPolicy(watched: ids, rotateUnwatched: 0)
    }
}

/// A subscription to the whole roster, polled **once** per tick.
///
/// `FRICTION.md` §2 and §25 together: status wants to be a subscription, and
/// the cost is per run while a roster screen needs every Bot at once. The old
/// shape was `agent_list` plus one `agent_status` per hired Bot, every tick,
/// each one an SSH round trip that reads files inside the Space — ten round
/// trips per tick for nine Bots.
///
/// Here the base cost is one `agent_list` regardless of roster size. Output is
/// expensive and most of the roster does not need it, so a caller names the
/// runs whose transcript is actually on screen with `watch(_:)`; only those get
/// a `agent_status`. `roundTrips` on every update makes the cost visible.
public final class RosterStream: AsyncSequence, Sendable {
    public typealias Element = RosterUpdate

    private let space: Space
    private let interval: Duration
    private let tail: Int
    private let rotateUnwatched: Int
    private let includesForeignRuns: Bool
    private let state = State()

    private final class State: @unchecked Sendable {
        private let lock = NSLock()
        private var detailed: Set<RunID> = []
        func set(_ ids: Set<RunID>) { lock.lock(); detailed = ids; lock.unlock() }
        func insert(_ id: RunID) { lock.lock(); detailed.insert(id); lock.unlock() }
        func remove(_ id: RunID) { lock.lock(); detailed.remove(id); lock.unlock() }
        var current: Set<RunID> { lock.lock(); defer { lock.unlock() }; return detailed }
    }

    init(space: Space, interval: Duration, detailed: Set<RunID>, tail: Int) {
        self.space = space
        self.interval = interval
        self.tail = tail
        self.rotateUnwatched = 0
        self.includesForeignRuns = true
        state.set(detailed)
    }

    init(space: Space, policy: RosterPolicy) {
        self.space = space
        self.interval = policy.interval
        self.tail = policy.tail
        self.rotateUnwatched = policy.rotateUnwatched
        self.includesForeignRuns = policy.includesForeignRuns
        state.set(policy.watched)
    }

    /// Also fetch output for this run from the next tick on — the thread the
    /// user is looking at.
    public func watch(_ id: RunID) { state.insert(id) }

    /// Stop fetching output for this run.
    public func unwatch(_ id: RunID) { state.remove(id) }

    /// Replace the watched set wholesale.
    public func watchOnly(_ ids: Set<RunID>) { state.set(ids) }

    public var watched: Set<RunID> { state.current }

    public func makeAsyncIterator() -> AsyncStream<RosterUpdate>.Iterator {
        stream().makeAsyncIterator()
    }

    /// The updates, as a stream. Iterating it starts the poll loop; dropping it
    /// stops it.
    public func stream() -> AsyncStream<RosterUpdate> {
        AsyncStream { continuation in
            let task = Task { [space, interval, tail, state, rotateUnwatched,
                               includesForeignRuns] in
                var previous: [RunID: RunSnapshot] = [:]
                var rotationCursor = 0
                while !Task.isCancelled {
                    var trips = 0
                    var rotated: Set<RunID> = []
                    do {
                        var snapshots = try await space.runs()
                        trips += 1
                        let wanted = state.current
                        if !includesForeignRuns {
                            // A roster is "conversations I have had", not
                            // "everything happening in the Space".
                            let mine = await space.connection.runsStartedHere()
                            snapshots = snapshots.filter { mine.contains($0.id) }
                        }
                        if !wanted.isEmpty {
                            for (i, snapshot) in snapshots.enumerated()
                            where wanted.contains(snapshot.id) {
                                if let detail = try? await space.run(snapshot.id).status(tail: tail) {
                                    snapshots[i] = detail
                                    trips += 1
                                }
                            }
                        }
                        // Everyone else, eventually: `rotateUnwatched` of the
                        // unwatched runs per tick, cycling, so a roster can say
                        // "the open thread is live and the rest are fresh
                        // within N ticks" and mean it.
                        if rotateUnwatched > 0 {
                            let unwatched = snapshots.indices.filter { !wanted.contains(snapshots[$0].id) }
                            if !unwatched.isEmpty {
                                for step in 0..<Swift.min(rotateUnwatched, unwatched.count) {
                                    let index = unwatched[(rotationCursor + step) % unwatched.count]
                                    if let detail = try? await space.run(snapshots[index].id)
                                        .status(tail: tail) {
                                        rotated.insert(detail.id)
                                        snapshots[index] = detail
                                        trips += 1
                                    }
                                }
                                rotationCursor = (rotationCursor + rotateUnwatched) % unwatched.count
                            }
                        }
                        var changed: Set<RunID> = []
                        for snapshot in snapshots {
                            if let old = previous[snapshot.id] {
                                if snapshot.differsVisibly(from: old) { changed.insert(snapshot.id) }
                            } else {
                                changed.insert(snapshot.id)
                            }
                            previous[snapshot.id] = snapshot
                        }
                        for gone in Set(previous.keys).subtracting(snapshots.map(\.id)) {
                            previous[gone] = nil
                            changed.insert(gone)
                        }
                        continuation.yield(RosterUpdate(runs: snapshots, changed: changed,
                                                        roundTrips: trips, rotated: rotated))
                    } catch {
                        // A failed roster read is reported as an empty tick
                        // rather than ending the subscription: a Space that
                        // blips should not make every consumer re-subscribe.
                        continuation.yield(RosterUpdate(runs: Array(previous.values),
                                                        changed: [], roundTrips: trips))
                    }
                    try? await Task.sleep(for: interval)
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
