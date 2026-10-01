import Charts
import SwiftUI

struct ProcessDetailView: View {
    @Environment(Store.self) private var store
    let name: String
    let onBack: () -> Void

    @State private var metrics: MetricsReport?
    @State private var logs = ""
    @State private var loadError: String?
    @State private var chart: ChartKind = .cpu

    enum ChartKind: String, CaseIterable {
        case cpu = "CPU"
        case memory = "Memory"
    }

    private var process: ProcessRow? { store.processes.first { $0.name == name } }
    private var samples: [MetricsReport.Sample] { metrics?.samples ?? [] }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().opacity(0.5)
            FittingScrollView(maxHeight: 500) {
                VStack(alignment: .leading, spacing: 14) {
                    if let process {
                        tiles(process)
                        chartSection
                        info(process)
                    }
                    events
                    logsSection
                }
                .padding(14)
            }
        }
        .task(id: name) {
            while !Task.isCancelled {
                await load()
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            IconButton(symbol: "chevron.left", help: "Back", action: onBack)
            if let process { StatusDot(status: process.status) }
            VStack(alignment: .leading, spacing: 1) {
                Text(name)
                    .font(.system(size: 13, weight: .semibold))
                    .lineLimit(1)
                Text(process?.status.capitalized ?? "Gone")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if let process {
                if process.running {
                    Button {
                        store.restart(name)
                    } label: {
                        Label("Restart", systemImage: "arrow.clockwise")
                    }
                    Button(role: .destructive) {
                        store.stop(name)
                    } label: {
                        Label("Stop", systemImage: "stop.fill")
                    }
                    .tint(Brand.failed)
                } else {
                    Button(role: .destructive) {
                        store.stop(name)
                        onBack()
                    } label: {
                        Label("Remove", systemImage: "trash")
                    }
                    .tint(Brand.failed)
                }
            }
        }
        .glassButtonStyle()
        .controlSize(.small)
        .labelStyle(.titleAndIcon)
        .padding(.leading, 8)
        .padding(.trailing, 14)
        .padding(.top, 12)
        .padding(.bottom, 8)
    }

    // MARK: Stats

    private func tiles(_ process: ProcessRow) -> some View {
        HStack(spacing: 8) {
            StatTile(title: "CPU", value: process.cpu.map(Format.cpu) ?? "–",
                     detail: peak(\.cpu).map { "peak \(Format.cpu($0))" })
            StatTile(title: "Memory", value: process.rssKb.map { Format.memory(kb: $0) } ?? "–",
                     detail: peak { Double($0.rssKb) }.map { "peak \(Format.memory(kb: UInt64($0)))" })
            StatTile(title: "Uptime",
                     value: process.running ? process.startedDate.map { Format.uptime(since: $0) } ?? "–" : "–",
                     detail: process.restarts == 1 ? "1 restart" : "\(process.restarts) restarts")
        }
    }

    private func peak(_ value: (MetricsReport.Sample) -> Double) -> Double? {
        samples.map(value).max()
    }

    @ViewBuilder
    private var chartSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Last hour")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Picker("", selection: $chart) {
                    ForEach(ChartKind.allCases, id: \.self) { Text($0.rawValue) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .controlSize(.small)
                .fixedSize()
            }
            if samples.count >= 2 {
                usageChart.frame(height: 90)
            } else {
                Text(process?.running == true ? "Collecting samples… the first one arrives within 10 seconds." : "No samples.")
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, minHeight: 60)
            }
        }
        .padding(10)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    private var usageChart: some View {
        let color: Color = chart == .cpu ? Brand.cpu : Brand.memory
        let value: (MetricsReport.Sample) -> Double = chart == .cpu ? { $0.cpu } : { $0.rssMB }
        // A floor on the scale keeps an idle process from looking busy.
        let top = max((samples.map(value).max() ?? 0) * 1.25, chart == .cpu ? 5 : 10)
        let span = (samples.last?.date ?? .now).timeIntervalSince(samples.first?.date ?? .now)
        let timeFormat: Date.FormatStyle = span < 300
            ? .dateTime.hour().minute().second()
            : .dateTime.hour().minute()
        return Chart(samples) { sample in
            AreaMark(x: .value("Time", sample.date), y: .value(chart.rawValue, value(sample)))
                .foregroundStyle(.linearGradient(colors: [color.opacity(0.35), color.opacity(0.02)],
                                                 startPoint: .top, endPoint: .bottom))
                .interpolationMethod(.monotone)
            LineMark(x: .value("Time", sample.date), y: .value(chart.rawValue, value(sample)))
                .foregroundStyle(color)
                .lineStyle(StrokeStyle(lineWidth: 1.5))
                .interpolationMethod(.monotone)
        }
        .chartXAxis {
            AxisMarks(values: .automatic(desiredCount: 3)) { mark in
                AxisGridLine().foregroundStyle(.quaternary)
                // The last label would run past the plot into the y-axis.
                if mark.index < mark.count - 1 {
                    AxisValueLabel(format: timeFormat)
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .trailing, values: .automatic(desiredCount: 3)) { axis in
                AxisGridLine().foregroundStyle(.quaternary)
                AxisValueLabel {
                    if let v = axis.as(Double.self) {
                        Text(chart == .cpu ? Format.cpu(v) : String(format: "%.0f MB", v))
                    }
                }
            }
        }
        .chartYScale(domain: 0...top)
        .font(.system(size: 9))
    }

    // MARK: Info

    private func info(_ process: ProcessRow) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if let project = process.project {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    InfoLabel(title: "Project")
                    ProjectTag(project: project)
                    Spacer(minLength: 4)
                    Button {
                        Finder.reveal(project.root)
                    } label: {
                        Image(systemName: "arrow.up.forward.app")
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                    .help("Reveal in Finder")
                }
            }
            InfoRow(title: "Command", value: process.cmd, monospaced: true)
            if let cwd = process.cwd {
                InfoRow(title: "Directory", value: cwd.replacingOccurrences(of: NSHomeDirectory(), with: "~"))
            }
            if !process.ports.isEmpty {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    InfoLabel(title: "Ports")
                    HStack(spacing: 4) { ForEach(process.ports, id: \.self) { PortChip(port: $0) } }
                }
            }
            InfoRow(title: "PID", value: process.childPid.map { "\(process.pid) → \($0)" } ?? "\(process.pid)",
                    monospaced: true)
            if let code = process.lastExitCode {
                InfoRow(title: "Last exit", value: "code \(code)")
            }
            if let error = process.lastError {
                InfoRow(title: "Error", value: error, tint: .red)
            }
        }
    }

    // MARK: Events

    @ViewBuilder
    private var events: some View {
        let recent = Array((metrics?.events ?? []).suffix(6).reversed())
        if !recent.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                Text("Activity")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .padding(.bottom, 6)
                ForEach(Array(recent.enumerated()), id: \.element.id) { index, event in
                    HStack(alignment: .top, spacing: 8) {
                        VStack(spacing: 0) {
                            Circle()
                                .fill(color(for: event))
                                .frame(width: 7, height: 7)
                                .padding(.top, 4)
                            if index < recent.count - 1 {
                                Rectangle().fill(.quaternary).frame(width: 1)
                            }
                        }
                        .frame(width: 7)
                        Text(describe(event)).font(.system(size: 11.5))
                        Spacer()
                        Text(event.date, format: .relative(presentation: .named))
                            .font(.system(size: 10.5))
                            .foregroundStyle(.tertiary)
                    }
                    .frame(minHeight: 20, alignment: .top)
                }
            }
        }
    }

    // MARK: Logs

    private var logsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Logs")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(logs, forType: .string)
                } label: {
                    Label("Copy", systemImage: "doc.on.doc")
                        .font(.system(size: 10.5))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .disabled(logs.isEmpty)
            }
            ScrollView([.vertical, .horizontal]) {
                Text(logs.isEmpty ? (loadError ?? "No output yet.") : logs)
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Color(white: 0.85))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
            }
            .defaultScrollAnchor(.bottom)
            .frame(height: 150)
            .background(Color(white: 0.09).opacity(0.9), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
    }

    private func load() async {
        guard let client = store.client else { return }
        async let report = try? client.metrics(name)
        do {
            let output = try await client.logs(name)
            // `runa logs` prints section headers even when both logs are empty.
            let hasOutput = output.split(separator: "\n").contains { !$0.hasPrefix("--- ") }
            logs = hasOutput ? output : ""
            loadError = nil
        } catch {
            loadError = error.localizedDescription
        }
        metrics = await report
    }

    private func color(for event: MetricsReport.Event) -> Color {
        switch event.event {
        case "start": Brand.running
        case "exit": event.code == 0 ? .gray : Brand.failed
        case "restart": Brand.accent
        default: .gray
        }
    }

    private func describe(_ event: MetricsReport.Event) -> String {
        switch event.event {
        case "start": "Started" + (event.pid.map { " · PID \($0)" } ?? "")
        case "exit": event.code.map { "Exited with code \($0)" } ?? "Killed by a signal"
        case "restart": "Restart requested"
        default: "Stopped"
        }
    }
}

struct StatTile: View {
    let title: String
    let value: String
    var detail: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 15, weight: .semibold).monospacedDigit())
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(detail ?? " ")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

struct InfoLabel: View {
    let title: String

    var body: some View {
        Text(title)
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .frame(width: 62, alignment: .leading)
    }
}

struct InfoRow: View {
    let title: String
    let value: String
    var monospaced = false
    var tint: Color?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            InfoLabel(title: title)
            Text(value)
                .font(monospaced ? .system(size: 11, design: .monospaced) : .system(size: 11))
                .foregroundStyle(tint ?? .primary)
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
