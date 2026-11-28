import Foundation
import Virtualization

/// Devices that connect the guest to the host's desktop session: a virtio
/// sound device wired to the host's audio input and output, and a SPICE agent
/// console port that shares the host clipboard. Both are attached by default;
/// `lume run --no-audio-device` / `--no-clipboard-device` (or the run API's
/// `noAudioDevice` / `noClipboardDevice`) leave them out of the VM entirely.
struct HostDeviceOptions: Equatable, Sendable {
    /// Attach a virtio sound device with host audio input and output.
    var audio: Bool = true
    /// Attach the SPICE agent console port that shares the host clipboard,
    /// and allow lume's own clipboard sync over SSH.
    var clipboard: Bool = true

    /// Every host device attached (lume's historic behavior).
    static let all = HostDeviceOptions()

    /// Adds the selected host devices to `vzConfig`. A device that is not
    /// selected is not created at all, so the guest never sees it.
    func apply(to vzConfig: VZVirtualMachineConfiguration) {
        if audio {
            let sound = VZVirtioSoundDeviceConfiguration()
            let input = VZVirtioSoundDeviceInputStreamConfiguration()
            input.source = VZHostAudioInputStreamSource()
            let output = VZVirtioSoundDeviceOutputStreamConfiguration()
            output.sink = VZHostAudioOutputStreamSink()
            sound.streams = [input, output]
            vzConfig.audioDevices = [sound]
        } else {
            vzConfig.audioDevices = []
        }

        if clipboard {
            let console = VZVirtioConsoleDeviceConfiguration()
            let port = VZVirtioConsolePortConfiguration()
            port.name = VZSpiceAgentPortAttachment.spiceAgentPortName
            let attachment = VZSpiceAgentPortAttachment()
            attachment.sharesClipboard = true
            port.attachment = attachment
            console.ports[0] = port
            vzConfig.consoleDevices.append(console)
        }
    }

    /// Rejects clipboard sync (`--clipboard`) on a run with the clipboard
    /// device turned off: that run shares no clipboard with the host.
    func validate(clipboardSync: Bool) throws {
        if clipboardSync && !clipboard {
            throw VMError.clipboardDeviceConflict
        }
    }
}
