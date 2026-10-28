import Foundation

@MainActor
final class UsersDirectoryViewModel: ObservableObject {
    @Published var query = ""
    /// Only operators seen in the last 3 minutes (?online=1), matching the
    /// web Directory's "Online now" toggle.
    @Published var onlineOnly = false
    @Published var users: [AccountSummary] = []
    @Published var isLoading = true
    @Published var errorMessage: String?

    private let core = Core.shared

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            users = try await core.directory(query: query, onlineOnly: onlineOnly).users ?? []
            errorMessage = nil
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load the directory.")
        }
    }

    func follow(_ accountId: Int, following: Bool) async {
        do {
            _ = try await core.follow(accountId: accountId, following: following)
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Follow failed.")
        }
    }
}
