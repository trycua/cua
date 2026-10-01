import AppKit
import Cua
import Foundation

/// Entry point. Headless (default, `CUA_HEADLESS=1`) runs the scenario on
/// a background task and blocks the main thread on it; window mode
/// (`CUA_HEADLESS=0`) runs the same scenario while an AppKit window renders
/// the decoded BGRA frames and AVAudioEngine plays the PCM.
@main
enum CuaStreamingMain {
    static func main() {
        setvbuf(stdout, nil, _IOLBF, 0)
        let cfg: Config
        do { cfg = try Config.fromEnvironment() } catch {
            fputs("error: \(error)\n", stderr)
            exit(2)
        }
        // The bench lane never opens a window.
        if cfg.headless || cfg.benchJsonl != nil {
            exit(runBlocking { try await runScenario(cfg: cfg, presenter: nil) })
        }
        WindowMode.run(cfg: cfg)
    }

    static func runScenario(cfg: Config, presenter: StreamPresenter?) async throws -> Int32 {
        let env = try await Scenario.connect(cfg: cfg)
        let scenario = Scenario(cfg: cfg, env: env, presenter: presenter)
        if let path = cfg.benchJsonl { return try await scenario.runBench(path: path) }
        return try await scenario.run()
    }

    /// Runs `body` on the cooperative pool and waits for it on this thread.
    static func runBlocking(_ body: @escaping @Sendable () async throws -> Int32) -> Int32 {
        final class Box: @unchecked Sendable { var code: Int32 = 1 }
        let box = Box()
        let done = DispatchSemaphore(value: 0)
        Task.detached {
            do { box.code = try await body() } catch {
                fputs("error: \(error)\n", stderr)
                box.code = 1
            }
            done.signal()
        }
        done.wait()
        return box.code
    }
}
