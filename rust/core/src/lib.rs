//! SwarmPanel app core, shared by the iOS app, its widget extension and the
//! Apple TV app. Exposed to Swift through UniFFI (module `SwarmPanelCore`):
//! the Swift side keeps only SwiftUI views and thin ObservableObject bindings.

uniffi::setup_scaffolding!();

pub mod client;
pub mod error;
pub mod helpers;
pub mod lenient;
pub mod live;
pub mod models;
pub mod store;
pub mod telemetry;

use std::future::Future;
use std::sync::LazyLock;

/// One runtime for all networking and background work. Calls from Swift
/// run their future here and await the JoinHandle, so they work no matter
/// which executor polls them.
pub(crate) static RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("swarmpanel-core").enable_all().build().expect("tokio runtime")
});

pub(crate) async fn on_rt<F, T>(f: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    RT.spawn(f).await.expect("core task panicked")
}

/// Core library version (shown in Settings / diagnostics).
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
