import Foundation

/// Fleet dashboard: first paint from the core's cached snapshot, then live
/// pushes over the socket, with a 15s REST fallback while it's down.
@MainActor
final class DashboardViewModel: ObservableObject {
    @Published var response: DashboardResponse?
    @Published var isLoading = true
    @Published var errorMessage: String?
    @Published var topTrack: LeaderboardTrack?
    @Published var lastUpdatedAt: Date?

    private let socket = SwarmLiveSocket.shared
    private let core = Core.shared
    private var pollTask: Task<Void, Never>?
    private var started = false

    init() {
        if let cached = core.cachedDashboard() {
            response = cached.dashboard
            lastUpdatedAt = Date(timeIntervalSince1970: cached.savedAt)
        }
    }

    func start() {
        guard !started else { return }
        started = true
        socket.watch("dashboard", as: DashboardResponse.self) { [weak self] result in
            guard let self, case .success(let snapshot) = result else { return }
            self.response = snapshot
            self.lastUpdatedAt = Date()
            self.core.saveDashboardCache(dashboard: snapshot)
        }
        socket.connect()
        pollTask = Task { [weak self] in
            guard let self else { return }
            await self.loadOnce()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(15))
                if Task.isCancelled { break }
                if !self.socket.isConnected {
                    await self.loadOnce(silent: true)
                }
            }
        }
    }

    func stop() {
        started = false
        pollTask?.cancel()
        pollTask = nil
        socket.unwatch("dashboard")
    }

    func refresh() async {
        await loadOnce()
    }

    func loadTopTrack(botKey: String, guildId: String) async {
        guard !botKey.isEmpty, !guildId.isEmpty else {
            topTrack = nil
            return
        }
        do {
            topTrack = try await core.guildLeaderboard(guildId: guildId, botKey: botKey).data.topTracks?.first
        } catch {
            if !error.isCancellation { topTrack = nil }
        }
    }

    private func loadOnce(silent: Bool = false) async {
        if !silent { isLoading = true }
        do {
            response = try await core.dashboard()
            lastUpdatedAt = Date()
            errorMessage = nil
        } catch {
            if !silent, !error.isCancellation {
                errorMessage = error.message("Failed to load dashboard.")
            }
        }
        if !silent { isLoading = false }
    }
}
