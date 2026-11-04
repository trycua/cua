// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import SwiftUI

/// The routines surface.
///
/// Two strings on this screen are fixed: the empty-state sentence
/// ("Routines are recurring tasks this agent runs on a schedule.") and the
/// button label ("Create Routine"). Everything else (the row, the schedule
/// line, the enable switch, the editor, the firing log) is free to change.
///
/// It is a self-contained view so the app shell can mount it without this file
/// knowing anything about the shell. Mount points:
///
/// - desktop: `RoutinesPanel(...)` in the right panel, under the Agent Computer
///   preview, replacing the shell's inline empty state;
/// - mobile: `RoutinesPanel(...)` pushed from the thread header's overflow.
struct RoutinesPanel: View {
    let botID: String
    let botName: String
    @ObservedObject var store: RoutineStore
    var dark: Bool = false
    /// Panel scale. The desktop right panel is ~248pt wide; the mobile canvas
    /// is 1024pt of design space, so the same view is used at two type scales.
    var scale: CGFloat = 1

    @State private var editing: Routine? = nil
    @State private var isCreating = false

    private var rows: [Routine] { store.routines(for: botID) }
    private var text: Color { dark ? DS.onDark : DS.onSurface }
    private var subtle: Color { dark ? DS.dtSecondary : DS.secondary }
    private func f(_ s: CGFloat, _ w: Font.Weight = .regular) -> Font { DS.font(s * scale, w) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10 * scale) {
            HStack {
                Text("Routines").font(f(12, .semibold)).foregroundStyle(text)
                Spacer(minLength: 0)
                if !rows.isEmpty { createButton }
            }

            if rows.isEmpty {
                emptyState
            } else {
                VStack(spacing: 8 * scale) {
                    ForEach(rows) { r in row(r) }
                }
                if let last = store.log.first(where: { $0.routineID == rows.first?.id ?? "" })
                    ?? store.log.first {
                    lastFiringLine(last)
                }
            }
        }
        .padding(.horizontal, 14 * scale)
        .padding(.vertical, 12 * scale)
        .sheet(isPresented: $isCreating) {
            RoutineEditor(botID: botID, botName: botName, existing: nil, scale: scale) { draft in
                store.create(botID: draft.botID, title: draft.title, prompt: draft.prompt,
                             schedule: draft.schedule, enabled: draft.isEnabled)
                isCreating = false
            } onCancel: { isCreating = false }
        }
        .sheet(item: $editing) { r in
            RoutineEditor(botID: botID, botName: botName, existing: r, scale: scale) { draft in
                store.update(draft)
                editing = nil
            } onCancel: { editing = nil }
        }
    }

    // MARK: Empty state: the one part of this panel with an authoritative string.

    private var emptyState: some View {
        VStack(spacing: 12 * scale) {
            Text("Routines are recurring tasks this agent runs on a schedule.")
                .font(f(11))
                .multilineTextAlignment(.center)
                .lineSpacing(3 * scale)
                .foregroundStyle(subtle)
                .fixedSize(horizontal: false, vertical: true)
            createButton
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 14 * scale)
    }

    private var createButton: some View {
        Button {
            isCreating = true
        } label: {
            Text("Create Routine")
                .font(f(12, .medium))
                .foregroundStyle(text)
                .padding(.horizontal, 16 * scale)
                .frame(height: 30 * scale)
                .background(Capsule().stroke(dark ? DS.dtHairline : DS.hairline,
                                             lineWidth: 1 * scale))
        }
        .buttonStyle(.plain)
    }

    // MARK: A routine

    private func row(_ r: Routine) -> some View {
        VStack(alignment: .leading, spacing: 4 * scale) {
            HStack(spacing: 8 * scale) {
                Circle()
                    .fill(Color(hex: r.isEnabled ? 0x8B5CF6 : 0x9A9A9A))
                    .frame(width: 7 * scale, height: 7 * scale)
                Text(r.title)
                    .font(f(11.5, .medium))
                    .foregroundStyle(text)
                    .lineLimit(1)
                Spacer(minLength: 0)
                Toggle("", isOn: Binding(
                    get: { r.isEnabled },
                    set: { store.setEnabled($0, for: r.id) }))
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .scaleEffect(0.7 * scale, anchor: .trailing)
                    .frame(width: 30 * scale)
                    .help(r.isEnabled ? "Disable this routine" : "Enable this routine")
            }
            Text(r.schedule.label)
                .font(f(10))
                .foregroundStyle(subtle)
            if let outcome = r.lastOutcome, let at = r.lastFiredAt {
                Text("Last run \(BotStore.timestamp(at)): \(outcome)")
                    .font(f(9.5))
                    .foregroundStyle(subtle)
                    .lineLimit(2)
            }
            HStack(spacing: 12 * scale) {
                smallAction("Edit") { editing = r }
                smallAction("Run now") { Task { await store.fire(r) } }
                smallAction("Delete") { store.delete(r.id) }
            }
            .padding(.top, 2 * scale)
        }
        .padding(10 * scale)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 8 * scale, style: .continuous)
            .fill(dark ? DS.dtCard : DS.surface))
        .opacity(r.isEnabled ? 1 : 0.6)
    }

    private func smallAction(_ label: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label).font(f(10, .medium)).foregroundStyle(subtle)
        }
        .buttonStyle(.plain)
    }

    private func lastFiringLine(_ record: RoutineStore.FiringRecord) -> some View {
        HStack(spacing: 6 * scale) {
            Image(systemName: {
                switch record.firing {
                case .started: return "checkmark.circle.fill"
                case .refused: return "exclamationmark.circle.fill"
                case .failed:  return "xmark.circle.fill"
                }
            }())
            .font(.system(size: 10 * scale))
            .foregroundStyle(Color(hex: {
                switch record.firing {
                case .started: return 0x18BE4B
                case .refused: return 0xE0AE09
                case .failed:  return 0xF4234B
                }
            }()))
            Text("\(record.title): \(record.firing.summary)")
                .font(f(9.5))
                .foregroundStyle(subtle)
                .lineLimit(2)
        }
    }
}

/// Create or edit one routine.
struct RoutineEditor: View {
    let botID: String
    let botName: String
    let existing: Routine?
    var scale: CGFloat = 1
    var onSave: (Routine) -> Void
    var onCancel: () -> Void

    @State private var title: String = ""
    @State private var prompt: String = ""
    @State private var kind: Kind = .dailyAt
    @State private var minutes: Int = 30
    @State private var hour: Int = 8
    @State private var minute: Int = 0
    @State private var weekday: Int = 2
    @State private var enabled: Bool = true

    enum Kind: String, CaseIterable, Identifiable {
        case everyMinutes = "Every N minutes"
        case dailyAt = "Daily"
        case weeklyOn = "Weekly"
        var id: String { rawValue }
    }

    private var schedule: RoutineSchedule {
        switch kind {
        case .everyMinutes: return .everyMinutes(max(1, minutes))
        case .dailyAt:      return .dailyAt(hour: hour, minute: minute)
        case .weeklyOn:     return .weeklyOn(weekday: weekday, hour: hour, minute: minute)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(existing == nil ? "Create Routine" : "Edit Routine")
                .font(.system(size: 15, weight: .semibold))
            Text("\(botName) runs this on a schedule.")
                .font(.system(size: 11)).foregroundStyle(.secondary)

            TextField("Name", text: $title)
            TextField("What should \(botName) do?", text: $prompt, axis: .vertical)
                .lineLimit(3...6)

            Picker("Repeats", selection: $kind) {
                ForEach(Kind.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)

            switch kind {
            case .everyMinutes:
                Stepper("Every \(minutes) minute\(minutes == 1 ? "" : "s")",
                        value: $minutes, in: 1...720)
            case .dailyAt:
                clockPicker
            case .weeklyOn:
                HStack {
                    Picker("On", selection: $weekday) {
                        ForEach(1...7, id: \.self) {
                            Text(RoutineSchedule.weekdayName($0)).tag($0)
                        }
                    }
                    clockPicker
                }
            }

            Toggle("Enabled", isOn: $enabled)

            Text("Next run: \(nextRunLabel)")
                .font(.system(size: 11)).foregroundStyle(.secondary)

            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                Button(existing == nil ? "Create" : "Save") {
                    var r = existing ?? Routine(botID: botID, title: "", prompt: "",
                                                schedule: schedule)
                    r.title = title.isEmpty ? "Untitled routine" : title
                    r.prompt = prompt
                    r.schedule = schedule
                    r.isEnabled = enabled
                    onSave(r)
                }
                .keyboardShortcut(.defaultAction)
                .disabled(prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 380)
        .onAppear(perform: seed)
    }

    private var clockPicker: some View {
        HStack(spacing: 6) {
            Text("at")
            Stepper("\(RoutineSchedule.clock(hour, minute))",
                    onIncrement: { bump(5) }, onDecrement: { bump(-5) })
        }
    }

    private func bump(_ delta: Int) {
        var total = hour * 60 + minute + delta
        total = (total % 1440 + 1440) % 1440
        hour = total / 60
        minute = total % 60
    }

    private var nextRunLabel: String {
        var probe = existing ?? Routine(botID: botID, title: title, prompt: prompt,
                                        schedule: schedule)
        probe.schedule = schedule
        probe.isEnabled = enabled
        guard let next = probe.nextFireDate(after: Date()) else { return "disabled" }
        let f = DateFormatter()
        f.dateFormat = "EEE d MMM, h:mm a"
        return f.string(from: next)
    }

    private func seed() {
        guard let r = existing else { return }
        title = r.title
        prompt = r.prompt
        enabled = r.isEnabled
        switch r.schedule {
        case .everyMinutes(let m): kind = .everyMinutes; minutes = m
        case .dailyAt(let h, let mi): kind = .dailyAt; hour = h; minute = mi
        case .weeklyOn(let wd, let h, let mi):
            kind = .weeklyOn; weekday = wd; hour = h; minute = mi
        }
    }
}
