import AppKit
import AVFoundation
import Speech

// Only settled text goes to Rust; partial recognition is presentation-only.
struct Utterance {
    var text = ""
    var changedAt = Date.distantPast

    mutating func update(_ value: String, at time: Date = Date()) {
        if value != text {
            text = value
            changedAt = time
        }
    }

    mutating func take(at time: Date = Date(), final: Bool = false) -> String? {
        guard !text.isEmpty, final || time.timeIntervalSince(changedAt) >= 1.0 else { return nil }
        let value = text
        self = Utterance()
        return value
    }
}

final class Microphone {
    var onText: (String) -> Void = { _ in }
    var onPartial: (String) -> Void = { _ in }
    var onStatus: (String) -> Void = { _ in }
    private let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
    private let engine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var timer: Timer?
    private var utterance = Utterance()
    private var generation = 0
    private var tapped = false
    private var starting = false
    private var beganAt = Date.distantPast
    private(set) var listening = false

    func start() {
        guard !listening, !starting else { return }
        starting = true
        generation += 1
        let current = generation
        onStatus("Waiting for microphone and speech permissions…")
        SFSpeechRecognizer.requestAuthorization { status in
            DispatchQueue.main.async {
                guard current == self.generation else { return }
                guard status == .authorized else {
                    self.starting = false
                    self.onStatus("Allow geniewish in System Settings → Privacy & Security → Speech Recognition.")
                    return
                }
                AVCaptureDevice.requestAccess(for: .audio) { allowed in
                    DispatchQueue.main.async {
                        guard current == self.generation else { return }
                        self.starting = false
                        guard allowed else {
                            self.onStatus("Allow geniewish in System Settings → Privacy & Security → Microphone.")
                            return
                        }
                        guard self.recognizer?.supportsOnDeviceRecognition == true else {
                            self.onStatus("On-device English recognition is unavailable. Audio was not sent to a cloud service.")
                            return
                        }
                        self.listening = true
                        self.begin()
                        guard self.listening else { return }
                        self.timer = Timer.scheduledTimer(withTimeInterval: 0.15, repeats: true) { _ in
                            self.finish()
                            if self.listening, self.utterance.text.isEmpty, Date().timeIntervalSince(self.beganAt) > 45 { self.begin() }
                        }
                    }
                }
            }
        }
    }

    func stop() {
        listening = false
        starting = false
        timer?.invalidate()
        timer = nil
        end()
        onPartial("")
        onStatus("Microphone paused")
    }

    private func end() {
        generation += 1
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus: 0); tapped = false }
        request?.endAudio()
        task?.cancel()
        task = nil
        request = nil
        utterance = Utterance()
    }

    private func begin() {
        guard listening else { return }
        end()
        guard let recognizer = recognizer, recognizer.isAvailable else {
            stop()
            onStatus("English speech recognition is unavailable. Try Start listening again.")
            return
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
        request.addsPunctuation = false
        request.contextualStrings = ["geniewish", "genie", "Mike", "Lucas", "Annie", "Ben", "Clara", "Allan", "Oscar", "Ruby", "Zoe", "Hugo"]
        self.request = request
        let current = generation
        beganAt = Date()
        let node = engine.inputNode
        let format = node.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else {
            stop()
            onStatus("No microphone is available. Connect one and try again.")
            return
        }
        node.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in request.append(buffer) }
        tapped = true
        task = recognizer.recognitionTask(with: request) { result, error in
            DispatchQueue.main.async {
                guard current == self.generation, self.listening else { return }
                if let result = result {
                    self.utterance.update(result.bestTranscription.formattedString)
                    self.onPartial(self.utterance.text)
                    if result.isFinal {
                        if self.utterance.text.isEmpty { self.begin() }
                        else { self.finish(final: true) }
                    }
                }
                if let error = error, current == self.generation {
                    self.stop()
                    self.onStatus("Recognition stopped: \(error.localizedDescription). Try Start listening again.")
                }
            }
        }
        do {
            engine.prepare()
            try engine.start()
            onStatus("Listening · pause briefly to send · use headphones")
        } catch {
            stop()
            onStatus("Microphone could not start: \(error.localizedDescription)")
        }
    }

    private func finish(final: Bool = false) {
        guard let text = utterance.take(final: final) else { return }
        end()
        onText(text)
        onPartial("")
        let current = generation
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
            if self.generation == current { self.begin() }
        }
    }
}

// say chooses the same installed voices as the roster; no speech service or audio files.
final class Speaker {
    var onDelivered: ([String: Any]) -> Void = { _ in }
    var onError: (String) -> Void = { _ in }
    private var queue: [[String: Any]] = []
    private var playing: Process?
    private var generation = 0
    var humanSpeaking = false {
        didSet {
            if humanSpeaking && !oldValue { stop() }
            if !humanSpeaking { next() }
        }
    }

    func enqueue(_ event: [String: Any]) {
        queue.append(event)
        next()
    }

    func stop() {
        generation += 1
        queue.removeAll()
        if playing?.isRunning == true { playing?.terminate() }
        playing = nil
        // An interrupted result is never acknowledged as delivered.
    }

    private func next() {
        guard playing == nil, !humanSpeaking, !queue.isEmpty else { return }
        let event = queue.removeFirst()
        guard let text = event["text"] as? String, !text.isEmpty else { next(); return }
        let process = Process()
        let input = Pipe()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/say")
        process.arguments = ["-v", event["voice"] as? String ?? "Daniel"]
        process.standardInput = input
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        let current = generation
        playing = process
        process.terminationHandler = { ended in
            DispatchQueue.main.async {
                guard current == self.generation else { return }
                self.playing = nil
                if ended.terminationStatus == 0 {
                    if let delivery = event["delivery"] as? [String: Any] { self.onDelivered(delivery) }
                } else {
                    self.onError("Speech playback failed. The message remains unread.")
                }
                self.next()
            }
        }
        do {
            try process.run()
            // Keep long answers off the main/UI thread and avoid pipe backpressure.
            DispatchQueue.global().async {
                try? input.fileHandleForWriting.write(contentsOf: Data(text.utf8))
                try? input.fileHandleForWriting.close()
            }
        } catch {
            playing = nil
            onError("Speech could not start: \(error.localizedDescription)")
            next()
        }
    }
}
