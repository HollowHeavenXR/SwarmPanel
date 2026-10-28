import Foundation

/// Session state for the SwiftUI app. Login, registration, token storage,
/// the 30-minute refresh loop and 401 expiry all live in the Rust core; this
/// object just mirrors the core's SessionState for the views.
@MainActor
final class AppState: ObservableObject {
    @Published private(set) var isAuthenticated = false
    @Published private(set) var isBootstrapping = true
    @Published private(set) var username = ""
    @Published private(set) var role = ""
    @Published private(set) var guildId: String?
    @Published private(set) var isOwner = false
    @Published private(set) var isAdmin = false
    @Published private(set) var isModerator = false
    @Published private(set) var canGallery = false
    @Published var errorMessage: String?

    private let core = Core.shared
    private lazy var observer = Observer(owner: self)

    init() {
        core.addSessionObserver(observer: observer)
    }

    func bootstrap() async {
        isBootstrapping = true
        defer { isBootstrapping = false }
        if let state = await core.bootstrap() { apply(state) }
    }

    func login(username: String, password: String, guildId: String?) async {
        errorMessage = nil
        do {
            apply(try await core.login(username: username, password: password, guildId: guildId))
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Login failed.")
        }
    }

    func register(username: String, guildId: String, password: String, email: String, verificationWebhookUrl: String, discordUserId: String = "") async {
        errorMessage = nil
        do {
            apply(try await core.register(username: username, guildId: guildId, password: password, email: email,
                                          verificationWebhookUrl: verificationWebhookUrl, discordUserId: discordUserId))
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Registration failed.")
        }
    }

    func setAdminMode(_ enabled: Bool) async {
        errorMessage = nil
        do {
            apply(try await core.setAdminMode(enabled: enabled))
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to change admin mode.")
        }
    }

    func logout() {
        core.logout()
    }

    func refreshSession() async {
        guard core.hasToken() else { return }
        if let state = try? await core.refreshSession(reason: "foreground") { apply(state) }
    }

    fileprivate func apply(_ s: SessionState) {
        isAuthenticated = s.isAuthenticated
        username = s.username
        role = s.role
        guildId = s.guildId
        isOwner = s.isOwner
        isAdmin = s.isAdmin
        isModerator = s.isModerator
        canGallery = s.canGallery
    }

    private final class Observer: SessionObserver {
        weak var owner: AppState?
        init(owner: AppState) { self.owner = owner }
        func sessionChanged(state: SessionState, change: SessionChange) {
            Task { @MainActor [weak owner] in owner?.apply(state) }
        }
    }
}
