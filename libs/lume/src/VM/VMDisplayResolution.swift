import Foundation
import ArgumentParser

struct VMDisplayResolution: Codable, ExpressibleByArgument {
        let width: Int
        let height: Int
        
        init?(string: String) {
            let components = string.components(separatedBy: "x")
            guard components.count == 2,
                  let width = Int(components[0]),
                  let height = Int(components[1]),
                  width > 0, height > 0 else {
                return nil
            }
            self.width = width
            self.height = height
        }
        
        var string: String {
            "\(width)x\(height)"
        }
        
        init?(argument: String) {
            guard let resolution = VMDisplayResolution(string: argument) else { return nil }
            self = resolution
        }
    }