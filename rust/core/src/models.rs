//! API models. Each is a Swift struct (via UniFFI) with the same name and
//! (camelCase) fields the SwiftUI views already use.

use crate::lenient::SwiftInt;
use crate::model;
use std::collections::HashMap;

type I = SwiftInt;

// ---- session ---------------------------------------------------------------------------------
model!(SessionPayload {
    ok: Option<bool>, authenticated: Option<bool>, token: Option<String>, username: Option<String>, role: Option<String>,
    guild_id: Option<String>, account_guild_id: Option<String>, site_owner: Option<bool>, admin_mode: Option<bool>, moderator: Option<bool>,
    image_gallery_owner: Option<bool>, verification_sent: Option<bool>, pages_public_url: Option<String>, expires_in: Option<I>,
});

model!(OkResponse { ok: bool, message: Option<String> });

// ---- dashboard ---------------------------------------------------------------------------------
model!(DashboardResponse { bots: Option<Vec<DashboardBot>>, sessions: Option<Vec<DashboardSession>>, generated_at: Option<String>, summary: crate::helpers::FleetSummary }
    finish(s) { s.summary = crate::helpers::summarize(s.bots.as_deref().unwrap_or(&[])); });

model!(MedicSummary { pending_repairs: Option<I>, pending_infra: Option<I>, critical_health: Option<I>, recoverable_health: Option<I> });

model!(DashboardBot {
    key: String, display_name: Option<String>, kind: Option<String>, status: Option<String>, heartbeat_status: Option<String>,
    heartbeat_age_seconds: Option<f64>, playback_mode: Option<String>, active_playing_count: Option<I>, known_guild_count: Option<I>,
    queue_depth: Option<I>, backup_queue_depth: Option<I>, sessions: Option<Vec<DashboardSession>>, memory_kb: Option<f64>,
    uptime_seconds: Option<f64>, recent_interaction_count: Option<I>, medic_summary: Option<MedicSummary>,
    is_offline: bool, is_orchestrator: bool, featured_session: Option<DashboardSession>,
} finish(s) {
    s.is_orchestrator = s.kind.as_deref() == Some("orchestrator");
    s.is_offline = s.status.as_deref().is_some_and(|x| x.eq_ignore_ascii_case("offline")) || s.heartbeat_age_seconds.is_some_and(|a| a > 120.0);
    let all = s.sessions.as_deref().unwrap_or(&[]);
    s.featured_session = all.iter().find(|x| x.is_playing == Some(true)).or(all.first()).cloned();
});

model!(DashboardSession {
    guild_id: Option<String>, channel_id: Option<String>, guild_name: Option<String>, channel_name: Option<String>, home_channel_name: Option<String>,
    title: Option<String>, video_url: Option<String>, is_playing: Option<bool>, is_paused: Option<bool>, session_state: Option<String>,
    session_state_label: Option<String>, queue_count: Option<I>, backup_queue_count: Option<I>, position_seconds: Option<I>,
    duration_seconds: Option<I>, position_observed_at: Option<String>, thumbnail: Option<String>, media_source_label: Option<String>,
    cached: Option<bool>, bot_key: Option<String>, bot_display: Option<String>, observed_at_unix: Option<f64>,
} finish(s) { s.observed_at_unix = crate::helpers::observed_at_unix(s.position_observed_at.clone()); });

// ---- bots / inventory / control ----------------------------------------------------------------
model!(BotsResponse { bots: Option<Vec<BotSummary>>, invite_bots: Option<Vec<InviteBot>> });
model!(InviteBot {
    key: String, display_name: Option<String>, kind: Option<String>, capability_summary: Option<String>, accent: Option<String>,
    token_configured: Option<bool>, connected_to_session_guild: Option<bool>, icon_url: Option<String>, identity_name: Option<String>,
    identity_error: Option<String>, invite_url: Option<String>, permissions: Option<Vec<String>>,
});
model!(BotSummary { key: String, display_name: Option<String>, kind: Option<String> });
model!(BotInventoryResponse { guilds: Option<Vec<InventoryGuild>> });
model!(InventoryGuild { id: String, name: Option<String>, channels: Option<Vec<InventoryChannel>>, channels_error: Option<String> });
model!(InventoryChannel { id: String, name: Option<String>, channel_type as "type": Option<I>, is_voice: bool }
    finish(s) { s.is_voice = matches!(s.channel_type.map(|t| t.0), Some(2) | Some(13)); });

model!(ControlStateResponse { session: Option<ControlStateSession> });
model!(ControlStateSession {
    title: Option<String>, video_url: Option<String>, is_playing: Option<bool>, is_paused: Option<bool>, session_state_label: Option<String>,
    queue_count: Option<I>, queue_preview: Option<Vec<QueueItem>>, backup_queue_count: Option<I>, home_channel_id: Option<String>,
    channel_id: Option<String>, loop_mode: Option<String>, filter_mode: Option<String>, position_seconds: Option<I>, duration_seconds: Option<I>,
    position_observed_at: Option<String>, volume: Option<I>, pending_direct_orders: Option<I>, latest_direct_order: Option<DirectOrder>,
    thumbnail_url: Option<String>,
} finish(s) { s.thumbnail_url = s.video_url.clone().and_then(crate::helpers::youtube_thumbnail); });
model!(DirectOrder { command: Option<String>, data: Option<String> });
model!(QueueItem { video_url: String, title: Option<String> });
model!(SavedQueuesResponse { queues: Option<Vec<SavedQueue>> });
model!(SavedQueue { id: I, name: String, items: Option<Vec<QueueItem>> });
model!(SavedQueueEnvelope { queue: SavedQueue });

model!(ControlMatrixResponse { bots: Option<Vec<ControlMatrixBot>> });
model!(ControlMatrixBot { key: Option<String>, bot_key: Option<String>, display_name: Option<String>, bot_display: Option<String>, session: Option<GuildOverviewSession>, error: Option<String> });
model!(GuildOverviewSession {
    bot_key: Option<String>, bot_display: Option<String>, title: Option<String>, is_playing: Option<bool>, is_paused: Option<bool>,
    session_state_label: Option<String>, queue_count: Option<I>, channel_id: Option<String>, channel_name: Option<String>,
});
model!(ChannelConvertResponse { converted: Option<Vec<ConvertedChannel>>, failed: Option<Vec<ConvertFailure>>, skipped_no_home_channel: Option<I> });
model!(ConvertedChannel { warning: Option<String> });
model!(ConvertFailure { error: Option<String> });

// ---- leaderboards / learning --------------------------------------------------------------------
model!(LeaderboardEnvelope { data: LeaderboardData });
model!(LeaderboardData { top_tracks: Option<Vec<LeaderboardTrack>>, top_listeners: Option<Vec<LeaderboardListener>> });
model!(LeaderboardTrack { title: Option<String>, video_url: Option<String>, play_count: Option<I>, like_count: Option<I>, dislike_count: Option<I> });
model!(LeaderboardListener { user_id: Option<I>, track_count: Option<I>, play_count: Option<I> });
model!(SwarmLeaderboardResponse { window_days: Option<I>, generated_at: Option<String>, bots_queried: Option<I>, bots_errored: Option<I>, tracks: Option<Vec<SwarmLeaderboardTrack>> });
model!(SwarmLeaderboardTrack {
    title: Option<String>, video_url: Option<String>, play_count: Option<I>, finish_count: Option<I>, skip_count: Option<I>, like_count: Option<I>,
    dislike_count: Option<I>, smart_score: Option<I>, bot_key: Option<String>, bot_display: Option<String>,
});
model!(MusicIntelligenceEnvelope { data: MusicIntelligenceData });
model!(MusicIntelligenceData { totals: Option<MusicIntelligenceTotals>, bots: Option<Vec<MusicIntelligenceBot>> });
model!(MusicIntelligenceTotals { learned_tracks: Option<I>, plays: Option<I>, finishes: Option<I>, skips: Option<I>, likes: Option<I>, dislikes: Option<I>, recommendations: Option<I> });
model!(MusicIntelligenceBot {
    bot_key: Option<String>, bot_display: Option<String>, learned_tracks: Option<I>, plays: Option<I>, finishes: Option<I>, skips: Option<I>,
    likes: Option<I>, dislikes: Option<I>, recommendations: Option<I>, top_tracks: Option<Vec<MusicIntelligenceTrack>>,
});
model!(MusicIntelligenceTrack { title: Option<String>, video_url: Option<String>, play_count: Option<I>, finish_count: Option<I>, skip_count: Option<I>, like_count: Option<I>, smart_score: Option<I> });

// ---- notifications / community -------------------------------------------------------------------
model!(NotificationsResponse { notifications: Option<Vec<PanelNotification>> });
model!(UnreadCountResponse { unread_count: Option<I> });
model!(CommunityCounts { unread_messages: Option<I>, pending_friend_requests: Option<I> });
model!(NotificationsSnapshot { unread_count: Option<I>, notifications: Option<Vec<PanelNotification>> });
model!(PanelNotification { id: I, kind: Option<String>, title: String, body: Option<String>, link_path: Option<String>, read_at: Option<String>, created_at: Option<String> });

model!(UserSearchResponse { users: Option<Vec<AccountSummary>> });
model!(AccountSummary {
    id: I, username: String, display_name: Option<String>, bio: Option<String>, server_name: Option<String>, is_online: Option<bool>,
    avatar_url: Option<String>, server_icon_url: Option<String>, profile_headline: Option<String>, guild_id: Option<String>,
    favorite_bot: Option<String>, followed_by_me: Option<bool>, friend_status: Option<String>, follower_count: Option<I>, friend_count: Option<I>,
});
model!(FriendRequestsResponse { incoming: Option<Vec<FriendRequest>>, outgoing: Option<Vec<FriendRequest>> });
model!(FriendRequest { id: I, user_id: Option<I>, username: Option<String>, display_name: Option<String>, status: Option<String> });
model!(FriendsResponse { friends: Option<Vec<AccountSummary>> });
model!(FriendsSnapshot { friends: Option<Vec<AccountSummary>>, incoming: Option<Vec<FriendRequest>>, outgoing: Option<Vec<FriendRequest>> });
model!(MessageThreadsResponse { threads: Option<Vec<MessageThread>> });
model!(MessageThread { account_id: I, username: Option<String>, display_name: Option<String>, last_message: Option<String>, unread_count: Option<I>, is_online: Option<bool>, avatar_url: Option<String> });
model!(MessagesResponse { messages: Option<Vec<ChatMessage>> });
model!(ChatMessage { id: I, sender_account_id: Option<I>, recipient_account_id: Option<I>, body: String, created_at: Option<String>, mine: Option<bool> });

// ---- profiles ------------------------------------------------------------------------------------
model!(MeResponse { editable: Option<bool>, profile: MeProfile });
model!(MeProfileEnvelope { profile: MeProfile });
model!(MeProfile {
    id: Option<I>, username: Option<String>, display_name: Option<String>, bio: Option<String>, public_profile: Option<bool>, theme_accent: Option<String>,
    server_name: Option<String>, server_icon_url: Option<String>, profile_headline: Option<String>, guild_id: Option<String>,
    verification_verified: Option<bool>, avatar_url: Option<String>, email: Option<String>, has_password: Option<bool>,
    verification_pending: Option<bool>, verification_webhook_url: Option<String>, discord_user_id: Option<String>,
});
model!(VerificationSendResponse { profile: MeProfile, verification_sent: Option<bool> });
model!(ResendVerificationResponse { verification_sent: Option<bool>, already_verified: Option<bool> });
model!(PanelPreferencesEnvelope { editable: Option<bool>, preferences: PanelPreferences });
model!(PanelPreferences {
    theme_mode: Option<String>, accent_color: Option<String>, accent_secondary: Option<String>, accent_contrast_text: Option<String>,
    background_mode: Option<String>, background_color: Option<String>, background_image_url: Option<String>, density: Option<String>,
    motion: Option<String>,
});

model!(PublicProfileEnvelope { profile: PublicProfile, social_permissions: Option<SocialPermissions>, friends: Option<Vec<AccountSummary>> });
model!(SocialPermissions { can_friend: Option<bool>, can_message: Option<bool>, can_follow: Option<bool> });
model!(PublicProfile {
    id: I, username: String, display_name: Option<String>, bio: Option<String>, profile_quote: Option<String>, profile_headline: Option<String>,
    avatar_url: Option<String>, theme_accent: Option<String>, is_online: Option<bool>, favorite_bot: Option<String>, server_name: Option<String>,
    server_invite_url: Option<String>, guild_id: Option<String>, profile_tags: Option<Vec<String>>, profile_links: Option<Vec<ProfileLink>>,
    follower_count: Option<I>, following_count: Option<I>, friend_count: Option<I>, friend_status: Option<String>, followed_by_me: Option<bool>,
    activity: Option<PublicProfileActivity>,
});
model!(ProfileLink { label: String, url: String });
model!(PublicProfileActivity { total_plays: Option<I>, top_tracks: Option<Vec<PublicProfileTrack>>, active_sessions: Option<Vec<PublicProfileSession>> });
model!(PublicProfileTrack { title: Option<String>, video_url: Option<String>, plays: Option<I> });
model!(PublicProfileSession { title: Option<String>, bot_display: Option<String>, bot_name: Option<String>, is_playing: Option<bool>, channel_name: Option<String> });

// The "account" live key (profile summary + derived preferences).
model!(AccountSnapshot { profile: Option<AccountProfile>, preferences: Option<PanelPreferences> });
model!(AccountProfile {
    username: Option<String>, display_name: Option<String>, avatar_url: Option<String>, server_name: Option<String>, server_icon_url: Option<String>,
    profile_headline: Option<String>, guild_id: Option<String>, favorite_bot: Option<String>, theme_accent: Option<String>, role: Option<String>, admin_mode: Option<bool>,
});

// ---- admin ---------------------------------------------------------------------------------------
model!(AuditLogEnvelope { data: AuditLogData });
model!(AuditLogData { entries: Option<Vec<AuditLogEntry>>, total: Option<I> });
model!(AuditLogEntry { id: I, actor_username: Option<String>, action: String, target_type: Option<String>, target_id: Option<String>, details: Option<String>, created_at: Option<String>, diff: Option<Vec<AuditDiffPair>> }
    finish(s) { s.diff = crate::helpers::audit_diff_pairs(s.details.clone()); });
model!(AuditDiffPair { key: String, before: String, after: String });

model!(AlertRulesResponse { rules: Option<Vec<AlertRule>> });
model!(AlertRuleEnvelope { rule: AlertRule });
model!(AlertRule { id: I, rule_type: String, threshold_minutes: I, enabled: bool, escalation_minutes: Option<I>, escalate_email: Option<bool> });

model!(LumisoundAdminEnvelope { data: LumisoundAdminData });
model!(LumisoundAdminData { users: Option<Vec<LumisoundUser>>, uploads: Option<Vec<LumisoundUpload>>, bug_reports: Option<Vec<LumisoundBugReport>> });
model!(LumisoundUser { id: I, username: Option<String>, is_active: Option<bool> });
model!(LumisoundUpload { id: I, filename: Option<String>, title: Option<String>, username: Option<String> });
model!(LumisoundBugReport { id: I, category: Option<String>, description: Option<String>, status: Option<String>, username: Option<String> });

model!(GalleryAdminEnvelope { data: GalleryAdminData });
model!(GalleryAdminData { comments: Option<Vec<GalleryComment>>, reports: Option<Vec<GalleryReport>>, media: Option<Vec<GalleryMedia>>, users: Option<Vec<GalleryUser>> });
model!(GalleryMedia {
    id: I, title: Option<String>, media_kind: Option<String>, file_size: Option<I>, views: Option<I>, downloads: Option<I>, is_adult: Option<bool>,
    moderation_status: Option<String>, moderation_reason: Option<String>, username: Option<String>,
});
model!(GalleryUser {
    id: I, username: String, display_name: Option<String>, email: Option<String>, email_verified_at: Option<String>, age_verified_at: Option<String>,
    public_profile: Option<bool>, adult_content_consent: Option<bool>, media_count: Option<I>, comment_count: Option<I>,
});
model!(GalleryUserResendVerificationResponse { email_verification_sent: Option<bool>, already_verified: Option<bool> });
model!(GalleryComment { id: I, body: Option<String>, username: Option<String>, media_title: Option<String> });
model!(GalleryReport { id: I, reason: Option<String>, details: Option<String>, status: Option<String>, username: Option<String>, media_title: Option<String> });

model!(EventsResponse { events: Option<Vec<FeedEvent>> });
model!(FeedEvent { timestamp: Option<String>, source: Option<String>, title: Option<String>, description: Option<String>, event_type as "type": Option<String>, level: Option<String> });
model!(AdminOverviewStats { bots_online: Option<I>, bots_total: Option<I>, alert_rules_enabled: Option<I>, audit_entries_recent: Option<I>, open_reports: Option<I> });

model!(SwarmAccountsAdminEnvelope { data: SwarmAccountsAdminData });
model!(SwarmAccountsAdminData { users: Option<Vec<SwarmAccountSummary>> });
model!(SwarmAccountSummary {
    id: I, username: String, guild_id: Option<String>, display_name: Option<String>, email: Option<String>, verification_verified: Option<bool>,
    public_profile: Option<bool>, panel_role: Option<String>, server_name: Option<String>,
});
model!(SwarmAccountUpdateResponse { ok: Option<bool>, account: Option<SwarmAccountSummary> });
model!(SwarmAccountResendVerificationResponse { ok: Option<bool>, verification_sent: Option<bool>, already_verified: Option<bool> });
model!(SwarmAccountBulkResult { ok: Option<bool>, succeeded: Option<Vec<I>>, failed: Option<Vec<SwarmAccountBulkFailure>> });
model!(SwarmAccountBulkFailure { id: I, error: Option<String> });

model!(SchemasResponse { schemas: Option<Vec<SchemaEntry>> });
// `tables` is reduced to plain names in Rust (the server sends strings or objects).
model!(SchemaEntry { schema: String, table_names: Vec<String> });
model!(TableData { columns: Vec<String>, rows: Vec<TableRow>, count: Option<I> });
model!(TableRow { cells: Vec<String> });
model!(TruncateResponse { ok: Option<bool>, message: Option<String> });

model!(ExportsResponse { enabled: Option<bool>, snapshots: Option<Vec<ExportSnapshot>> });
model!(ExportSnapshot { date: String, files: Option<Vec<ExportFile>> });
model!(ExportFile { name: String, size_bytes: Option<I> });

model!(MetricHistoryResponse { points: Option<Vec<MetricPoint>> });
model!(MetricPoint { metric_value: f64, captured_at: Option<String> });
model!(MetricAnomaliesResponse { anomalies: Option<Vec<MetricAnomaly>> });
model!(MetricAnomaly { metric_value: f64, captured_at: Option<String>, z_score: Option<f64> });

// Pretty-printed JSON for raw diagnostic panels (stability, metrics).
model!(RawDocument { pretty: String });

// ---- widget / derived ----------------------------------------------------------------------------
model!(WidgetSnapshot {
    title: String, subtitle: String, is_playing: bool, is_paused: bool, position: f64, duration: f64, thumbnail: Option<String>,
    bots_count: I, live_count: I, queued_count: I,
});
model!(OwnSession { bot: DashboardBot, session: DashboardSession });
model!(ControlActionInfo { raw_value: String, label: String, needs_voice_channel: bool, needs_source_url: bool, needs_loop_mode: bool, needs_filter_mode: bool });
model!(ActionOutcome { ok: bool, message: String });

/// Silence "unused" for the HashMap import used by generated Lenient impls in other modules.
#[allow(dead_code)]
type _Map = HashMap<String, String>;
