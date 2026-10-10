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

    mutating func take(at time: Date = Date(), final: Bool = false, lastVoiceAt: Date? = nil) -> String? {
        guard !text.isEmpty, final || time.timeIntervalSince(max(changedAt, lastVoiceAt ?? changedAt)) >= 1.0 else { return nil }
        let value = text
        self = Utterance()
        return value
    }
}

// Capture stays running between utterances; recognition can renew independently.
final class AudioFeed {
    private let lock = NSLock()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var position: Double = 0
    private var voicedAt = Date.distantPast

    func attach(_ request: SFSpeechAudioBufferRecognitionRequest?) {
        lock.lock(); defer { lock.unlock() }
        self.request = request
        position = 0
    }
    func append(_ buffer: AVAudioPCMBuffer) {
        var power: Double = 0
        if let channels = buffer.floatChannelData {
            let frames = Int(buffer.frameLength)
            let count = Int(buffer.format.channelCount)
            if frames > 0, count > 0 {
                for channel in 0..<count { for frame in 0..<frames { power += Double(channels[channel][frame] * channels[channel][frame]) } }
                power = sqrt(power / Double(frames * count))
            }
        }
        lock.lock(); defer { lock.unlock() }
        if power > 0.01 { voicedAt = Date() }
        if let request = request {
            position += Double(buffer.frameLength) / buffer.format.sampleRate
            request.append(buffer)
        }
    }
    func snapshot() -> (position: Double, voicedAt: Date) {
        lock.lock(); defer { lock.unlock() }
        return (position, voicedAt)
    }
}

struct WordSpan {
    var text: String
    var end: Double
}

struct ContinuousTranscript {
    private var cutoff: Double = 0
    private var carried = ""
    func text(_ words: [WordSpan]) -> String {
        ([carried] + words.filter { $0.end > cutoff }.map { $0.text }).filter { !$0.isEmpty }.joined(separator: " ")
    }
    mutating func commit(at position: Double) { cutoff = position; carried = "" }
    mutating func renew(carrying text: String) { cutoff = 0; carried = text }
}

final class Microphone {
    var onText: (String) -> Void = { _ in }
    var onPartial: (String) -> Void = { _ in }
    var onStatus: (String) -> Void = { _ in }
    private let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
    private let engine = AVAudioEngine()
    private let feed = AudioFeed()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var timer: Timer?
    private var utterance = Utterance()
    private var transcript = ContinuousTranscript()
    private var generation = 0
    private var tapped = false
    private var starting = false
    private var beganAt = Date.distantPast
    private var retries = 0
    private(set) var listening = false

    func start() {
        guard !listening, !starting else { return }
        starting = true
        generation += 1
        let current = generation
        Diagnostics.shared?.event("microphone.start", ["generation":current])
        onStatus("Waiting for microphone and speech permissions…")
        SFSpeechRecognizer.requestAuthorization { status in
            DispatchQueue.main.async {
                guard current == self.generation else { return }
                Diagnostics.shared?.event("speech.authorization", ["status":status.rawValue])
                guard status == .authorized else {
                    self.starting = false
                    self.onStatus("Allow geniewish in System Settings → Privacy & Security → Speech Recognition.")
                    return
                }
                AVCaptureDevice.requestAccess(for: .audio) { allowed in
                    DispatchQueue.main.async {
                        guard current == self.generation else { return }
                        self.starting = false
                        Diagnostics.shared?.event("microphone.authorization", ["allowed":allowed])
                        guard allowed else { self.onStatus("Allow geniewish in System Settings → Privacy & Security → Microphone."); return }
                        guard self.recognizer?.supportsOnDeviceRecognition == true else {
                            self.onStatus("On-device English recognition is unavailable. Audio was not sent to a cloud service."); return
                        }
                        self.listening = true
                        self.startCapture()
                    }
                }
            }
        }
    }

    private func startCapture() {
        let node = engine.inputNode
        let format = node.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else {
            stop("no_microphone"); onStatus("No microphone is available. Connect one and try again."); return
        }
        node.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in self.feed.append(buffer) }
        tapped = true
        do {
            engine.prepare()
            try engine.start()
            Diagnostics.shared?.event("capture.running", ["sample_rate":format.sampleRate,"channels":format.channelCount])
            beginRecognition()
            timer = Timer.scheduledTimer(withTimeInterval: 0.15, repeats: true) { _ in
                self.finish()
                if self.listening, self.request != nil, self.utterance.text.isEmpty, Date().timeIntervalSince(self.beganAt) > 45 { self.renew(after:0.1) }
            }
        } catch {
            Diagnostics.shared?.error("microphone.error", error)
            stop("capture_error")
            onStatus("Microphone could not start: \(error.localizedDescription)")
        }
    }

    func stop(_ reason: String = "requested") {
        Diagnostics.shared?.event("microphone.stop", ["reason":reason,"generation":generation])
        listening = false
        starting = false
        timer?.invalidate(); timer = nil
        cancelRecognition()
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus:0); tapped = false }
        utterance = Utterance()
        transcript = ContinuousTranscript()
        onPartial("")
        onStatus("Microphone paused")
    }

    private func cancelRecognition() {
        generation += 1
        feed.attach(nil)
        request?.endAudio()
        task?.cancel()
        request = nil; task = nil
    }

    private func renew(after delay: Double) {
        let pending = utterance.text
        cancelRecognition()
        transcript.renew(carrying:pending)
        let current = generation
        Diagnostics.shared?.event("recognition.retry", ["delay_seconds":delay,"capture_running":engine.isRunning])
        DispatchQueue.main.asyncAfter(deadline:.now()+delay) {
            if self.listening, self.generation == current { self.beginRecognition() }
        }
    }

    private func beginRecognition() {
        guard listening else { return }
        guard let recognizer = recognizer, recognizer.isAvailable else {
            retries += 1
            onStatus("Microphone on · reconnecting speech recognition…")
            renew(after:min(Double(retries),5)); return
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
        request.addsPunctuation = false
        request.contextualStrings = ["geniewish", "genie", "hire a new genie", "Mike", "Lucas", "Annie", "Ben", "Clara", "Allan", "Oscar", "Ruby", "Zoe", "Hugo"]
        self.request = request
        feed.attach(request)
        beganAt = Date()
        let current = generation
        Diagnostics.shared?.event("recognition.start", ["generation":current,"on_device":true,"capture_running":engine.isRunning])
        task = recognizer.recognitionTask(with:request) { result, error in
            DispatchQueue.main.async {
                guard current == self.generation, self.listening else { return }
                if let result = result {
                    let words = result.bestTranscription.segments.map { WordSpan(text:$0.substring,end:$0.timestamp+$0.duration) }
                    let text = self.transcript.text(words)
                    if self.utterance.text != text {
                        let scores = result.bestTranscription.segments.map { $0.confidence }
                        Diagnostics.shared?.event("recognition.partial", ["chars":text.count,"final":result.isFinal,"confidence":scores.isEmpty ? 0 : scores.reduce(0,+)/Float(scores.count)])
                    }
                    self.utterance.update(text)
                    self.onPartial(text)
                    self.retries = 0
                    if result.isFinal {
                        self.finish()
                        self.renew(after:0.1)
                        return
                    }
                }
                if let error = error {
                    Diagnostics.shared?.error("recognition.error", error)
                    if SFSpeechRecognizer.authorizationStatus() != .authorized || AVCaptureDevice.authorizationStatus(for:.audio) != .authorized {
                        self.stop("permission_revoked"); self.onStatus("Microphone permission changed. Allow access to continue."); return
                    }
                    self.retries += 1
                    self.onStatus("Microphone on · reconnecting speech recognition…")
                    self.renew(after:min(Double(self.retries),5))
                }
            }
        }
        onStatus("Listening · pause briefly to send · use headphones")
    }

    private func finish() {
        let audio = feed.snapshot()
        guard let text = utterance.take(lastVoiceAt:audio.voicedAt) else { return }
        transcript.commit(at:audio.position)
        Diagnostics.shared?.text("recognition.committed", text, fields:["endpoint":"silence","generation":generation,"capture_running":engine.isRunning])
        onText(text)
        onPartial("")
        // The capture engine and current recognizer keep running. Old words are excluded by the audio cutoff.
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
        Diagnostics.shared?.event("playback.stop", ["interrupted":playing != nil,"queued":queue.count])
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
        Diagnostics.shared?.event("playback.start", ["voice":event["voice"] ?? "Daniel","chars":text.count,"has_delivery":event["delivery"] != nil])
        playing = process
        process.terminationHandler = { ended in
            Diagnostics.shared?.event("playback.end", ["status":ended.terminationStatus,"generation":current])
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
            Diagnostics.shared?.error("playback.error", error)
            playing = nil
            onError("Speech could not start: \(error.localizedDescription)")
            next()
        }
    }
}
