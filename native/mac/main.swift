import AppKit
import AVFoundation
import Speech

func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.lastIndex(of: name), index + 1 < CommandLine.arguments.count else { return nil }
    return CommandLine.arguments[index + 1]
}

func check(_ condition: Bool, _ text: String) {
    if !condition { fputs("Failed: \(text)\n", stderr); exit(1) }
}

if CommandLine.arguments.contains("--diagnose") {
    let recognizer = SFSpeechRecognizer(locale: Locale(identifier: "en-US"))
    let result: [String: Any] = [
        "onDeviceRecognition": recognizer?.supportsOnDeviceRecognition ?? false,
        "recognizerAvailable": recognizer?.isAvailable ?? false,
        "speechAuthorization": SFSpeechRecognizer.authorizationStatus().rawValue,
        "microphoneAuthorization": AVCaptureDevice.authorizationStatus(for: .audio).rawValue,
        "audioSaved": false
    ]
    print(String(data: try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), encoding: .utf8)!)
    exit(0)
}

if CommandLine.arguments.contains("--self-test") {
    let now = Date()
    var buffer = Utterance()
    buffer.update("Talk to", at: now)
    check(buffer.take(at: now.addingTimeInterval(0.7)) == nil, "partial speech must not execute")
    buffer.update("Talk to Allan", at: now.addingTimeInterval(0.8))
    check(buffer.take(at: now.addingTimeInterval(1.5)) == nil, "a correction resets the endpoint")
    buffer.update("Talk to Allan", at: now.addingTimeInterval(1.7))
    check(buffer.take(at: now.addingTimeInterval(1.9)) == "Talk to Allan", "duplicate partials do not delay sending")
    check(buffer.take(at: now.addingTimeInterval(3)) == nil, "an utterance is sent once")
    buffer.update("Stop talking", at: now)
    check(buffer.take(at: now, final: true) == "Stop talking", "final recognition commits immediately")
    buffer.update("Keep every requirement", at: now)
    check(buffer.take(at:now.addingTimeInterval(1.5),lastVoiceAt:now.addingTimeInterval(1.4)) == nil, "ongoing audio prevents premature submission when text is unchanged")
    check(buffer.take(at:now.addingTimeInterval(2.5),lastVoiceAt:now.addingTimeInterval(1.4)) == "Keep every requirement", "a real silence ends the phrase")
    var transcript = ContinuousTranscript()
    check(transcript.text([WordSpan(text:"first",end:0.5)]) == "first", "first utterance is recognized")
    transcript.commit(at:1.5)
    check(transcript.text([WordSpan(text:"revised-first",end:0.6),WordSpan(text:"next",end:2.5)]) == "next", "old cumulative words are not submitted twice")
    transcript.renew(carrying:"unfinished phrase")
    check(transcript.text([WordSpan(text:"continued",end:0.4)]) == "unfinished phrase continued", "recognizer renewal preserves an unfinished utterance")
    let feed = AudioFeed()
    let format = AVAudioFormat(standardFormatWithSampleRate:16000,channels:1)!
    let audio = AVAudioPCMBuffer(pcmFormat:format,frameCapacity:160)!
    audio.frameLength = 160
    for index in 0..<160 { audio.floatChannelData![0][index] = 0 }
    feed.append(audio)
    check(feed.snapshot().voicedAt == .distantPast, "silence does not count as speaking")
    for index in 0..<160 { audio.floatChannelData![0][index] = 0.1 }
    feed.append(audio)
    check(feed.snapshot().voicedAt > now, "voiced audio updates the pause clock")
    print("Utterance checks passed")
    do {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("geniewish-log-test-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let logger = try Diagnostics(directory: directory, transcripts: false)
        logger.text("input", "not-for-the-log")
        let file = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil).first!
        check(!(try String(contentsOf: file, encoding: .utf8)).contains("not-for-the-log"), "transcripts are opt-in")
        let raw = try Diagnostics(directory: directory.appendingPathComponent("debug"), transcripts: true)
        raw.text("input", "debug-transcript")
        let debug = try FileManager.default.contentsOfDirectory(at: raw.directory, includingPropertiesForKeys: nil).first!
        check((try String(contentsOf: debug, encoding: .utf8)).contains("debug-transcript"), "debug transcripts are available")
        for _ in 0..<30 { logger.event("rotate", ["padding":String(repeating: "x", count:120_000)]) }
        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil).filter { !$0.hasDirectoryPath }
        check(files.count == 3, "log rotation is bounded")
        for file in files {
            let permissions = try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as! NSNumber
            check(permissions.intValue & 0o777 == 0o600, "logs are private")
            for line in try String(contentsOf: file, encoding: .utf8).split(separator:"\n") { _ = try JSONSerialization.jsonObject(with: Data(line.utf8)) }
        }
        print("Diagnostics checks passed")
    } catch { fputs("Diagnostics checks failed: \(error)\n", stderr); exit(1) }
    exit(0)
}

final class Application: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let microphone = Microphone()
    private let speaker = Speaker()
    private let core = Process()
    private let input = Pipe()
    private var window: NSWindow!
    private let title = NSTextField(labelWithString: "geniewish")
    private let status = NSTextField(wrappingLabelWithString: "Microphone off · click Start listening")
    private let partial = NSTextField(wrappingLabelWithString: "Use headphones so genies don't hear their own replies.")
    private let log = NSTextView()
    private let textInput = NSTextField()
    private let roster = NSStackView()
    private var active: String?
    private var chimeAt = Date.distantPast
    private var closing = false
    private var stderrText = ""
    private var ready = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            let saved = Bundle.main.url(forResource: "workspace", withExtension: "txt").flatMap { try? String(contentsOf: $0, encoding: .utf8) }.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            let workspace = argument("--workspace") ?? saved ?? FileManager.default.currentDirectoryPath
            let state = argument("--state-dir") ?? URL(fileURLWithPath: workspace).appendingPathComponent(".geniewish").path
            let directory = URL(fileURLWithPath: argument("--log-dir") ?? URL(fileURLWithPath: state).appendingPathComponent("logs").path)
            Diagnostics.shared = try Diagnostics(directory: directory, transcripts: CommandLine.arguments.contains("--debug-transcripts"))
            let build = Bundle.main.url(forResource: "build", withExtension: "txt").flatMap { try? String(contentsOf: $0, encoding: .utf8) } ?? "unknown"
            Diagnostics.shared?.event("startup", ["build": build.trimmingCharacters(in: .whitespacesAndNewlines), "transcripts": Diagnostics.shared?.transcripts ?? false])
        } catch { fputs("geniewish troubleshooting logs could not be opened.\n", stderr) }
        buildWindow()
        microphone.onStatus = { [weak self] text in
            Diagnostics.shared?.event("microphone.status", ["status":text])
            self?.status.stringValue = text
        }
        microphone.onPartial = { [weak self] text in
            self?.partial.stringValue = text.isEmpty ? "Use headphones · pause briefly to send" : "Hearing: \(text)"
            self?.speaker.humanSpeaking = !text.isEmpty
        }
        microphone.onText = { [weak self] text in self?.sendHuman(text, source: "speech") }
        speaker.onDelivered = { [weak self] delivery in self?.sendJSON(["type": "delivered", "delivery": delivery]) }
        speaker.onError = { [weak self] text in self?.append("geniewish", text) }
        connect()
    }

    private func buildWindow() {
        NSApp.setActivationPolicy(.regular)
        let menu = NSMenu()
        let applicationMenu = NSMenu()
        applicationMenu.addItem(withTitle: "Quit geniewish", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        applicationMenu.addItem(withTitle: "Open troubleshooting logs", action: #selector(openLogs), keyEquivalent: "l")
        let menuItem = NSMenuItem()
        menuItem.submenu = applicationMenu
        menu.addItem(menuItem)
        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        let editItem = NSMenuItem()
        editItem.submenu = edit
        menu.addItem(editItem)
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 920, height: 620),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "geniewish · voice preview"
        window.minSize = NSSize(width: 680, height: 440)
        window.delegate = self
        window.isReleasedWhenClosed = false
        title.font = .systemFont(ofSize: 22, weight: .semibold)
        status.textColor = .secondaryLabelColor
        partial.textColor = .secondaryLabelColor
        status.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        partial.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let start = NSButton(title: "Start listening", target: self, action: #selector(startListening))
        let pause = NSButton(title: "Pause listening", target: self, action: #selector(pauseListening))
        let stop = NSButton(title: "Stop speaking", target: self, action: #selector(stopSpeaking))
        let controls = NSStackView(views: [start, pause, stop])
        controls.spacing = 8
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        log.isEditable = false
        log.isSelectable = true
        log.font = .systemFont(ofSize: 15)
        log.textColor = .labelColor
        log.backgroundColor = .textBackgroundColor
        log.textContainerInset = NSSize(width: 14, height: 14)
        log.isHorizontallyResizable = false
        log.isVerticallyResizable = true
        log.autoresizingMask = [.width]
        log.textContainer?.widthTracksTextView = true
        scroll.documentView = log
        textInput.placeholderString = "Keyboard fallback: Mike, hire a new genie"
        textInput.target = self
        textInput.action = #selector(typed)
        let conversation = NSStackView(views: [title, status, controls, scroll, partial, textInput])
        conversation.orientation = .vertical
        conversation.alignment = .leading
        conversation.spacing = 12
        roster.orientation = .vertical
        roster.alignment = .leading
        roster.spacing = 10
        roster.addArrangedSubview(NSTextField(labelWithString: "Your genies"))
        let layout = NSStackView(views: [roster, conversation])
        layout.alignment = .top
        layout.spacing = 22
        let container = window.contentView!
        container.addSubview(layout)
        layout.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            layout.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 20),
            layout.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -20),
            layout.topAnchor.constraint(equalTo: container.topAnchor, constant: 20),
            layout.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -20),
            roster.widthAnchor.constraint(equalToConstant: 180),
            conversation.bottomAnchor.constraint(equalTo: layout.bottomAnchor),
            scroll.widthAnchor.constraint(equalTo: conversation.widthAnchor),
            scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 160),
            textInput.widthAnchor.constraint(equalTo: conversation.widthAnchor),
            status.widthAnchor.constraint(equalTo: conversation.widthAnchor),
            partial.widthAnchor.constraint(equalTo: conversation.widthAnchor)
        ])
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func connect() {
        let corePath = argument("--core") ?? Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/geniewish-core").path
        let savedWorkspace = Bundle.main.url(forResource: "workspace", withExtension: "txt").flatMap { try? String(contentsOf: $0, encoding: .utf8) }.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        let workspace = argument("--workspace") ?? savedWorkspace ?? FileManager.default.currentDirectoryPath
        let bundledCodex = "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
        let codex = argument("--codex") ?? (FileManager.default.isExecutableFile(atPath: bundledCodex) ? bundledCodex : "codex")
        core.executableURL = URL(fileURLWithPath: corePath)
        core.currentDirectoryURL = URL(fileURLWithPath: workspace)
        core.arguments = ["--live", "--ack-delivery", "--workspace", workspace, "--codex", codex]
        for option in ["--router-model", "--hr-model", "--worker-model", "--output-dir"] {
            if let value = argument(option) { core.arguments! += [option, value] }
        }
        if let state = argument("--state-dir") { core.arguments! += ["--state-dir", state] }
        if let logs = Diagnostics.shared?.directory.path { core.arguments! += ["--log-dir", logs] }
        if CommandLine.arguments.contains("--debug-transcripts") { core.arguments!.append("--debug-transcripts") }
        let output = Pipe()
        let errors = Pipe()
        core.standardInput = input
        core.standardOutput = output
        core.standardError = errors
        core.terminationHandler = { ended in
            Diagnostics.shared?.event("core.exit", ["status": ended.terminationStatus, "reason": ended.terminationReason.rawValue])
            DispatchQueue.main.async {
                self.ready = false
                self.microphone.stop()
                self.speaker.stop()
                if self.closing { NSApp.reply(toApplicationShouldTerminate: true) }
                else {
                    self.status.stringValue = "Work runtime stopped · restart geniewish to reconnect"
                    if ended.terminationStatus != 0 { self.append("geniewish", self.stderrText.isEmpty ? "Runtime connection failed." : self.stderrText) }
                }
            }
        }
        do {
            try core.run()
            // Read complete JSON lines off the UI thread; no partial events are processed.
            DispatchQueue.global().async {
                var buffer = Data()
                while true {
                    let data = output.fileHandleForReading.availableData
                    if data.isEmpty { break }
                    buffer.append(data)
                    while let end = buffer.firstIndex(of: 10) {
                        let line = buffer.prefix(upTo: end)
                        buffer.removeSubrange(...end)
                        if let event = try? JSONSerialization.jsonObject(with: line) as? [String: Any] {
                            DispatchQueue.main.async { self.event(event) }
                        } else {
                            Diagnostics.shared?.event("core.invalid_event", ["bytes": line.count])
                        }
                    }
                }
            }
            DispatchQueue.global().async {
                while true {
                    let data = errors.fileHandleForReading.availableData
                    if data.isEmpty { break }
                    let text = String(decoding: data, as: UTF8.self)
                    Diagnostics.shared?.text("core.stderr", text)
                    DispatchQueue.main.async { self.stderrText = String((self.stderrText + text).suffix(4000)) }
                }
            }
        } catch {
            Diagnostics.shared?.error("core.launch_error", error)
            append("geniewish", "Could not launch the Rust core: \(error.localizedDescription)")
        }
    }

    private func event(_ event: [String: Any]) {
        Diagnostics.shared?.event("core.event", ["type":event["type"] ?? "unknown", "genie":event["genie"] ?? NSNull(), "method":event["method"] ?? NSNull()])
        switch event["type"] as? String {
        case "ready":
            ready = true
            let text = event["text"] as? String ?? "Your team is ready."
            append("geniewish", text)
            speaker.enqueue(["text": text, "voice": "Daniel"])
        case "team":
            active = event["active"] as? String
            title.stringValue = active.map { "Talking to \($0)" } ?? "Name your HR manager"
            for view in roster.arrangedSubviews { roster.removeArrangedSubview(view); view.removeFromSuperview() }
            roster.addArrangedSubview(NSTextField(labelWithString: "Your genies"))
            for genie in event["genies"] as? [[String: Any]] ?? [] {
                let name = genie["name"] as? String ?? ""
                let role = genie["role"] as? String ?? ""
                let button = NSButton(title: name, target: self, action: #selector(chooseGenie(_:)))
                button.toolTip = role.isEmpty ? "Role not assigned" : role
                roster.addArrangedSubview(button)
                if !role.isEmpty { let label = NSTextField(wrappingLabelWithString: role); label.textColor = .secondaryLabelColor; roster.addArrangedSubview(label) }
            }
        case "reply":
            append(event["genie"] as? String ?? "geniewish", event["text"] as? String ?? "")
            speaker.enqueue(event)
        case "working": break
        case "update":
            append("geniewish", "New message from \(event["genie"] as? String ?? "a genie"). Say 'Mike, any messages?' to check.")
            chime()
        case "approval":
            let genie = event["genie"] as? String ?? "Genie"
            let reason = event["reason"] as? String ?? event["command"] as? String ?? "a tool action"
            let text = "\(genie) needs approval for \(reason). Say '\(genie), approve once' or '\(genie), deny'."
            append("geniewish", text)
            if event["background"] as? Bool == true { chime() }
            else { speaker.enqueue(["text": text, "voice": "Daniel"]) }
        case "completed":
            if let error = event["error"], !(error is NSNull) { append("geniewish", "Work failed: \(error)") }
        case "error":
            let text = event["text"] as? String ?? "A command failed."
            append("geniewish", text)
            speaker.enqueue(["text": text, "voice": "Daniel"])
        case "stop_speech": speaker.stop()
        case "open_logs": openLogs()
        case "quit": NSApp.terminate(nil)
        case "unsupported_request": append("geniewish", "This runtime question is not supported yet.")
        default: break
        }
    }

    private func chime() {
        guard Date().timeIntervalSince(chimeAt) >= 3 else { return }
        chimeAt = Date()
        let sound = NSSound(named: "Tink")
        sound?.volume = 0.15
        sound?.play()
    }

    private func append(_ owner: String, _ text: String) {
        log.textStorage?.append(NSAttributedString(string: "\(owner)\n\(text)\n\n", attributes: [.font: NSFont.systemFont(ofSize: 15), .foregroundColor: NSColor.labelColor]))
        log.scrollToEndOfDocument(nil)
    }

    private func send(_ text: String) {
        guard core.isRunning else { return }
        do { try input.fileHandleForWriting.write(contentsOf: Data((text + "\n").utf8)) }
        catch { append("geniewish", "Could not send: \(error.localizedDescription)") }
    }

    private func sendJSON(_ value: [String: Any]) {
        if let data = try? JSONSerialization.data(withJSONObject: value), let text = String(data: data, encoding: .utf8) { send(text) }
    }

    private func sendHuman(_ text: String, source: String = "text") {
        Diagnostics.shared?.text("input", text, fields: ["source":source,"active":active ?? "none"])
        guard ready else { append("geniewish", "Wait for your team to connect before sending a command."); return }
        speaker.stop()
        append("Human", text)
        sendJSON(["type": "input", "text": text, "source":source])
    }

    @objc private func startListening() { if ready { microphone.start() } }
    @objc private func pauseListening() { microphone.stop() }
    @objc private func stopSpeaking() { speaker.stop() }
    @objc private func openLogs() { if let directory = Diagnostics.shared?.directory { NSWorkspace.shared.open(directory) } }
    @objc private func typed() {
        let text = textInput.stringValue
        textInput.stringValue = ""
        if !text.trimmingCharacters(in: .whitespaces).isEmpty { sendHuman(text) }
    }
    @objc private func chooseGenie(_ button: NSButton) { sendHuman("Talk to \(button.title)") }
    func windowWillClose(_ notification: Notification) { NSApp.terminate(nil) }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        microphone.stop()
        speaker.stop()
        guard core.isRunning else { return .terminateNow }
        closing = true
        send("quit")
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
            if self.core.isRunning { self.core.terminate() }
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

let delegate = Application()
NSApplication.shared.delegate = delegate
NSApplication.shared.run()
