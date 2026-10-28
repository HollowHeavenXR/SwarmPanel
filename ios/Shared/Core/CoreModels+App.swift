import Foundation

// SwiftUI conveniences on the Rust core's generated model types: Identifiable
// conformances and the small display helpers the views use. Anything with
// real logic calls into the core (youtubeThumbnail, auditDiffPairs, ...).

// MARK: - Errors

typealias APIError = ApiError
typealias OKResponse = OkResponse

extension Error {
    /// Task / URL cancellation is a normal lifecycle event, never shown to the user.
    var isCancellation: Bool {
        if self is CancellationError { return true }
        let ns = self as NSError
        return ns.domain == NSURLErrorDomain && ns.code == NSURLErrorCancelled
    }

    /// Telemetry label ("server_500", "network_-1009", "decoding", ...).
    var telemetryKind: String {
        if let api = self as? ApiError { return apiErrorKind(error: api) }
        let ns = self as NSError
        return "\(ns.domain)_\(ns.code)"
    }

    /// Text for an error banner. (UniFFI's own LocalizedError conformance
    /// on ApiError returns a debug dump, so core errors get their message
    /// from the core instead.)
    func message(_ fallback: String) -> String {
        if let api = self as? ApiError { return apiErrorMessage(error: api) }
        return (self as? LocalizedError)?.errorDescription ?? fallback
    }
}

// MARK: - Fixed option lists (defined in the core)

let loopModes = allLoopModes()
let filterModes = allFilterModes()
let alertRuleTypes = allAlertRuleTypes()
let galleryModerationStatuses = allGalleryModerationStatuses()
let galleryReportStatuses = allGalleryReportStatuses()

typealias ControlAction = ControlActionInfo

extension ControlActionInfo: Identifiable {
    public var id: String { rawValue }
    static var allCases: [ControlActionInfo] { controlActions() }
    static func named(_ raw: String) -> ControlActionInfo {
        allCases.first { $0.rawValue == raw } ?? ControlActionInfo(rawValue: raw, label: raw.capitalized, needsVoiceChannel: false, needsSourceUrl: false, needsLoopMode: false, needsFilterMode: false)
    }
    static var play: ControlActionInfo { named("PLAY") }
    static var smartRecommend: ControlActionInfo { named("SMART_RECOMMEND") }
    static var pause: ControlActionInfo { named("PAUSE") }
    static var resume: ControlActionInfo { named("RESUME") }
    static var skip: ControlActionInfo { named("SKIP") }
    static var loop: ControlActionInfo { named("LOOP") }
    static var filter: ControlActionInfo { named("FILTER") }
    static var setHome: ControlActionInfo { named("SET_HOME") }
    var needsSourceURL: Bool { needsSourceUrl }
}

// MARK: - Fleet

extension DashboardBot: Identifiable {
    public var id: String { key }
    var name: String { displayName ?? key }
    var label: String { displayName?.isEmpty == false ? displayName! : key }
    var isOrchestrator: Bool { kind == "orchestrator" }
    var isOffline: Bool { botIsOffline(bot: self) }
    var featuredSession: DashboardSession? { botFeaturedSession(bot: self) }
}

extension DashboardSession: Identifiable {
    public var id: String { (guildId ?? "g") + ":" + (channelId ?? "c") }

    /// Live position (seconds), advancing while playing.
    func position(at now: Date) -> Double {
        sessionPosition(positionSeconds: positionSeconds, durationSeconds: durationSeconds, isPlaying: isPlaying,
                        positionObservedAt: positionObservedAt, nowUnix: now.timeIntervalSince1970)
    }
}

extension InviteBot: Identifiable {
    public var id: String { key }
    var label: String { displayName?.isEmpty == false ? displayName! : key }
}

extension BotSummary: Identifiable {
    public var id: String { key }
    var label: String { displayName?.isEmpty == false ? displayName! : key }
}

extension InventoryGuild: Identifiable {}

extension InventoryChannel: Identifiable {
    var type: Int? { channelType }
    var isVoice: Bool { isVoiceChannel(channelType: channelType) }
}

extension ControlStateSession {
    var derivedThumbnailURL: String? { videoUrl.flatMap { youtubeThumbnail(videoUrl: $0) } }
}

extension SavedQueue: Identifiable {
    var itemCount: Int { items?.count ?? 0 }
}

extension ControlMatrixBot: Identifiable {
    public var id: String { resolvedKey.isEmpty ? label : resolvedKey }
    var resolvedKey: String { key ?? botKey ?? session?.botKey ?? "" }
    var label: String { displayName ?? botDisplay ?? session?.botDisplay ?? resolvedKey }
    var isActive: Bool { session?.isPlaying == true || session?.isPaused == true }
}

// MARK: - Leaderboards

extension LeaderboardTrack: Identifiable {
    public var id: String { videoUrl ?? title ?? "\(hashValue)" }
}

extension LeaderboardListener: Identifiable {
    public var id: String { userId.map(String.init) ?? "\(hashValue)" }
}

extension SwarmLeaderboardTrack: Identifiable {
    public var id: String { (botKey ?? "") + (videoUrl ?? title ?? "\(hashValue)") }
}

extension MusicIntelligenceBot: Identifiable {
    public var id: String { botKey ?? "\(hashValue)" }
}

extension MusicIntelligenceTrack: Identifiable {
    public var id: String { videoUrl ?? title ?? "\(hashValue)" }
}

// MARK: - Community

extension PanelNotification: Identifiable {
    var isUnread: Bool { readAt == nil }
}

extension AccountSummary: Identifiable {
    var name: String { displayName?.isEmpty == false ? displayName! : username }
    var isFollowedByMe: Bool { followedByMe ?? false }
}

extension FriendRequest: Identifiable {
    var name: String { displayName?.isEmpty == false ? displayName! : (username ?? "Unknown") }
}

extension MessageThread: Identifiable {
    public var id: Int { accountId }
    var name: String { displayName?.isEmpty == false ? displayName! : (username ?? "Unknown") }
}

extension ChatMessage: Identifiable {}

extension PublicProfile {
    var name: String { displayName?.isEmpty == false ? displayName! : username }
}

extension ProfileLink: Identifiable {
    public var id: String { url }
}

extension PublicProfileTrack: Identifiable {
    public var id: String { videoUrl ?? title ?? "\(hashValue)" }
}

extension PublicProfileSession: Identifiable {
    public var id: String { (botDisplay ?? "") + (title ?? "") }
}

// MARK: - Admin

extension AuditLogEntry: Identifiable {
    /// One row per changed field, or nil when details aren't a before/after diff.
    var diffPairs: [(key: String, before: String, after: String)]? {
        auditDiffPairs(details: details)?.map { (key: $0.key, before: $0.before, after: $0.after) }
    }
}

extension AlertRule: Identifiable {}
extension LumisoundUser: Identifiable {}
extension LumisoundUpload: Identifiable {}
extension LumisoundBugReport: Identifiable {}
extension GalleryComment: Identifiable {}
extension GalleryReport: Identifiable {}

extension GalleryMedia: Identifiable {
    var displayTitle: String { title?.isEmpty == false ? title! : "Untitled" }
}

extension GalleryUser: Identifiable {
    var name: String { displayName?.isEmpty == false ? displayName! : username }
    var isEmailVerified: Bool { emailVerifiedAt != nil }
    var isAgeVerified: Bool { ageVerifiedAt != nil }
}

extension FeedEvent: Identifiable {
    public var id: String { "\(timestamp ?? "")-\(source ?? "")-\(title ?? "")" }
    var type: String? { eventType }
}

extension SwarmAccountSummary: Identifiable {
    var name: String { displayName?.isEmpty == false ? displayName! : username }
    var isModerator: Bool { panelRole == "moderator" }
}

extension SwarmAccountBulkFailure: Identifiable {}

extension SchemaEntry: Identifiable {
    public var id: String { schema }
}

extension ExportSnapshot: Identifiable {
    public var id: String { date }
}

extension ExportFile: Identifiable {
    public var id: String { name }
    var sizeDescription: String {
        guard let sizeBytes else { return "" }
        return ByteCountFormatter.string(fromByteCount: Int64(sizeBytes), countStyle: .file)
    }
}

// MARK: - Live snapshots

extension LiveSnapshot {
    /// The decoded value inside the snapshot (for SwarmLiveSocket's typed watch).
    var payload: Any {
        switch self {
        case .dashboard(let v): return v
        case .notifications(let v): return v
        case .friends(let v): return v
        case .communityCounts(let v): return v
        case .threads(let v): return v
        case .threadMessages(let v): return v
        case .controlState(let v): return v
        case .account(let v): return v
        case .other(let json): return json
        }
    }
}
