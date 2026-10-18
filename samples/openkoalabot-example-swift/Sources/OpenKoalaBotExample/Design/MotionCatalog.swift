// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The **thirty-nine** avatar motion states in the persona catalogue, and the
/// projection from what a Bot is actually doing onto one of them.
///
/// The persona mark accepts the complete state catalogue; the onboarding scene
/// uses only a subset, while conversation surfaces project activity into the
/// same catalogue.
///
/// So this is the catalogue, in declaration order, plus the projection that
/// conversation surfaces perform. Each state carries a `family`: the six drawn
/// behaviours in `BotMotionState` are what the avatar can actually do, and
/// thirty-nine names collapse onto them. Naming a state the app cannot draw
/// distinctly would be a lie; dropping thirty-three names from the catalogue
/// would be the opposite mistake.
enum AvatarMotionState: String, CaseIterable, Hashable {
    case sleeping, waking, idle, listening, thinking, searching, working
    case excited, surprised, suspicious, angry, drowsy, happy, curious
    case confused, bored, proud, shy, sad, laughing, scared, playful
    case celebrate, orbit, radar, progress, spawning, humming, loading
    case dictating, writing, sending, receiving, uploading, notifying
    case alerting, dragging, bouncing
    case poweringDown = "powering-down"

    /// The drawn behaviour this state resolves to. Six families, because six
    /// is what is distinguishable at avatar size without a label.
    var family: BotMotionState {
        switch self {
        case .sleeping, .idle, .drowsy, .bored, .humming, .shy, .waking:
            return .idle
        case .listening, .curious, .surprised, .suspicious, .dictating, .playful:
            return .listening
        case .thinking, .searching, .confused, .orbit, .radar, .loading, .spawning:
            return .thinking
        case .working, .progress, .writing, .uploading, .sending, .receiving, .dragging:
            return .working
        case .excited, .happy, .laughing, .proud, .celebrate, .bouncing, .notifying:
            return .speaking
        case .angry, .sad, .scared, .alerting, .poweringDown:
            return .blocked
        }
    }

    /// Whether the state loops forever. A stopped Bot must not animate
    /// indefinitely, or "stuck" and "busy" look the same.
    var isContinuous: Bool { family.isContinuous }

    var accessibilityLabel: String {
        switch self {
        case .sleeping:     return "Asleep"
        case .waking:       return "Waking"
        case .idle:         return "Idle"
        case .listening:    return "Listening"
        case .thinking:     return "Thinking"
        case .searching:    return "Searching"
        case .working:      return "Working"
        case .excited:      return "Excited"
        case .surprised:    return "Surprised"
        case .suspicious:   return "Unsure"
        case .angry:        return "Blocked"
        case .drowsy:       return "Winding down"
        case .happy:        return "Pleased"
        case .curious:      return "Curious"
        case .confused:     return "Confused"
        case .bored:        return "Waiting"
        case .proud:        return "Finished"
        case .shy:          return "Quiet"
        case .sad:          return "Failed"
        case .laughing:     return "Laughing"
        case .scared:       return "Stopped"
        case .playful:      return "Playful"
        case .celebrate:    return "Done"
        case .orbit:        return "Thinking"
        case .radar:        return "Scanning"
        case .progress:     return "Working"
        case .spawning:     return "Starting"
        case .humming:      return "Idle"
        case .loading:      return "Loading"
        case .dictating:    return "Taking dictation"
        case .writing:      return "Writing"
        case .sending:      return "Sending"
        case .receiving:    return "Receiving"
        case .uploading:    return "Uploading"
        case .notifying:    return "Replying"
        case .alerting:     return "Needs you"
        case .dragging:     return "Moving a file"
        case .bouncing:     return "Replying"
        case .poweringDown: return "Shutting down"
        }
    }

    // MARK: Projection

    /// What a Bot is doing, as the conversation surfaces know it. Every field
    /// is something the caller actually has; nothing here is guessed from a
    /// string in the Bot's output.
    struct Activity {
        var state: AgentState = .unknown
        var hasThread: Bool = true
        var exitCode: Int? = nil
        /// The user is typing at this Bot right now.
        var isComposing: Bool = false
        /// Output arrived since the previous poll.
        var hasFreshOutput: Bool = false
        /// The Bot is waiting on the user — an approval, a choice card.
        var awaitingUser: Bool = false
        /// A file is being uploaded into, or downloaded out of, the Space.
        var isTransferring: Bool = false
        /// The run was started within the last beat.
        var justStarted: Bool = false
        /// A file is being dragged over this Bot's thread.
        var isDropTarget: Bool = false
    }

    /// Project an activity onto the catalogue.
    ///
    /// Ordered most-specific first, because several of these are true at once:
    /// a Bot can be running *and* have a file landing on it, and the drag is
    /// the thing the user needs told about.
    static func project(_ a: Activity) -> AvatarMotionState {
        if a.isDropTarget                      { return .dragging }
        if a.isComposing                       { return .dictating }
        if a.isTransferring                    { return .uploading }
        if a.justStarted                       { return .spawning }
        if a.awaitingUser                      { return .alerting }
        switch a.state {
        case .running:       return a.hasFreshOutput ? .writing : .working
        case .awaitingInput: return .listening
        case .idle:          return a.hasThread ? .idle : .sleeping
        case .finished:      return (a.exitCode ?? 0) == 0 ? .proud : .sad
        case .failed:        return .sad
        case .crashed:       return .poweringDown
        case .unknown:       return a.hasThread ? .confused : .sleeping
        }
    }

    /// Convenience over a live presence.
    static func project(_ presence: BotPresence, isComposing: Bool = false,
                        hasFreshOutput: Bool = false) -> AvatarMotionState {
        project(Activity(state: presence.state,
                         hasThread: presence.hasThread,
                         exitCode: presence.exitCode,
                         isComposing: isComposing,
                         hasFreshOutput: hasFreshOutput))
    }
}
