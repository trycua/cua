/// The state the screen shows; the unit tests drive it without a screen.
struct Counter: Equatable {
    private(set) var value = 0

    mutating func tap() { value += 1 }
    var label: String { value == 0 ? "Not tapped yet" : "Tapped \(value) time\(value == 1 ? "" : "s")" }
}
