// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation

/// The signed-in account, as the shell shows it.
///
/// There is no account system behind this — the sign-in gate is local — so this
/// holds the few strings the chrome needs rather than scattering them through
/// the views. It used to carry a six-row account menu as well; every row was
/// inert, so the menu is gone and what is left is the identity the sidebar
/// displays.
enum Account {
    /// The name on the sidebar's account row.
    static let displayName = "Alex Rivera"

    /// The monogram in the account row's circle.
    static var initials: String {
        let parts = displayName.split(separator: " ")
        let letters = parts.prefix(2).compactMap { $0.first }
        return String(letters).uppercased()
    }

    /// What a Bot calls the user. A greeting reads "Hey Alex, …", so it is
    /// the first name, not the display name.
    static var firstName: String {
        String(displayName.split(separator: " ").first ?? "there")
    }

}

/// Strings the shell shows when there is nothing to show.
///
/// Empty states are where careless copy hides most easily, and the copy this
/// app used to carry ("not hired yet") is the defect that prompted this
/// rewrite. They are kept together here so they are reviewed as a set.
enum EmptyStateCopy {
    /// The sidebar, with no conversations in it.
    static let noBots = "No saved Bots yet."
    /// The stage, on a brand-new account: a sentence and an action, not a
    /// shrug.
    static let newAccount = "This account doesn\u{2019}t have a Bot yet."
    static let createFirst = "Create your first Bot"
    /// The stage, with nothing selected but Bots to select.
    static let noChats = "No Bots yet"
    /// Every Bot exists but every Bot is hidden.
    static let hiddenSection = "Hidden Bots"
    static let showHidden = "Show Hidden Bots"
    static let hiddenExplainer =
        "Hidden Bots stay active and keep their history, they just don\u{2019}t show in the sidebar."
    /// What a teammate's shared Bots say for themselves. Team bots arrive
    /// hidden by construction, so this is the copy that explains why the
    /// sidebar did not grow when one was shared.
    static let teamBotsExplainer =
        "Agents your teammates shared. Add one to keep it in your sidebar; the rest stay here."
    /// The roster could not be read. Two lines: what happened, and the
    /// reassurance. The second line is the one that matters and is the one
    /// most easily dropped.
    static let unreachableTitle = "Can\u{2019}t reach your computer"
    static let unreachableBody =
        "Your agents are safe \u{2014} they just can\u{2019}t be loaded right now."
    /// The subtitle a row carries between its agent starting and its first
    /// word arriving. The **only** transient state a row has, and it is about
    /// something that is actually happening.
    static let creating = "Creating\u{2026}"
    /// The sidebar **hover card** for a Bot with no messages. Deliberately not
    /// the row: a row with no messages shows its title and no subtitle line at
    /// all, with nothing standing in for the missing text.
    static let noMessagesYet = "No messages yet"
}
