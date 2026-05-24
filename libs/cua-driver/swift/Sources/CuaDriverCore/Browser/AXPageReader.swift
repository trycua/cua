import Foundation

/// Extract page content from a WKWebView / Tauri app's AX tree markdown.
///
/// Used by `PageTool` as a JS-free fallback for apps where the WebKit remote
/// inspector is blocked (macOS 15+, Tauri without `TAURI_WEBVIEW_AUTOMATION`).
///
/// ## Input format (treeMarkdown)
///
/// Each line is an AX element rendered as:
/// ```
///   - [idx] AXRole "title" = "value" (description) help="..." id=identifier
/// ```
/// Indentation (two spaces per depth level) encodes the tree structure.
/// Interactive elements have `[idx]`; non-interactive elements omit it.
public enum AXPageReader {

    // MARK: - Parsed element

    public struct Element {
        public let role: String
        public let title: String
        public let value: String
        public let description: String
        public let index: Int?    // element_index if interactive, nil otherwise
    }

    // MARK: - Text extraction

    /// Return the visible text content of the page by collecting all
    /// `AXStaticText`, `AXHeading`, and value-bearing elements from the tree.
    ///
    /// - Parameter treeMarkdown: The `treeMarkdown` string from an `AppStateSnapshot`.
    /// - Returns: Concatenated text content, newline-separated. Empty if none found.
    public static func extractText(from treeMarkdown: String) -> String {
        var lines: [String] = []
        for line in treeMarkdown.split(separator: "\n", omittingEmptySubsequences: false) {
            let str = String(line)
            guard let parsed = parseLine(str) else { continue }
            switch parsed.role {
            case "AXStaticText", "AXHeading", "AXWebArea":
                // title first, then value, then description as fallback.
                // WebKit/Tauri AX trees sometimes store text content in description.
                let text = !parsed.title.isEmpty ? parsed.title
                         : !parsed.value.isEmpty ? parsed.value
                         : parsed.description
                if !text.isEmpty { lines.append(text) }
            default:
                // Include text for inputs, links, buttons with meaningful content.
                let text = !parsed.title.isEmpty ? parsed.title
                         : !parsed.value.isEmpty ? parsed.value
                         : parsed.description
                if !text.isEmpty, parsed.role != "AXWindow", parsed.role != "AXApplication",
                   parsed.role != "AXGroup", parsed.role != "AXScrollArea",
                   parsed.role != "AXSplitGroup", parsed.role != "AXSplitter",
                   parsed.role != "AXMenuBar", parsed.role != "AXMenu",
                   parsed.role != "AXMenuBarItem", parsed.role != "AXUnknown" {
                    lines.append(text)
                }
            }
        }
        // De-duplicate consecutive identical lines (AX trees often repeat titles).
        var deduped: [String] = []
        for line in lines {
            if deduped.last != line { deduped.append(line) }
        }
        return deduped.joined(separator: "\n")
    }

    // MARK: - DOM query (CSS selector → AX role mapping)

    /// Query the AX tree using a CSS selector, mapping element types to AX roles.
    ///
    /// Supported selector forms:
    /// - Tag selectors: `a`, `button`, `input`, `h1`–`h6`, `p`, `img`, `select`, `li`
    /// - Class/id selectors: ignored (AX tree has no class/id concept); returns all
    ///   elements of the mapped role if a class/id selector is combined with a tag,
    ///   or all interactive elements if only a class/id is given.
    /// - `*` — returns all elements.
    ///
    /// - Parameter selector: CSS selector string.
    /// - Parameter treeMarkdown: The `treeMarkdown` string from an `AppStateSnapshot`.
    /// - Returns: Array of matched elements (may be empty).
    public static func query(selector: String, from treeMarkdown: String) -> [Element] {
        let roles = cssToAXRoles(selector)
        let matchAll = roles.isEmpty  // empty means wildcard

        var results: [Element] = []
        for line in treeMarkdown.split(separator: "\n", omittingEmptySubsequences: false) {
            guard let parsed = parseLine(String(line)) else { continue }
            if matchAll || roles.contains(parsed.role) {
                results.append(parsed)
            }
        }
        return results
    }

    // MARK: - CSS → AX role mapping

    /// Map a simplified CSS selector to a set of matching AX role strings.
    /// Returns an empty set for wildcards or unrecognised selectors (caller
    /// should treat empty as "match everything").
    private static func cssToAXRoles(_ selector: String) -> Set<String> {
        // Strip pseudo-classes, attribute selectors, combinators for simplicity.
        let cleaned = selector
            .components(separatedBy: CharacterSet(charactersIn: ":>+~["))
            .first?
            .trimmingCharacters(in: .whitespaces) ?? selector

        // If it's purely a class (.foo) or id (#foo) selector we can't map it.
        if cleaned.hasPrefix(".") || cleaned.hasPrefix("#") || cleaned == "*" || cleaned.isEmpty {
            return []
        }

        // Extract the tag portion (before any class/id modifier).
        let tag = cleaned
            .components(separatedBy: CharacterSet(charactersIn: ".#"))
            .first?
            .lowercased() ?? cleaned.lowercased()

        switch tag {
        case "a", "link":                 return ["AXLink"]
        case "button":                    return ["AXButton"]
        case "input":                     return ["AXTextField", "AXCheckBox", "AXRadioButton",
                                                   "AXSlider", "AXComboBox", "AXSearchField",
                                                   "AXSecureTextField"]
        case "select":                    return ["AXComboBox", "AXPopUpButton"]
        case "textarea":                  return ["AXTextArea"]
        case "img", "image":              return ["AXImage"]
        case "h1", "h2", "h3",
             "h4", "h5", "h6":           return ["AXHeading"]
        case "p", "span", "div",
             "section", "article",
             "main", "header", "footer": return ["AXStaticText", "AXGroup"]
        case "li":                        return ["AXCell", "AXStaticText"]
        case "table":                     return ["AXTable"]
        case "tr":                        return ["AXRow"]
        case "td", "th":                  return ["AXCell"]
        case "nav":                       return ["AXToolbar"]
        case "form":                      return ["AXGroup"]
        default:                          return []
        }
    }

    // MARK: - Line parser

    /// Parse one treeMarkdown line into an `Element`.
    /// Returns `nil` for blank lines or lines that don't match the format.
    ///
    /// Format: `<indent>- [idx] AXRole "title" = "value" (description) ...`
    private static func parseLine(_ line: String) -> Element? {
        // Must start with "- " (possibly indented).
        guard let dashRange = line.range(of: "- ") else { return nil }
        var rest = String(line[dashRange.upperBound...])

        // Optional element index: [42]
        var index: Int? = nil
        if rest.hasPrefix("[") {
            if let close = rest.firstIndex(of: "]") {
                let idxStr = String(rest[rest.index(after: rest.startIndex)..<close])
                index = Int(idxStr)
                rest = String(rest[rest.index(after: close)...]).trimmingCharacters(in: .init(charactersIn: " "))
            }
        }

        // Role: first whitespace-delimited token.
        let tokens = rest.components(separatedBy: " ")
        guard let role = tokens.first, !role.isEmpty else { return nil }
        rest = rest.dropFirst(role.count).trimmingCharacters(in: .whitespaces)

        // Title: optional quoted string.
        var title = ""
        if rest.hasPrefix("\"") {
            if let end = rest.dropFirst().firstIndex(of: "\"") {
                title = String(rest[rest.index(after: rest.startIndex)..<end])
                rest = String(rest[rest.index(after: end)...]).trimmingCharacters(in: .whitespaces)
            }
        }

        // Value: optional = "..."
        var value = ""
        if rest.hasPrefix("= \"") {
            let afterEq = rest.dropFirst(3)  // drop `= "`
            if let end = afterEq.firstIndex(of: "\"") {
                value = String(afterEq[afterEq.startIndex..<end])
                rest = String(afterEq[afterEq.index(after: end)...]).trimmingCharacters(in: .whitespaces)
            }
        }

        // Description: optional (...)
        var description = ""
        if rest.hasPrefix("(") {
            if let end = rest.firstIndex(of: ")") {
                description = String(rest[rest.index(after: rest.startIndex)..<end])
            }
        }

        return Element(role: role, title: title, value: value, description: description, index: index)
    }
}
