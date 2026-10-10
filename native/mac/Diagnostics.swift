import Foundation

final class Diagnostics {
    static var shared: Diagnostics?
    private let path: URL
    private var file: FileHandle
    private var bytes = 0
    private let lock = NSLock()
    let transcripts: Bool
    let directory: URL

    init(directory: URL, transcripts: Bool) throws {
        self.directory = directory
        self.transcripts = transcripts
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let stamp = Int64(Date().timeIntervalSince1970 * 1000)
        path = directory.appendingPathComponent("mac-\(stamp)-\(ProcessInfo.processInfo.processIdentifier).jsonl")
        guard FileManager.default.createFile(atPath: path.path, contents: nil, attributes: [.posixPermissions: 0o600]) else {
            throw NSError(domain: "geniewish.Diagnostics", code: 1, userInfo: [NSLocalizedDescriptionKey: "Could not create a troubleshooting log."])
        }
        file = try FileHandle(forWritingTo: path)
        let old = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.lastPathComponent.hasPrefix("mac-") && $0.pathExtension == "jsonl" }.sorted { $0.path < $1.path }
        for old in old.prefix(max(0, old.count - 10)) {
            for suffix in ["", ".1", ".2"] { try? FileManager.default.removeItem(atPath: old.path + suffix) }
        }
    }

    func event(_ event: String, _ fields: [String: Any] = [:]) {
        lock.lock()
        defer { lock.unlock() }
        do {
            let record: [String: Any] = ["time_ms": Int64(Date().timeIntervalSince1970 * 1000), "pid": ProcessInfo.processInfo.processIdentifier, "component": "mac", "event": event, "fields": fields]
            var data = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
            data.append(10)
            if bytes > 0 && bytes + data.count > 1_048_576 {
                try file.close()
                try? FileManager.default.removeItem(atPath: path.path + ".2")
                if FileManager.default.fileExists(atPath: path.path + ".1") { try FileManager.default.moveItem(atPath: path.path + ".1", toPath: path.path + ".2") }
                try FileManager.default.moveItem(atPath: path.path, toPath: path.path + ".1")
                guard FileManager.default.createFile(atPath: path.path, contents: nil, attributes: [.posixPermissions: 0o600]) else { return }
                file = try FileHandle(forWritingTo: path)
                bytes = 0
            }
            try file.write(contentsOf: data)
            bytes += data.count
        } catch { fputs("geniewish troubleshooting log could not be written.\n", stderr) }
    }

    func text(_ event: String, _ text: String, fields: [String: Any] = [:]) {
        var fields = fields
        fields["chars"] = text.count
        fields["transcript_logged"] = transcripts
        if transcripts { fields["text"] = String(text.prefix(4096)) }
        self.event(event, fields)
    }

    func error(_ event: String, _ error: Error) {
        let error = error as NSError
        self.event(event, ["domain": error.domain, "code": error.code])
        if transcripts { text(event + ".detail", error.localizedDescription) }
    }
}
