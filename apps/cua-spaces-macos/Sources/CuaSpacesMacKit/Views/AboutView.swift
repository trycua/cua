// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// Settings → About: the icon, name and version, the links, the copyright,
/// then the update controls. Everything shown is the core's `appAboutView`;
/// the updater is Sparkle's.
struct AboutSettingsView: View {
    @Bindable var updates: UpdatesModel
    @State private var showingNotices = false
    @Environment(\.openURL) private var openURL

    var body: some View {
        let v = updates.view
        VStack(spacing: 0) {
            VStack(spacing: 6) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable()
                    .frame(width: 72, height: 72)
                    .accessibilityHidden(true)
                Text(v.title)
                    .font(.title3.weight(.semibold))
                    .accessibilityIdentifier("about-title")
                Text(v.versionLine)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("about-version")
            }
            .padding(.bottom, 14)
            VStack(spacing: 4) {
                ForEach(v.links, id: \.label) { link in
                    Button(link.label) { open(link) }
                        .buttonStyle(.link)
                        .accessibilityIdentifier("about-\(Self.id(link.id))")
                }
            }
            .padding(.bottom, 14)
            Text(v.copyright)
                .font(.caption)
                .foregroundStyle(.secondary)
            if let u = v.updates {
                Divider().padding(.vertical, 16)
                updateControls(u)
            }
        }
        .padding(24)
        .frame(width: 520)
        .sheet(isPresented: $showingNotices) { NoticesView(done: { showingNotices = false }) }
    }

    @ViewBuilder private func updateControls(_ u: AppAboutUpdates) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 8, verticalSpacing: 10) {
            GridRow {
                Color.clear.gridCellUnsizedAxes([.horizontal, .vertical])
                Toggle(u.autoCheckLabel, isOn: Binding(get: { u.autoCheck }, set: updates.setAutoCheck))
                    .toggleStyle(.checkbox)
                    .accessibilityIdentifier("about-auto-check")
            }
            GridRow {
                Color.clear.gridCellUnsizedAxes([.horizontal, .vertical])
                Toggle(u.autoInstallLabel, isOn: Binding(get: { u.autoInstall }, set: updates.setAutoInstall))
                    .toggleStyle(.checkbox)
                    .disabled(!u.autoInstallEnabled)
                    .accessibilityIdentifier("about-auto-install")
            }
            GridRow {
                Text(u.channelLabel).gridColumnAlignment(.trailing)
                HStack(spacing: 6) {
                    Picker(u.channelLabel, selection: Binding(
                        get: { u.channels.first(where: \.active)?.id ?? "stable" },
                        set: { updates.choose(channel: $0) })) {
                        ForEach(u.channels, id: \.id) { Text($0.label).tag($0.id) }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .fixedSize()
                    .accessibilityIdentifier("about-channel")
                    Image(systemName: "info.circle")
                        .foregroundStyle(.secondary)
                        .help(u.channelHelp)
                        .accessibilityLabel(u.channelHelp)
                }
            }
            GridRow {
                Color.clear.gridCellUnsizedAxes([.horizontal, .vertical])
                HStack(spacing: 10) {
                    Button(u.checkLabel) { updates.checkNow() }
                        .disabled(!u.checkEnabled)
                        .accessibilityIdentifier("about-check-now")
                    Text(u.lastCheck)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("about-last-check")
                }
            }
        }
        .fixedSize()
    }

    private func open(_ link: AppAboutLink) {
        if let url = link.url.flatMap(URL.init(string:)) {
            openURL(url)
        } else {
            showingNotices = true
        }
    }

    static func id(_ id: AppAboutLinkId) -> String {
        switch id {
        case .acknowledgements: "acknowledgements"
        case .privacy: "privacy"
        case .terms: "terms"
        case .issue: "issue"
        }
    }
}

/// The bundled third-party notices (THIRD_PARTY_NOTICES.md, copied into
/// Contents/Resources by scripts/build-app.sh).
struct NoticesView: View {
    let done: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(NoticeBlock.parse(Self.text).enumerated()), id: \.offset) { _, block in
                        blockView(block)
                    }
                }
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
            }
            Divider()
            HStack {
                Spacer()
                Button("Done", action: done).keyboardShortcut(.defaultAction)
            }
            .padding(12)
        }
        .frame(width: 620, height: 520)
    }

    static var text: String {
        guard let url = Bundle.main.url(forResource: "THIRD_PARTY_NOTICES", withExtension: "md"),
              let text = try? String(contentsOf: url, encoding: .utf8) else {
            return "The third-party notices are missing from this build (THIRD_PARTY_NOTICES.md)."
        }
        return text
    }

    @ViewBuilder private func blockView(_ block: NoticeBlock) -> some View {
        switch block {
        case .heading(let level, let text):
            Text(inline(text)).font(level == 1 ? .title2.weight(.semibold) : .headline)
                .padding(.top, level == 1 ? 0 : 6)
        case .paragraph(let text):
            Text(inline(text)).fixedSize(horizontal: false, vertical: true)
        case .row(let cells):
            VStack(alignment: .leading, spacing: 2) {
                Text(inline(cells.first ?? ""))
                ForEach(Array(cells.dropFirst().enumerated()), id: \.offset) { _, cell in
                    Text(inline(cell)).font(.callout).foregroundStyle(.secondary)
                }
            }
        case .verbatim(let text):
            Text(text).font(.system(.caption, design: .monospaced))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func inline(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
    }
}

/// The few Markdown blocks the notices use: headings, paragraphs, table
/// rows (the header and separator rows dropped) and fenced license texts.
enum NoticeBlock: Equatable {
    case heading(Int, String)
    case paragraph(String)
    case row([String])
    case verbatim(String)

    static func parse(_ text: String) -> [NoticeBlock] {
        var blocks: [NoticeBlock] = []
        var paragraph: [String] = []
        var fence: [String]?
        var tableRow = 0
        func flush() {
            if !paragraph.isEmpty { blocks.append(.paragraph(paragraph.joined(separator: " "))) }
            paragraph = []
        }
        for raw in text.components(separatedBy: "\n") {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("```") {
                if let f = fence {
                    blocks.append(.verbatim(f.joined(separator: "\n")))
                    fence = nil
                } else {
                    flush()
                    fence = []
                }
                continue
            }
            if fence != nil { fence!.append(raw); continue }
            if line.hasPrefix("|") {
                flush()
                tableRow += 1
                // The header row and the |---| separator.
                if tableRow <= 2 { continue }
                let cells = line.trimmingCharacters(in: CharacterSet(charactersIn: "|"))
                    .components(separatedBy: " | ").map { $0.trimmingCharacters(in: .whitespaces) }
                blocks.append(.row(cells))
                continue
            }
            tableRow = 0
            if line.isEmpty { flush(); continue }
            if line.hasPrefix("#") {
                flush()
                let level = line.prefix(while: { $0 == "#" }).count
                blocks.append(.heading(level, line.dropFirst(level).trimmingCharacters(in: .whitespaces)))
                continue
            }
            paragraph.append(line)
        }
        flush()
        if let f = fence { blocks.append(.verbatim(f.joined(separator: "\n"))) }
        return blocks
    }
}
