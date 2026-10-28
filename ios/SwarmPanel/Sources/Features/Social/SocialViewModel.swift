import Foundation

@MainActor
final class SocialViewModel: ObservableObject {
    @Published var searchQuery = ""
    @Published var searchResults: [AccountSummary] = []
    @Published var incomingRequests: [FriendRequest] = []
    @Published var outgoingRequests: [FriendRequest] = []
    @Published var friends: [AccountSummary] = []
    @Published var threads: [MessageThread] = []
    @Published var errorMessage: String?
    @Published var isLoading = false

    private let core = Core.shared
    private let socket = SwarmLiveSocket.shared
    private var watching = false

    func loadAll() async {
        isLoading = true
        defer { isLoading = false }
        async let requestsTask: Void = loadFriendRequests()
        async let friendsTask: Void = loadFriends()
        async let threadsTask: Void = loadThreads()
        _ = await (requestsTask, friendsTask, threadsTask)
        startWatching()
    }

    /// Mirrors the web Friends page's window.swarmLive.watch("friends"/
    /// "threads", ...) -- keeps the lists current between explicit loads
    /// without a poll timer.
    private func startWatching() {
        guard !watching else { return }
        watching = true
        socket.watch("friends", as: FriendsSnapshot.self) { [weak self] result in
            guard let self, case .success(let snapshot) = result else { return }
            self.friends = snapshot.friends ?? self.friends
            self.incomingRequests = snapshot.incoming ?? self.incomingRequests
            self.outgoingRequests = snapshot.outgoing ?? self.outgoingRequests
        }
        socket.watch("threads", as: MessageThreadsResponse.self) { [weak self] result in
            guard let self, case .success(let response) = result else { return }
            self.threads = response.threads ?? self.threads
        }
        socket.connect()
    }

    func stopWatching() {
        guard watching else { return }
        watching = false
        socket.unwatch("friends")
        socket.unwatch("threads")
    }

    func search() async {
        do {
            let response = try await core.searchUsers(query: searchQuery)
            searchResults = response.users ?? []
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Search failed.")
        }
    }

    func loadFriendRequests() async {
        do {
            let response = try await core.friendRequests()
            incomingRequests = response.incoming ?? []
            outgoingRequests = response.outgoing ?? []
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load friend requests.")
        }
    }

    func loadFriends() async {
        do {
            let response = try await core.friends()
            friends = response.friends ?? []
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load friends.")
        }
    }

    func loadThreads() async {
        do {
            let response = try await core.messageThreads()
            threads = response.threads ?? []
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load messages.")
        }
    }

    func follow(_ accountId: Int, following: Bool = true) async {
        do {
            _ = try await core.follow(accountId: accountId, following: following)
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Follow failed.")
        }
    }

    func sendFriendRequest(_ accountId: Int) async {
        do {
            _ = try await core.sendFriendRequest(accountId: accountId)
            await loadFriendRequests()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Friend request failed.")
        }
    }

    func respondToRequest(_ request: FriendRequest, action: String) async {
        do {
            _ = try await core.respondFriendRequest(requestId: request.id, action: action)
            await loadFriendRequests()
            await loadFriends()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Action failed.")
        }
    }
}
