import AppKit
import SwiftUI

struct MenuContent: View {
    @Environment(Store.self) private var store
    @State private var selected: String?
    /// Reports the content's natural size so the panel can match it.
    let onSize: (CGSize) -> Void

    var body: some View {
        VStack(spacing: 0) {
            if store.client == nil {
                SetupView()
            } else if let name = selected {
                ProcessDetailView(name: name) { selected = nil }
            } else {
                OverviewView { selected = $0 }
            }
        }
        .modifier(PanelChrome())
        .onGeometryChange(for: CGSize.self) { $0.size } action: { onSize($0) }
    }
}

struct OverviewView: View {
    @Environment(Store.self) private var store
    let onSelect: (String) -> Void

    var body: some View {
        VStack(spacing: 0) {
            header
            if let error = store.error {
                ErrorBanner(message: error) { store.error = nil }
            }
            FittingScrollView(maxHeight: 480) {
                VStack(alignment: .leading, spacing: 0) {
                    processes
                    listeners
                }
                .padding(.bottom, 12)
            }
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            AppLogo(size: 28)
            VStack(alignment: .leading, spacing: 1) {
                Text("Runa").font(.system(size: 13, weight: .semibold))
                Text(summary)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 4)
            HealthPill(trouble: store.troubleCount, running: store.runningCount)
            IconButton(symbol: "arrow.clockwise", help: "Refresh", spinning: store.isRefreshing) {
                Task { await store.refresh() }
            }
            MoreMenu()
        }
        .padding(.horizontal, 14)
        .padding(.top, 14)
        .padding(.bottom, 6)
    }

    private var summary: String {
        var parts = ["\(store.runningCount) running"]
        if store.troubleCount > 0 { parts.append("\(store.troubleCount) stopped") }
        let servers = store.otherListeners.count
        if servers > 0 {
            parts.append(servers == 1 ? "1 dev server" : "\(servers) dev servers")
        }
        return parts.joined(separator: " · ")
    }

    @ViewBuilder
    private var processes: some View {
        SectionHeader(title: "Processes", count: store.processes.count)
        Group {
            if store.processes.isEmpty {
                EmptyProcesses()
            } else {
                VStack(spacing: 0) {
                    ForEach(store.processes) { process in
                        ProcessRowView(process: process) { onSelect(process.name) }
                    }
                }
            }
        }
        .card()
    }

    @ViewBuilder
    private var listeners: some View {
        @Bindable var store = store
        SectionHeader(title: store.showAllPorts ? "All ports" : "Dev servers", count: store.otherListeners.count) {
            Toggle("Show all", isOn: $store.showAllPorts)
                .toggleStyle(.switch)
                .controlSize(.mini)
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
                .help("Also show system services, app helpers and random high ports")
        }
        Group {
            if store.otherListeners.isEmpty {
                HStack(spacing: 6) {
                    Image(systemName: "antenna.radiowaves.left.and.right.slash")
                    Text("No other servers listening")
                }
                .font(.system(size: 11.5))
                .foregroundStyle(.tertiary)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 10)
            } else {
                VStack(spacing: 0) {
                    ForEach(store.otherListeners) { ListenerRowView(port: $0) }
                }
            }
        }
        .card()
    }
}

struct MoreMenu: View {
    @Environment(Store.self) private var store

    var body: some View {
        @Bindable var store = store
        Menu {
            Toggle("Show All Ports", isOn: $store.showAllPorts)
            Divider()
            Button("Stop All Processes") { store.stopAll() }
                .disabled(store.runningCount == 0)
            Button("Resurrect Saved Processes") { store.resurrect() }
            Divider()
            Button("Choose runa Binary…") { chooseRunaBinary(store) }
            Divider()
            Button("Quit Runa") { NSApp.terminate(nil) }
                .keyboardShortcut("q")
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 11, weight: .semibold))
                .frame(width: 24, height: 24)
                .contentShape(Circle())
        }
        .menuStyle(.button)
        .buttonStyle(HoverButtonStyle())
        .menuIndicator(.hidden)
        .fixedSize()
        .help("More")
    }
}

/// "All good" at a glance, or how many processes need a look.
struct HealthPill: View {
    let trouble: Int
    let running: Int

    var body: some View {
        if trouble > 0 || running > 0 {
            let color = trouble > 0 ? Brand.failed : Brand.running
            HStack(spacing: 5) {
                Circle().fill(color).frame(width: 6, height: 6)
                Text(trouble > 0 ? "\(trouble) down" : "Healthy")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(trouble > 0 ? color : .secondary)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(Color.primary.opacity(0.06), in: Capsule())
        }
    }
}

struct ErrorBanner: View {
    let message: String
    let onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(Brand.failed)
            Text(message)
                .font(.system(size: 11))
                .lineLimit(3)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button(action: onDismiss) {
                Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(8)
        .background(Brand.failed.opacity(0.12), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.horizontal, 10)
        .padding(.top, 4)
    }
}

struct EmptyProcesses: View {
    private let example = #"runa run --name web --cmd "npm run dev" -d"#
    @State private var copied = false

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "waveform.path.ecg")
                .font(.system(size: 20, weight: .regular))
                .foregroundStyle(.tertiary)
            VStack(spacing: 2) {
                Text("No processes yet")
                    .font(.system(size: 12.5, weight: .semibold))
                Text("Start one from a terminal and it shows up here.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(example, forType: .string)
                copied = true
            } label: {
                HStack(spacing: 6) {
                    Text(example).font(.system(size: 10.5, design: .monospaced))
                    Image(systemName: copied ? "checkmark" : "doc.on.doc").font(.system(size: 9))
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 5)
                .background(Color.primary.opacity(0.07), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .help("Copy")
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 14)
    }
}

struct ProcessRowView: View {
    @Environment(Store.self) private var store
    let process: ProcessRow
    let onSelect: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            StatusDot(status: process.status)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 5) {
                    Text(process.name)
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(1)
                    ForEach(process.ports, id: \.self) { PortChip(port: $0) }
                }
                HStack(spacing: 4) {
                    if !process.running {
                        Image(systemName: "exclamationmark.circle.fill")
                            .foregroundStyle(Brand.failed)
                    }
                    // The project says more than a truncated command; the
                    // command stays in the tooltip and the detail view.
                    if process.running, let project = process.project {
                        ProjectTag(project: project)
                    } else {
                        Text(subtitle)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
                .font(.system(size: 11))
                .help(process.cmd)
            }
            Spacer(minLength: 6)
            ZStack(alignment: .trailing) {
                usage.opacity(hovering ? 0 : 1)
                actions.opacity(hovering ? 1 : 0)
            }
        }
        .rowBackground(hovering)
        .onHover { hovering = $0 }
        .onTapGesture(perform: onSelect)
        .contextMenu {
            ForEach(process.ports, id: \.self) { port in
                Button("Open localhost:\(String(port))") { store.open(port: port) }
            }
            if let root = process.project?.root ?? process.cwd {
                Button("Reveal Project in Finder") { Finder.reveal(root) }
            }
            Button("Copy Command") { Finder.copy(process.cmd) }
            Divider()
            if process.running {
                Button("Restart") { store.restart(process.name) }
                Button("Stop") { store.stop(process.name) }
            } else {
                Button("Remove") { store.stop(process.name) }
            }
        }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }

    private var subtitle: String {
        if !process.running {
            if let error = process.lastError { return error }
            if let code = process.lastExitCode { return "Exited with code \(code)" }
            return "Supervisor is gone"
        }
        if process.restarts > 0 {
            return "↻ \(process.restarts) · \(process.cmd)"
        }
        return process.cmd
    }

    private var usage: some View {
        VStack(alignment: .trailing, spacing: 2) {
            Text(process.cpu.map(Format.cpu) ?? (process.running ? "–" : ""))
                .font(.system(size: 11, weight: .medium).monospacedDigit())
            Text(process.rssKb.map { Format.memory(kb: $0) } ?? "")
                .font(.system(size: 10.5).monospacedDigit())
                .foregroundStyle(.secondary)
        }
    }

    private var actions: some View {
        HStack(spacing: 0) {
            if process.running {
                IconButton(symbol: "arrow.clockwise", help: "Restart") { store.restart(process.name) }
                IconButton(symbol: "stop.fill", help: "Stop", tint: Brand.failed) { store.stop(process.name) }
            } else {
                IconButton(symbol: "trash", help: "Remove", tint: Brand.failed) { store.stop(process.name) }
            }
            Image(systemName: "chevron.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.tertiary)
                .padding(.leading, 2)
        }
    }
}

struct ListenerRowView: View {
    @Environment(Store.self) private var store
    let port: PortRow
    @State private var hovering = false
    @State private var confirmKill = false

    var body: some View {
        HStack(spacing: 8) {
            PortChip(port: port.port)
                .frame(width: 58, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 5) {
                    Text(port.displayName)
                        .font(.system(size: 12))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .foregroundStyle(port.isDev ? .primary : .secondary)
                    if !port.isDev {
                        Text(port.kind)
                            .font(.system(size: 9, weight: .medium))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(.quaternary, in: Capsule())
                    }
                    if port.conflict {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 10))
                            .foregroundStyle(.orange)
                            .help("More than one process listens on this port")
                    }
                }
                .help(port.exe ?? port.command)
                if let project = port.project {
                    ProjectTag(project: project)
                }
            }
            Spacer(minLength: 6)
            ZStack(alignment: .trailing) {
                Text(verbatim: "PID \(port.pid)")
                    .font(.system(size: 10.5).monospacedDigit())
                    .foregroundStyle(.tertiary)
                    .opacity(hovering || confirmKill ? 0 : 1)
                killControl.opacity(hovering || confirmKill ? 1 : 0)
            }
        }
        .rowBackground(hovering)
        .contextMenu {
            Button("Open localhost:\(String(port.port))") { store.open(port: port.port) }
            if let root = port.project?.root ?? port.cwd {
                Button("Reveal Project in Finder") { Finder.reveal(root) }
            }
            Button("Copy PID") { Finder.copy(String(port.pid)) }
            Divider()
            Button("Stop Process") { store.kill(pid: port.pid) }
        }
        .onHover { inside in
            hovering = inside
            if !inside { confirmKill = false }
        }
        .animation(.easeOut(duration: 0.12), value: hovering)
    }

    @ViewBuilder
    private var killControl: some View {
        if confirmKill {
            Button {
                store.kill(pid: port.pid)
                confirmKill = false
            } label: {
                Text("Stop?")
                    .font(.system(size: 10.5, weight: .semibold))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(Brand.failed, in: Capsule())
                    .foregroundStyle(.white)
            }
            .buttonStyle(.plain)
        } else {
            IconButton(symbol: "xmark", help: "Stop this process (SIGTERM)", tint: Brand.failed) { confirmKill = true }
        }
    }
}

struct SetupView: View {
    @Environment(Store.self) private var store

    var body: some View {
        VStack(spacing: 10) {
            AppLogo(size: 48)
            Text("runa isn't installed").font(.system(size: 13, weight: .semibold))
            Text("Install it with `cargo install runa-cli` or Homebrew, or choose the binary yourself.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Button("Choose runa…") { chooseRunaBinary(store) }
                    .buttonStyle(.borderedProminent)
                Button("Retry") { store.connect() }
                Button("Quit") { NSApp.terminate(nil) }
            }
            .controlSize(.small)
        }
        .padding(24)
    }
}

@MainActor
func chooseRunaBinary(_ store: Store) {
    let panel = NSOpenPanel()
    panel.canChooseDirectories = false
    panel.allowsMultipleSelection = false
    panel.message = "Choose the runa executable"
    NSApp.activate(ignoringOtherApps: true)
    if panel.runModal() == .OK, let url = panel.url {
        store.choose(runaPath: url.path)
    }
}
