import Foundation

struct ImagesPrinter {
    private struct Column: Sendable {
        let header: String
        let width: Int
        let getValue: @Sendable (String) -> String
    }
    
    private static let columns: [Column] = [
        Column(header: "name", width: 28) { $0.split(separator: ":").first.map(String.init) ?? $0 },
        Column(header: "image_id", width: 16) { $0.split(separator: ":").last.map(String.init) ?? "-" }
    ]
    
    static func print(images: [String]) {
        if images.isEmpty {
            Swift.print("No images found")
            return
        }
        
        printHeader()
        images.sorted().forEach(printImage)
    }
    
    private static func printHeader() {
        let paddedHeaders = columns.map { $0.header.paddedToWidth($0.width) }
        Swift.print(paddedHeaders.joined())
    }
    
    private static func printImage(_ image: String) {
        let paddedColumns = columns.map { column in
            column.getValue(image).paddedToWidth(column.width)
        }
        Swift.print(paddedColumns.joined())
    }
}

private extension String {
    func paddedToWidth(_ width: Int) -> String {
        padding(toLength: width, withPad: " ", startingAt: 0)
    }
} 