import Foundation

enum LeaderboardScope: String, CaseIterable, Identifiable {
    case guild = "This Guild"
    case swarm = "Swarm-Wide"
    var id: String { rawValue }
}

@MainActor
final class LeaderboardViewModel: ObservableObject {
    @Published var bots: [BotSummary] = []
    @Published var selectedBotKey: String = ""
    @Published var guildId: String = ""
    @Published var data: LeaderboardData?
    @Published var isLoading = true
    @Published var errorMessage: String?

    // Swarm-wide (admin-only) leaderboard state — separate from the
    // per-guild fields above since it fans out across every bot's database
    // instead of scoping to one bot+guild.
    @Published var scope: LeaderboardScope = .guild
    @Published var swarmWindowDays: Int = 7
    @Published var swarmData: SwarmLeaderboardResponse?

    private let core = Core.shared

    func loadBots() async {
        do {
            bots = (try await core.bots().bots ?? []).filter { $0.kind == "music" }
            if selectedBotKey.isEmpty { selectedBotKey = bots.first?.id ?? "" }
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load bots.")
        }
    }

    func loadLeaderboard() async {
        guard !selectedBotKey.isEmpty, !guildId.isEmpty else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            data = try await core.guildLeaderboard(guildId: guildId, botKey: selectedBotKey).data
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load leaderboard.")
        }
    }

    /// Admin-only — see GET /api/swarm-leaderboard's own 403 for non-admins.
    /// Callers should only invoke this when AppState.isAdmin is true; a
    /// non-admin hitting it just surfaces the server's rejection as
    /// errorMessage rather than crashing, so this is defense in depth, not
    /// the only gate.
    func loadSwarmLeaderboard() async {
        isLoading = true
        defer { isLoading = false }
        do {
            swarmData = try await core.swarmLeaderboard(days: swarmWindowDays, limit: 25)
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load swarm leaderboard.")
        }
    }
}
