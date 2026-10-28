import Foundation

@MainActor
final class PublicProfileViewModel: ObservableObject {
    @Published var profile: PublicProfile?
    @Published var permissions: SocialPermissions?
    @Published var isLoading = true
    @Published var isActing = false
    @Published var errorMessage: String?

    private let core = Core.shared
    let accountId: Int

    init(accountId: Int) {
        self.accountId = accountId
    }

    func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            let envelope = try await core.publicProfile(accountId: accountId)
            profile = envelope.profile
            permissions = envelope.socialPermissions
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load profile.")
        }
    }

    func toggleFollow() async {
        guard let profile else { return }
        isActing = true
        defer { isActing = false }
        do {
            _ = try await core.follow(accountId: accountId, following: !(profile.followedByMe ?? false))
            Haptics.success()
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to update follow.")
            Haptics.error()
        }
    }

    func sendFriendRequest() async {
        isActing = true
        defer { isActing = false }
        do {
            _ = try await core.sendFriendRequest(accountId: accountId)
            Haptics.success()
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to send friend request.")
            Haptics.error()
        }
    }
}
