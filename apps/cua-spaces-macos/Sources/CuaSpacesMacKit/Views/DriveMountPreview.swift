// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import SwiftUI

/// The first run's Cua Volume page: its animated miniature over one checkbox
/// line, like the AI agents page's background computer-use card
/// (`DriverCardView`). The miniature is a Space whose files fly over, one
/// after another, into the "Cua Volume" volume that appears in a Finder
/// window's sidebar. The scene and every frame are the core's
/// (`appDriveMountPreview`, `appDriveMountPreviewFrame`), so the Tauri app
/// plays the same beats; the card's words and state are the core's
/// `AppDriveCard`.
struct DriveCardView: View {
    let card: AppDriveCard
    let toggle: (Bool) -> Void
    var openSettings: ((URL) -> Void)?
    /// Where the files live: `local`, `s3`, `later`.
    var chooseStorage: ((String) -> Void)?
    /// A bucket row's field edit, choice or button (the row's id).
    var editStorage: ((String, String) -> Void)?
    var chooseStorageRow: ((String, String) -> Void)?
    var pressStorageRow: ((String) -> Void)?
    /// Show a path in Finder (the mounted volume).
    var reveal: ((String) -> Void)?
    var fixedMs: UInt32?
    /// Overrides the system's Reduce Motion (tests).
    var still: Bool?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            // The bucket's prompt or fields take the picture's place.
            if card.storageRows.isEmpty {
                DriveMountPreview(fixedMs: fixedMs, stillOverride: still)
                    .clipShape(.rect(cornerRadius: 6))
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(card.imageLabel)
            }
            if let title = card.storageTitle, !card.storageOptions.isEmpty {
                Text(title).font(.callout).foregroundStyle(.secondary)
                Picker(title, selection: Binding(
                    get: { card.storageOptions.first(where: \.active)?.id ?? "local" },
                    set: { chooseStorage?($0) })) {
                    ForEach(card.storageOptions, id: \.id) { Text($0.label).tag($0.id) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .disabled(card.busy)
                .accessibilityIdentifier("onboarding-storage")
                if !card.storageRows.isEmpty {
                    StorageRowsView(rows: card.storageRows, edit: { editStorage?($0, $1) },
                                    choose: { chooseStorageRow?($0, $1) }, press: { pressStorageRow?($0) })
                }
                if let note = card.storageNote {
                    Text(note).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                if let stored = card.storedIn {
                    PathLine(text: stored, path: card.storedPath)
                }
                if let mounted = card.mountedAt {
                    HStack(spacing: 6) {
                        PathLine(text: mounted, path: card.mountedPath)
                        if let path = card.mountedPath, let reveal {
                            Button("Show in Finder") { reveal(path) }.buttonStyle(.link).font(.callout)
                        }
                    }
                }
                Divider()
            }
            HStack(spacing: 8) {
                Toggle(card.label, isOn: Binding(get: { card.checked }, set: { toggle($0) }))
                    .toggleStyle(.checkbox)
                    .lineLimit(1)
                    .disabled(!card.enabled)
                    .accessibilityIdentifier("onboarding-drive")
                if card.busy { ProgressView().controlSize(.small) }
            }
            if let error = card.error {
                Text(error).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            } else if let note = card.note {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(note).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    if let label = card.settingsLabel, let url = card.settingsUrl.flatMap(URL.init(string:)) {
                        Button(label) { openSettings?(url) }.controlSize(.small)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The bucket's fields on the first run's page, two columns (label, then
/// field), each row one line: the core's rows (`AppSettingsRow`).
struct StorageRowsView: View {
    let rows: [AppSettingsRow]
    let edit: (String, String) -> Void
    let choose: (String, String) -> Void
    let press: (String) -> Void

    var body: some View {
        Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 6) {
            ForEach(rows, id: \.id) { r in
                switch r.kind {
                case .field, .secret:
                    GridRow {
                        Text(r.label).foregroundStyle(.secondary).lineLimit(1)
                        Group {
                            if r.kind == .secret {
                                SecureField("", text: Binding(get: { r.value ?? "" }, set: { edit(r.id, $0) }),
                                            prompt: r.placeholder.map { Text($0) })
                            } else {
                                TextField("", text: Binding(get: { r.value ?? "" }, set: { edit(r.id, $0) }),
                                          prompt: r.placeholder.map { Text($0) })
                            }
                        }
                        .textFieldStyle(.roundedBorder)
                        .controlSize(.small)
                        .disabled(!r.enabled)
                        .accessibilityIdentifier("onboarding-\(r.id)")
                    }
                case .choice:
                    GridRow {
                        Text(r.label).foregroundStyle(.secondary).lineLimit(1)
                        Picker(r.label, selection: Binding(
                            get: { r.options.first(where: \.active)?.id ?? "" },
                            set: { choose(r.id, $0) })) {
                            ForEach(r.options, id: \.id) { Text($0.label).tag($0.id) }
                        }
                        .labelsHidden().pickerStyle(.segmented).controlSize(.small).fixedSize()
                        .disabled(!r.enabled)
                    }
                case .text:
                    GridRow {
                        Text(r.value ?? r.label).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        HStack {
                            Spacer()
                            if let b = r.button { Button(b) { press(r.id) }.controlSize(.small).disabled(!r.enabled) }
                        }
                    }
                case .error, .note:
                    GridRow {
                        Text(r.label).foregroundStyle(r.kind == .error ? Color.red : Color.secondary)
                            .fixedSize(horizontal: false, vertical: true).gridCellColumns(2)
                    }
                case .prompt:
                    GridRow {
                        PromptBox(label: r.label, text: r.value ?? "", copyLabel: r.button ?? "Copy")
                            .gridCellColumns(2)
                    }
                case .toggle:
                    GridRow {
                        Text(r.label).foregroundStyle(.secondary).lineLimit(1)
                        Toggle(r.label, isOn: Binding(
                            get: { r.options.first(where: { $0.id == "on" })?.active ?? false },
                            set: { choose(r.id, $0 ? "on" : "off") }))
                            .labelsHidden().controlSize(.small)
                            .disabled(!r.enabled)
                    }
                case .link:
                    GridRow {
                        Button(r.label) { press(r.id) }.buttonStyle(.link).disabled(!r.enabled)
                            .gridCellColumns(2)
                            .accessibilityIdentifier("onboarding-\(r.id)")
                    }
                }
            }
        }
        .font(.callout)
    }
}

/// A path line: middle-truncated, the full path in the tooltip.
struct PathLine: View {
    let text: String
    let path: String?

    var body: some View {
        Text(text).font(.callout).foregroundStyle(.secondary)
            .lineLimit(1).truncationMode(.middle).help(path ?? text)
    }
}

/// A prompt to hand to a coding agent: a label, the text in a quiet
/// read-only box (selectable, wrapping) and Copy.
struct PromptBox: View {
    let label: String
    let text: String
    let copyLabel: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(label).foregroundStyle(.secondary)
                Spacer()
                Button(copyLabel) {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                    copied = true
                }
                .controlSize(.small)
                .accessibilityIdentifier("storage-prompt-copy")
            }
            Text(text)
                .font(.callout)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
                .background(Color(nsColor: .textBackgroundColor), in: .rect(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color(nsColor: .separatorColor)))
                .accessibilityIdentifier("storage-prompt")
        }
    }
}

/// The page's miniature. It loops while it is on screen in the key window
/// and stops otherwise; with Reduce Motion it is the core's still.
struct DriveMountPreview: View {
    /// A fixed moment in the loop (snapshots); nil plays it.
    var fixedMs: UInt32?
    /// Overrides the system's Reduce Motion (tests).
    var stillOverride: Bool?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.appearsActive) private var appearsActive
    @State private var onScreen = false
    @State private var origin = Date()

    var body: some View {
        let scene = Self.scene
        Group {
            if let fixedMs {
                DriveMountCanvas(scene: scene, frame: appDriveMountPreviewFrame(tMs: fixedMs))
            } else if stillOverride ?? reduceMotion {
                DriveMountCanvas(scene: scene, frame: appDriveMountPreviewStill())
            } else {
                TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !onScreen || !appearsActive)) { context in
                    let elapsed = max(0, context.date.timeIntervalSince(origin)) * 1000
                    let ms = UInt32(elapsed.truncatingRemainder(dividingBy: Double(scene.loopMs)))
                    DriveMountCanvas(scene: scene, frame: appDriveMountPreviewFrame(tMs: ms))
                }
            }
        }
        .frame(width: scene.width, height: scene.height)
        // The desktop spans the card; the stage sits centred in it.
        .frame(maxWidth: .infinity)
        .background {
            LinearGradient(colors: [PreviewColors.wallpaperTop, PreviewColors.wallpaperBottom],
                           startPoint: .top, endPoint: .bottom)
        }
        .onAppear {
            origin = Date()
            onScreen = true
        }
        .onDisappear { onScreen = false }
    }

    @MainActor static let scene = appDriveMountPreview()
}

/// One frame of the miniature on its stage (points, origin top left): the
/// Space, the Finder window with its sidebar and the volume, then the file
/// in flight above both.
struct DriveMountCanvas: View {
    let scene: AppDriveMountPreview
    let frame: AppDriveMountFrame

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.clear.frame(width: scene.width, height: scene.height)
            MiniWindow(window: scene.space) {
                ForEach(Array(scene.sourceIcons.enumerated()), id: \.offset) { _, icon in
                    FileGlyph().frame(width: icon.width, height: icon.height)
                        .offset(x: icon.x - scene.space.frame.x, y: icon.y - scene.space.frame.y)
                }
                ForEach(Array(scene.sourceLabels.enumerated()), id: \.offset) { _, bar in
                    MiniBar(rect: bar, origin: scene.space.frame)
                }
            }
            MiniWindow(window: scene.finder) {
                let f = scene.finder.frame
                Rectangle()
                    .fill(Color.secondary.opacity(0.1))
                    .frame(width: scene.sidebar.width, height: scene.sidebar.height)
                    .offset(x: scene.sidebar.x - f.x, y: scene.sidebar.y - f.y)
                ForEach(Array(scene.places.enumerated()), id: \.offset) { _, bar in
                    MiniBar(rect: bar, origin: f)
                }
                VolumeRow(scene: scene, shown: frame.volume)
                    .offset(x: scene.volume.x - f.x, y: scene.volume.y - f.y)
                ForEach(Array(scene.destIcons.enumerated()), id: \.offset) { i, icon in
                    let shown = i < frame.arrived.count ? frame.arrived[i] : 0
                    Group {
                        FileGlyph().frame(width: icon.width, height: icon.height)
                            .offset(x: icon.x - f.x, y: icon.y - f.y)
                        MiniBar(rect: scene.destLabels[i], origin: f)
                    }
                    .opacity(shown)
                }
            }
            if let at = frame.flight, let icon = scene.sourceIcons.first {
                FileGlyph()
                    .frame(width: icon.width, height: icon.height)
                    .shadow(color: .black.opacity(0.2), radius: 1.5, y: 1)
                    .offset(x: at.x, y: at.y)
            }
        }
        .frame(width: scene.width, height: scene.height, alignment: .topLeading)
        .clipped()
        .accessibilityHidden(true)
    }
}

/// The volume's sidebar row: the selection highlight, a drive glyph and the
/// name, all fading in with `shown`.
private struct VolumeRow: View {
    let scene: AppDriveMountPreview
    let shown: Double

    var body: some View {
        let v = scene.volume
        let icon = scene.volumeIcon
        ZStack(alignment: .topLeading) {
            RoundedRectangle(cornerRadius: 2.5)
                .fill(Color.accentColor.opacity(0.18))
                .frame(width: v.width, height: v.height)
            RoundedRectangle(cornerRadius: 1.2)
                .strokeBorder(Color.secondary.opacity(0.8), lineWidth: 0.7)
                .frame(width: icon.width, height: icon.height)
                .offset(x: icon.x - v.x, y: icon.y - v.y)
            Text(scene.volumeLabel)
                .font(.system(size: scene.fontSize, weight: .medium))
                .foregroundStyle(.primary)
                .fixedSize()
                .frame(height: v.height)
                .offset(x: scene.volumeLabelX - v.x)
        }
        .frame(width: v.width, height: v.height, alignment: .topLeading)
        .opacity(shown)
    }
}

/// A document: a page with a folded corner.
private struct FileGlyph: View {
    var body: some View {
        GeometryReader { g in
            let w = g.size.width, h = g.size.height, fold = w * 0.35
            let page = Path { p in
                p.move(to: .zero)
                p.addLine(to: CGPoint(x: w - fold, y: 0))
                p.addLine(to: CGPoint(x: w, y: fold))
                p.addLine(to: CGPoint(x: w, y: h))
                p.addLine(to: CGPoint(x: 0, y: h))
                p.closeSubpath()
            }
            ZStack {
                page.fill(Color(nsColor: .windowBackgroundColor))
                page.stroke(Color.secondary.opacity(0.6), lineWidth: 0.6)
                Path { p in
                    p.move(to: CGPoint(x: w - fold, y: 0))
                    p.addLine(to: CGPoint(x: w - fold, y: fold))
                    p.addLine(to: CGPoint(x: w, y: fold))
                }
                .stroke(Color.secondary.opacity(0.6), lineWidth: 0.6)
            }
        }
    }
}
