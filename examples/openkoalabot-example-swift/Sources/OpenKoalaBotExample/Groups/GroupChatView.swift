// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import SwiftUI

/// The group chat surface. It is not part of the export renders. What it
/// shares with the one-Bot thread is the design vocabulary: the same bubble
/// radius, gutters, avatars and type scale.
///
/// The one thing the design is *for*: in a one-Bot thread every bubble on the
/// left is the same speaker, so no attribution is needed. In a group it is the
/// whole problem. So each run of consecutive lines from one Bot carries that
/// Bot's avatar and name, and the bubble is tinted with a hairline in the Bot's
/// own colour: legible at a glance, not a rainbow of filled bubbles.
///
/// Mount point for the app shell: `GroupChatScreen(chatID:store:bots:)`, pushed
/// from the roster's "New group" action.
struct GroupChatScreen: View {
    let chatID: String
    @ObservedObject var store: GroupChatStore
    /// Roster lookup for avatars and names.
    var bots: [Bot]
    var onBack: (() -> Void)? = nil

    @State private var draft: String = ""
    @State private var showingMembers = false

    private var chat: GroupChat? { store.chat(chatID) }
    private func bot(_ id: String) -> Bot? { bots.first { $0.id == id } }

    var body: some View {
        VStack(spacing: 0) {
            header
            if let chat {
                transcript(chat)
            } else {
                Spacer()
                Text("This group no longer exists.")
                    .font(DS.font(34)).foregroundStyle(DS.secondary)
                Spacer()
            }
            if let error = store.lastError {
                limitBanner(error)
            }
            composer
        }
        .background(DS.bg)
        .sheet(isPresented: $showingMembers) {
            if let chat {
                GroupMembersSheet(chat: chat, store: store, bots: bots) {
                    showingMembers = false
                }
            }
        }
    }

    // MARK: Header: the membership count is always on screen.

    private var header: some View {
        HStack(spacing: 0) {
            Button { onBack?() } label: {
                CircleIcon(symbol: "arrow.left", weight: .semibold, scale: 0.40,
                           diameter: DS.iconButtonLg)
            }
            .buttonStyle(.plain)

            HStack(spacing: 18) {
                stackedAvatars
                VStack(alignment: .leading, spacing: 2) {
                    Text(chat?.title ?? "Group")
                        .font(DS.font(36, .bold))
                        .foregroundStyle(DS.onSurface)
                    Text(chat?.membershipLabel ?? "")
                        .font(DS.font(26))
                        .foregroundStyle(DS.secondary)
                }
            }
            .padding(.leading, 13)
            .padding(.trailing, 30)
            .frame(height: DS.headerChipH)
            .background(Capsule().fill(DS.surface))
            .padding(.leading, 14)

            Spacer(minLength: 0)

            Button { showingMembers = true } label: {
                CircleIcon(symbol: "person.2", scale: 0.34, diameter: DS.iconButtonLg)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, DS.gutter)
        .padding(.top, 13)
    }

    /// Overlapping avatars, capped at four with a "+n": six full-size avatars
    /// would not fit the header chip at any of the design type sizes.
    private var stackedAvatars: some View {
        let ids = chat?.memberIDs ?? []
        let shown = Array(ids.prefix(4))
        let extra = ids.count - shown.count
        return HStack(spacing: -18) {
            ForEach(shown, id: \.self) { id in
                if let b = bot(id) {
                    BotAvatar(bot: b, size: DS.headerChipH * 0.60)
                        .overlay(Circle().stroke(DS.surface, lineWidth: 4))
                }
            }
            if extra > 0 {
                Text("+\(extra)")
                    .font(DS.font(24, .bold))
                    .foregroundStyle(DS.secondary)
                    .padding(.leading, 22)
            }
        }
    }

    // MARK: Transcript

    private func transcript(_ chat: GroupChat) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 26) {
                ForEach(Array(chat.messages.enumerated()), id: \.element.id) { i, m in
                    let previous = i > 0 ? chat.messages[i - 1].speaker : nil
                    GroupMessageRow(message: m,
                                    bot: m.speaker.botID.flatMap(bot),
                                    showsAttribution: m.speaker != previous)
                }
                ForEach(store.workingBots(in: chat.id), id: \.self) { id in
                    if let b = bot(id) {
                        HStack(spacing: 16) {
                            BotAvatar(bot: b, size: 54)
                            TypingIndicator(tint: b.color)
                        }
                    }
                }
            }
            .padding(.horizontal, DS.transcriptLead)
            .padding(.top, 34)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func limitBanner(_ text: String) -> some View {
        HStack(spacing: 14) {
            Image(systemName: "exclamationmark.circle.fill")
                .font(.system(size: 28))
                .foregroundStyle(Color(hex: 0xE0AE09))
            Text(text)
                .font(DS.font(27, .medium))
                .foregroundStyle(DS.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 78)
        .padding(.bottom, 10)
    }

    private var composer: some View {
        HStack(spacing: 18) {
            HStack(spacing: 0) {
                TextField("Message the group", text: $draft)
                    .textFieldStyle(.plain)
                    .font(DS.font(DS.bodySize))
                    .foregroundStyle(DS.onSurface)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 34)
            .frame(height: DS.composerH)
            .background(Capsule().fill(DS.surface))

            Button {
                let text = draft
                draft = ""
                Task {
                    await store.send(text, in: chatID)
                    await store.collectReplies(in: chatID)
                }
            } label: {
                ZStack {
                    Circle().fill(DS.userBubble)
                    Image(systemName: "arrow.up")
                        .font(.system(size: 40, weight: .semibold))
                        .foregroundStyle(DS.onUserBubble)
                }
                .frame(width: DS.composerH, height: DS.composerH)
            }
            .buttonStyle(.plain)
            .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(.horizontal, 78)
        .padding(.bottom, 92)
    }
}

/// One attributed line of a group transcript.
struct GroupMessageRow: View {
    var message: GroupMessage
    var bot: Bot?
    var showsAttribution: Bool

    var body: some View {
        switch message.speaker {
        case .system:
            Text(message.text)
                .font(DS.font(30))
                .foregroundStyle(message.undelivered ? Color(hex: 0xE0AE09) : DS.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .center)

        case .human:
            HStack(spacing: 0) {
                Spacer(minLength: 0)
                bubble(fill: DS.userBubble, text: DS.onUserBubble, stroke: nil)
            }

        case .bot(let id):
            HStack(alignment: .top, spacing: 16) {
                // The avatar column keeps its width on continuation lines so
                // consecutive bubbles from one Bot stay in one visual run.
                Group {
                    if showsAttribution, let bot {
                        BotAvatar(bot: bot, size: 54)
                    } else {
                        Color.clear
                    }
                }
                .frame(width: 54, height: 54)

                VStack(alignment: .leading, spacing: 8) {
                    if showsAttribution {
                        Text(bot?.name ?? id)
                            .font(DS.font(27, .semibold))
                            .foregroundStyle(bot?.color ?? DS.secondary)
                    }
                    bubble(fill: DS.surface, text: DS.onSurface,
                           stroke: message.undelivered ? Color(hex: 0xE0AE09)
                                                       : (bot?.color.opacity(0.45) ?? .clear))
                }
                Spacer(minLength: 0)
            }
        }
    }

    private func bubble(fill: Color, text: Color, stroke: Color?) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(message.text)
                .font(DS.font(DS.bodySize))
                .foregroundStyle(text)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, DS.bubblePadH)
                .padding(.vertical, DS.bubblePadV)
                .background(
                    RoundedRectangle(cornerRadius: DS.bubbleRadius, style: .continuous)
                        .fill(fill)
                        .overlay(RoundedRectangle(cornerRadius: DS.bubbleRadius,
                                                  style: .continuous)
                            .stroke(stroke ?? .clear, lineWidth: stroke == nil ? 0 : 3)))
                .frame(maxWidth: DS.bubbleMax, alignment: .leading)
            if let r = message.reaction {
                ReactionChip(emoji: r).offset(x: 14, y: -6)
            }
        }
    }
}

/// Add and remove members, with the 2–6 bound visible rather than enforced by
/// a disabled control with no explanation.
struct GroupMembersSheet: View {
    let chat: GroupChat
    @ObservedObject var store: GroupChatStore
    var bots: [Bot]
    var onDone: () -> Void

    @State private var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(chat.title).font(.system(size: 15, weight: .semibold))
            Text("\(chat.membershipLabel), \(GroupChat.minBots) to \(GroupChat.maxBots) bots "
                 + "plus you.")
                .font(.system(size: 11)).foregroundStyle(.secondary)

            if chat.isFull {
                label("This group is full. Remove a bot to add another.", 0xE0AE09)
            } else if chat.isAtFloor {
                label("At the minimum of \(GroupChat.minBots) bots: none can be removed.",
                      0xE0AE09)
            }

            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(bots) { b in
                        let member = chat.memberIDs.contains(b.id)
                        HStack(spacing: 10) {
                            BotAvatar(bot: b, size: 20)
                            Text(b.name).font(.system(size: 12))
                            Spacer()
                            Button(member ? "Remove" : "Add") {
                                do {
                                    if member { try store.remove(b.id, from: chat.id) }
                                    else { try store.add(b.id, to: chat.id) }
                                    error = nil
                                } catch { self.error = "\(error)" }
                            }
                            .disabled(member ? chat.isAtFloor : chat.isFull)
                        }
                    }
                }
            }
            .frame(height: 220)

            if let error { label(error, 0xF4234B) }

            HStack { Spacer(); Button("Done", action: onDone).keyboardShortcut(.defaultAction) }
        }
        .padding(20)
        .frame(width: 360)
    }

    private func label(_ text: String, _ hex: UInt32) -> some View {
        Text(text).font(.system(size: 11)).foregroundStyle(Color(hex: hex))
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// Pick 2–6 bots and open a group. The Create button stays
/// disabled below two and selection stops at six, and in both cases the reason
/// is written on screen rather than left to be inferred from a greyed control.
struct NewGroupSheet: View {
    var bots: [Bot]
    @ObservedObject var store: GroupChatStore
    var onCreated: (GroupChat) -> Void
    var onCancel: () -> Void

    @State private var title: String = "New group"
    @State private var selected: [String] = []
    @State private var error: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("New group chat").font(.system(size: 15, weight: .semibold))
            TextField("Name", text: $title)

            Text("\(selected.count) of \(GroupChat.maxBots) selected, pick at least "
                 + "\(GroupChat.minBots).")
                .font(.system(size: 11))
                .foregroundStyle(selected.count > GroupChat.maxBots ? Color(hex: 0xF4234B)
                                                                    : .secondary)

            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(bots) { b in
                        let on = selected.contains(b.id)
                        let blocked = !on && selected.count >= GroupChat.maxBots
                        Button {
                            if on { selected.removeAll { $0 == b.id } }
                            else if !blocked { selected.append(b.id) }
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: on ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(on ? Color(hex: 0x8B5CF6) : .secondary)
                                BotAvatar(bot: b, size: 20)
                                Text(b.name).font(.system(size: 12))
                                Spacer()
                            }
                        }
                        .buttonStyle(.plain)
                        .opacity(blocked ? 0.4 : 1)
                        .help(blocked ? "A group holds at most \(GroupChat.maxBots) bots." : "")
                    }
                }
            }
            .frame(height: 240)

            if let error {
                Text(error).font(.system(size: 11)).foregroundStyle(Color(hex: 0xF4234B))
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                Button("Create") {
                    do { onCreated(try store.create(title: title, members: selected)) }
                    catch { self.error = "\(error)" }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!GroupChatStore.canCreate(with: selected))
            }
        }
        .padding(20)
        .frame(width: 360)
    }
}
