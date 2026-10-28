import Foundation

@MainActor
final class LearningViewModel: ObservableObject {
    @Published var data: MusicIntelligenceData?
    @Published var isLoading = true
    @Published var errorMessage: String?

    private let core = Core.shared

    /// guildId nil/empty means fleet-wide for an admin (the server rejects
    /// it for a non-admin with no guild — surfaced as errorMessage, same
    /// defense-in-depth pattern LeaderboardViewModel's swarm call uses).
    func load(guildId: String?) async {
        isLoading = true
        defer { isLoading = false }
        do {
            data = try await core.musicIntelligence(guildId: guildId?.isEmpty == false ? guildId : nil, botKey: nil).data
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load music intelligence.")
        }
    }
}
