// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesFFI
import SwiftUI

/// What the add or replace sheet is for.
struct AgentKeySheetTarget: Identifiable, Equatable {
    let provider: String
    let env: String?
    var id: String { "\(provider):\(env ?? "")" }
}

/// Settings → Agents: Anthropic and OpenAI (Not set, or •••• and the last
/// four characters with when it was added), then other keys by variable
/// name; Add or Replace opens a sheet with a secure field (the key is never
/// shown again), Remove asks first. Everything shown is the core's
/// `appAgentKeysView`.
struct AgentKeysSettingsView: View {
    let keys: AgentKeysModel
    @State private var sheet: AgentKeySheetTarget?
    @State private var removing: String?
    @State private var failure: String?

    var body: some View {
        let v = keys.view
        Form {
            Section {
                Text(v.intro).font(.callout).foregroundStyle(.secondary)
                    .accessibilityIdentifier("agent-keys-intro")
                if let notice = v.notice {
                    Label(notice, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                        .accessibilityIdentifier("agent-keys-notice")
                }
            }
            Section(v.title) {
                ForEach(v.rows, id: \.env) { row($0, v) }
            }
            Section {
                HStack(alignment: .firstTextBaseline) {
                    Button(v.addOtherLabel) { sheet = AgentKeySheetTarget(provider: "other", env: nil) }
                        .disabled(!v.canEdit)
                        .accessibilityIdentifier("agent-keys-add-other")
                    Text(v.otherHelp).font(.callout).foregroundStyle(.secondary)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
        .frame(minHeight: 360)
        .task { await keys.load() }
        .sheet(item: $sheet) { target in
            AgentKeySheet(keys: keys, target: target) { sheet = nil }
        }
        .alert(pendingRemove?.confirm.title ?? "", isPresented: Binding(get: { pendingRemove != nil }, set: { if !$0 { removing = nil } }),
               presenting: pendingRemove) { item in
            Button(item.confirm.confirmLabel, role: .destructive) {
                Task {
                    do { try await keys.remove(env: item.env) } catch { failure = error.localizedDescription }
                }
            }
            Button(item.confirm.cancelLabel, role: .cancel) {}
        } message: { item in
            Text(item.confirm.message)
        }
        .alert("Couldn't remove the key", isPresented: Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(failure ?? "")
        }
    }

    /// The key a Remove asks about, with the core's question.
    private var pendingRemove: PendingRemove? {
        guard let env = removing, let confirm = keys.removeConfirm(env: env) else { return nil }
        return PendingRemove(env: env, confirm: confirm)
    }

    struct PendingRemove {
        let env: String
        let confirm: AppAgentKeyConfirm
    }

    @ViewBuilder private func row(_ r: AppAgentKeyRow, _ v: AppAgentKeysView) -> some View {
        LabeledContent {
            HStack(spacing: 8) {
                VStack(alignment: .trailing, spacing: 1) {
                    Text(r.status).font(r.set ? .body.monospaced() : .body).foregroundStyle(r.set ? .primary : .secondary)
                    if let ms = r.addedMs {
                        Text(Self.added(v.addedLabel, ms)).font(.caption).foregroundStyle(.secondary)
                    }
                }
                Button(r.actionLabel) { sheet = AgentKeySheetTarget(provider: r.provider, env: r.provider == "other" ? r.env : nil) }
                    .disabled(!v.canEdit)
                    .accessibilityIdentifier("agent-key-action-\(r.env)")
                if let remove = r.removeLabel {
                    Button(remove) { removing = r.env }
                        .disabled(!v.canEdit)
                        .accessibilityIdentifier("agent-key-remove-\(r.env)")
                }
            }
        } label: {
            VStack(alignment: .leading, spacing: 1) {
                Text(r.title)
                Text(r.detail).font(.caption).foregroundStyle(.secondary)
            }
        }
        .accessibilityIdentifier("agent-key-\(r.env)")
    }

    /// "Added Oct 4, 2026".
    static func added(_ label: String, _ ms: UInt64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        return "\(label) \(date.formatted(.dateTime.month(.abbreviated).day().year()))"
    }
}

/// Add or replace one key. The key stays in the secure field until Save,
/// then the sheet closes and it is gone.
struct AgentKeySheet: View {
    let keys: AgentKeysModel
    let target: AgentKeySheetTarget
    let done: () -> Void
    @State private var name = ""
    @State private var value = ""
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        let v = keys.form(provider: target.provider, env: target.env, name: name,
                          hasValue: !value.trimmingCharacters(in: .whitespaces).isEmpty)
        VStack(alignment: .leading, spacing: 12) {
            Text(v.title).font(.headline)
            Text(v.lede).font(.callout).foregroundStyle(.secondary)
            if let label = v.nameLabel {
                TextField(label, text: $name, prompt: v.namePlaceholder.map { Text($0) })
                    .font(.body.monospaced())
                    .autocorrectionDisabled()
                    .accessibilityIdentifier("agent-key-name")
                if let e = v.nameError { Text(e).font(.caption).foregroundStyle(.red) }
            }
            SecureField(v.valueLabel, text: $value, prompt: Text(v.valuePlaceholder))
                .accessibilityIdentifier("agent-key-value")
            Text(v.valueHelp).font(.caption).foregroundStyle(.secondary)
            if let error { Text(error).font(.callout).foregroundStyle(.red) }
            HStack {
                Spacer()
                Button(v.cancelLabel, role: .cancel) { done() }
                    .keyboardShortcut(.cancelAction)
                Button(busy ? "Saving…" : v.saveLabel) { Task { await save(v) } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!v.canSave || busy)
                    .accessibilityIdentifier("agent-key-save")
            }
        }
        .padding(20)
        .frame(width: 420)
    }

    private func save(_ v: AppAgentKeyFormView) async {
        busy = true
        error = nil
        do {
            try await keys.save(provider: v.provider, env: v.env, value: value)
            value = ""
            done()
        } catch {
            self.error = error.localizedDescription
            busy = false
        }
    }
}
