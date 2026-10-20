// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import Foundation

/// The live shell's own metrics.
///
/// These are **not** `DT`. `DT` holds the numbers the D1/D2/D3 export renders
/// use (245pt sidebar, 562pt column, 11pt radius) on the fixed 2560x1515
/// desktop canvas. The live shell is a different size and a different radius
/// scale, and the export renders must not move, so the two live side by side
/// rather than one overwriting the other.
enum Metrics {
    /// Sidebar: 280 by default, draggable between 240 and 400, and 88 when
    /// collapsed to a rail.
    ///
    /// These are the design values, written down once here. Every size in this
    /// file comes from this table, never from an estimate made by eye.
    static let sidebarW: CGFloat = 280
    static let sidebarMinW: CGFloat = 240
    static let sidebarMaxW: CGFloat = 400
    static let sidebarRailW: CGFloat = 88

    /// The transcript column's *preferred* width. The layout does not size the
    /// transcript directly (it sizes the two side panes and the transcript
    /// takes what is left), so this is a cap, not an allocation.
    static let transcriptW: CGFloat = 690
    /// The width the transcript is guaranteed. The details pane's width is
    /// computed as `windowWidth - 424 - sidebar`, so this reservation is what
    /// stops the pane from squeezing the conversation out of existence.
    static let transcriptReservedW: CGFloat = 424

    /// The details / settings pane: 320 by default, 280 minimum, no maximum.
    static let detailsW: CGFloat = 320
    static let detailsMinW: CGFloat = 280

    /// The details pane's width in a window of `windowWidth`, with the sidebar
    /// at `sidebar`: what is left after the sidebar and the transcript's
    /// reservation, then clamped.
    static func detailsWidth(windowWidth: CGFloat, sidebar: CGFloat = sidebarW) -> CGFloat {
        let available = windowWidth - transcriptReservedW - sidebar
        return max(detailsMinW, min(detailsW, available))
    }

    /// Whether a window is wide enough to show the pane at all. Below this the
    /// pane cannot reach its 280 minimum without eating the transcript's 424.
    static func fitsDetails(windowWidth: CGFloat, sidebar: CGFloat = sidebarW) -> Bool {
        windowWidth - transcriptReservedW - sidebar >= detailsMinW
    }

    /// A bubble is **not** full width. Its cap is the smallest of 88% of the
    /// column, 640pt, and the column minus 82pt, which at a 690pt column
    /// resolves to 608pt: the 82pt gutter wins, not the 640pt cap and not the 88%.
    static let bubbleFraction: CGFloat = 0.88
    static let bubbleAbsoluteMax: CGFloat = 640
    static let bubbleGutter: CGFloat = 82

    /// The width a bubble may occupy inside a column of `width`.
    static func bubbleMaxWidth(in width: CGFloat) -> CGFloat {
        min(width * bubbleFraction, bubbleAbsoluteMax, width - bubbleGutter)
    }

    static let headerH: CGFloat = 48
    static let rowH: CGFloat = 46
    static let composerH: CGFloat = 40
    static let avatar: CGFloat = 24
}

/// The radius scale: 8/10/14/16/18.
///
/// An earlier 6/8/12/14/16 scale is two points smaller at every step, which is
/// small enough to look like a rendering difference and never be questioned.
/// The live values are below.
enum Corner {
    static let base: CGFloat = 8
    static let lg: CGFloat = 10
    static let xl: CGFloat = 14
    static let xxl: CGFloat = 16
    static let xxxl: CGFloat = 18

    /// The transcript bubble's corner. The largest step, not a bespoke number.
    static var bubble: CGFloat { xxxl }
}

/// What the right-hand pane is showing, and what the header controls therefore
/// are.
///
/// This exists as a value rather than two booleans because the header's
/// controls are a function of it and getting that function wrong is the defect
/// the user named.
///
/// **The monitor control does not morph.** It *unmounts* when the pane opens,
/// and the pane's **own** header mounts a separate close button (a double
/// chevron) whose accessibility label and tooltip are both `Close details`,
/// at roughly the same place on screen. Two controls in one position, not one control with
/// two icons. That is why `headerControls` (the conversation header) and
/// `paneControls` (the pane's header) are separate lists: built as a morphing
/// toggle, the focus order and the layout are both subtly wrong, and the
/// wrongness is invisible in a screenshot.
enum DetailsPane: Equatable {
    /// No pane. The header shows the monitor control; the transcript is full width.
    case closed
    /// The conversation-details pane. The monitor control is gone; a gear and a
    /// `»` are mounted in its place. The transcript **narrows**: the pane is
    /// not an overlay.
    case details
    /// `Settings`, in the same pane. The gear is gone; a back `‹` is mounted in
    /// its place, highlighted. The `»` stays at the far right.
    case settings

    var isOpen: Bool { self != .closed }

    /// One header control: what it is, what it does, and what a screen reader
    /// and a tooltip call it.
    struct Control: Equatable {
        enum Kind: Equatable { case monitor, gear, back, collapse }
        var kind: Kind
        /// The tooltip.
        var tooltip: String
        /// Drawn with the highlighted (filled) treatment.
        var isHighlighted: Bool = false
    }

    /// The controls mounted in the header's trailing group, in order,
    /// left to right.
    ///
    /// `botName` is in the monitor control's label because the label names
    /// whose computer it is: `<Bot>'s Computer`, and `<Bot>'s Computer, in use`
    /// while the Bot is driving it.
    /// `viewerIsOwner` and `inSubpage` gate the gear: it is mounted only when
    /// a settings handler exists, the viewer owns the Bot, and the pane is not
    /// already inside a subpage.
    func headerControls(botName: String, computerInUse: Bool = false,
                        viewerIsOwner: Bool = true,
                        canOpenSettings: Bool = true) -> [Control] {
        // There is no share control. It was mounted, it was tooltipped, and
        // tapping it returned the pane unchanged, a control that looked alive
        // and did nothing. Deleted rather than disabled.
        let gearAllowed = viewerIsOwner && canOpenSettings
        switch self {
        case .closed:
            // The monitor control is mounted only while the pane is closed.
            return [Control(kind: .monitor,
                            tooltip: computerInUse ? "\(botName)\u{2019}s Computer, in use"
                                                   : "\(botName)\u{2019}s Computer")]
        case .details:
            return gearAllowed ? [Control(kind: .gear, tooltip: "Settings")] : []
        case .settings:
            // In a subpage, so no gear: the back control replaces it.
            return [Control(kind: .back, tooltip: "Back", isHighlighted: true)]
        }
    }

    /// The controls mounted in the **pane's own** header. The close button
    /// lives here, not in the conversation header, and it is a different
    /// control from the monitor button it appears to replace.
    func paneControls() -> [Control] {
        isOpen ? [Control(kind: .collapse, tooltip: "Close details")] : []
    }

    /// Where a control takes the pane.
    func tapping(_ kind: Control.Kind) -> DetailsPane {
        switch kind {
        case .monitor:  return .details
        case .gear:     return .settings
        case .back:     return .details
        case .collapse: return .closed
        }
    }

    /// The pane's title, when it has one. The details pane has no title bar;
    /// `Settings` does.
    var title: String? { self == .settings ? "Settings" : nil }

    /// The pane's own width in a window of `total`. Zero when closed.
    func paneWidth(in total: CGFloat, sidebar: CGFloat = Metrics.sidebarW) -> CGFloat {
        guard isOpen, Metrics.fitsDetails(windowWidth: total, sidebar: sidebar) else { return 0 }
        return Metrics.detailsWidth(windowWidth: total, sidebar: sidebar)
    }

    /// The width the transcript column gets inside `total`. The pane takes its
    /// width out of the content, never over it, which is the other half of
    /// what the user saw go wrong: an overlaid panel hides the conversation it
    /// is describing.
    func transcriptWidth(in total: CGFloat, sidebar: CGFloat = Metrics.sidebarW) -> CGFloat {
        let available = total - sidebar - paneWidth(in: total, sidebar: sidebar)
        return max(Metrics.transcriptReservedW, min(Metrics.transcriptW, available))
    }
}

/// The header's compose mode.
///
/// The sidebar `+` does not open a sheet: it **replaces** the conversation
/// header with a `To:` field and hides the monitor control, because
/// there is no conversation yet for them to act on. The composer's placeholder
/// changes with it.
struct ComposeState: Equatable {
    var isComposing: Bool = false
    var query: String = ""

    /// Fixed strings, pinned by tests.
    static let toLabel = "To:"
    static let placeholder = "Search or create Bots"
    /// The composer's placeholder while composing: `Bot`, not a Bot's name,
    /// because no Bot has been chosen.
    static let composerPlaceholder = "Message Bot"

    /// The dropdown's two fixed rows, above the list of existing Bots.
    static let createNewBot = "Create new Bot"
    static let createGroupChat = "Create group chat"

    /// What the dropdown offers for a query: the two create rows always, then
    /// the Bots whose names match.
    static func suggestions(query: String, bots: [Bot]) -> (fixed: [String], bots: [Bot]) {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let matched = q.isEmpty ? bots : bots.filter { $0.name.lowercased().contains(q) }
        return ([createNewBot, createGroupChat], matched)
    }
}

/// The details pane's strings.
enum DetailsStrings {
    static func screenCaption(_ botName: String) -> String { "\(botName)\u{2019}s screen" }
    static let routines =
        "Routines are recurring tasks this Bot runs on a schedule. "
        + "Ask it in chat to set one up."
}

/// The settings pane's fields, in order.
enum SettingsStrings {
    static let title = "Settings"
    static let name = "Name"
    static let description = "Description"
    static let voice = "Voice"
    static let speed = "Speed"
    static let language = "Language"
    static let notifications = "Notifications"
    static let notificationsDetail = "Get notified when this Bot finishes or needs input"

    static let voiceDefault = "Not set"
    static let speedDefault = "1x"
    static let languageDefault = "Auto-detect"
    static let descriptionDefault =
        "A teammate with its own computer that takes real work off your plate."

    /// The dropdown choices. Each list contains its default above.
    static let speeds = ["0.5x", "0.75x", "1x", "1.25x", "1.5x", "2x"]
    static let languages = ["Auto-detect", "English", "Español", "Français",
                            "Deutsch", "日本語", "中文"]
    static let voices = ["Not set", "Calm", "Bright", "Warm"]
}

/// The pinned bottom of the sidebar.
enum SidebarStrings {
    static let search = "Search"
}
