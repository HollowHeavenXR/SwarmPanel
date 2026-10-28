import Foundation

/// Apple TV sign-in state. Login (as client "tvos", admin mode off,
/// remember-device), token storage, the refresh loop and expiry live in the
/// Rust core; this mirrors it for SwiftUI.
@MainActor
final class TVSession: ObservableObject {
    @Published private(set) var isAuthenticated: Bool
    @Published private(set) var username: String
    @Published var errorMessage: String?
    @Published private(set) var isWorking = false

    private let core = Core.shared
    private lazy var observer = Observer(owner: self)
    private var lastRefresh: Date?

    init() {
        isAuthenticated = core.hasToken()
        username = core.session().username
        core.addSessionObserver(observer: observer)
        if isAuthenticated {
            Task { await refresh(reason: "launch") }
        }
    }

    func signIn(username: String, password: String) async {
        isWorking = true
        errorMessage = nil
        defer { isWorking = false }
        do {
            let state = try await core.login(username: username, password: password, guildId: nil)
            self.username = state.username
            isAuthenticated = true
            lastRefresh = Date()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Sign-in failed.")
        }
    }

    func signOut() {
        SwarmLiveSocket.shared.disconnect()
        core.logout()
        isAuthenticated = false
    }

    func appBecameActive() {
        guard isAuthenticated else { return }
        if let lastRefresh, Date().timeIntervalSince(lastRefresh) < 5 * 60 { return }
        Task { await refresh(reason: "foreground") }
    }

    func refresh(reason: String) async {
        if let state = try? await core.refreshSession(reason: reason) {
            username = state.username
            lastRefresh = Date()
        }
    }

    fileprivate func sessionChanged(_ state: SessionState, _ change: SessionChange) {
        switch change {
        case .expired:
            guard isAuthenticated else { return }
            SwarmLiveSocket.shared.disconnect()
            isAuthenticated = false
            errorMessage = "Your session expired. Please sign in again."
        case .signedOut:
            isAuthenticated = false
        case .signedIn, .refreshed:
            if !state.username.isEmpty { username = state.username }
        }
    }

    private final class Observer: SessionObserver {
        weak var owner: TVSession?
        init(owner: TVSession) { self.owner = owner }
        func sessionChanged(state: SessionState, change: SessionChange) {
            Task { @MainActor [weak owner] in owner?.sessionChanged(state, change) }
        }
    }
}
