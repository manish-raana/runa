import AppKit
import Foundation
import Observation

@MainActor
@Observable
final class Store {
    private(set) var client: RunaClient?
    private(set) var processes: [ProcessRow] = []
    private(set) var ports: [PortRow] = []
    private(set) var lastRefresh: Date?
    private(set) var isRefreshing = false
    var error: String?
    /// Also list system services, app helpers and random high ports.
    var showAllPorts = UserDefaults.standard.bool(forKey: "showAllPorts") {
        didSet {
            UserDefaults.standard.set(showAllPorts, forKey: "showAllPorts")
            Task { await refresh() }
        }
    }
    /// Poll faster while the menu is on screen.
    var isMenuOpen = false {
        didSet { if isMenuOpen { Task { await refresh() } } }
    }

    private var environment: [String: String] = ProcessInfo.processInfo.environment
    private var loop: Task<Void, Never>?

    init() {
        Task { await setUp() }
    }

    func setUp() async {
        environment = await RunaClient.loginEnvironment()
        connect()
    }

    /// (Re)locates the runa binary and starts polling.
    func connect() {
        loop?.cancel()
        guard let url = RunaClient.locate(environment: environment) else {
            client = nil
            return
        }
        client = RunaClient(executable: url, environment: environment)
        loop = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                let open = self?.isMenuOpen ?? false
                try? await Task.sleep(for: open ? .seconds(3) : .seconds(15))
            }
        }
    }

    func choose(runaPath path: String) {
        UserDefaults.standard.set(path, forKey: RunaClient.pathDefaultsKey)
        connect()
    }

    func refresh() async {
        guard let client else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        // Fetched separately so one failing does not hide the other.
        async let status = Result { try await client.status() }
        async let ports = Result { try await client.ports(all: showAllPorts) }
        var failures: [String] = []
        switch await status {
        case let .success(rows): processes = rows
        case let .failure(error): failures.append(error.localizedDescription)
        }
        switch await ports {
        case let .success(rows): self.ports = rows
        case let .failure(error): failures.append(error.localizedDescription)
        }
        lastRefresh = .now
        error = failures.isEmpty ? nil : failures.joined(separator: "\n")
    }

    // MARK: Derived

    var runningCount: Int { processes.filter(\.running).count }
    var troubleCount: Int { processes.filter { !$0.running }.count }

    /// Listening TCP ports of runa processes and dev servers; system and app
    /// ports never count, even with "Show all" on.
    var devPortCount: Int {
        Set(ports.filter { $0.isDev && $0.protocol == "TCP" }.map(\.port)).count
    }

    /// TCP listeners runa does not manage, one row per port and process.
    /// `runa ports` already leaves out non-dev ports unless asked.
    var otherListeners: [PortRow] {
        var seen = Set<String>()
        return ports
            .filter { $0.runa == nil && $0.protocol == "TCP" }
            .filter { seen.insert("\($0.port)-\($0.pid)").inserted }
    }

    // MARK: Actions

    func restart(_ name: String) { perform { try await $0.restart(name) } }
    func stop(_ name: String) { perform { try await $0.stop(name) } }
    func stopAll() { perform { try await $0.stopAll() } }
    func resurrect() { perform { try await $0.resurrect() } }

    /// Sends SIGTERM to a process runa does not manage.
    func kill(pid: Int32) {
        if Darwin.kill(pid, SIGTERM) != 0 {
            error = "Could not stop PID \(pid): \(String(cString: strerror(errno)))"
        }
        Task {
            try? await Task.sleep(for: .milliseconds(500))
            await refresh()
        }
    }

    func open(port: UInt16) {
        if let url = URL(string: "http://localhost:\(port)") {
            NSWorkspace.shared.open(url)
        }
    }

    private func perform(_ action: @escaping (RunaClient) async throws -> Void) {
        guard let client else { return }
        Task {
            do {
                try await action(client)
                error = nil
            } catch {
                self.error = error.localizedDescription
            }
            // Give the supervisor a moment to update its state file.
            try? await Task.sleep(for: .milliseconds(600))
            await refresh()
        }
    }
}

extension Result where Failure == Error {
    init(_ body: () async throws -> Success) async {
        do { self = .success(try await body()) } catch { self = .failure(error) }
    }
}
