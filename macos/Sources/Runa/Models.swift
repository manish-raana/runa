import Foundation

/// One row of `runa status --json`.
struct ProcessRow: Decodable, Identifiable, Hashable {
    var id: String { name }
    let name: String
    let running: Bool
    /// "running", "failed" or "dead".
    let status: String
    let pid: Int32
    let childPid: Int32?
    let cmd: String
    let cwd: String?
    let project: Workspace?
    let ports: [UInt16]
    let restarts: UInt64
    let lastExitCode: Int32?
    let lastError: String?
    let cpu: Double?
    let rssKb: UInt64?
    let startedAt: String

    var startedDate: Date? { ISO8601DateFormatter().date(from: startedAt) }
}

/// One row of `runa ports --json`.
struct PortRow: Decodable, Identifiable, Hashable {
    var id: String { "\(`protocol`)-\(address)-\(port)-\(pid)" }
    let `protocol`: String
    let address: String
    let port: UInt16
    let pid: Int32
    let command: String
    let user: String
    /// The executable path, or a renamed process title.
    let exe: String?
    /// "runa", "dev", "ephemeral", "app" or "system".
    let kind: String
    let cwd: String?
    let project: Workspace?
    /// The runa process that owns the socket, if any.
    let runa: String?
    let conflict: Bool

    var isDev: Bool { kind == "runa" || kind == "dev" }

    /// A readable name: lsof truncates and escapes its COMMAND column.
    var displayName: String {
        guard let exe, !exe.isEmpty else { return command }
        guard exe.hasPrefix("/") else { return exe }
        // Name an app helper after its app, e.g. "Visual Studio Code".
        if let app = exe.components(separatedBy: "/").first(where: { $0.hasSuffix(".app") }) {
            return String(app.dropLast(4))
        }
        return URL(fileURLWithPath: exe).lastPathComponent
    }
}

/// The project a process runs in, detected from its working directory.
struct Workspace: Decodable, Hashable {
    let name: String
    /// The monorepo package, e.g. "web".
    let sub: String?
    let root: String
    let branch: String?

    var label: String { sub.map { "\(name) › \($0)" } ?? name }
}

/// `runa metrics <name> --json`.
struct MetricsReport: Decodable {
    let name: String
    let intervalSecs: UInt64
    let samples: [Sample]
    let events: [Event]

    struct Sample: Decodable, Identifiable, Hashable {
        var id: UInt64 { t }
        let t: UInt64
        let cpu: Double
        let rssKb: UInt64
        let procs: UInt32

        var date: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
        var rssMB: Double { Double(rssKb) / 1024 }
    }

    struct Event: Decodable, Identifiable, Hashable {
        var id: String { "\(t)-\(event)-\(pid ?? 0)" }
        let t: UInt64
        /// "start", "exit", "restart" or "stop".
        let event: String
        let pid: Int32?
        let code: Int32?

        var date: Date { Date(timeIntervalSince1970: TimeInterval(t)) }
    }
}

enum Format {
    static func memory(kb: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .memory
        return formatter.string(fromByteCount: Int64(kb) * 1024)
    }

    static func cpu(_ percent: Double) -> String {
        percent < 10 ? String(format: "%.1f%%", percent) : String(format: "%.0f%%", percent)
    }

    static func uptime(since date: Date) -> String {
        let formatter = DateComponentsFormatter()
        formatter.allowedUnits = [.day, .hour, .minute, .second]
        formatter.unitsStyle = .abbreviated
        formatter.maximumUnitCount = 2
        return formatter.string(from: date, to: .now) ?? ""
    }
}
