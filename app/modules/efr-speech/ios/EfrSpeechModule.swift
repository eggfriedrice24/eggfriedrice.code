// Voice input on the iPhone: speech becomes text with SpeechAnalyzer of iOS 26. The audio
// buffers of AVAudioEngine go to the analyzer live, so no audio file is written and no
// audio leaves the phone. The app puts the text into the draft; this module sends nothing.
//
// - `isAvailable` is false before iOS 26, and when SpeechTranscriber does not run on this
//   iPhone or does not support its language. The app then shows no microphone.
// - `start` asks for the microphone, gets the speech model of the language once through
//   AssetInventory (the event `phase` with `model`), and records (`phase` with
//   `recording`).
// - Each result gives the event `transcript` with the whole text so far: the final parts,
//   then the newest volatile part, which a later result can still change.
// - `stop` ends the input, and the analyzer finalizes the last part. A phone call or Siri
//   stops the recording too. Either way the last `transcript` comes, then `end`.
//
// This module shares no code with the other modules of the app. If it does not compile,
// one line in app/package.json (`expo.autolinking.exclude`) takes it out of the build, and
// the app then shows no microphone.

import AVFoundation
import ExpoModulesCore
import Foundation
import Speech

@ExpoModule("EfrSpeech")
public class EfrSpeechModule: Module {
  /// `model` while the speech model downloads, then `recording`.
  @Event
  var onPhase: (SpeechPhaseEvent) -> Void

  /// The whole text so far.
  @Event
  var onTranscript: (SpeechTranscriptEvent) -> Void

  /// The recording ended: `stopped`, `interrupted` or `failed`.
  @Event
  var onEnd: (SpeechEndEvent) -> Void

  /// The one recording, while it runs. It is a `Recording`, which exists only on iOS 26.
  private var current: AnyObject?
  private let lock = NSLock()

  /// True when this iPhone can turn speech into text in its language.
  @JS(.concurrent)
  func isAvailable() async -> Bool {
    guard #available(iOS 26.0, *) else {
      return false
    }
    return await Recording.locale() != nil
  }

  /// Starts a recording. It resolves when the recording runs.
  @JS(.concurrent)
  func start() async throws {
    guard #available(iOS 26.0, *) else {
      throw SpeechUnavailableException()
    }
    guard let locale = await Recording.locale() else {
      throw SpeechUnavailableException()
    }
    guard await AVAudioApplication.requestRecordPermission() else {
      throw MicrophoneDeniedException()
    }
    let recording = Recording(locale: locale, events: events())
    try lock.withLock {
      guard current == nil else {
        throw AlreadyRecordingException()
      }
      current = recording
    }
    do {
      try await recording.start()
    } catch {
      release(recording)
      throw error
    }
  }

  /// Stops the recording and resolves when the analyzer is done; the last `transcript`
  /// and `end` follow as events. With no recording, it does nothing.
  @JS(.concurrent)
  func stop() async {
    guard #available(iOS 26.0, *) else {
      return
    }
    guard let recording = lock.withLock({ current }) as? Recording else {
      return
    }
    await recording.stop(reason: "stopped")
  }

  private func events() -> RecordingEvents {
    return RecordingEvents(
      phase: { [weak self] phase in
        self?.onPhase(SpeechPhaseEvent(phase: phase))
      },
      transcript: { [weak self] text in
        self?.onTranscript(SpeechTranscriptEvent(text: text))
      },
      end: { [weak self] recording, reason, message in
        self?.release(recording)
        self?.onEnd(SpeechEndEvent(reason: reason, message: message))
      }
    )
  }

  private func release(_ recording: AnyObject) {
    lock.withLock {
      if current === recording {
        current = nil
      }
    }
  }
}

@Record
struct SpeechPhaseEvent {
  var phase: String
}

@Record
struct SpeechTranscriptEvent {
  var text: String
}

@Record
struct SpeechEndEvent {
  var reason: String
  /// Why a `failed` recording failed.
  var message: String?
}

/// What a recording tells the module.
private struct RecordingEvents {
  let phase: (String) -> Void
  let transcript: (String) -> Void
  let end: (AnyObject, String, String?) -> Void
}

/// One recording: the microphone, the converter to the format of the analyzer, the
/// analyzer and the reader of its results.
@available(iOS 26.0, *)
private final class Recording: @unchecked Sendable {
  private let transcriber: SpeechTranscriber
  private let analyzer: SpeechAnalyzer
  private let engine = AVAudioEngine()
  /// The buffers wait here until the analyzer takes them. The app stops a recording after
  /// 5 minutes, which limits them.
  private let input: AsyncStream<AnalyzerInput>
  private let feed: AsyncStream<AnalyzerInput>.Continuation
  private let events: RecordingEvents
  private let lock = NSLock()
  private var ended = false
  private var results: Task<Void, Never>?
  private var interruption: NSObjectProtocol?
  private var configuration: NSObjectProtocol?

  init(locale: Locale, events: RecordingEvents) {
    transcriber = SpeechTranscriber(
      locale: locale,
      transcriptionOptions: [],
      reportingOptions: [.volatileResults],
      attributeOptions: []
    )
    analyzer = SpeechAnalyzer(modules: [transcriber])
    (input, feed) = AsyncStream.makeStream(of: AnalyzerInput.self)
    self.events = events
  }

  /// The language of the iPhone as SpeechTranscriber names it, or nil when it has none.
  static func locale() async -> Locale? {
    guard SpeechTranscriber.isAvailable else {
      return nil
    }
    return await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current)
  }

  func start() async throws {
    do {
      if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
        events.phase("model")
        try await request.downloadAndInstall()
      }
      guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber])
      else {
        throw AudioFormatException()
      }
      // A stop during the download ends the recording before it uses the microphone.
      try checkRunning()
      let session = AVAudioSession.sharedInstance()
      try session.setCategory(.record, mode: .measurement, options: .duckOthers)
      // iOS mutes haptics while the input records; the haptics of the start and the
      // stop must play.
      try session.setAllowHapticsAndSystemSoundsDuringRecording(true)
      try session.setActive(true, options: .notifyOthersOnDeactivation)
      interruption = NotificationCenter.default.addObserver(
        forName: AVAudioSession.interruptionNotification, object: session, queue: nil
      ) { [weak self] note in
        let type = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
        if type == AVAudioSession.InterruptionType.began.rawValue {
          Task { await self?.stop(reason: "interrupted") }
        }
      }
      // A new input (AirPods, CarPlay) stops the engine itself, and no more buffers come.
      configuration = NotificationCenter.default.addObserver(
        forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil
      ) { [weak self] _ in
        Task { await self?.stop(reason: "interrupted") }
      }
      try await analyzer.start(inputSequence: input)
      results = Task { await self.read() }
      try checkRunning()
      try record(in: format)
    } catch {
      await finish()
      throw error
    }
    events.phase("recording")
  }

  /// Ends the recording once, then tells the module why.
  func stop(reason: String, message: String? = nil) async {
    guard
      lock.withLock({
        defer { ended = true }
        return !ended
      })
    else {
      return
    }
    await finish()
    events.end(self, reason, message)
  }

  private func checkRunning() throws {
    if lock.withLock({ ended }) {
      throw StoppedException()
    }
  }

  /// Sends the buffers of the microphone to the analyzer, in its format.
  private func record(in format: AVAudioFormat) throws {
    let node = engine.inputNode
    let microphone = node.outputFormat(forBus: 0)
    guard let converter = AVAudioConverter(from: microphone, to: format) else {
      throw AudioFormatException()
    }
    // Without it the converter adds frames at the start, and the time of the audio drifts.
    converter.primeMethod = .none
    node.installTap(onBus: 0, bufferSize: Recording.tapFrames, format: microphone) {
      [feed] buffer, _ in
      if let converted = Recording.convert(buffer, with: converter, to: format) {
        feed.yield(AnalyzerInput(buffer: converted))
      }
    }
    engine.prepare()
    try engine.start()
  }

  /// The frames of one buffer of the microphone: about 0.1 s at 44.1 kHz.
  private static let tapFrames: AVAudioFrameCount = 4096

  private static func convert(
    _ buffer: AVAudioPCMBuffer, with converter: AVAudioConverter, to format: AVAudioFormat
  ) -> AVAudioPCMBuffer? {
    let ratio = format.sampleRate / buffer.format.sampleRate
    let capacity = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up))
    guard let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacity) else {
      return nil
    }
    var given = false
    var error: NSError?
    let status = converter.convert(to: output, error: &error) { _, state in
      if given {
        state.pointee = .noDataNow
        return nil
      }
      given = true
      state.pointee = .haveData
      return buffer
    }
    return status == .error ? nil : output
  }

  /// Sends the text so far after each result. A volatile part stays until a final result
  /// for the same audio comes.
  private func read() async {
    var settled = ""
    do {
      for try await result in transcriber.results {
        let text = String(result.text.characters)
        if result.isFinal {
          settled += text
          events.transcript(settled)
        } else {
          events.transcript(settled + text)
        }
      }
    } catch {
      // `stop` waits for this task, so it runs in a task of its own.
      Task { await self.stop(reason: "failed", message: error.localizedDescription) }
    }
  }

  /// Stops the microphone and, when the analyzer runs, lets it finalize the last part and
  /// waits for its results.
  private func finish() async {
    if engine.isRunning {
      engine.stop()
    }
    engine.inputNode.removeTap(onBus: 0)
    feed.finish()
    for observer in [interruption, configuration].compactMap({ $0 }) {
      NotificationCenter.default.removeObserver(observer)
    }
    if let results {
      try? await analyzer.finalizeAndFinishThroughEndOfInput()
      await results.value
    }
    try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
  }
}

private final class SpeechUnavailableException: Exception {
  override var reason: String {
    "This iPhone cannot turn speech into text in its language"
  }
}

private final class MicrophoneDeniedException: Exception {
  override var reason: String {
    "efr may not use the microphone. Allow it in the iOS Settings of efr"
  }
}

private final class AlreadyRecordingException: Exception {
  override var reason: String {
    "A recording runs already"
  }
}

private final class StoppedException: Exception {
  override var reason: String {
    "The recording stopped before it started"
  }
}

private final class AudioFormatException: Exception {
  override var reason: String {
    "The audio of the microphone does not convert to the format of the speech model"
  }
}
