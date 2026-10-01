import Foundation

struct ProgressLogger {
    private var lastLoggedProgress: Double = 0.0
    private let threshold: Double

    init(threshold: Double = 0.05) {
        self.threshold = threshold
    }

    mutating func logProgress(current: Double, context: String) {
        if current - lastLoggedProgress >= threshold {
            lastLoggedProgress = current
            let percentage = Int(current * 100)
            Logger.info("\(context) Progress: \(percentage)%")
        }
    }
} 