import Foundation

/// Represents a parsed boot command for VNC automation
enum BootCommand: Sendable {
    /// Wait for text to appear on screen (OCR-based)
    case waitForText(String, timeout: TimeInterval)

    /// Click on text detected on screen
    case clickText(String)

    /// Click on text with X and/or Y offset (positive = right/down, negative = left/up)
    case clickTextWithOffset(String, xOffset: Int, yOffset: Int)

    /// Click on text with index to select specific match (0 = first, -1 = last)
    case clickTextWithIndex(String, index: Int, xOffset: Int, yOffset: Int)

    /// Click at specific screen coordinates
    case clickAt(x: Int, y: Int)

    /// Type text string with optional delay between characters (in milliseconds)
    case typeText(String, delay: Int?)

    /// Press a special key
    case keyPress(SpecialKey)

    /// Press a hotkey combination with a special key (e.g., cmd+space, ctrl+alt+delete)
    case hotkey(modifiers: [SpecialKey], key: SpecialKey)

    /// Press a hotkey combination with a character key (e.g., cmd+q, cmd+c, cmd+v)
    case hotkeyChar(modifiers: [SpecialKey], char: Character)

    /// Wait for a specified duration
    case delay(TimeInterval)
}

/// Special keys for keyboard automation
enum SpecialKey: String, Sendable {
    case enter
    case tab
    case escape = "esc"
    case leftSuper  // Command key
    case leftShift
    case leftAlt    // Option key
    case leftCtrl
    case backspace
    case delete
    case up
    case down
    case left
    case right
    case space
    case f1, f2, f3, f4, f5, f6, f7, f8, f9, f10, f11, f12
}

/// Parser for boot command DSL
struct BootCommandParser {
    /// Default timeout for wait commands in seconds
    static let defaultWaitTimeout: TimeInterval = 120

    /// Parse a single command string into a BootCommand
    func parse(_ commandString: String) throws -> BootCommand {
        let trimmed = commandString.trimmingCharacters(in: .whitespaces)

        // Check for <wait 'text'> pattern
        if let match = try parseWaitCommand(trimmed) {
            return match
        }

        // Check for <click 'text', offset=N> pattern (with Y offset)
        if let match = try parseClickTextWithOffsetCommand(trimmed) {
            return match
        }

        // Check for <click 'text'> pattern
        if let match = try parseClickTextCommand(trimmed) {
            return match
        }

        // Check for <click_at x,y> pattern
        if let match = try parseClickAtCommand(trimmed) {
            return match
        }

        // Check for <type 'text'> pattern
        if let match = try parseTypeCommand(trimmed) {
            return match
        }

        // Check for <delay N> pattern
        if let match = try parseDelayCommand(trimmed) {
            return match
        }

        // Check for hotkey combination (e.g., <cmd+space>)
        if let match = parseHotkeyCommand(trimmed) {
            return match
        }

        // Check for special key commands
        if let match = parseSpecialKeyCommand(trimmed) {
            return match
        }

        throw BootCommandParserError.unknownCommand(trimmed)
    }

    /// Parse multiple command strings
    func parseAll(_ commands: [String]) throws -> [BootCommand] {
        try commands.map { try parse($0) }
    }

    // MARK: - Private parsing methods

    private func parseWaitCommand(_ input: String) throws -> BootCommand? {
        // Match <wait 'text'> or <wait "text"> with optional timeout
        let pattern = #"^<wait\s+['"](.+?)['"]\s*(?:,\s*timeout\s*=\s*(\d+))?\s*>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input))
        else {
            return nil
        }

        guard let textRange = Range(match.range(at: 1), in: input) else {
            return nil
        }

        let text = String(input[textRange])
        var timeout = Self.defaultWaitTimeout

        if match.numberOfRanges > 2,
           let timeoutRange = Range(match.range(at: 2), in: input),
           let timeoutValue = TimeInterval(input[timeoutRange]) {
            timeout = timeoutValue
        }

        return .waitForText(text, timeout: timeout)
    }

    private func parseClickTextWithOffsetCommand(_ input: String) throws -> BootCommand? {
        // Match <click 'text', xoffset=N, yoffset=N, index=N> or combinations
        // Also supports just xoffset, yoffset, or index individually
        // index: 0 = first match, 1 = second match, -1 = last match, -2 = second to last, etc.
        var xOffset = 0
        var yOffset = 0
        var index: Int?
        var text: String?

        // Check if this looks like a click command with parameters
        let hasParams = input.contains(",")
        guard hasParams else { return nil }

        // Extract text and parameters using a more flexible approach
        // Pattern: <click 'text', param=value, param=value, ...>
        let textPattern = #"^<click\s+['"](.+?)['"]"#
        guard let textRegex = try? NSRegularExpression(pattern: textPattern, options: []),
              let textMatch = textRegex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let textRange = Range(textMatch.range(at: 1), in: input)
        else {
            return nil
        }
        text = String(input[textRange])

        // Extract xoffset if present
        let xoffsetPattern = #"xoffset\s*=\s*(-?\d+)"#
        if let xRegex = try? NSRegularExpression(pattern: xoffsetPattern, options: []),
           let xMatch = xRegex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
           let xRange = Range(xMatch.range(at: 1), in: input),
           let x = Int(input[xRange]) {
            xOffset = x
        }

        // Extract yoffset if present
        let yoffsetPattern = #"yoffset\s*=\s*(-?\d+)"#
        if let yRegex = try? NSRegularExpression(pattern: yoffsetPattern, options: []),
           let yMatch = yRegex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
           let yRange = Range(yMatch.range(at: 1), in: input),
           let y = Int(input[yRange]) {
            yOffset = y
        }

        // Extract index if present
        let indexPattern = #"index\s*=\s*(-?\d+)"#
        if let indexRegex = try? NSRegularExpression(pattern: indexPattern, options: []),
           let indexMatch = indexRegex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
           let indexRange = Range(indexMatch.range(at: 1), in: input),
           let idx = Int(input[indexRange]) {
            index = idx
        }

        // Legacy format: <click 'text', offset=N> (Y offset only, no index)
        if index == nil && xOffset == 0 && yOffset == 0 {
            let legacyPattern = #"offset\s*=\s*(-?\d+)"#
            if let legacyRegex = try? NSRegularExpression(pattern: legacyPattern, options: []),
               let legacyMatch = legacyRegex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
               let offsetRange = Range(legacyMatch.range(at: 1), in: input),
               let offset = Int(input[offsetRange]) {
                yOffset = offset
            }
        }

        // Return appropriate command type
        if let idx = index {
            return .clickTextWithIndex(text!, index: idx, xOffset: xOffset, yOffset: yOffset)
        } else if xOffset != 0 || yOffset != 0 {
            return .clickTextWithOffset(text!, xOffset: xOffset, yOffset: yOffset)
        }

        return nil
    }

    private func parseClickTextCommand(_ input: String) throws -> BootCommand? {
        // Match <click 'text'> or <click "text">
        let pattern = #"^<click\s+['"](.+?)['"]\s*>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let textRange = Range(match.range(at: 1), in: input)
        else {
            return nil
        }

        let text = String(input[textRange])
        return .clickText(text)
    }

    private func parseClickAtCommand(_ input: String) throws -> BootCommand? {
        // Match <click_at x,y>
        let pattern = #"^<click_at\s+(\d+)\s*,\s*(\d+)\s*>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let xRange = Range(match.range(at: 1), in: input),
              let yRange = Range(match.range(at: 2), in: input),
              let x = Int(input[xRange]),
              let y = Int(input[yRange])
        else {
            return nil
        }

        return .clickAt(x: x, y: y)
    }

    private func parseTypeCommand(_ input: String) throws -> BootCommand? {
        // Match <type 'text'> or <type "text"> with optional delay parameter
        // Examples: <type 'hello'>, <type 'hello', delay=100>
        let pattern = #"^<type\s+['"](.*)['"](?:\s*,\s*delay\s*=\s*(\d+))?\s*>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let textRange = Range(match.range(at: 1), in: input)
        else {
            return nil
        }

        let text = String(input[textRange])
        var delay: Int? = nil

        if match.numberOfRanges > 2,
           let delayRange = Range(match.range(at: 2), in: input),
           let delayValue = Int(input[delayRange]) {
            delay = delayValue
        }

        return .typeText(text, delay: delay)
    }

    private func parseDelayCommand(_ input: String) throws -> BootCommand? {
        // Match <delay N> where N is seconds (can be decimal)
        let pattern = #"^<delay\s+([\d.]+)\s*>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let durationRange = Range(match.range(at: 1), in: input),
              let duration = TimeInterval(input[durationRange])
        else {
            return nil
        }

        return .delay(duration)
    }

    private func parseHotkeyCommand(_ input: String) -> BootCommand? {
        // Match <key1+key2+key3> for hotkey combinations
        // Examples: <cmd+space>, <ctrl+alt+delete>, <shift+cmd+3>
        let pattern = #"^<([\w+]+)>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let contentRange = Range(match.range(at: 1), in: input)
        else {
            return nil
        }

        let content = String(input[contentRange])

        // Check if it contains a + (indicating a hotkey combo)
        guard content.contains("+") else {
            return nil
        }

        let parts = content.split(separator: "+").map { String($0).lowercased() }
        guard parts.count >= 2 else {
            return nil
        }

        // Map key names to SpecialKey enum
        let keyMap: [String: SpecialKey] = [
            "enter": .enter,
            "return": .enter,
            "tab": .tab,
            "esc": .escape,
            "escape": .escape,
            "command": .leftSuper,
            "cmd": .leftSuper,
            "super": .leftSuper,
            "shift": .leftShift,
            "option": .leftAlt,
            "alt": .leftAlt,
            "ctrl": .leftCtrl,
            "control": .leftCtrl,
            "backspace": .backspace,
            "delete": .delete,
            "up": .up,
            "down": .down,
            "left": .left,
            "right": .right,
            "space": .space,
            "f1": .f1, "f2": .f2, "f3": .f3, "f4": .f4,
            "f5": .f5, "f6": .f6, "f7": .f7, "f8": .f8,
            "f9": .f9, "f10": .f10, "f11": .f11, "f12": .f12
        ]

        // Modifier keys
        let modifierKeys: Swift.Set<String> = ["command", "cmd", "super", "shift", "option", "alt", "ctrl", "control"]

        // Parse all parts - last non-modifier is the main key, rest are modifiers
        var modifiers: [SpecialKey] = []
        var mainSpecialKey: SpecialKey?
        var mainChar: Character?

        for (index, part) in parts.enumerated() {
            // If it's the last part, it's the main key
            if index == parts.count - 1 {
                // Check if it's a special key
                if let key = keyMap[part] {
                    mainSpecialKey = key
                } else if part.count == 1, let char = part.first {
                    // Single character key (a-z, 0-9, etc.)
                    mainChar = char
                } else {
                    return nil  // Unknown key
                }
            } else {
                // Must be a modifier
                guard let key = keyMap[part], modifierKeys.contains(part) else {
                    return nil  // Unknown or non-modifier key in modifier position
                }
                modifiers.append(key)
            }
        }

        // Return appropriate hotkey type
        if let specialKey = mainSpecialKey {
            return .hotkey(modifiers: modifiers, key: specialKey)
        } else if let char = mainChar {
            return .hotkeyChar(modifiers: modifiers, char: char)
        }

        return nil
    }

    private func parseSpecialKeyCommand(_ input: String) -> BootCommand? {
        // Match <keyname> for special keys
        let pattern = #"^<(\w+)>$"#
        guard let regex = try? NSRegularExpression(pattern: pattern, options: []),
              let match = regex.firstMatch(in: input, range: NSRange(input.startIndex..., in: input)),
              let keyRange = Range(match.range(at: 1), in: input)
        else {
            return nil
        }

        let keyName = String(input[keyRange]).lowercased()

        // Map key names to SpecialKey enum
        let keyMap: [String: SpecialKey] = [
            "enter": .enter,
            "return": .enter,
            "tab": .tab,
            "esc": .escape,
            "escape": .escape,
            "leftsuper": .leftSuper,
            "command": .leftSuper,
            "cmd": .leftSuper,
            "leftshift": .leftShift,
            "shift": .leftShift,
            "leftalt": .leftAlt,
            "option": .leftAlt,
            "alt": .leftAlt,
            "leftctrl": .leftCtrl,
            "ctrl": .leftCtrl,
            "control": .leftCtrl,
            "backspace": .backspace,
            "delete": .delete,
            "up": .up,
            "down": .down,
            "left": .left,
            "right": .right,
            "space": .space,
            "f1": .f1, "f2": .f2, "f3": .f3, "f4": .f4,
            "f5": .f5, "f6": .f6, "f7": .f7, "f8": .f8,
            "f9": .f9, "f10": .f10, "f11": .f11, "f12": .f12
        ]

        guard let key = keyMap[keyName] else {
            return nil
        }

        return .keyPress(key)
    }
}

/// Errors from boot command parsing
enum BootCommandParserError: Error, LocalizedError {
    case unknownCommand(String)
    case invalidSyntax(String)

    var errorDescription: String? {
        switch self {
        case .unknownCommand(let cmd):
            return "Unknown boot command: \(cmd)"
        case .invalidSyntax(let msg):
            return "Invalid boot command syntax: \(msg)"
        }
    }
}
