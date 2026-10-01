import Foundation

extension String {
    func padding(_ toLength: Int) -> String {
        return self.padding(toLength: toLength, withPad: " ", startingAt: 0)
    }
}
