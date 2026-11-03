// Put the main display in its largest 1x mode, or in the WIDTHxHEIGHT given, and keep it across restarts. Also give
// it the sRGB colour profile.
//
// A macOS guest boots in the mode its disk last had, not in the size lume's VM config gives the virtual display, so
// a clone of an image built at one size would otherwise come up at that old size. The virtual display's own profile
// ("Display") makes screenshots a colour-managed conversion of what apps drew, which moves single channel values by
// one (an app's #00ffff reads back as 1,255,255); with the sRGB profile a screenshot holds the pixels as drawn.
// Run at login in the GUI session.
import ColorSync
import CoreGraphics
import Foundation

let id = CGMainDisplayID()

let displayUUID = CGDisplayCreateUUIDFromDisplayID(id).takeRetainedValue()
let sRGB = URL(fileURLWithPath: "/System/Library/ColorSync/Profiles/sRGB Profile.icc")
let profiles = [kColorSyncDeviceDefaultProfileID.takeUnretainedValue() as String: sRGB] as CFDictionary
if !ColorSyncDeviceSetCustomProfiles(kColorSyncDisplayDeviceClass.takeUnretainedValue(), displayUUID, profiles) {
    FileHandle.standardError.write(Data("couldn't set the sRGB profile\n".utf8))
}

let options = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
let modes = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
let oneX = modes.filter { $0.width == $0.pixelWidth && $0.height == $0.pixelHeight && $0.isUsableForDesktopGUI() }

var wanted: CGDisplayMode?
if CommandLine.arguments.count > 1 {
    let size = CommandLine.arguments[1].split(separator: "x").compactMap { Int($0) }
    guard size.count == 2 else { FileHandle.standardError.write(Data("usage: set-display [WIDTHxHEIGHT]\n".utf8)); exit(2) }
    wanted = oneX.first { $0.width == size[0] && $0.height == size[1] }
} else {
    wanted = oneX.max { $0.width * $0.height < $1.width * $1.height }
}
guard let mode = wanted else { FileHandle.standardError.write(Data("no such display mode\n".utf8)); exit(1) }
if let now = CGDisplayCopyDisplayMode(id), now.width == mode.width, now.height == mode.height {
    print("display already \(mode.width)x\(mode.height)")
    exit(0)
}
var config: CGDisplayConfigRef?
guard CGBeginDisplayConfiguration(&config) == .success,
      CGConfigureDisplayWithDisplayMode(config, id, mode, nil) == .success,
      CGCompleteDisplayConfiguration(config, .permanently) == .success else {
    FileHandle.standardError.write(Data("couldn't set \(mode.width)x\(mode.height)\n".utf8)); exit(1)
}
print("display set to \(mode.width)x\(mode.height)")
