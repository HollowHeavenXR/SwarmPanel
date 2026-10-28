import SwiftUI

private let relativeTimeFormatter: RelativeDateTimeFormatter = {
    let formatter = RelativeDateTimeFormatter()
    formatter.unitsStyle = .abbreviated
    return formatter
}()

struct DashboardView: View {
    @EnvironmentObject private var appState: AppState
    @EnvironmentObject private var notificationsViewModel: NotificationsViewModel
    @EnvironmentObject private var toastCenter: ToastCenter
    @StateObject private var viewModel = DashboardViewModel()
    @StateObject private var recentBots = RecentBotsStore()
    @StateObject private var pinnedBots = PinnedBotsStore()
    @ObservedObject private var live = SwarmLiveSocket.shared
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.resAccent) private var accent
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// Where a tap on a hive cell goes.
    @State private var hiveRoute: HiveRoute?

    private var allBots: [DashboardBot] { viewModel.response?.bots ?? [] }
    private var musicBots: [DashboardBot] { allBots.filter { $0.kind != "orchestrator" } }
    private var allSessions: [DashboardSession] {
        allBots.flatMap { $0.sessions ?? [] }
    }
    /// Pairs each session with its owning bot and keys identity on
    /// `bot.key`, not just `session.id` (guildId+channelId) — two different
    /// bots can report a session for the same guild/channel (the whole
    /// point of a "swarm" of bots covering overlapping guilds), which would
    /// otherwise give ForEach/NavigationLink duplicate identifiers and crash
    /// SwiftUI's diffing on the very next tap anywhere on this screen.
    private var allBotSessions: [BotSession] {
        allBots.flatMap { bot in (bot.sessions ?? []).map { BotSession(bot: bot, session: $0) } }
    }
    private var ownBotKey: String? {
        guard let guildId = appState.guildId else { return nil }
        return allBots.first { bot in (bot.sessions ?? []).contains { $0.guildId == guildId } }?.key
    }
    private var ownSession: DashboardSession? {
        guard let guildId = appState.guildId else { return nil }
        return allSessions.first { $0.guildId == guildId }
    }
    private var liveCount: Int { allSessions.filter { $0.isPlaying == true && $0.isPaused != true }.count }
    private var queuedCount: Int { allSessions.reduce(0) { $0 + ($1.queueCount ?? 0) } }
    private var onlineCount: Int { musicBots.filter { hiveState(for: $0) != .offline }.count }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    header

                    if viewModel.isLoading && viewModel.response == nil {
                        loadingState
                    } else {
                        if let error = viewModel.errorMessage {
                            ErrorBanner(message: error).padding(.horizontal)
                        }
                        signalStrip
                        hivePanel
                        ownGuildSection
                        shortcutsSection
                        sessionsSection
                    }
                }
                .padding(.top, 4)
                .padding(.bottom, 24)
                .dockClearance()
            }
            .resonanceScreen()
            .refreshable {
                Haptics.light()
                await viewModel.refresh()
            }
            .refreshOnForeground { await viewModel.refresh() }
            .navigationTitle("Fleet")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                // Invites belongs to the Fleet section (web: /invites).
                ToolbarItem(placement: .navigationBarLeading) {
                    NavigationLink { InvitesView() } label: {
                        Image(systemName: "envelope.badge.person.crop")
                    }
                    .accessibilityLabel("Invite Bots")
                }
                if !allBots.isEmpty {
                    ToolbarItem(placement: .navigationBarTrailing) {
                        ShareLink(item: fleetStatusShareText) {
                            Image(systemName: "square.and.arrow.up")
                        }
                        .accessibilityLabel("Share fleet status")
                    }
                }
            }
            .notificationsBell(notificationsViewModel)
            .navigationDestination(isPresented: Binding(get: { hiveRoute != nil }, set: { if !$0 { hiveRoute = nil } })) {
                if let hiveRoute {
                    BotDetailView(botKey: hiveRoute.botKey, botDisplayName: hiveRoute.name, guildId: hiveRoute.guildId)
                }
            }
            .environmentObject(recentBots)
            .environmentObject(pinnedBots)
        }
        // BUGFIX (kept from the TabView era): viewModel.start() is
        // idempotent, so calling it on every reappearance is harmless; only
        // a real background transition tears the live connection down.
        .onAppear { viewModel.start() }
        .onChange(of: scenePhase) { newPhase in
            switch newPhase {
            case .active: viewModel.start()
            case .background: viewModel.stop()
            default: break
            }
        }
        // Uses the emitted value directly rather than re-reading
        // viewModel.response — @Published's publisher fires from willSet,
        // so the backing property isn't guaranteed updated yet at the point
        // this closure runs.
        .onReceive(viewModel.$response) { newResponse in
            guard let newResponse else { return }
            WidgetDataService.shared.update(dashboard: newResponse, guildId: appState.guildId)
        }
    }

    // MARK: Sections

    private var header: some View {
        ResScreenHeader(eyebrow: greeting, title: "Fleet", subtitle: subtitleLine) {
            ConnectionBeacon(isLive: live.isConnected, lastUpdatedAt: viewModel.lastUpdatedAt)
        }
    }

    /// "Good evening, Jamie" -- the eyebrow knows what time it is.
    private var greeting: String {
        let hour = Calendar.current.component(.hour, from: Date())
        let part: String
        switch hour {
        case 5..<12: part = "Good morning"
        case 12..<17: part = "Good afternoon"
        case 17..<22: part = "Good evening"
        default: part = "Late session"
        }
        return appState.username.isEmpty ? part : "\(part), \(appState.username)"
    }

    private var subtitleLine: String? {
        guard viewModel.response != nil else { return nil }
        if liveCount == 0 { return "The hive is quiet. \(musicBots.count) bots standing by." }
        return "\(liveCount) \(liveCount == 1 ? "bot is" : "bots are") on air right now."
    }

    private var signalStrip: some View {
        HStack(alignment: .center, spacing: 0) {
            ResReadout(value: "\(onlineCount)/\(musicBots.count)", label: "Online", tint: Res.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
            ResReadout(value: "\(liveCount)", label: "On air", tint: liveCount > 0 ? Res.live : Res.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
            ResReadout(value: "\(queuedCount)", label: "Queued", tint: accent)
                .frame(maxWidth: .infinity, alignment: .leading)
            EqualizerBars(isActive: liveCount > 0, color: liveCount > 0 ? Res.live : Res.mist, barCount: 5, seed: 11, animated: !reduceMotion)
                .frame(width: 34, height: 30)
        }
        .padding(18)
        .resGlass(radius: Res.Radius.card)
        .padding(.horizontal)
    }

    private var hiveBots: [HiveBot] {
        allBots.map { bot in
            HiveBot(
                key: bot.key,
                name: bot.displayName?.isEmpty == false ? bot.displayName! : bot.key.capitalized,
                state: hiveState(for: bot),
                detail: hiveDetail(for: bot),
                isOrchestrator: bot.kind == "orchestrator"
            )
        }
    }

    private var hivePanel: some View {
        VStack(alignment: .leading, spacing: 14) {
            ResSectionHeader(eyebrow: "The hive", title: "Every bot, live") {
                Text("Tap a cell")
                    .font(.caption)
                    .foregroundStyle(Res.mist)
            }
            .padding(.horizontal, -20)
            if allBots.isEmpty {
                EmptyStateView(icon: "hexagon", title: "No bots are reporting yet.")
            } else {
                HiveMap(bots: hiveBots, cellWidth: 70) { bot in openHive(bot) }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 6)
                HiveLegend(bots: hiveBots)
            }
        }
        .padding(18)
        .resGlass(radius: Res.Radius.panel)
        .padding(.horizontal)
    }

    @ViewBuilder
    private var ownGuildSection: some View {
        if let ownSession, let ownBotKey {
            VStack(alignment: .leading, spacing: 12) {
                ResSectionHeader(eyebrow: "Your guild", title: ownSession.guildName ?? "Now playing")
                Group {
                    NowPlayingQuickControl(session: ownSession, botKey: ownBotKey)
                    if let topTrack = viewModel.topTrack {
                        TopTrackTeaser(track: topTrack)
                    }
                }
                .padding(.horizontal)
            }
            .task(id: "\(ownBotKey):\(ownSession.guildId ?? "")") {
                await viewModel.loadTopTrack(botKey: ownBotKey, guildId: ownSession.guildId ?? "")
            }
        }
    }

    @ViewBuilder
    private var shortcutsSection: some View {
        if !pinnedBots.pinned.isEmpty || !recentBots.visits.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                ResSectionHeader(eyebrow: "Jump back in", title: pinnedBots.pinned.isEmpty ? "Recently viewed" : "Pinned & recent")
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 10) {
                        ForEach(pinnedBots.pinned) { pin in
                            NavigationLink {
                                BotDetailView(botKey: pin.botKey, botDisplayName: pin.displayName, guildId: pin.guildId)
                            } label: {
                                BotPill(botKey: pin.botKey, title: pin.displayName, systemImage: "pin.fill")
                            }
                            .buttonStyle(ResPressStyle())
                            .contextMenu {
                                Button(role: .destructive) {
                                    pinnedBots.toggle(botKey: pin.botKey, guildId: pin.guildId, displayName: pin.displayName)
                                } label: {
                                    Label("Unpin", systemImage: "pin.slash")
                                }
                            }
                        }
                        ForEach(recentBots.visits) { visit in
                            NavigationLink {
                                BotDetailView(botKey: visit.botKey, botDisplayName: visit.displayName, guildId: visit.guildId)
                            } label: {
                                BotPill(botKey: visit.botKey, title: visit.displayName, systemImage: "clock.arrow.circlepath")
                            }
                            .buttonStyle(ResPressStyle())
                        }
                    }
                    .padding(.horizontal)
                }
            }
        }
    }

    private var sessionsSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            ResSectionHeader(eyebrow: "On air", title: "Live sessions") {
                Text("\(allSessions.count)")
                    .font(Res.readout(15))
                    .foregroundStyle(Res.mist)
            }
            if allBotSessions.isEmpty {
                EmptyStateView(icon: "waveform.slash", title: "No active sessions right now.")
                    .resGlass()
                    .padding(.horizontal)
            } else {
                LazyVStack(spacing: 12) {
                    ForEach(allBotSessions) { entry in
                        NavigationLink {
                            BotDetailView(
                                botKey: entry.bot.key,
                                botDisplayName: entry.bot.displayName?.isEmpty == false ? entry.bot.displayName! : entry.bot.key,
                                guildId: entry.session.guildId ?? ""
                            )
                        } label: {
                            SessionCard(bot: entry.bot, session: entry.session, animated: !reduceMotion)
                        }
                        .buttonStyle(ResPressStyle(scale: 0.98))
                        .contextMenu {
                            sessionQuickActions(bot: entry.bot, session: entry.session)
                        }
                    }
                }
                .padding(.horizontal)
            }
        }
    }

    private var loadingState: some View {
        VStack(alignment: .leading, spacing: 16) {
            SkeletonCard(lines: 1).padding(.horizontal)
            ResonanceEmblem(size: 150)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 24)
                .resGlass(radius: Res.Radius.panel)
                .padding(.horizontal)
            SkeletonList(rowCount: 3).padding(.horizontal)
        }
    }

    // MARK: Hive helpers

    private func hiveState(for bot: DashboardBot) -> HiveCellState {
        let status = (bot.status ?? "").lowercased()
        if status == "offline" || (bot.heartbeatStatus ?? "").lowercased().contains("offline") { return .offline }
        let sessions = bot.sessions ?? []
        if sessions.contains(where: { $0.isPlaying == true && $0.isPaused != true }) || (bot.activePlayingCount ?? 0) > 0 { return .playing }
        if sessions.contains(where: { $0.isPaused == true }) { return .paused }
        return .idle
    }

    private func hiveDetail(for bot: DashboardBot) -> String? {
        let sessions = bot.sessions ?? []
        if let playing = sessions.first(where: { $0.isPlaying == true }), let title = playing.title, !title.isEmpty {
            return title
        }
        let queued = bot.queueDepth ?? 0
        return queued > 0 ? "\(queued) queued" : nil
    }

    private func openHive(_ cell: HiveBot) {
        guard !cell.isOrchestrator, let bot = allBots.first(where: { $0.key == cell.key }) else {
            toastCenter.success("Aria conducts the swarm. Open a music bot to see its deck.")
            return
        }
        let sessions = bot.sessions ?? []
        let guildId = sessions.first(where: { $0.isPlaying == true })?.guildId
            ?? sessions.first?.guildId
            ?? appState.guildId
            ?? ""
        hiveRoute = HiveRoute(botKey: bot.key, name: cell.name, guildId: guildId)
    }

    private var fleetStatusShareText: String {
        let liveCount = allSessions.filter { $0.isPlaying == true }.count
        let queued = allSessions.reduce(0) { $0 + ($1.queueCount ?? 0) }
        var lines = ["SwarmPanel Fleet Status", "\(allBots.count) bots · \(liveCount) live · \(queued) queued"]
        for session in allSessions where session.isPlaying == true {
            let name = session.channelName ?? session.guildName ?? "Guild \(session.guildId ?? "?")"
            lines.append("• \(name): \(session.title?.isEmpty == false ? session.title! : "Untitled")")
        }
        return lines.joined(separator: "\n")
    }

    @ViewBuilder
    private func sessionQuickActions(bot: DashboardBot, session: DashboardSession) -> some View {
        let isPlaying = session.isPlaying == true && session.isPaused != true
        Button {
            Task { await sendQuickAction(isPlaying ? "PAUSE" : "RESUME", bot: bot, session: session) }
        } label: {
            Label(isPlaying ? "Pause" : "Resume", systemImage: isPlaying ? "pause.fill" : "play.fill")
        }
        Button {
            Task { await sendQuickAction("SKIP", bot: bot, session: session) }
        } label: {
            Label("Skip", systemImage: "forward.fill")
        }
    }

    private func sendQuickAction(_ action: String, bot: DashboardBot, session: DashboardSession) async {
        guard let guildId = session.guildId else { return }
        do {
            _ = try await Core.shared.control(botKey: bot.key, guildId: guildId, action: action, payload: [:])
            Haptics.success()
            toastCenter.success("Sent \(action.capitalized)")
            await viewModel.refresh()
        } catch {
            if !error.isCancellation {
                Haptics.error()
                toastCenter.failure("Action failed")
            }
        }
    }
}

/// A session paired with the bot that reported it. `DashboardSession.id`
/// alone (guildId+channelId) isn't guaranteed unique across bots — two bots
/// covering the same guild would collide — so identity here is keyed on
/// `bot.key` too.
private struct BotSession: Identifiable {
    let bot: DashboardBot
    let session: DashboardSession
    var id: String { "\(bot.key):\(session.id)" }
}

private struct HiveRoute: Equatable {
    let botKey: String
    let name: String
    let guildId: String
}

/// One live session as a card: artwork ringed in the bot's colour, the
/// track, where it's playing, queue depth and a status badge.
private struct SessionCard: View {
    let bot: DashboardBot
    let session: DashboardSession
    let animated: Bool

    private var state: HiveCellState {
        if session.isPlaying == true && session.isPaused != true { return .playing }
        if session.isPaused == true { return .paused }
        return .idle
    }

    private var botColor: Color { BotPalette.color(for: bot.key) }
    private var botName: String { bot.displayName?.isEmpty == false ? bot.displayName! : bot.key.capitalized }

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            thumbnail
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Circle().fill(botColor).frame(width: 7, height: 7)
                    Text(botName.uppercased())
                        .font(Res.eyebrow)
                        .tracking(1)
                        .foregroundStyle(botColor)
                }
                Text(session.title?.isEmpty == false ? session.title! : "No title")
                    .font(.system(.subheadline, design: .rounded).weight(.bold))
                    .foregroundStyle(Res.ink)
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                Text(session.channelName.map { "#\($0)" } ?? session.guildName ?? "Guild \(session.guildId ?? "?")")
                    .font(.caption)
                    .foregroundStyle(Res.mist)
                    .lineLimit(1)
                HStack(spacing: 12) {
                    Label("\(session.queueCount ?? 0)", systemImage: "music.note.list")
                    Label("\(session.backupQueueCount ?? 0)", systemImage: "arrow.triangle.2.circlepath")
                }
                .font(.system(.caption2, design: .rounded).monospacedDigit())
                .foregroundStyle(Res.mist)
            }
            Spacer(minLength: 4)
            VStack(alignment: .trailing, spacing: 10) {
                ResStatusBadge(state: state)
                EqualizerBars(isActive: state == .playing, color: botColor, barCount: 4,
                              seed: bot.key.utf8.reduce(UInt64(5)) { $0 &* 31 &+ UInt64($1) }, animated: animated)
                    .frame(width: 20, height: 16)
            }
        }
        .padding(14)
        .resGlass(radius: Res.Radius.card, edge: state == .playing ? botColor : nil)
    }

    @ViewBuilder
    private var thumbnail: some View {
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        Group {
            if let thumbnail = session.thumbnail, let url = URL(string: thumbnail) {
                AsyncImage(url: url) { phase in
                    switch phase {
                    case .success(let image):
                        image.resizable().aspectRatio(contentMode: .fill)
                    default:
                        placeholder
                    }
                }
            } else {
                placeholder
            }
        }
        .frame(width: 58, height: 58)
        .clipShape(shape)
        .overlay(shape.strokeBorder(botColor.opacity(state == .playing ? 0.85 : 0.3), lineWidth: 1.5))
    }

    private var placeholder: some View {
        ZStack {
            LinearGradient(colors: [botColor.opacity(0.45), botColor.opacity(0.1)], startPoint: .topLeading, endPoint: .bottomTrailing)
            Text(BotPalette.monogram(for: bot.key, name: bot.displayName))
                .font(.system(size: 18, weight: .heavy, design: .rounded))
                .foregroundStyle(.white)
        }
    }
}

/// A chip for one bot (pinned or recently viewed), dotted in its colour.
private struct BotPill: View {
    let botKey: String
    let title: String
    let systemImage: String

    var body: some View {
        HStack(spacing: 7) {
            Hexagon()
                .fill(BotPalette.color(for: botKey))
                .frame(width: 12, height: 14)
            Text(title)
                .font(.system(.footnote, design: .rounded).weight(.semibold))
                .foregroundStyle(Res.ink)
            Image(systemName: systemImage)
                .font(.caption2)
                .foregroundStyle(Res.mist)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .background(Capsule().fill(.ultraThinMaterial).overlay(Capsule().fill(Res.surface)))
        .overlay(Capsule().strokeBorder(BotPalette.color(for: botKey).opacity(0.4), lineWidth: 1))
    }
}

/// Counts under the hive: how many bots are live, paused, idle, offline.
private struct HiveLegend: View {
    let bots: [HiveBot]

    var body: some View {
        HStack(spacing: 14) {
            entry(.playing, "Live", Res.live)
            entry(.paused, "Paused", Res.warn)
            entry(.idle, "Idle", Res.mist)
            entry(.offline, "Offline", Res.danger)
        }
        .font(.system(.caption, design: .rounded).weight(.semibold))
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private func entry(_ state: HiveCellState, _ label: String, _ color: Color) -> some View {
        let count = bots.filter { $0.state == state && !$0.isOrchestrator }.count
        if count > 0 || state == .playing {
            HStack(spacing: 5) {
                Circle().fill(color).frame(width: 7, height: 7)
                Text("\(count) \(label)").foregroundStyle(Res.mist)
            }
        }
    }
}

/// Live-connection beacon in the Fleet header: green rings while the
/// socket is pushing updates, amber "reconnecting" otherwise.
private struct ConnectionBeacon: View {
    let isLive: Bool
    let lastUpdatedAt: Date?

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            HStack(spacing: 6) {
                if isLive {
                    SwarmPulseRings(color: Res.live, ringCount: 2).frame(width: 12, height: 12)
                } else {
                    Circle().fill(Res.warn).frame(width: 8, height: 8)
                }
                Text(isLive ? "LIVE" : "SYNCING")
                    .font(Res.eyebrow)
                    .tracking(1.2)
                    .foregroundStyle(isLive ? Res.live : Res.warn)
            }
            if let lastUpdatedAt {
                TimelineView(.periodic(from: .now, by: 30)) { timeline in
                    Text(relativeTimeFormatter.localizedString(for: lastUpdatedAt, relativeTo: timeline.date))
                        .font(.caption2)
                        .foregroundStyle(Res.mist)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

private struct NowPlayingQuickControl: View {
    let session: DashboardSession
    let botKey: String
    @State private var isBusy = false

    private var isCurrentlyPlaying: Bool {
        session.isPlaying == true && session.isPaused != true
    }

    var body: some View {
        NowPlayingCard(
            title: session.title ?? "",
            subtitle: session.channelName ?? session.guildName,
            thumbnailURL: session.thumbnail,
            isPlaying: session.isPlaying ?? false,
            isPaused: session.isPaused ?? false,
            positionSeconds: session.positionSeconds ?? 0,
            durationSeconds: session.durationSeconds ?? 0,
            positionObservedAt: session.positionObservedAt,
            mediaSourceLabel: session.mediaSourceLabel,
            cached: session.cached,
            isBusy: isBusy,
            botKey: botKey,
            onPause: isCurrentlyPlaying ? { Task { await send("PAUSE") } } : nil,
            onResume: !isCurrentlyPlaying ? { Task { await send("RESUME") } } : nil,
            onSkip: { Task { await send("SKIP") } },
            onSeek: { seconds in Task { await seek(Int(seconds)) } }
        )
    }

    private func send(_ action: String, payload: [String: String] = [:]) async {
        guard let guildId = session.guildId else { return }
        isBusy = true
        Haptics.light()
        defer { isBusy = false }
        do {
            _ = try await Core.shared.control(botKey: botKey, guildId: guildId, action: action, payload: payload)
        } catch {
            // Best-effort — the Controls screen surfaces errors properly;
            // a failed quick-action here just leaves the button re-enabled.
            if !error.isCancellation { Haptics.error() }
        }
    }

    private func seek(_ seconds: Int) async {
        guard let guildId = session.guildId else { return }
        isBusy = true
        Haptics.light()
        defer { isBusy = false }
        do {
            _ = try await Core.shared.seek(botKey: botKey, guildId: guildId, positionSeconds: seconds)
        } catch {
            if !error.isCancellation { Haptics.error() }
        }
    }
}

private struct TopTrackTeaser: View {
    let track: LeaderboardTrack

    @Environment(\.resAccent) private var accent

    var body: some View {
        NavigationLink {
            LeaderboardView()
        } label: {
            HStack(spacing: 12) {
                Image(systemName: "trophy.fill")
                    .font(.subheadline)
                    .foregroundStyle(BotPalette.color(for: "dazzle"))
                    .frame(width: 34, height: 34)
                    .background(BotPalette.color(for: "dazzle").opacity(0.16), in: Circle())

                VStack(alignment: .leading, spacing: 2) {
                    Text("TOP TRACK HERE")
                        .font(Res.eyebrow)
                        .tracking(1)
                        .foregroundStyle(Res.mist)
                    Text(track.title?.isEmpty == false ? track.title! : "Unknown title")
                        .font(.system(.subheadline, design: .rounded).weight(.bold))
                        .foregroundStyle(Res.ink)
                        .lineLimit(1)
                }

                Spacer()
                Label("\(track.playCount ?? 0)", systemImage: "play.fill")
                    .font(.system(.caption, design: .rounded).monospacedDigit())
                    .foregroundStyle(Res.mist)
            }
            .padding(14)
            .resGlass()
        }
        .buttonStyle(ResPressStyle(scale: 0.98))
        .contextMenu {
            ShareLink(item: "🏆 Top track: \(track.title?.isEmpty == false ? track.title! : "Unknown title") — \(track.playCount ?? 0) plays") {
                Label("Share", systemImage: "square.and.arrow.up")
            }
        }
    }
}

#Preview {
    DashboardView()
        .environmentObject(AppState())
        .environmentObject(NotificationsViewModel())
        .environmentObject(ToastCenter())
}
