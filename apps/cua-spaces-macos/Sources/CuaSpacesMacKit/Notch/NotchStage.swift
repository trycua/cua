// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
import Foundation
import SwiftUI

/// The notch's presentation state machine: which shape the panel draws and
/// which content shows, and the order and motion that get it there. The
/// core decides the phase (`AppNotchView`); this decides how the change
/// looks, from the core's shared motion (`appNotchMotion`):
///
/// - open (closed or cue to prompt or tiles): the shape springs out of the
///   hardware notch first, then the content fades and scales in after
///   `contentDelayMs`;
/// - morph (prompt to tiles and back): the shape springs to the new size
///   while the content cross-fades;
/// - close: the content fades out first, then the shape closes with a
///   critically damped spring, so it never rises above the notch;
/// - cue: while the core's dwell runs the closed notch grows a little
///   (`hoverScale`) and settles back when the pointer leaves.
///
/// Reduce Motion swaps every step for a short opacity fade.
struct NotchStage: Equatable {
    /// What the shape is sized to.
    enum Shape: Equatable {
        case closed, cue, prompt, tiles

        var isOpen: Bool { self == .prompt || self == .tiles }
    }

    /// How a step animates.
    enum Motion: Equatable {
        case open
        case close(delay: Double)
        case cue
        case contentIn(delay: Double)
        case contentOut
        case fade
    }

    /// One change the view applies, in order.
    enum Step: Equatable {
        case shape(Shape, Motion)
        case content(Shape?, Motion)
    }

    private(set) var shape: Shape
    /// The content showing (`prompt` or `tiles`), none while closed.
    private(set) var content: Shape?

    /// Settled on `view`: the first frame draws without animating.
    init(settledOn phase: AppNotchPhase, cue: Bool = false) {
        let target = Self.target(phase, cue: cue)
        shape = target
        content = target.isOpen ? target : nil
    }

    static func target(_ phase: AppNotchPhase, cue: Bool) -> Shape {
        switch phase {
        case .closed: return cue ? .cue : .closed
        case .prompt: return .prompt
        case .tiles: return .tiles
        }
    }

    /// Moves to the core's phase; returns the steps to animate, in order.
    mutating func update(phase: AppNotchPhase, cue: Bool, motion: AppNotchMotion,
                         reduceMotion: Bool) -> [Step] {
        let target = Self.target(phase, cue: cue)
        guard target != shape else { return [] }
        let from = shape
        shape = target
        let newContent: Shape? = target.isOpen ? target : nil
        let contentChanged = newContent != content
        content = newContent
        if reduceMotion {
            var steps: [Step] = [.shape(target, .fade)]
            if contentChanged { steps.append(.content(newContent, .fade)) }
            return steps
        }
        let delay = Double(motion.contentDelayMs) / 1000
        switch (from.isOpen, target.isOpen) {
        case (false, true):
            return [.shape(target, .open), .content(newContent, .contentIn(delay: delay))]
        case (true, true):
            return [.shape(target, .open), .content(newContent, .contentIn(delay: delay / 2))]
        case (true, false):
            return [.content(nil, .contentOut), .shape(target, .close(delay: motion.contentOut))]
        case (false, false):
            return [.shape(target, .cue)]
        }
    }

    /// The SwiftUI animation for a motion.
    static func animation(_ m: Motion, _ motion: AppNotchMotion) -> Animation {
        switch m {
        case .open:
            return .spring(response: motion.openResponse, dampingFraction: motion.openDamping)
        case .close(let delay):
            return .spring(response: motion.closeResponse, dampingFraction: motion.closeDamping).delay(delay)
        case .cue:
            return .spring(response: motion.hoverResponse, dampingFraction: motion.hoverDamping)
        case .contentIn(let delay):
            return .easeOut(duration: motion.contentIn).delay(delay)
        case .contentOut:
            return .easeIn(duration: motion.contentOut)
        case .fade:
            return .easeInOut(duration: motion.reducedDuration)
        }
    }
}
