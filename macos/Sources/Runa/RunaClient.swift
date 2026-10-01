import Foundation

enum RunaClientError: LocalizedError {
    case failed(command: String, message: String)

    var errorDescription: String? {
        switch self {
        case let .failed(command, message):
            return message.isEmpty ? "`runa \(command)` failed" : message
        }
    }
}

/// Talks to runa through its CLI, so the app never drifts from what
/// `runa status` and friends report.
struct RunaClient: Sendable {
    let executable: URL
    /// Environment for runa: the login shell's PATH, so processes started
    /// from the app find the same tools as in a terminal.
    let environment: [String: String]

    static let pathDefaultsKey = "runaPath"

    /// Finds runa: the user's choice, common install locations, then PATH.
    static func locate(environment: [String: String]) -> URL? {
        let fm = FileManager.default
        var candidates: [String] = []
        if let chosen = UserDefaults.standard.string(forKey: pathDefaultsKey) {
            candidates.append(chosen)
        }
        let home = fm.homeDirectoryForCurrentUser.path
        candidates += ["/opt/homebrew/bin/runa", "/usr/local/bin/runa", "\(home)/.cargo/bin/runa"]
        candidates += (environment["PATH"] ?? "").split(separator: ":").map { "\($0)/runa" }
        return candidates.first { fm.isExecutableFile(atPath: $0) }.map(URL.init(fileURLWithPath:))
    }

    /// The environment of a login shell; GUI apps start with a bare PATH.
    static func loginEnvironment() async -> [String: String] {
        var env = ProcessInfo.processInfo.environment
        let shell = env["SHELL"] ?? "/bin/zsh"
        if let data = try? await run(URL(fileURLWithPath: shell), ["-lc", "printf %s \"$PATH\""], env: env),
           let path = String(data: data, encoding: .utf8), !path.isEmpty {
            env["PATH"] = path
        }
        return env
    }

    func status() async throws -> [ProcessRow] {
        try decode([ProcessRow].self, from: await run(["status", "--json"]))
    }

    /// Dev ports only, unless `all`.
    func ports(all: Bool = false) async throws -> [PortRow] {
        try decode([PortRow].self, from: await run(["ports", "--json"] + (all ? ["--all"] : [])))
    }

    func metrics(_ name: String, since seconds: Int = 3600) async throws -> MetricsReport {
        try decode(MetricsReport.self, from: await run(["metrics", name, "--json", "--since", String(seconds)]))
    }

    func logs(_ name: String, lines: Int = 40) async throws -> String {
        String(decoding: try await run(["logs", name, "-n", String(lines)]), as: UTF8.self)
    }

    func restart(_ name: String) async throws { _ = try await run(["restart", name]) }
    func stop(_ name: String) async throws { _ = try await run(["stop", name]) }
    func stopAll() async throws { _ = try await run(["stop", "--all"]) }
    func resurrect() async throws { _ = try await run(["resurrect"]) }

    func run(_ args: [String]) async throws -> Data {
        try await Self.run(executable, args, env: environment)
    }

    private func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(type, from: data)
    }

    private static func run(_ executable: URL, _ args: [String], env: [String: String]) async throws -> Data {
        try await Task.detached {
            let process = Process()
            process.executableURL = executable
            process.arguments = args
            process.environment = env
            process.standardInput = FileHandle.nullDevice
            let stdout = Pipe()
            let stderr = Pipe()
            process.standardOutput = stdout
            process.standardError = stderr
            try process.run()
            // Read before waiting so a full pipe cannot block the child.
            let errTask = Task.detached { stderr.fileHandleForReading.readDataToEndOfFile() }
            let out = stdout.fileHandleForReading.readDataToEndOfFile()
            let err = await errTask.value
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                let message = String(decoding: err, as: UTF8.self)
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .replacingOccurrences(of: "Error: ", with: "")
                throw RunaClientError.failed(command: args.joined(separator: " "), message: message)
            }
            return out
        }.value
    }
}
