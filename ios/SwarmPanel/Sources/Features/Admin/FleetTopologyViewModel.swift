import Foundation

extension GuildOverlap: Identifiable {
    public var id: String { guildId }
}

@MainActor
final class FleetTopologyViewModel: ObservableObject {
    @Published var bots: [DashboardBot] = []
    @Published var isLoading = true
    @Published var errorMessage: String?
    @Published var statusMessage: String?
    @Published var restartingBotKey: String?
    @Published var isRecoveringAll = false

    private let core = Core.shared

    private var dashboard: DashboardResponse { DashboardResponse(bots: bots, sessions: nil, generatedAt: nil) }

    var recoverableSessions: [(bot: DashboardBot, session: DashboardSession)] {
        findRecoverableSessions(dashboard: dashboard).map { ($0.bot, $0.session) }
    }

    var overlaps: [GuildOverlap] { fleetOverlaps(dashboard: dashboard) }

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            bots = try await core.dashboard().bots ?? []
            errorMessage = nil
        } catch {
            if !error.isCancellation { errorMessage = error.message("Failed to load fleet topology.") }
        }
    }

    func recoverAllStale() async {
        guard !recoverableSessions.isEmpty else { return }
        isRecoveringAll = true
        defer { isRecoveringAll = false }
        do {
            statusMessage = try await core.recoverAllStale().message
            Haptics.success()
        } catch {
            if !error.isCancellation { errorMessage = error.message("Recovery failed.") }
        }
        await load()
    }

    func restart(_ bot: DashboardBot) async {
        restartingBotKey = bot.key
        defer { restartingBotKey = nil }
        do {
            _ = try await core.restartBot(botKey: bot.key)
            Haptics.success()
            statusMessage = "Restart sent to \(bot.label)."
            await load()
        } catch {
            if !error.isCancellation {
                Haptics.error()
                errorMessage = error.message("Restart failed.")
            }
        }
    }
}
