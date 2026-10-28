import SwiftUI

/// The signed-in account's display profile + panel preferences, kept in sync
/// with the web panel through the "account" live key (routes.lua), with a
/// REST fallback (GET /api/users/me + /api/users/preferences) at start and
/// whenever the socket is down. Changing the background, accent or profile
/// on the web shows up on the TV within ~10 seconds.
@MainActor
final class TVAccountModel: ObservableObject {
    @Published private(set) var profile: TVProfile?
    @Published private(set) var preferences: TVPreferences?

    private let socket = SwarmLiveSocket.shared
    private let core = Core.shared
    private let telemetry = TVTelemetry.shared
    private var pollTask: Task<Void, Never>?


    /// The last known profile + appearance from disk, so the TV comes up in
    /// the account's own background straight away instead of flashing the
    /// default one until the network answers.
    init() {
        guard let cached = core.cachedAccount() else { return }
        profile = cached.profile
        preferences = cached.preferences
    }

    /// The core clears its caches on sign-out; kept for call-site compatibility.
    static func clearCache() {}

    func start() {
        guard pollTask == nil else { return }
        socket.watch("account", as: TVAccountSnapshot.self) { [weak self] result in
            guard let self, case .success(let snapshot) = result else { return }
            self.update(profile: snapshot.profile, preferences: snapshot.preferences, source: "live")
        }
        socket.connect()
        pollTask = Task { [weak self] in
            await self?.loadOnce()
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(60))
                if Task.isCancelled { break }
                guard let self else { break }
                if !self.socket.isConnected { await self.loadOnce() }
            }
        }
    }

    func stop() {
        pollTask?.cancel()
        pollTask = nil
        socket.unwatch("account")
        profile = nil
        preferences = nil
    }

    private func loadOnce() async {
        var newProfile: TVProfile?
        var newPreferences: TVPreferences?
        do {
            let me = try await core.me().profile
            newProfile = AccountProfile(username: me.username, displayName: me.displayName, avatarUrl: me.avatarUrl, serverName: me.serverName,
                                        serverIconUrl: me.serverIconUrl, profileHeadline: me.profileHeadline, guildId: me.guildId,
                                        favoriteBot: nil, themeAccent: me.themeAccent, role: nil, adminMode: nil)
            newPreferences = try await core.preferences().preferences
        } catch {
            if !error.isCancellation {
                telemetry.log("account_load_failure", ["kind": TVTelemetry.kind(of: error)])
            }
        }
        update(profile: newProfile, preferences: newPreferences, source: "rest")
    }

    /// Applies whatever arrived, logs what actually changed (so a web-side
    /// Appearance edit reaching the TV is visible in telemetry), and saves
    /// the result for the next launch.
    private func update(profile newProfile: TVProfile?, preferences newPreferences: TVPreferences?, source: String) {
        var changed: [String] = []
        if let newPreferences {
            if newPreferences.backgroundMode != preferences?.backgroundMode
                || newPreferences.backgroundColor != preferences?.backgroundColor
                || newPreferences.backgroundImageUrl != preferences?.backgroundImageUrl { changed.append("background") }
            if newPreferences.accentColor != preferences?.accentColor { changed.append("accent") }
            preferences = newPreferences
        }
        if let newProfile {
            if newProfile.displayName != profile?.displayName || newProfile.avatarUrl != profile?.avatarUrl
                || newProfile.serverName != profile?.serverName { changed.append("profile") }
            profile = newProfile
        }
        guard !changed.isEmpty else { return }
        telemetry.log("account_sync", ["source": source, "changed": changed.joined(separator: ",")])
        core.saveAccountCache(snapshot: AccountSnapshot(profile: profile, preferences: preferences))
    }

    // MARK: - Appearance (mirrors html.lua's panel_style())

    var accent: Color { Color(hex: preferences?.accentColor) ?? Color(hex: "#89b4fa")! }

    /// Base colour: the chosen preset, or the custom colour.
    var backgroundColor: Color {
        let mode = preferences?.backgroundMode ?? "default"
        if mode == "custom_color", let custom = Color(hex: preferences?.backgroundColor) { return custom }
        let presets = ["default": "#0d1117", "midnight": "#090b12", "aurora": "#101821", "ember": "#17100d"]
        return Color(hex: presets[mode] ?? presets["default"]!)!
    }

    /// Only used in custom_image mode, and only for http(s) URLs -- same
    /// rule as the web panel.
    var backgroundImageURL: URL? {
        guard preferences?.backgroundMode == "custom_image",
              let raw = preferences?.backgroundImageUrl?.trimmingCharacters(in: .whitespaces),
              raw.hasPrefix("http://") || raw.hasPrefix("https://") else { return nil }
        return URL(string: raw)
    }
}

extension Color {
    /// "#rrggbb" -> Color; nil for anything else.
    init?(hex: String?) {
        guard var text = hex?.trimmingCharacters(in: .whitespaces), text.hasPrefix("#") else { return nil }
        text.removeFirst()
        guard text.count == 6, let value = UInt32(text, radix: 16) else { return nil }
        self.init(
            red: Double((value >> 16) & 0xFF) / 255,
            green: Double((value >> 8) & 0xFF) / 255,
            blue: Double(value & 0xFF) / 255
        )
    }
}

/// Full-screen panel background: the account's colour, its custom image
/// (if set), and the same darkening + accent wash the web shell draws over
/// it so cards stay readable.
struct TVPanelBackground: View {
    @ObservedObject var account: TVAccountModel

    var body: some View {
        ZStack {
            account.backgroundColor
            if let url = account.backgroundImageURL {
                AsyncImage(url: url) { phase in
                    if let image = phase.image {
                        image.resizable().aspectRatio(contentMode: .fill)
                    }
                }
            }
            LinearGradient(
                colors: [Color.black.opacity(0.55), Color.black.opacity(0.85)],
                startPoint: .top, endPoint: .bottom
            )
            // Resonance: accent glow and slow sound-wave ribbons.
            TVResonanceOverlay(accent: account.accent)
        }
        .ignoresSafeArea()
        .animation(.easeInOut(duration: 0.6), value: account.backgroundImageURL)
    }
}
