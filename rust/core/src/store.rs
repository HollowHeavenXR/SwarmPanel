//! Persistence hooks implemented by the Swift app: the Keychain for the
//! bearer token, UserDefaults (standard or App Group) for everything else.
//! Rust decides what is stored; Swift only provides where.

#[uniffi::export(with_foreign)]
pub trait PlatformStore: Send + Sync {
    fn load_token(&self) -> Option<String>;
    fn save_token(&self, token: Option<String>);
    /// `shared` = the App Group suite (visible to the widget extension).
    fn get(&self, key: String, shared: bool) -> Option<String>;
    fn set(&self, key: String, value: Option<String>, shared: bool);
}

pub mod keys {
    pub const BASE_URL: &str = "swarmpanel.baseURL";
    pub const LAST_GUILD: &str = "swarmpanel.lastKnownGuildId";
    pub const TELEMETRY_QUEUE: &str = "swarmpanel.client.telemetryQueue.v2";
    pub const DASHBOARD_CACHE: &str = "swarmpanel.dashboardSnapshot.v2";
    pub const DASHBOARD_CACHE_AT: &str = "swarmpanel.dashboardSnapshotAt.v2";
    pub const ACCOUNT_CACHE: &str = "swarmpanel.accountSnapshot.v2";
    pub const TV_USERNAME: &str = "swarmpanel.tv.username";
    pub const LAST_NOTIFIED: &str = "swarmpanel.lastNotifiedNotificationId";
}
