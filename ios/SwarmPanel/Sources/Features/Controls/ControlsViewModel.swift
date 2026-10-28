import Foundation

/// Controls screen state. Payload building/validation, the command preview,
/// guild picking and conversion summaries come from the Rust core.
@MainActor
final class ControlsViewModel: ObservableObject {
    @Published var bots: [BotSummary] = []
    @Published var guilds: [InventoryGuild] = []
    @Published var savedQueues: [SavedQueue] = []
    @Published var controlState: ControlStateSession?
    @Published var selectedBotKey: String = ""
    @Published var guildId: String = ""
    @Published var voiceChannelId: String = ""
    @Published var action: ControlAction = .play
    @Published var sourceURL: String = ""
    @Published var loopMode: String = "queue"
    @Published var filterMode: String = "none"
    @Published var guildOverview: [ControlMatrixBot] = []
    @Published var guildOverviewGuildId: String?
    @Published var isLoadingOverview = false
    @Published var isConverting = false
    @Published var isLoadingBots = true
    @Published var isSending = false
    @Published var errorMessage: String?
    @Published var statusMessage: String?

    private let core = Core.shared
    private let socket = SwarmLiveSocket.shared

    var voiceChannels: [InventoryChannel] {
        guilds.first(where: { $0.id == guildId })?.channels?.filter(\.isVoice) ?? []
    }

    var commandPreviewSummary: String {
        let botLabel = bots.first(where: { $0.key == selectedBotKey })?.label ?? selectedBotKey
        let guild = guilds.first(where: { $0.id == guildId })
        let guildLabel = guild?.name?.isEmpty == false ? guild!.name! : "guild \(guildId)"
        return commandPreview(botLabel: botLabel, guildLabel: guildLabel, action: action.rawValue,
                              sourceUrl: sourceURL, loopMode: loopMode, filterMode: filterMode)
    }

    func loadBots(defaultGuildId: String?) async {
        isLoadingBots = true
        defer { isLoadingBots = false }
        do {
            bots = (try await core.bots().bots ?? []).filter { $0.kind == "music" }
            if selectedBotKey.isEmpty { selectedBotKey = bots.first?.id ?? "" }
            if guildId.isEmpty, let defaultGuildId, !defaultGuildId.isEmpty { guildId = defaultGuildId }
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load bots.")
        }
    }

    func botDidChange() async {
        if !selectedBotKey.isEmpty, let dashboard = try? await core.dashboard(),
           let guild = pickGuildForBot(dashboard: dashboard, botKey: selectedBotKey) {
            guildId = guild
            voiceChannelId = ""
        }
        await loadInventory()
        await loadControlStateAndQueues()
    }

    func loadInventory() async {
        guard !selectedBotKey.isEmpty else { guilds = []; return }
        do {
            guilds = try await core.botInventory(botKey: selectedBotKey).guilds ?? []
            if let matched = guilds.first(where: { $0.id == guildId }), let channelsError = matched.channelsError {
                errorMessage = "Couldn't load channels: \(channelsError). Enter the voice channel ID manually below."
            } else if !guildId.isEmpty, !guilds.contains(where: { $0.id == guildId }) {
                errorMessage = "Couldn't confirm this bot's channel list for this guild — enter the voice channel ID manually below."
            }
        } catch {
            guard !error.isCancellation else { return }
            guilds = []
            errorMessage = error.message("Failed to load bot inventory.")
        }
    }

    private func watchLiveControlState() {
        guard !selectedBotKey.isEmpty, !guildId.isEmpty else {
            socket.unwatch("control_state")
            return
        }
        socket.watch("control_state", params: ["bot_key": selectedBotKey, "guild_id": guildId], as: ControlStateResponse.self) { [weak self] result in
            guard let self, case .success(let state) = result else { return }
            self.controlState = state.session
        }
        socket.connect()
    }

    func stopWatchingControlState() {
        socket.unwatch("control_state")
    }

    func loadControlStateAndQueues() async {
        guard !selectedBotKey.isEmpty, !guildId.isEmpty else { return }
        watchLiveControlState()
        do {
            let session = try await core.controlState(botKey: selectedBotKey, guildId: guildId).session
            controlState = session
            errorMessage = nil
            if voiceChannelId.isEmpty, let home = session?.homeChannelId ?? session?.channelId, !home.isEmpty {
                voiceChannelId = home
            }
            if let loop = session?.loopMode, !loop.isEmpty { loopMode = loop }
            if let filter = session?.filterMode, !filter.isEmpty { filterMode = filter }
        } catch {
            controlState = nil
            if !error.isCancellation { errorMessage = error.message("Failed to load current session.") }
        }
        savedQueues = (try? await core.savedQueues(guildId: guildId, botKey: selectedBotKey).queues) ?? []
    }

    func sendAction() async {
        guard !selectedBotKey.isEmpty, !guildId.isEmpty else { return }
        statusMessage = nil
        errorMessage = nil
        do {
            let payload = try controlPayload(action: action.rawValue, sourceUrl: sourceURL, voiceChannelId: voiceChannelId,
                                             loopMode: loopMode, filterMode: filterMode)
            isSending = true
            defer { isSending = false }
            _ = try await core.control(botKey: selectedBotKey, guildId: guildId, action: action.rawValue, payload: payload)
            statusMessage = "\(action.label) sent."
            Haptics.success()
            await loadControlStateAndQueues()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Control action failed.")
            Haptics.error()
        }
    }

    func saveCurrentQueue(name: String) async {
        do {
            _ = try await core.saveQueue(guildId: guildId, botKey: selectedBotKey, name: name, items: controlState?.queuePreview ?? [])
            statusMessage = "Queue saved."
            Haptics.success()
            await loadControlStateAndQueues()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to save queue.")
            Haptics.error()
        }
    }

    func loadSavedQueue(_ queue: SavedQueue) async {
        isSending = true
        defer { isSending = false }
        do {
            let outcome = try await core.loadSavedQueue(botKey: selectedBotKey, guildId: guildId, voiceChannelId: voiceChannelId, queue: queue)
            statusMessage = outcome.message
            Haptics.success()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load queue.")
        }
    }

    func deleteSavedQueue(_ queue: SavedQueue) async {
        do {
            _ = try await core.deleteSavedQueue(queueId: queue.id, guildId: guildId)
            await loadControlStateAndQueues()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to delete queue.")
        }
    }

    func renameSavedQueue(_ queue: SavedQueue, to newName: String) async {
        do {
            _ = try await core.renameSavedQueue(queueId: queue.id, guildId: guildId, name: newName)
            await loadControlStateAndQueues()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to rename queue.")
        }
    }

    func loadGuildOverview() async {
        guard !guildId.isEmpty else {
            errorMessage = "Choose a guild before loading the overview."
            return
        }
        let target = guildId
        isLoadingOverview = true
        defer { isLoadingOverview = false }
        do {
            guildOverview = try await core.controlMatrix(guildId: target).bots ?? []
            guildOverviewGuildId = target
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Failed to load the guild overview.")
        }
    }

    func sendOverviewAction(_ action: String, to bot: ControlMatrixBot) async {
        guard let target = guildOverviewGuildId, !bot.resolvedKey.isEmpty else { return }
        do {
            _ = try await core.control(botKey: bot.resolvedKey, guildId: target, action: action, payload: [:])
            statusMessage = "\(action.capitalized) sent to \(bot.label)."
            Haptics.success()
            await loadGuildOverview()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Control action failed.")
            Haptics.error()
        }
    }

    func convertChannels(toStage: Bool) async {
        isConverting = true
        statusMessage = nil
        errorMessage = nil
        defer { isConverting = false }
        do {
            let outcome = try await core.convertChannels(guildId: guildId, toStage: toStage)
            if outcome.ok {
                statusMessage = outcome.message
                Haptics.success()
            } else {
                errorMessage = outcome.message
                Haptics.error()
            }
            await loadControlStateAndQueues()
        } catch {
            guard !error.isCancellation else { return }
            errorMessage = error.message("Channel conversion failed.")
            Haptics.error()
        }
    }
}
