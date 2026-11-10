// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaBotsCore
import CuaBotsUI
import SwiftUI

/// Every bot's scheduled work, on the routine clock.
struct ScheduledView: View {
    @EnvironmentObject var model: AppModel
    @State private var adding = false

    var body: some View {
        List {
            ForEach(model.store.bots) { bot in
                let tasks = model.store.tasks(for: bot.id).filter { $0.schedule != nil }
                Section {
                    if tasks.isEmpty {
                        Text("No scheduled work").foregroundStyle(.secondary)
                    }
                    ForEach(tasks) { t in
                        HStack {
                            TaskRow(t)
                            Spacer()
                            if let last = t.lastRun {
                                Text("Last run \(last.formatted(date: .omitted, time: .shortened))")
                                    .font(.caption).foregroundStyle(.tertiary)
                            }
                            Toggle("", isOn: Binding(get: { t.state == .scheduled }, set: { on in
                                var u = t
                                u.state = on ? .scheduled : .paused
                                model.store.updateTask(u)
                            }))
                            .toggleStyle(.switch).labelsHidden().controlSize(.small)
                            .disabled(bot.isPaused)
                            Toggle(isOn: Binding(get: { t.notifyOnCompletion }, set: { on in
                                var u = t
                                u.notifyOnCompletion = on
                                model.store.updateTask(u)
                            })) { Image(systemName: "bell") }
                            .toggleStyle(.button).controlSize(.small)
                            .help("Notify me when it finishes")
                        }
                        .contextMenu {
                            Button("Run now") { Task { _ = await model.store.fire(taskID: t.id, botID: bot.id) } }
                            Button("Delete", role: .destructive) { model.store.deleteTask(t) }
                        }
                    }
                } header: {
                    HStack(spacing: 6) {
                        KoalaAvatar(bot.avatar, mood: bot.mood).frame(width: 16, height: 16)
                        Text(bot.name)
                        if bot.isPaused { Text("Paused").foregroundStyle(.secondary) }
                    }
                }
            }
            Section("Recent firings") {
                ForEach(model.clock.routines.log.prefix(8)) { r in
                    HStack {
                        Text(r.title)
                        Spacer()
                        Text(r.firing.summary).font(.caption).foregroundStyle(.secondary)
                        Text(r.at.formatted(date: .omitted, time: .shortened)).font(.caption).foregroundStyle(.tertiary)
                    }
                }
            }
        }
        .navigationTitle("Scheduled")
        .toolbar {
            Button { adding = true } label: { Label("New routine", systemImage: "plus") }
                .disabled(model.store.bots.isEmpty)
        }
        .sheet(isPresented: $adding) { NewRoutineSheet() }
    }
}

struct NewRoutineSheet: View {
    @EnvironmentObject var model: AppModel
    @Environment(\.dismiss) var dismiss
    @State var botID = ""
    @State var title = ""
    @State var prompt = ""
    @State var kind = 0
    @State var time = Calendar.current.date(bySettingHour: 9, minute: 0, second: 0, of: Date())!
    @State var weekday = 2
    @State var minutes = 30

    var body: some View {
        Form {
            Picker("Bot", selection: $botID) {
                ForEach(model.store.bots) { Text($0.name).tag($0.id) }
            }
            TextField("Title", text: $title, prompt: Text("Morning briefing"))
            TextField("What to do", text: $prompt, prompt: Text("Summarize what came in overnight"), axis: .vertical)
                .lineLimit(2...4)
            Picker("Repeat", selection: $kind) {
                Text("Daily").tag(0)
                Text("Weekly").tag(1)
                Text("Every few minutes").tag(2)
            }
            if kind == 2 {
                Stepper("Every \(minutes) minutes", value: $minutes, in: 5...720, step: 5)
            } else {
                if kind == 1 {
                    Picker("Day", selection: $weekday) {
                        ForEach(1...7, id: \.self) { Text(Calendar.current.weekdaySymbols[$0 - 1]).tag($0) }
                    }
                }
                DatePicker("Time", selection: $time, displayedComponents: .hourAndMinute)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Schedule") {
                    let c = Calendar.current.dateComponents([.hour, .minute], from: time)
                    let schedule: TaskSchedule = kind == 0 ? .daily(hour: c.hour!, minute: c.minute!)
                        : kind == 1 ? .weekly(weekday: weekday, hour: c.hour!, minute: c.minute!)
                        : .every(minutes: minutes)
                    model.store.schedule(botID, title: title, prompt: prompt.isEmpty ? title : prompt, schedule: schedule)
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .disabled(title.isEmpty || botID.isEmpty)
            }
        }
        .formStyle(.grouped)
        .frame(width: 440)
        .onAppear { botID = model.selectedBot?.id ?? model.store.bots.first?.id ?? "" }
    }
}

/// Everything waiting for you, across bots, and Keyvault requests.
struct ApprovalsView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                let pending = model.store.pendingApprovals()
                if pending.isEmpty && (model.keyvault.overview?.pending.isEmpty ?? true) {
                    Text("Nothing is waiting for you.").foregroundStyle(.secondary).padding(.top, 40)
                        .frame(maxWidth: .infinity)
                }
                ForEach(pending) { a in
                    if let bot = model.store.bot(a.botID) {
                        HStack(alignment: .top, spacing: 10) {
                            KoalaAvatar(bot.avatar, mood: .needsApproval).frame(width: 30, height: 30)
                            ApprovalCard(a, botName: bot.name, onDecide: { model.decide(a, $0) },
                                         onOpenComputer: { model.route = .bot(bot.id); model.showComputer = true })
                        }
                    }
                }
                if let kv = model.keyvault.overview, !kv.pending.isEmpty {
                    Text("Keyvault requests").font(.subheadline.weight(.medium)).foregroundStyle(.secondary)
                    ForEach(kv.pending, id: \.id) { p in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(p.callerDisplay).font(.callout.weight(.medium))
                                Text(p.request.reason).font(.caption).foregroundStyle(.secondary)
                                Text(p.items.map(\.label).joined(separator: ", ")).font(.caption)
                            }
                            Spacer()
                            Button("Deny") { Task { try? await model.keyvault.decide(requestID: p.id, approve: false) } }
                            Button("Approve") { Task { try? await model.keyvault.decide(requestID: p.id, approve: true) } }
                                .buttonStyle(.borderedProminent)
                        }
                        .padding(12)
                        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(.primary.opacity(0.1)))
                    }
                }
            }
            .padding(20)
            .frame(maxWidth: 640, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .navigationTitle("Approvals")
        .task { await model.keyvault.refresh() }
    }
}

/// Every bot's finished files, from the Volume.
struct OutputsView: View {
    @EnvironmentObject var model: AppModel

    var body: some View {
        List {
            ForEach(model.store.bots) { bot in
                let files = model.store.outputs(for: bot.id)
                Section(bot.name) {
                    if files.isEmpty { Text("Nothing yet").foregroundStyle(.secondary) }
                    ForEach(files) { f in
                        Button { model.openOutput = f.path } label: {
                            HStack {
                                Image(systemName: ProfilePanel.icon(for: f.name)).foregroundStyle(.secondary)
                                Text(f.name)
                                Spacer()
                                if let m = f.modified {
                                    Text(m.formatted(date: .abbreviated, time: .shortened))
                                        .font(.caption).foregroundStyle(.tertiary)
                                }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .navigationTitle("Outputs")
    }
}
