import Foundation

/// Telemetry front door. Queueing, persistence, batching and upload to
/// /api/telemetry/client all happen in the Rust core.
@MainActor
final class ClientTelemetry {
    static let shared = ClientTelemetry()
    nonisolated static var clientName: String { Core.clientName }

    private init() {}

    /// Kept for call-site compatibility; the core starts flushing on its own.
    func start() {}

    func log(_ name: String, value: Double? = nil, text: String? = nil, _ metadata: [String: String] = [:]) {
        Core.shared.log(name: name, value: value, text: text, metadata: metadata)
    }

    func flush() async {
        await Core.shared.flushTelemetry()
    }

    static func ms(since start: Date) -> Double {
        (Date().timeIntervalSince(start) * 1000).rounded()
    }

    static func kind(of error: Error) -> String {
        error.telemetryKind
    }
}

/// The Apple TV app's name for the same thing.
typealias TVTelemetry = ClientTelemetry
