import Foundation

@MainActor
final class InvitesViewModel: ObservableObject {
    @Published var inviteBots: [InviteBot] = []
    @Published var isLoading = true
    @Published var errorMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            inviteBots = try await core.bots().inviteBots ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load invite roster.")
        }
    }
}
