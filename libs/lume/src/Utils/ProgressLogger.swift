import Foundation

/// Progress of one long step (an IPSW download, a disk reassembly), drawn as
/// one live line through `Console`. Piped, it logs a line every `threshold`.
struct ProgressLogger {
    private var lastLoggedProgress: Double = 0.0
    private let threshold: Double
    private let console: Console?
    private var region: Console.Region?

    init(threshold: Double = 0.05, console: Console? = .shared) {
        self.threshold = threshold
        self.console = console
    }

    mutating func logProgress(current: Double, context: String) {
        guard current - lastLoggedProgress >= threshold || current >= 1 else { return }
        lastLoggedProgress = current
        let percentage = Int((min(max(current, 0), 1) * 100).rounded(.down))
        if region == nil { region = console?.region(.transfer, pipedInterval: 0) }
        if current >= 1 {
            region?.end(final: "\(context): done")
            region = nil
        } else {
            region?.update(["  \(context)  \(ProgressFormat.bar(current))  \(percentage)%"])
        }
    }
}
