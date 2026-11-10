import SwiftUI

struct ContentView: View {
    @State private var counter = Counter()

    var body: some View {
        VStack(spacing: 24) {
            Text("Built in a macOS Space")
                .font(.title2.bold())
            Text(counter.label)
                .font(.headline)
                .accessibilityIdentifier("label")
            Button("Tap") { counter.tap() }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("tap")
        }
        .padding()
    }
}
