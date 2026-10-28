import Foundation

@MainActor
final class BotDetailViewModel: ObservableObject {
    @Published var session: ControlStateSession?
    @Published var isLoading = true
    @Published var errorMessage: String?
    @Published var isSending = false

    private let core = Core.shared

    func load(botKey: String, guildId: String) async {
        guard !botKey.isEmpty, !guildId.isEmpty else {
            errorMessage = "Missing bot or guild."
            isLoading = false
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            session = try await core.controlState(botKey: botKey, guildId: guildId).session
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load bot detail.")
        }
    }

    @discardableResult
    func sendAction(botKey: String, guildId: String, action: String, payload: [String: String] = [:]) async -> Bool {
        isSending = true
        defer { isSending = false }
        do {
            _ = try await core.control(botKey: botKey, guildId: guildId, action: action, payload: payload)
            await load(botKey: botKey, guildId: guildId)
            return true
        } catch {
            guard !error.isCancellation else { return false }
            errorMessage = error.message("Action failed.")
            return false
        }
    }
}
