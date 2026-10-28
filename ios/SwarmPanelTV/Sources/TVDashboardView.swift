import SwiftUI
import UIKit

/// The TV version of the web Dashboard, in Resonance: the account header,
/// the fleet as a focusable hive beside its readouts and Aria's conductor
/// card, then an "On air" row with one card per music bot. Read-only --
/// selecting a hive cell or card opens that bot's sessions, nothing sends
/// orders.
struct TVDashboardView: View {
    @ObservedObject var session: TVSession
    @ObservedObject var account: TVAccountModel
    @StateObject private var model = TVDashboardModel()
    @ObservedObject private var live = SwarmLiveSocket.shared
    @State private var selectedBot: TVBot?
    @State private var focusedHiveKey: String?
    @State private var confirmSignOut = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 48) {
                header
                if model.dashboard == nil {
                    loadingState
                } else {
                    HStack(alignment: .top, spacing: 48) {
                        hivePanel
                        VStack(alignment: .leading, spacing: 32) {
                            metrics
                            if let aria = model.orchestrator {
                                TVAriaCard(bot: aria, accent: account.accent)
                            }
                        }
                    }
                    onAir
                }
            }
            .padding(.horizontal, 80)
            .padding(.vertical, 50)
        }
        .onAppear {
            model.start()
            // A dashboard is meant to stay up on the TV -- don't let the
            // screen saver take over while it's showing.
            UIApplication.shared.isIdleTimerDisabled = true
        }
        .onDisappear {
            model.stop()
            UIApplication.shared.isIdleTimerDisabled = false
        }
        .sheet(item: $selectedBot) { bot in
            TVBotDetailView(bot: bot, accent: account.accent)
        }
        .alert("Sign out of SwarmPanel?", isPresented: $confirmSignOut) {
            Button("Sign Out", role: .destructive) { session.signOut() }
            Button("Cancel", role: .cancel) {}
        }
    }

    private var header: some View {
        HStack(spacing: 32) {
            TVAvatar(url: account.profile?.avatarUrl ?? account.profile?.serverIconUrl,
                     name: account.profile?.name ?? session.username, accent: account.accent, size: 104)
                .overlay(Circle().strokeBorder(account.accent, lineWidth: 3))
                .shadow(color: account.accent.opacity(0.5), radius: 20)
            VStack(alignment: .leading, spacing: 6) {
                Text((account.profile?.serverName ?? "SwarmPanel fleet").uppercased())
                    .font(TVRes.eyebrow)
                    .tracking(2)
                    .foregroundStyle(account.accent)
                Text(account.profile?.name ?? session.username)
                    .font(TVRes.display(58))
                    .lineLimit(1)
                Text(headline)
                    .font(.system(size: 28, weight: .medium, design: .rounded))
                    .foregroundStyle(TVRes.mist)
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 10) {
                HStack(spacing: 12) {
                    Circle().fill(live.isConnected ? TVRes.live : TVRes.warn).frame(width: 14, height: 14)
                    Text(live.isConnected ? "LIVE" : "RECONNECTING")
                        .font(TVRes.eyebrow)
                        .tracking(2)
                        .foregroundStyle(live.isConnected ? TVRes.live : TVRes.warn)
                }
                if let updated = model.lastUpdated, model.isFromCache || !live.isConnected {
                    // Cached or stale data on screen: say how old it is.
                    Text("Updated \(updated, style: .relative) ago")
                        .font(.caption)
                        .foregroundStyle(TVRes.mist)
                }
            }
            Button("Sign Out") {
                TVTelemetry.shared.log("sign_out_prompted")
                confirmSignOut = true
            }
        }
    }

    private var headline: String {
        guard model.dashboard != nil else { return "Tuning in to the fleet…" }
        let live = model.liveSessionCount
        if live == 0 { return "The hive is quiet — \(model.musicBots.count) bots standing by." }
        return "\(live) \(live == 1 ? "session" : "sessions") on air across \(model.guildsServed) guilds."
    }

    private var hivePanel: some View {
        VStack(alignment: .leading, spacing: 24) {
            Text("THE HIVE")
                .font(TVRes.eyebrow)
                .tracking(2)
                .foregroundStyle(account.accent)
            TVHiveMap(bots: model.dashboard?.bots ?? [], cellWidth: 132, onSelect: open, focusedKey: $focusedHiveKey)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
            focusedCaption
        }
        .padding(36)
        .frame(width: 860)
        .tvGlass(radius: 40)
        .focusSection()
    }

    /// Name and track of whichever hive cell has focus.
    @ViewBuilder
    private var focusedCaption: some View {
        let bot = focusedHiveKey.flatMap { key in model.dashboard?.bots?.first { $0.key == key } }
        HStack(spacing: 16) {
            if let bot {
                Hexagon().fill(BotPalette.color(for: bot.key)).frame(width: 26, height: 30)
                VStack(alignment: .leading, spacing: 4) {
                    Text(bot.name).font(.system(size: 30, weight: .bold, design: .rounded))
                    Text(bot.isOrchestrator ? "Autonomous swarm orchestrator" : (bot.featuredSession?.title ?? "Waiting for live playback."))
                        .font(.system(size: 24, design: .rounded))
                        .foregroundStyle(TVRes.mist)
                        .lineLimit(1)
                }
                Spacer()
                TVStateBadge(state: bot.hiveState)
            } else {
                Text("Move across the hive to see what each bot is playing.")
                    .font(.system(size: 24, design: .rounded))
                    .foregroundStyle(TVRes.mist)
            }
        }
        .frame(height: 70)
        .animation(.easeInOut(duration: 0.2), value: focusedHiveKey)
    }

    private var metrics: some View {
        VStack(alignment: .leading, spacing: 30) {
            HStack(spacing: 24) {
                TVReadout(value: "\(model.onlineCount)/\(model.musicBots.count)", label: "Online")
                TVReadout(value: "\(model.liveSessionCount)", label: "On air", tint: model.liveSessionCount > 0 ? TVRes.live : TVRes.ink)
                TVReadout(value: "\(model.queueDepth)", label: "Queued", tint: account.accent)
            }
            HStack(spacing: 24) {
                TVReadout(value: "\(model.backupDepth)", label: "Backup")
                TVReadout(value: "\(model.guildsServed)", label: "Guilds")
                TVReadout(value: model.audioNodesHealthy ? "Healthy" : "Checking", label: "Voice links",
                          tint: model.audioNodesHealthy ? TVRes.live : TVRes.warn)
            }
        }
        .padding(36)
        .tvGlass(radius: 40)
        .focusable()
    }

    @ViewBuilder
    private var onAir: some View {
        let bots = model.musicBots.sorted { a, b in
            let rank: (TVBot) -> Int = { bot in
                switch bot.hiveState {
                case .playing: return 0
                case .paused: return 1
                case .idle: return 2
                case .offline: return 3
                }
            }
            if rank(a) != rank(b) { return rank(a) < rank(b) }
            return (BotPalette.fleetOrder.firstIndex(of: a.key) ?? 99) < (BotPalette.fleetOrder.firstIndex(of: b.key) ?? 99)
        }
        VStack(alignment: .leading, spacing: 24) {
            Text("ON AIR")
                .font(TVRes.eyebrow)
                .tracking(2)
                .foregroundStyle(account.accent)
            if bots.isEmpty {
                Text("No bots are serving your guild yet.")
                    .font(.title3)
                    .foregroundStyle(TVRes.mist)
            } else {
                ScrollView(.horizontal, showsIndicators: false) {
                    LazyHStack(spacing: 40) {
                        ForEach(bots) { bot in
                            Button { open(bot) } label: {
                                TVBotCard(bot: bot, accent: account.accent)
                            }
                            .buttonStyle(.card)
                        }
                    }
                    .padding(.vertical, 30)
                    .padding(.horizontal, 6)
                }
                .focusSection()
            }
        }
    }

    @ViewBuilder
    private var loadingState: some View {
        VStack(spacing: 30) {
            ResonanceEmblem(size: 220)
            if let error = model.errorMessage {
                Text(error).font(.title3).foregroundStyle(TVRes.mist)
            } else {
                Text("Tuning in to the fleet…").font(.title3).foregroundStyle(TVRes.mist)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 80)
    }

    private func open(_ bot: TVBot) {
        TVTelemetry.shared.log("bot_detail_opened", ["bot": bot.key, "offline": bot.isOffline ? "true" : "false"])
        guard !bot.isOrchestrator else { return }
        selectedBot = bot
    }
}

// MARK: - Cards

struct TVBotCard: View {
    let bot: TVBot
    let accent: Color

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var color: Color { BotPalette.color(for: bot.key) }

    var body: some View {
        let session = bot.featuredSession
        VStack(alignment: .leading, spacing: 20) {
            HStack(spacing: 16) {
                ZStack {
                    Hexagon().fill(LinearGradient(colors: [color, color.opacity(0.45)], startPoint: .top, endPoint: .bottom))
                    Text(BotPalette.monogram(for: bot.key, name: bot.displayName))
                        .font(.system(size: 22, weight: .heavy, design: .rounded))
                        .foregroundStyle(.white)
                }
                .frame(width: 52, height: 60)
                Text(bot.name)
                    .font(.system(size: 34, weight: .heavy, design: .rounded))
                    .lineLimit(1)
                Spacer()
                TVStateBadge(state: bot.hiveState)
            }
            HStack(spacing: 22) {
                TVThumbnail(url: session?.thumbnail)
                    .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(color.opacity(bot.hiveState == .playing ? 0.9 : 0.3), lineWidth: 3))
                VStack(alignment: .leading, spacing: 8) {
                    Text(session?.title ?? "Waiting for live playback.")
                        .font(.system(size: 28, weight: .bold, design: .rounded))
                        .lineLimit(2)
                    Text(session?.mediaSourceLabel ?? session?.sessionStateLabel ?? session?.guildName ?? "Idle")
                        .font(.system(size: 22, design: .rounded))
                        .foregroundStyle(TVRes.mist)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                if bot.hiveState == .playing {
                    EqualizerBars(isActive: true, color: color, barCount: 5,
                                  seed: bot.key.utf8.reduce(UInt64(3)) { $0 &* 31 &+ UInt64($1) }, animated: !reduceMotion)
                        .frame(width: 44, height: 40)
                }
            }
            if let session, (session.durationSeconds ?? 0) > 0 {
                TVPlaybackBar(session: session, accent: color)
            }
            HStack(spacing: 14) {
                TVChip(text: "\(bot.queueDepth ?? 0) queued")
                TVChip(text: "\(bot.knownGuildCount ?? 0) guilds")
                if let age = bot.heartbeatAgeSeconds { TVChip(text: "heartbeat \(Int(age))s") }
            }
        }
        .padding(32)
        .frame(width: 640, height: 400, alignment: .topLeading)
        .background(
            LinearGradient(colors: [color.opacity(bot.hiveState == .playing ? 0.28 : 0.1), Color.black.opacity(0.45)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
        )
        .opacity(bot.isOffline ? 0.6 : 1)
    }
}

/// Aria as the conductor: the swarm's orchestrator, with the Medic's repair
/// queue and process stats.
struct TVAriaCard: View {
    let bot: TVBot
    let accent: Color

    var body: some View {
        let medic = bot.medicSummary
        let color = BotPalette.color(for: "aria")
        VStack(alignment: .leading, spacing: 22) {
            HStack(spacing: 20) {
                ZStack {
                    Hexagon().fill(LinearGradient(colors: [color, color.opacity(0.4)], startPoint: .top, endPoint: .bottom))
                    Image(systemName: "gearshape.2.fill").font(.system(size: 30, weight: .semibold)).foregroundStyle(.white)
                }
                .frame(width: 70, height: 80)
                .shadow(color: color.opacity(0.6), radius: 18)
                VStack(alignment: .leading, spacing: 4) {
                    Text("THE CONDUCTOR")
                        .font(TVRes.eyebrow)
                        .tracking(2)
                        .foregroundStyle(color)
                    Text(bot.name).font(TVRes.display(40))
                }
                Spacer()
                TVStateBadge(state: bot.isOffline ? .offline : .playing)
            }
            HStack(spacing: 24) {
                TVReadout(value: "\(medic?.pendingRepairs ?? 0)", label: "Repairs", tint: (medic?.pendingRepairs ?? 0) > 0 ? TVRes.warn : TVRes.ink)
                TVReadout(value: "\(medic?.criticalHealth ?? 0)", label: "Critical", tint: (medic?.criticalHealth ?? 0) > 0 ? TVRes.danger : TVRes.ink)
                TVReadout(value: "\(medic?.recoverableHealth ?? 0)", label: "Recoverable")
            }
            HStack(spacing: 14) {
                if let uptime = bot.uptimeSeconds { TVChip(text: "up " + TVFormat.uptime(uptime)) }
                if let memory = bot.memoryKb { TVChip(text: "\(Int(memory / 1024)) MB mem") }
                TVChip(text: "\(medic?.pendingInfra ?? 0) infra tasks")
                TVChip(text: "\(bot.recentInteractionCount ?? 0) interactions")
            }
        }
        .padding(36)
        .tvGlass(radius: 40, edge: color)
        .focusable()
    }
}

struct TVBotDetailView: View {
    let bot: TVBot
    let accent: Color

    var body: some View {
        let color = BotPalette.color(for: bot.key)
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                HStack(spacing: 28) {
                    ZStack {
                        Hexagon().fill(LinearGradient(colors: [color, color.opacity(0.4)], startPoint: .top, endPoint: .bottom))
                        Text(BotPalette.monogram(for: bot.key, name: bot.displayName))
                            .font(.system(size: 40, weight: .heavy, design: .rounded))
                            .foregroundStyle(.white)
                    }
                    .frame(width: 110, height: 127)
                    .shadow(color: color.opacity(0.6), radius: 24)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("MUSIC BOT").font(TVRes.eyebrow).tracking(2).foregroundStyle(color)
                        Text(bot.name).font(TVRes.display(64))
                    }
                    Spacer()
                    TVStateBadge(state: bot.hiveState)
                }
                let sessions = bot.sessions ?? []
                if sessions.isEmpty {
                    Text("No sessions right now.").font(.title3).foregroundStyle(TVRes.mist)
                }
                ForEach(sessions) { session in
                    VStack(alignment: .leading, spacing: 16) {
                        HStack(spacing: 24) {
                            TVThumbnail(url: session.thumbnail)
                            VStack(alignment: .leading, spacing: 8) {
                                Text(session.title ?? "Nothing playing")
                                    .font(.system(size: 32, weight: .bold, design: .rounded))
                                    .lineLimit(2)
                                Text([session.guildName, session.channelName.map { "#\($0)" }].compactMap { $0 }.joined(separator: " · "))
                                    .font(.system(size: 24, design: .rounded)).foregroundStyle(TVRes.mist)
                                Text("\(session.sessionStateLabel ?? "") · \(session.queueCount ?? 0) queued")
                                    .font(.system(size: 24, design: .rounded)).foregroundStyle(TVRes.mist)
                            }
                        }
                        if (session.durationSeconds ?? 0) > 0 {
                            TVPlaybackBar(session: session, accent: color)
                        }
                    }
                    .padding(30)
                    .tvGlass(radius: 32, edge: session.isPlaying == true ? color : nil)
                    .focusable()
                }
            }
            .padding(70)
        }
        .background(
            ZStack {
                Color(red: 0.03, green: 0.05, blue: 0.08)
                RadialGradient(colors: [color.opacity(0.3), .clear], center: .topTrailing, startRadius: 20, endRadius: 900)
            }
            .ignoresSafeArea()
        )
    }
}

// MARK: - Small pieces

struct TVPlaybackBar: View {
    let session: DashboardSession
    let accent: Color

    var body: some View {
        // Advances locally once a second between server pushes, like the
        // web dashboard's playback counters.
        TimelineView(.periodic(from: .now, by: 1)) { context in
            let duration = Double(session.durationSeconds ?? 0)
            let position = TVFormat.position(of: session, at: context.date)
            VStack(alignment: .leading, spacing: 6) {
                GeometryReader { geo in
                    ZStack(alignment: .leading) {
                        Capsule().fill(Color.white.opacity(0.12))
                        Capsule().fill(LinearGradient(colors: [accent.opacity(0.7), accent], startPoint: .leading, endPoint: .trailing))
                            .shadow(color: accent.opacity(0.6), radius: 8)
                            .frame(width: duration > 0 ? geo.size.width * CGFloat(position / duration) : 0)
                    }
                }
                .frame(height: 8)
                HStack {
                    Text(TVFormat.duration(position))
                    Spacer()
                    Text(TVFormat.duration(duration))
                }
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }
        }
    }
}

struct TVThumbnail: View {
    let url: String?

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12).fill(Color.white.opacity(0.08))
            if let url, let parsed = URL(string: url) {
                AsyncImage(url: parsed) { phase in
                    if let image = phase.image {
                        image.resizable().aspectRatio(contentMode: .fill)
                    } else {
                        Image(systemName: "music.note").font(.title).foregroundStyle(.secondary)
                    }
                }
            } else {
                Image(systemName: "music.note").font(.title).foregroundStyle(.secondary)
            }
        }
        .frame(width: 192, height: 108)
        .clipShape(RoundedRectangle(cornerRadius: 14))
    }
}

struct TVAvatar: View {
    let url: String?
    let name: String
    let accent: Color
    let size: CGFloat

    var body: some View {
        ZStack {
            Circle().fill(accent.opacity(0.35))
            Text(initials).font(.system(size: size * 0.38, weight: .bold))
            if let url, let parsed = URL(string: url) {
                AsyncImage(url: parsed) { phase in
                    if let image = phase.image { image.resizable().aspectRatio(contentMode: .fill) }
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }

    private var initials: String {
        let parts = name.split(separator: " ").prefix(2)
        let letters = parts.compactMap { $0.first }.map(String.init).joined()
        return letters.isEmpty ? "SP" : letters.uppercased()
    }
}

struct TVChip: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 22, weight: .semibold, design: .rounded))
            .foregroundStyle(TVRes.mist)
            .padding(.horizontal, 16)
            .padding(.vertical, 7)
            .background(Color.white.opacity(0.08), in: Capsule())
            .overlay(Capsule().strokeBorder(TVRes.hairline, lineWidth: 1))
    }
}
