import ArgumentParser

enum FormatOption: String, CaseIterable, ExpressibleByArgument {
    case json
    case text
}
