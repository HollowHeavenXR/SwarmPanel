import Foundation

// The Apple TV app uses the Rust core's models directly.
typealias TVDashboardResponse = DashboardResponse
typealias TVBot = DashboardBot
typealias TVMedicSummary = MedicSummary
typealias TVAccountSnapshot = AccountSnapshot
typealias TVProfile = AccountProfile
typealias TVPreferences = PanelPreferences

extension AccountProfile {
    var name: String {
        if let displayName, !displayName.isEmpty { return displayName }
        return username ?? "Operator"
    }
}

/// Formatting helpers (implemented in the core).
enum TVFormat {
    static func duration(_ seconds: Double) -> String { formatDuration(seconds: seconds) }
    static func uptime(_ seconds: Double) -> String { formatUptime(seconds: seconds) }
    static func position(of session: DashboardSession, at now: Date) -> Double { session.position(at: now) }
}
