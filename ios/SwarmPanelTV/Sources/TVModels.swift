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

/// Display formatting, run every second while playing, so it stays in Swift.
enum TVFormat {
    static func duration(_ seconds: Double) -> String {
        let t = max(0, Int(seconds))
        return "\(t / 60):" + String(format: "%02d", t % 60)
    }

    static func uptime(_ seconds: Double) -> String {
        let s = max(0, Int(seconds))
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m" }
        if s < 86400 { return "\(s / 3600)h \(s / 60 % 60)m" }
        return "\(s / 86400)d \(s / 3600 % 24)h"
    }
    static func position(of session: DashboardSession, at now: Date) -> Double { session.position(at: now) }
}
