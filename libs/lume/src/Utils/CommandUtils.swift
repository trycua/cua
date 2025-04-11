import ArgumentParser
import Foundation

func completeVMName(_ arguments: [String]) -> [String] {
    (try? Home().getAllVMDirectories().map { $0.directory.name }) ?? []
}
