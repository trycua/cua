// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#if canImport(AppKit)
import AppKit
import Cua
import CuaSpacesFFI
import SwiftUI
import UniformTypeIdentifiers

/// "Teleport an app…": search, icons, recents, capability levels, the move
/// choice, the consent screen listing every install, path and secret, and
/// progress. A normal window's content (present it in a sheet or window).
public struct TeleportAppPicker: View {
    @ObservedObject var model: TeleportPickerModel
    var onClose: () -> Void
    @State private var importing = false

    public init(model: TeleportPickerModel, onClose: @escaping () -> Void) {
        self.model = model
        self.onClose = onClose
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            switch model.step {
            case .loading:
                header("Teleport an app to \(model.spaceName)")
                ProgressView("Looking for apps on this machine…").frame(maxWidth: .infinity, maxHeight: .infinity)
            case .pick: pick
            case .options: options
            case .planning:
                header(model.entry?.name ?? "")
                ProgressView("Working out what will move…").frame(maxWidth: .infinity, maxHeight: .infinity)
            case .consent: consent
            case .running: running
            case .done: done
            case .error: failure
            }
        }
        .padding(16)
        .frame(minWidth: 460, minHeight: 420)
        .task { if model.step == .loading { await model.load() } }
    }

    private func header(_ text: String) -> some View {
        Text(text).font(.headline)
    }

    private func icon(_ e: TeleportCatalogEntry) -> some View {
        Group {
            if let data = model.icons[e.id], let image = NSImage(data: data) {
                Image(nsImage: image).resizable().interpolation(.high)
            } else {
                RoundedRectangle(cornerRadius: 6).fill(Color.secondary.opacity(0.15))
                    .overlay(Text(String(e.name.prefix(1)).uppercased()).font(.system(size: 13, weight: .semibold)))
            }
        }
        .frame(width: 28, height: 28)
    }

    private var pick: some View {
        VStack(alignment: .leading, spacing: 10) {
            header("Teleport an app to \(model.spaceName)")
            TextField("Search apps", text: $model.query).textFieldStyle(.roundedBorder)
            List(selection: Binding(get: { model.selectedId }, set: { if let id = $0 { model.select(id) } })) {
                ForEach(model.sections, id: \.title) { section in
                    Section(section.title) {
                        ForEach(section.entries, id: \.id) { e in
                            row(e).tag(e.id)
                                .disabled(e.capability == .unsupported)
                                .onTapGesture(count: 2) { model.choose(e.id) }
                        }
                    }
                }
            }
            .accessibilityLabel("Apps")
            HStack {
                Spacer()
                Button("Cancel", action: onClose).keyboardShortcut(.cancelAction)
                Button("Continue") { model.choose() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(model.selectedId == nil)
            }
        }
    }

    private func row(_ e: TeleportCatalogEntry) -> some View {
        HStack(spacing: 10) {
            icon(e)
            VStack(alignment: .leading, spacing: 1) {
                Text(e.name)
                if e.capability == .unsupported, let why = e.reason {
                    Text(why).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            Text(TeleportPickerModel.label(e.capability))
                .font(.caption)
                .foregroundStyle(e.capability == .full ? Color.accentColor : .secondary)
        }
        .opacity(e.capability == .unsupported ? 0.55 : 1)
        .help(e.capability == .unsupported ? (e.reason ?? "") : TeleportPickerModel.label(e.capability))
    }

    private var options: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let e = model.entry {
                HStack { icon(e); header(e.name) }
                Text("What should move to \(model.spaceName)?").foregroundStyle(.secondary)
                Picker("", selection: Binding(get: { model.move ?? .appOnly }, set: { model.setMove($0) })) {
                    ForEach(e.moves, id: \.self) { m in Text(TeleportPickerModel.label(m)).tag(m) }
                }
                .pickerStyle(.radioGroup)
                .labelsHidden()
                if model.move == .appWithFiles {
                    ForEach(model.files, id: \.self) { f in
                        HStack {
                            Text(f).font(.system(.caption, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                            Spacer()
                            Button("Remove") { model.removeFile(f) }
                        }
                    }
                    Button("Add files or folders…") { importing = true }
                }
                if let why = e.reason { Text(why).font(.caption).foregroundStyle(.secondary) }
            }
            Spacer()
            HStack {
                Button("Back") { model.back() }
                Spacer()
                Button("Review") { Task { await model.makePlan() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!model.canPlan)
            }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.item, .folder], allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { model.addFiles(urls.map(\.path)) }
        }
    }

    private var consent: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let p = model.plan {
                header("Teleport \(p.app.name) to \(model.spaceName)?")
                Text(p.steps.map(\.summary).joined(separator: " · ")).font(.caption).foregroundStyle(.secondary)
                List(p.consent, id: \.key) { item in
                    HStack(alignment: .firstTextBaseline) {
                        Text(String(describing: item.kind).uppercased())
                            .font(.caption2)
                            .foregroundStyle(item.sensitive ? Color.red : .secondary)
                        VStack(alignment: .leading) {
                            Text(item.label)
                            Text(item.detail).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if item.bytes > 0 { Text(TeleportPickerModel.bytes(item.bytes)).font(.caption) }
                    }
                }
                Text(p.totalBytes > 0 ? "\(TeleportPickerModel.bytes(p.totalBytes)) leaves this machine."
                     : "No files or app data leave this machine.")
                    .font(.caption).foregroundStyle(.secondary)
                ForEach(p.warnings, id: \.self) { Text($0).font(.caption).foregroundStyle(.secondary) }
                if p.sensitive {
                    Toggle("I understand the secrets above leave this machine.", isOn: $model.acknowledged)
                }
            }
            HStack {
                Button("Back") { model.back() }
                Spacer()
                Button("Teleport") { Task { await model.confirm() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!model.canConfirm)
            }
        }
    }

    private var running: some View {
        VStack(alignment: .leading, spacing: 10) {
            header("Teleporting \(model.plan?.app.name ?? "") to \(model.spaceName)…")
            ProgressView(value: model.progress)
            Text(model.status ?? " ")
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .contentTransition(.opacity)
                .animation(.easeInOut(duration: 0.15), value: model.status)
                .accessibilityIdentifier("teleport-run-status")
            Spacer()
        }
    }

    private var done: some View {
        VStack(alignment: .leading, spacing: 10) {
            header("\(model.plan?.app.name ?? "The app") is in \(model.spaceName)")
            if let r = model.report {
                Text([
                    r.installed.isEmpty ? nil : "Installed \(r.installed.joined(separator: ", ")).",
                    r.sent.isEmpty ? nil : "Sent \(r.sent.count) item(s).",
                    r.imported.isEmpty ? nil : "Imported \(r.imported.count) item(s).",
                    r.launched ? "Opened." : nil,
                ].compactMap { $0 }.joined(separator: " ")).foregroundStyle(.secondary)
            }
            Spacer()
            HStack { Spacer(); Button("Done", action: onClose).keyboardShortcut(.defaultAction) }
        }
    }

    @ViewBuilder private var failure: some View {
        if let prompt = model.installPrompt {
            installCua(prompt)
        } else {
            rawFailure
        }
    }

    /// The Keyvault refused (embedded SDK, by design): say how to get Cua.
    private func installCua(_ prompt: InstallCuaPrompt) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            header(prompt.title)
            Text(prompt.message).foregroundStyle(.secondary)
            Spacer()
            HStack {
                Button("Close", action: onClose)
                Spacer()
                Button("Back") { model.back() }
                Link(prompt.actionLabel, destination: prompt.url)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
            }
        }
    }

    private var rawFailure: some View {
        VStack(alignment: .leading, spacing: 10) {
            header("Could not teleport\(model.entry.map { " \($0.name)" } ?? "")")
            Text(model.error ?? "").foregroundStyle(.red)
            Spacer()
            HStack {
                Button("Close", action: onClose)
                Spacer()
                Button("Back") { model.back() }.keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// The drop zone while a window is dragged: its preview (in memory), app,
/// capability and the target Space.
public struct WindowDropZone: View {
    let state: WindowDropState
    let spaceName: String

    public init(state: WindowDropState, spaceName: String) {
        self.state = state
        self.spaceName = spaceName
    }

    public var body: some View {
        if state.active, let app = state.app {
            HStack(spacing: 12) {
                Group {
                    if let png = state.thumbnail, let image = NSImage(data: png) {
                        Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
                    } else {
                        Color.secondary.opacity(0.15)
                    }
                }
                .frame(width: 96, height: 64)
                .clipShape(RoundedRectangle(cornerRadius: 6))
                VStack(alignment: .leading, spacing: 2) {
                    Text(app.name + (state.window.map { $0.title.isEmpty ? "" : " · \($0.title)" } ?? ""))
                        .font(.headline).lineLimit(1)
                    Text(TeleportPickerModel.label(app.capability)).font(.caption).foregroundStyle(.secondary)
                    Text(state.overId != nil ? "Release to teleport to \(spaceName)" : "Drop on a Space to teleport")
                        .font(.caption)
                }
            }
            .padding(12)
            .overlay(RoundedRectangle(cornerRadius: 10)
                .strokeBorder(state.overId != nil ? Color.accentColor : Color.secondary.opacity(0.4),
                              style: StrokeStyle(lineWidth: 2, dash: [6])))
        }
    }
}

public extension View {
    /// Accepts app drops (Finder or Dock `.app` bundles) onto this view:
    /// `onApp` gets the catalog row and any files dropped with it. Plain file
    /// drops go to `onFiles` (keep the existing transfer).
    func teleportAppDrop(host: TeleportAppHost,
                         onApp: @escaping (TeleportCatalogEntry, [String]) -> Void,
                         onFiles: (([URL]) -> Void)? = nil) -> some View {
        dropDestination(for: URL.self) { urls, _ in
            switch TeleportDropHandler.outcome(for: urls, host: host) {
            case .app(let e, let files):
                onApp(e, files)
                return true
            case .files(let f):
                guard let onFiles else { return false }
                onFiles(f)
                return true
            case .none:
                return false
            }
        }
    }
}
#endif
