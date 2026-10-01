import Foundation
import CryptoKit

final class PassphraseGenerator {
    private let words: [String]
    
    init(words: [String] = PassphraseGenerator.defaultWords) {
        self.words = words
    }
    
    func prefix(_ count: Int) -> [String] {
        guard count > 0 else { return [] }
        
        // Use secure random number generation
        var result: [String] = []
        for _ in 0..<count {
            let randomBytes = (0..<4).map { _ in UInt8.random(in: 0...255) }
            let randomNumber = Data(randomBytes).withUnsafeBytes { bytes in
                bytes.load(as: UInt32.self)
            }
            let index = Int(randomNumber % UInt32(words.count))
            result.append(words[index])
        }
        return result
    }
    
    // A much larger set of common, easy-to-type words
    private static let defaultWords = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
        "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
        "quebec", "romeo", "sierra", "tango", "uniform", "victor", "whiskey", "xray",
        "yankee", "zulu", "zero", "one", "two", "three", "four", "five",
        "six", "seven", "eight", "nine", "apple", "banana", "cherry", "date",
        "elder", "fig", "grape", "honey", "iris", "jade", "kiwi", "lemon",
        "mango", "nectarine", "orange", "peach", "quince", "raspberry", "strawberry", "tangerine",
        "red", "blue", "green", "yellow", "purple", "orange", "pink", "brown",
        "black", "white", "gray", "silver", "gold", "copper", "bronze", "steel",
        "north", "south", "east", "west", "spring", "summer", "autumn", "winter",
        "river", "ocean", "mountain", "valley", "forest", "desert", "island", "beach",
        "sun", "moon", "star", "cloud", "rain", "snow", "wind", "storm",
        "happy", "brave", "calm", "swift", "wise", "kind", "bold", "free",
        "safe", "strong", "bright", "clear", "light", "soft", "warm", "cool",
        "eagle", "falcon", "hawk", "owl", "robin", "sparrow", "swan", "dove",
        "tiger", "lion", "bear", "wolf", "deer", "horse", "dolphin", "whale",
        "maple", "oak", "pine", "birch", "cedar", "fir", "palm", "willow",
        "rose", "lily", "daisy", "tulip", "lotus", "orchid", "violet", "jasmine"
    ]
}