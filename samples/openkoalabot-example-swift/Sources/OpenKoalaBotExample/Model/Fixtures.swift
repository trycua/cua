// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// Fixture content for the export renders, fixed so that a render can be
/// diffed against its baseline string-for-string.
enum Fixtures {

    static let bots: [Bot] = [
        Bot(id: "cos", name: "Chief of Staff", shape: .circle, colorHex: 0x8B5CF6,
            pinned: true, preview: "Sent", timestamp: "7:58 AM", screenIndex: 0),
        Bot(id: "ea", name: "EA", shape: .teardrop, colorHex: 0x2F80F0,
            pinned: true, preview: "Calendar cleared for Thursday.", timestamp: "8:30 AM", screenIndex: 1),
        Bot(id: "inbox", name: "Inbox Manager", shape: .cloud, colorHex: 0x18BE4B,
            pinned: true, preview: "Done", timestamp: "8:00 AM", screenIndex: 2),

        Bot(id: "sales", name: "Sales Outbound", shape: .hexagon, colorHex: 0x11B5A0,
            preview: "Outreach drafts queued for approval.", timestamp: "8:41 AM", screenIndex: 3),
        Bot(id: "talent", name: "Talent Scout", shape: .wideEllipse, colorHex: 0x7B4B2A,
            preview: "Shortlist of 6 candidates ready.", timestamp: "8:22 AM", screenIndex: 4),
        Bot(id: "growth", name: "Growth Marketer", shape: .egg, colorHex: 0xF97316,
            preview: "A/B copy variants ready to review.", timestamp: "8:05 AM", screenIndex: 5),
        Bot(id: "support", name: "Customer Support", shape: .lozenge, colorHex: 0xF4234B,
            preview: "12 tickets resolved, 2 escalated.", timestamp: "Yesterday", screenIndex: 6),
        Bot(id: "expense", name: "Expense Manager", shape: .triangle, colorHex: 0xEE3E8F,
            eyeStyle: .dot,
            preview: "Receipts coded, one needs a look.", timestamp: "Yesterday", screenIndex: 7),
        Bot(id: "invoice", name: "Invoice Collector", shape: .squircle, colorHex: 0x5B5BE8,
            preview: "Pulled 9 invoices from vendor portals.", timestamp: "Tuesday", screenIndex: 8),
    ]

    static func bot(_ id: String) -> Bot { bots.first { $0.id == id }! }

    /// Sign-in screen confetti.
    /// Fractions are of the canvas; size is in design pt.
    static let signInBlobs: [(BlobShape, UInt32, CGFloat, CGFloat, CGFloat, Double, EyeStyle)] = [
        (.triangle, 0xEE3E8F, 0.5908, 0.0796, 152, -6,  .dot),
        (.circle,   0x7C4DF5, 0.2085, 0.1870, 151,  0,  .capsule),
        (.egg,      0xF97316, 0.8789, 0.2307, 134,  8,  .capsule),
        (.cylinder, 0x6B7280, 0.1215, 0.3566, 126, -8,  .capsule),
        (.hexagon,  0x11B5A0, 0.8979, 0.6056, 145, 10,  .capsule),
        (.teardrop, 0x2F80F0, 0.2183, 0.8042, 128,  0,  .capsule),
        (.arch,     0xE0AE09, 0.7378, 0.9396, 140,  6,  .capsule),
    ]

    // MARK: Threads

    /// Inbox Manager. Prose, prose, card, user, prose.
    static let inboxThread = Thread(botID: "inbox", messages: [
        Message(sender: .bot, body: .systemEvent("Today 8:00 AM")),
        Message(sender: .bot, body: .prose("8am routine done. Inbox triaged, 3 unread left.")),
        Message(sender: .bot, body: .prose("One email from Dan needs a reply. I drafted one in your inbox.")),
        Message(sender: .bot, body: .card(Card(
            title: "New email",
            bodyLines: ["Hi Dan,",
                        "2 PM works on our side. Review moved, invite updated.",
                        "Alex"],
            primary: "Send email",
            secondary: "Discard"))),
        Message(sender: .user, body: .prose("Looks good, send it.")),
        Message(sender: .bot, body: .prose("Done")),
    ])

    /// Chief of staff. Prose, prose, link card, user,
    /// prose, link card with a reaction, user, prose.
    static let cosThread = Thread(botID: "cos", messages: [
        Message(sender: .bot, body: .systemEvent("Today 7:58 AM")),
        Message(sender: .bot, body: .prose("GM! Can I get your approval for the board deck review at 2pm?")),
        Message(sender: .bot, body: .prose("Updated the numbers based on today's pull from Salesforce, Mercury, QuickBooks, and Hex.")),
        Message(sender: .bot, body: .linkFile(LinkFile(
            title: "Q3 board deck v4", subtitle: "docs.google.com", kind: .slides))),
        Message(sender: .user, body: .prose("Can you add a slide for headcount and pull from Rippling")),
        Message(sender: .bot, body: .prose("Pulled and added")),
        Message(sender: .bot, body: .linkFile(LinkFile(
            title: "Q3 board deck v5", subtitle: "docs.google.com", kind: .slides)),
                reaction: "👍"),
        Message(sender: .user, body: .prose("Looks good, send it to the team")),
        Message(sender: .bot, body: .prose("Sent")),
    ])

    /// A composition that exercises the message-type union's remaining arms:
    /// approval card, computer-status glyph, centred system events, attachment
    /// echo.
    static let salesThread = Thread(botID: "sales", messages: [
        Message(sender: .bot, body: .systemEvent("Today 8:35 AM")),
        Message(sender: .bot, body: .prose("I have 40 accounts from the Q3 list. Starting outreach drafts.")),
        Message(sender: .bot, body: .computerStatus(ComputerStatus(
            text: "Working in Acme CRM", active: true))),
        Message(sender: .bot, body: .approval(Approval(
            title: "Sign in to Acme CRM",
            detail: "Sales Outbound wants to use your saved credentials for crm.acme.com."))),
        Message(sender: .user, body: .prose("Go ahead, but check with me before anything sends.")),
        Message(sender: .bot, body: .systemEvent("Always allow set for crm.acme.com")),
        Message(sender: .bot, body: .linkFile(LinkFile(
            title: "Outreach drafts - 40 accounts", subtitle: "3.1 MB · CSV", kind: .sheet))),
    ])

    static func thread(for botID: String) -> Thread {
        switch botID {
        case "inbox": return inboxThread
        case "cos":   return cosThread
        case "sales": return salesThread
        default:
            return Thread(botID: botID, messages: [
                Message(sender: .bot, body: .systemEvent("Today")),
                Message(sender: .bot, body: .prose(bot(botID).preview)),
            ])
        }
    }
}
