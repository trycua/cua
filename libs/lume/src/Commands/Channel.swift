import ArgumentParser
import Foundation

struct ReleaseChannelCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "channel",
        abstract: "Inspect or change the stable/nightly update channel; selection does not install",
        subcommands: [ChannelStatus.self, ChannelSet.self],
        defaultSubcommand: ChannelStatus.self
    )
}

struct ChannelStatus: ParsableCommand {
    static let configuration = CommandConfiguration(commandName: "status")

    @Flag(help: "Emit machine-readable channel state as JSON")
    var json = false

    func run() throws {
        try printChannelState(selected: LumeReleaseChannel.selected(), json: json, changed: false)
    }
}

struct ChannelSet: ParsableCommand {
    static let configuration = CommandConfiguration(commandName: "set")

    @Argument(help: "Release channel: stable or nightly")
    var channel: String

    @Flag(help: "Emit machine-readable channel state as JSON")
    var json = false

    func run() throws {
        guard let selected = LumeReleaseChannel(rawValue: channel) else {
            throw ValidationError("release channel must be stable or nightly")
        }
        try LumeReleaseChannel.set(selected)
        try printChannelState(selected: selected, json: json, changed: true)
    }
}

private func printChannelState(
    selected: LumeReleaseChannel,
    json: Bool,
    changed: Bool
) throws {
    let current = LumeReleaseChannel.current(for: Lume.Version.current)
    if json {
        let payload: [String: Any] = [
            "selected_channel": selected.rawValue,
            "current_channel": current?.rawValue ?? NSNull(),
            "current_version": Lume.Version.current,
        ]
        let data = try JSONSerialization.data(withJSONObject: payload, options: [.sortedKeys])
        print(String(decoding: data, as: UTF8.self))
        return
    }
    print("Selected channel: \(selected.rawValue)")
    print("Current channel:  \(current?.rawValue ?? "development")")
    if changed && current != selected {
        print("Run `lume update --apply` to install the latest \(selected.rawValue) release.")
    }
}
