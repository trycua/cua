import Foundation
import Testing
import Virtualization

@testable import lume

@Test("Host audio and clipboard devices are attached by default")
func hostDevicesDefaultToAll() {
    let vzConfig = VZVirtualMachineConfiguration()
    HostDeviceOptions.all.apply(to: vzConfig)

    #expect(vzConfig.audioDevices.count == 1)
    let sound = vzConfig.audioDevices.first as? VZVirtioSoundDeviceConfiguration
    #expect(sound?.streams.count == 2)

    #expect(vzConfig.consoleDevices.count == 1)
    let console = vzConfig.consoleDevices.first as? VZVirtioConsoleDeviceConfiguration
    let port = console?.ports[0]
    #expect(port?.name == VZSpiceAgentPortAttachment.spiceAgentPortName)
    #expect((port?.attachment as? VZSpiceAgentPortAttachment)?.sharesClipboard == true)
}

@Test("Turning off the audio device leaves no sound device in the VM")
func noAudioDevice() {
    let vzConfig = VZVirtualMachineConfiguration()
    HostDeviceOptions(audio: false, clipboard: true).apply(to: vzConfig)

    #expect(vzConfig.audioDevices.isEmpty)
    #expect(vzConfig.consoleDevices.count == 1)
}

@Test("Turning off the clipboard device leaves no SPICE agent port in the VM")
func noClipboardDevice() {
    let vzConfig = VZVirtualMachineConfiguration()
    HostDeviceOptions(audio: true, clipboard: false).apply(to: vzConfig)

    #expect(vzConfig.audioDevices.count == 1)
    #expect(vzConfig.consoleDevices.isEmpty)
}

@Test("Turning off both leaves neither device and keeps other console devices")
func noHostDevices() {
    let vzConfig = VZVirtualMachineConfiguration()
    let other = VZVirtioConsoleDeviceConfiguration()
    vzConfig.consoleDevices = [other]
    HostDeviceOptions(audio: false, clipboard: false).apply(to: vzConfig)

    #expect(vzConfig.audioDevices.isEmpty)
    #expect(vzConfig.consoleDevices.count == 1)
    #expect(vzConfig.consoleDevices.first === other)
}

@Test("Clipboard sync is rejected when the clipboard device is off")
func clipboardSyncConflict() throws {
    try HostDeviceOptions.all.validate(clipboardSync: true)
    try HostDeviceOptions(audio: true, clipboard: false).validate(clipboardSync: false)
    #expect(throws: VMError.self) {
        try HostDeviceOptions(audio: true, clipboard: false).validate(clipboardSync: true)
    }
}

@MainActor
@Test("The native-display clipboard sync stays off without the clipboard device")
func nativeClipboardSyncNeedsClipboardDevice() {
    #expect(
        !VM.shouldStartClipboardWatcher(
            displayMode: .native,
            osType: "macOS",
            explicitlyRequested: false,
            clipboardAllowed: false
        )
    )
    #expect(
        !VM.shouldStartClipboardWatcher(
            displayMode: .vnc,
            osType: "macOS",
            explicitlyRequested: true,
            clipboardAllowed: false
        )
    )
}

@MainActor
@Test("lume run keeps both host devices unless asked otherwise")
func runParsesHostDeviceFlags() throws {
    let plain = try #require(try Lume.parseAsRoot(["run", "vm"]) as? Run)
    #expect(plain.hostDevices == .all)

    let hardened = try #require(
        try Lume.parseAsRoot(["run", "vm", "--no-audio-device", "--no-clipboard-device"]) as? Run)
    #expect(hardened.hostDevices == HostDeviceOptions(audio: false, clipboard: false))

    let audioOnly = try #require(try Lume.parseAsRoot(["run", "vm", "--no-audio-device"]) as? Run)
    #expect(audioOnly.hostDevices == HostDeviceOptions(audio: false, clipboard: true))
}

@MainActor
@Test("lume run rejects --clipboard with --no-clipboard-device")
func runRejectsClipboardWithoutDevice() {
    #expect(throws: (any Error).self) {
        _ = try Lume.parseAsRoot(["run", "vm", "--clipboard", "--no-clipboard-device"])
    }
}

@Test("The run API reads the host device options")
func runRequestHostDevices() throws {
    func decode(_ json: String) throws -> RunVMRequest {
        try JSONDecoder().decode(RunVMRequest.self, from: Data(json.utf8))
    }
    #expect(try decode("{}").validatedHostDevices() == .all)
    #expect(
        try decode(#"{"noAudioDevice": true, "noClipboardDevice": true}"#).validatedHostDevices()
            == HostDeviceOptions(audio: false, clipboard: false))
    #expect(
        try decode(#"{"noClipboardDevice": false}"#).validatedHostDevices() == .all)
    #expect(throws: VMError.self) {
        try decode(#"{"clipboard": true, "noClipboardDevice": true}"#).validatedHostDevices()
    }
}
