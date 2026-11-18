// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import SwiftUI

// MARK: - Bots

/// A Bot is a *roster entry*, not a conversation.
///
/// This is the single most important structural fact about the app and the one
/// thing a naive design gets wrong: the home screen is a roster of persistent
/// coworkers, each with exactly one long-lived thread, not a list of chat
/// sessions. Creating a "new chat" is not a concept; creating a new *Bot* is.
struct Bot: Identifiable, Hashable, Codable {
    let id: String
    var name: String
    var shape: BlobShape
    var colorHex: UInt32
    var eyeStyle: EyeStyle = .capsule
    var pinned: Bool = false
    /// Hidden from the sidebar's main list.
    ///
    /// Not deleted and not stopped: a hidden Bot keeps running and keeps its
    /// history, it just collapses into a `Hidden Bots` section that renders
    /// only when it is non-empty. Bots a teammate shares **arrive** hidden, so
    /// this is also why sharing one does not make the sidebar grow.
    var isHiddenFromSidebar: Bool = false
    /// Preview line shown in the roster — the Bot's latest utterance.
    var preview: String
    var timestamp: String
    /// Which screen of the user's single persistent Space this Bot drives.
    /// Screens are *not* separate security boundaries; they are
    /// a presentation split over one VM per user.
    var screenIndex: Int

    /// The Bot's one color: the SDK's stable presence color for its id, so
    /// the avatar's background and the Bot's cursor on the live desktop match.
    /// (`colorHex` is the persona's original tint, kept for stored rosters.)
    var color: Color { BotPresenceColor.fill(for: id) }
    /// Black or white, whichever reads on `color` (the avatar's eyes).
    var onColor: Color { BotPresenceColor.text(for: id) }

    static func == (a: Bot, b: Bot) -> Bool { a.id == b.id }
    func hash(into h: inout Hasher) { h.combine(id) }
}

// MARK: - The heterogeneous transcript

/// Every kind of thing that can appear in a thread, as a closed union.
///
/// The transcript is not a list of strings. Modelling it as one and
/// retrofitting cards later is the expensive mistake, so the union is the
/// day-one representation.
enum MessageBody {
    /// Ordinary prose, from the Bot or the user.
    case prose(String)
    /// A titled card with an inset body and action buttons — e.g. a drafted email.
    case card(Card)
    /// A permission gate. Blocks the Bot until resolved.
    case approval(Approval)
    /// A produced or fetched artefact: a link, a document, a file.
    case linkFile(LinkFile)
    /// A centred, un-bubbled line: date separators, teleports, sign-ins.
    case systemEvent(String)
    /// The Agent Computer status glyph — tier 1 of three.
    case computerStatus(ComputerStatus)
    /// A lettered multiple-choice panel with a free-text escape hatch.
    /// See `ChoiceCard`.
    case choices(ChoiceCard)
    /// What the agent did between messages (install progress, thinking,
    /// tool calls and results, turn ends, notices), one muted collapsible
    /// group per stretch. Classified by the SDK (`AgentTranscript`), never by
    /// reading text here.
    case activity(ActivityGroup)
}

/// Consecutive non-message agent events: a one-line summary (`5 steps`)
/// and one short line per step.
struct ActivityGroup: Equatable {
    var summary: String
    var steps: [String]
}

/// The inline choice panel: a heading, a dismiss `×`, N lettered options, and a
/// trailing free-text field.
///
/// Used in two places (a Bot's opener and the onboarding question a freshly
/// created Bot asks) with the same shape both times, which is why this is one reusable
/// component and not two hand-built panels. The letters are positional and
/// derived rather than stored, so an option cannot carry the wrong letter.
struct ChoiceCard: Equatable {
    var heading: String
    var options: [String]
    /// The trailing free-text field's placeholder.
    var freeTextPlaceholder: String = "Type your own answer"
    /// Set once the user picks or types. A resolved card stops offering
    /// choices rather than silently accepting a second answer.
    var resolved: String? = nil

    /// `A`, `B`, `C`, … for an option index. Positional, never stored.
    static func letter(_ index: Int) -> String {
        guard index >= 0, index < 26 else { return "" }
        return String(UnicodeScalar(UInt8(65 + index)))
    }

    /// **There are deliberately no canned cards here.**
    ///
    /// Example headings and options ("What can I take off your plate?",
    /// "Coding & projects" and the like) are **not app strings**. They are
    /// model output, arriving over the wire per conversation. Shipping them as
    /// literals is the single most tempting mistake available here, and the
    /// one that makes an app look right in a screenshot and behave wrong the
    /// moment anyone uses it: every Bot would ask the same four questions
    /// forever, and the questions would not be about anything.
    ///
    /// So the card is a widget and the content is data. `AgentOutputParser`
    /// builds one when a Bot emits the block described there; nothing else
    /// constructs one with fixed strings.
    static let freeTextPlaceholderDefault = "Type your own answer"
}

struct Card {
    var title: String
    var bodyLines: [String]
    var primary: String
    var secondary: String
    var resolved: String? = nil
}

struct Approval {
    var title: String
    var detail: String
    /// Desktop offers Allow once / Deny / Always allow; a local-command card
    /// offers only Allow once / Deny.
    var offersAlwaysAllow: Bool = true
    var resolved: String? = nil

    enum Answer: String, Equatable {
        case allowOnce = "Allow once"
        case deny = "Deny"
        case alwaysAllow = "Always allow"
    }

    /// The single source of truth for whether a human decision here can
    /// actually stop a Bot. **It cannot.**
    ///
    /// There is no approval primitive in the Spaces MCP: `agent_start` runs
    /// auto-approved because the Space *is* the sandbox, and nothing pauses a
    /// Bot while an approval is on screen. The card renders a gate that does
    /// not gate, and it must never claim otherwise. See `FRICTION.md` §43.
    ///
    /// This lived on the phone transcript's card view until that surface was
    /// deleted. The desktop still draws approvals, so the claim moved to the
    /// model rather than going away with the view.
    static let enforcementIsImplemented = false

    /// The desktop order, kept beside the model so the order and the wording
    /// cannot drift apart.
    static let desktopOrder: [Answer] = [.allowOnce, .deny, .alwaysAllow]
    /// The local-command card's order.
    static let localCommandOrder: [Answer] = [.allowOnce, .deny]
}

struct LinkFile {
    var title: String
    var subtitle: String
    var kind: Kind
    enum Kind { case slides, doc, sheet, pdf, image, file }
}

struct ComputerStatus {
    var text: String
    var active: Bool
}

enum Sender { case bot, user }

struct Message: Identifiable {
    let id = UUID()
    var sender: Sender
    var body: MessageBody
    /// Emoji reactions attached below the bubble.
    var reaction: String? = nil

    /// The prefix `BotStore` writes on an undelivered message.
    ///
    /// One constant so the writer and every reader cannot drift apart. It used
    /// to live on the phone transcript's row type; it belongs on the model,
    /// which is why deleting that surface did not take it with it.
    static let refusalPrefix = "Not delivered: "
    static func isRefusal(_ text: String) -> Bool { text.hasPrefix(refusalPrefix) }
}

struct Thread {
    var botID: String
    var messages: [Message]
}

// MARK: - Attachments

struct Attachment: Identifiable {
    let id = UUID()
    var name: String
    var byteCount: Int
}

// MARK: - Agent Computer presentation tiers

/// Three tiers, escalating. Tier 1 lives inline in the transcript, tier 2 pins
/// a live preview above the composer, tier 3 takes the whole screen and must
/// offer an explicit hand-back.
enum ComputerTier: Equatable {
    case glyph
    case pinnedPreview
    case fullScreen
}
