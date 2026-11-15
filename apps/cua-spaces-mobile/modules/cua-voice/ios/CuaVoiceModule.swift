// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AVFoundation
import ExpoModulesCore
import Speech

/// Push-to-talk dictation with the phone's own speech recognizer. On the phone when the language supports it
/// (`requiresOnDeviceRecognition`); otherwise the system speech service. Nothing is sent to a Cua server.
/// Permission is two separate system prompts (microphone, speech); both are asked on the first press and reported
/// to JavaScript as words (`undetermined`, `granted`, `denied`, `restricted`) for the app core's `voice.view`.
public class CuaVoiceModule: Module {
  private var recognizer: SFSpeechRecognizer?
  private let engine = AVAudioEngine()
  private var request: SFSpeechAudioBufferRecognitionRequest?
  private var task: SFSpeechRecognitionTask?
  private var listening = false

  public func definition() -> ModuleDefinition {
    Name("CuaVoice")

    Constants(["available": true])
    Events("onResult", "onEnd", "onError")

    AsyncFunction("status") { (locale: String?) -> [String: Any] in
      let r = self.makeRecognizer(locale)
      return [
        "microphone": CuaVoiceModule.microphone(),
        "speech": CuaVoiceModule.speech(),
        "recognizer": r?.isAvailable ?? false,
        "onDevice": r?.supportsOnDeviceRecognition ?? false,
        "listening": self.listening,
      ]
    }

    AsyncFunction("requestPermissions") { (promise: Promise) in
      SFSpeechRecognizer.requestAuthorization { _ in
        CuaVoiceModule.requestMicrophone { _ in
          promise.resolve(["microphone": CuaVoiceModule.microphone(), "speech": CuaVoiceModule.speech()])
        }
      }
    }

    AsyncFunction("start") { (locale: String?) throws in
      try self.begin(locale)
    }

    AsyncFunction("stop") { () in
      self.finish()
    }

    OnDestroy { self.teardown() }
  }

  private func makeRecognizer(_ locale: String?) -> SFSpeechRecognizer? {
    if let id = locale, !id.isEmpty { return SFSpeechRecognizer(locale: Locale(identifier: id)) }
    return SFSpeechRecognizer()
  }

  private func begin(_ locale: String?) throws {
    if listening { return }
    guard CuaVoiceModule.speech() == "granted", CuaVoiceModule.microphone() == "granted" else {
      throw CuaVoiceError("permission")
    }
    guard let rec = makeRecognizer(locale), rec.isAvailable else { throw CuaVoiceError("recognizer") }
    recognizer = rec
    let req = SFSpeechAudioBufferRecognitionRequest()
    req.shouldReportPartialResults = true
    // On the phone when the language supports it: no audio leaves it.
    req.requiresOnDeviceRecognition = rec.supportsOnDeviceRecognition
    request = req

    let session = AVAudioSession.sharedInstance()
    try session.setCategory(.record, mode: .measurement, options: .duckOthers)
    try session.setActive(true, options: .notifyOthersOnDeactivation)

    let input = engine.inputNode
    let format = input.outputFormat(forBus: 0)
    input.removeTap(onBus: 0)
    input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
      self?.request?.append(buffer)
    }
    engine.prepare()
    try engine.start()
    listening = true

    task = rec.recognitionTask(with: req) { [weak self] result, error in
      guard let self else { return }
      if let r = result {
        self.sendEvent("onResult", ["text": r.bestTranscription.formattedString, "final": r.isFinal])
      }
      if error != nil || (result?.isFinal ?? false) {
        let kind = CuaVoiceModule.kind(of: error)
        self.teardown()
        if let error, kind != "none" { self.sendEvent("onError", ["kind": kind, "message": error.localizedDescription]) }
        self.sendEvent("onEnd", [:])
      }
    }
  }

  /// Stops listening and lets the recognizer deliver what it has.
  private func finish() {
    guard listening else { return }
    if engine.isRunning { engine.stop() }
    engine.inputNode.removeTap(onBus: 0)
    request?.endAudio()
  }

  private func teardown() {
    if engine.isRunning { engine.stop() }
    engine.inputNode.removeTap(onBus: 0)
    request?.endAudio()
    task?.cancel()
    task = nil
    request = nil
    listening = false
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
  }

  static func microphone() -> String {
    if #available(iOS 17.0, *) {
      switch AVAudioApplication.shared.recordPermission {
      case .granted: return "granted"
      case .denied: return "denied"
      default: return "undetermined"
      }
    }
    switch AVAudioSession.sharedInstance().recordPermission {
    case .granted: return "granted"
    case .denied: return "denied"
    default: return "undetermined"
    }
  }

  static func speech() -> String {
    switch SFSpeechRecognizer.authorizationStatus() {
    case .authorized: return "granted"
    case .denied: return "denied"
    case .restricted: return "restricted"
    default: return "undetermined"
    }
  }

  static func requestMicrophone(_ done: @escaping (Bool) -> Void) {
    if #available(iOS 17.0, *) {
      AVAudioApplication.requestRecordPermission { done($0) }
    } else {
      AVAudioSession.sharedInstance().requestRecordPermission { done($0) }
    }
  }

  /// `no-speech` for a silent press, `network` for the speech service being out of reach, `audio`, else `other`; `none` for a cancel.
  static func kind(of error: Error?) -> String {
    guard let e = error as NSError? else { return "none" }
    if e.domain == "kAFAssistantErrorDomain" {
      switch e.code {
      case 1110: return "no-speech"
      case 216, 203: return "none"   // cancelled / the request ended
      case 1101, 1107: return "network"
      default: break
      }
    }
    if e.domain == NSURLErrorDomain { return "network" }
    if e.domain == "com.apple.coreaudio.avfaudio" { return "audio" }
    return "other"
  }
}

struct CuaVoiceError: Error, CustomStringConvertible {
  let description: String
  init(_ d: String) { description = d }
}
