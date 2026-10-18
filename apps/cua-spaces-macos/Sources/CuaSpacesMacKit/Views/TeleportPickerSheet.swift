// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// "Teleport an app…": pick, choose what moves, review every install, path
/// and secret, then run. The steps and the gate are the core's.
struct TeleportPickerSheet: View {
    @Bindable var teleport: TeleportModel
    let onClose: () -> Void

    var body: some View {
        let s = teleport.state
        VStack(alignment: .leading, spacing: 0) {
            Text(title(s)).font(.headline).padding(.horizontal, 20).padding(.top, 18).padding(.bottom, 10)
            Group {
                switch s.step {
                case .loading, .planning:
                    ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
                case .pick:
                    pick(s)
                case .options:
                    options(s)
                case .consent:
                    review
                case .running:
                    TeleportRunning(progress: teleport.progress, status: teleport.status)
                case .done:
                    Text("Done").padding(20)
                case .error:
                    Text(s.installPrompt?.message ?? s.error ?? "").foregroundStyle(.secondary).padding(20)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            Divider()
            footer(s).padding(14)
        }
        .frame(width: 640, height: 540)
    }


    private func title(_ s: AppPickerState) -> String {
        if let review = teleport.review { return review.title }
        if let entry = s.entry { return entry.name }
        return "Teleport an App to \(s.spaceName)"
    }

    /// The grid: tabs, search and the core's tiles (the same grid as the
    /// Tauri app's), arrow keys and Return.
    private func pick(_ s: AppPickerState) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("", selection: Binding(get: { teleport.tab }, set: { tab in
                teleport.tab = tab
                Task { await teleport.loadWindows() }
            })) {
                ForEach(teleport.tabs, id: \.label) { t in Text(t.label).tag(t.tab) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            TextField("Search", text: Binding(get: { teleport.query }, set: {
                teleport.setQuery($0)
                Task { await teleport.loadIcons() }
            }))
            .textFieldStyle(.roundedBorder)
            TeleportGrid(teleport: teleport)
        }
        .padding(.horizontal, 20)
        .task { await teleport.loadWindows() }
    }

    private func options(_ s: AppPickerState) -> some View {
        Form {
            Picker("Move", selection: Binding(
                get: { s.moves ?? .appOnly },
                set: { teleport.send(.move(moves: $0)) })) {
                ForEach(s.entry?.moves ?? [], id: \.self) { m in Text(m.label).tag(m) }
            }
            .pickerStyle(.radioGroup)
            // The core's opt-ins, each unchecked until chosen: "Keep me
            // signed in" (the session cookies), "Saved passwords",
            // "Browsing history".
            ForEach(appPickerSensitiveOptions(state: s), id: \.label) { option in
                Toggle(isOn: Binding(get: { option.checked },
                                     set: { teleport.send(.sensitive(group: option.group, value: $0)) })) {
                    Text(option.label)
                    Text(option.detail)
                }
                .toggleStyle(.checkbox)
            }
            ForEach(s.files, id: \.self) { f in
                LabeledContent(appDisplayPath(path: f, home: AppEnvironment.home.path)) {
                    Button("Remove") { teleport.send(.removeFile(path: f)) }
                }
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder private var review: some View {
        if let review = teleport.review {
            TeleportReview(teleport: teleport, review: review)
        }
    }

    @ViewBuilder private func footer(_ s: AppPickerState) -> some View {
        HStack {
            if let leaves = teleport.review?.leavesText {
                Text(leaves).foregroundStyle(.secondary).font(.callout)
            }
            Spacer()
            Button("Cancel", action: onClose).keyboardShortcut(.cancelAction)
            switch s.step {
            case .pick:
                // "Continue", "Teleport to <Space>" or "Stream to This Mac":
                // the core's, live with a choosable tile selected.
                let primary = teleport.primary
                Button(primary.label) {
                    if let tile = teleport.selectedTile { Task { await teleport.activate(tile) } }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!primary.enabled)
            case .options:
                Button("Back") { teleport.send(.back) }
                Button("Review") { Task { await teleport.plan() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!teleport.canPlan)
            case .consent:
                Button("Back") { teleport.send(.back) }
                Button("Teleport") { Task { await teleport.confirm() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(teleport.review?.canConfirm != true)
            case .error:
                Button("Back") { teleport.send(.back) }
            case .done:
                Button("Done", action: onClose).keyboardShortcut(.defaultAction)
            default:
                EmptyView()
            }
        }
    }
}

/// The picker's tiles: a section title per run, three columns of cards
/// (the live preview, the app's icon and one line of name), what the app can
/// take as the tooltip, a dimmed card for one that cannot move. Previews load
/// as tiles appear.
struct TeleportGrid: View {
    @Bindable var teleport: TeleportModel
    static let columns = 3
    @FocusState private var focused: Bool

    var body: some View {
        let grid = teleport.grid
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(Array(grid.sections.enumerated()), id: \.offset) { _, section in
                    if !section.title.isEmpty {
                        Text(section.title).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
                    }
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 12), count: Self.columns),
                              alignment: .leading, spacing: 12) {
                        ForEach(section.tiles, id: \.id) { tile in
                            TeleportTileView(tile: tile, thumbnail: teleport.thumbnail(for: tile),
                                             icon: teleport.icon(for: tile))
                                .onTapGesture(count: 2) { Task { await teleport.activate(tile) } }
                                .onTapGesture { if !tile.disabled { teleport.select(tile.id) } }
                                .task(id: tile.id) { await teleport.loadThumbnail(tile) }
                        }
                    }
                }
                if let empty = grid.emptyText {
                    Text(empty).foregroundStyle(.secondary).frame(maxWidth: .infinity).padding(.top, 40)
                }
            }
            .padding(.vertical, 4)
        }
        .focusable()
        .focused($focused)
        .focusEffectDisabled()
        .onAppear { focused = true }
        .onKeyPress(.rightArrow) { teleport.step(1); return .handled }
        .onKeyPress(.leftArrow) { teleport.step(-1); return .handled }
        .onKeyPress(.downArrow) { teleport.step(Int32(Self.columns)); return .handled }
        .onKeyPress(.upArrow) { teleport.step(-Int32(Self.columns)); return .handled }
        .task(id: grid.sections.flatMap(\.tiles).map(\.id)) { await teleport.loadIcons() }
    }
}

/// One card of the grid.
struct TeleportTileView: View {
    let tile: AppPickerTile
    let thumbnail: NSImage?
    let icon: NSImage?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ZStack {
                RoundedRectangle(cornerRadius: 8).fill(.quaternary)
                if let thumbnail {
                    Image(nsImage: thumbnail).resizable().aspectRatio(contentMode: .fill)
                } else if let icon {
                    // No window to preview: the app's icon, large.
                    Image(nsImage: icon).resizable().interpolation(.high).frame(width: 44, height: 44)
                }
            }
            .frame(height: 104)
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(tile.selected ? Color.accentColor : Color.primary.opacity(0.08),
                                  lineWidth: tile.selected ? 2.5 : 1)
            }
            HStack(spacing: 6) {
                if let icon {
                    Image(nsImage: icon).resizable().interpolation(.high).frame(width: 16, height: 16)
                }
                Text(tile.title).lineLimit(1).truncationMode(.tail)
            }
            .font(.callout)
        }
        .opacity(tile.disabled ? 0.5 : 1)
        .contentShape(Rectangle())
        .help(tile.help)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(tile.title)
        .accessibilityHint(tile.help)
        .accessibilityAddTraits(tile.selected ? [.isButton, .isSelected] : .isButton)
    }
}

/// The run: the bar, and under it the step it is on in words, so the
/// Keychain prompts that come with reading a browser's cookies are expected.
struct TeleportRunning: View {
    let progress: Double
    let status: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ProgressView(value: progress)
            Text(status ?? " ")
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
                .contentTransition(.opacity)
                .animation(.easeInOut(duration: 0.15), value: status)
                .accessibilityIdentifier("teleport-run-status")
        }
        .padding(20)
    }
}
