import Foundation

@MainActor
final class ThreadViewModel: ObservableObject {
    @Published var messages: [ChatMessage] = []
    @Published var draft = ""
    @Published var isSending = false
    @Published var errorMessage: String?

    let accountId: Int
    let peerName: String
    private let core = Core.shared
    private let socket = SwarmLiveSocket.shared
    private var pollTask: Task<Void, Never>?

    init(accountId: Int, peerName: String) {
        self.accountId = accountId
        self.peerName = peerName
    }

    /// Live-push equivalent of the web thread view's window.swarmLive.watch
    /// ("thread_messages", {account_id}) -- new incoming messages show up
    /// without leaving and re-entering the conversation.
    func startPolling() {
        guard pollTask == nil else { return }
        socket.watch("thread_messages", params: ["account_id": String(accountId)], as: MessagesResponse.self) { [weak self] result in
            guard let self, case .success(let response) = result else { return }
            self.messages = response.messages ?? self.messages
        }
        socket.connect()

        // Fallback poll only while the socket is down, same pattern as
        // DashboardViewModel/NotificationsViewModel.
        pollTask = Task { [weak self] in
            guard let self else { return }
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                if Task.isCancelled { break }
                if !self.socket.isConnected {
                    await self.load(silent: true)
                }
            }
        }
    }

    func stopPolling() {
        pollTask?.cancel()
        pollTask = nil
        socket.unwatch("thread_messages")
    }

    func load(silent: Bool = false) async {
        do {
            let response = try await core.messages(accountId: accountId)
            messages = response.messages ?? []
            if !silent { errorMessage = nil }
        } catch {
            guard !error.isCancellation, !silent else { return }
            errorMessage = error.message("Failed to load messages.")
        }
    }

    func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        isSending = true
        defer { isSending = false }
        do {
            _ = try await core.sendMessage(accountId: accountId, body: text)
            draft = ""
            await load()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to send message.")
        }
    }
}
