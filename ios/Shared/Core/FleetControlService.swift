import Foundation

/// Headless control for Siri/Shortcuts and widget buttons ("your guild's
/// bot"). The lookup and the order both run in the Rust core.
enum FleetControlService {
    static func ownSession() async throws -> (bot: DashboardBot, session: DashboardSession) {
        let own = try await Core.shared.ownSession()
        return (own.bot, own.session)
    }

    @discardableResult
    static func send(_ action: String) async throws -> DashboardSession {
        try await Core.shared.sendOwnControl(action: action)
    }
}
