//! SwarmPanel (Rust): JSON API + live socket for the web/iOS/tvOS/Linux
//! clients, server-rendered pages, and the WASM browser module.

mod accounts;
mod alerts;
mod api;
mod app;
mod audit;
mod auth;
mod browser;
mod cache;
mod channel_convert;
mod config;
mod control;
mod csv;
mod dashboard;
mod db;
mod discord;
mod fleet;
mod gallery;
mod http;
mod lumisound;
mod metrics;
mod notify;
mod pages;
mod profiles;
mod queues;
mod ratelimit;
mod social;
mod telemetry;
mod util;
mod ws;

use api::{accounts_admin as acc, admin as adm, community as com, fleet as flt, session as ses};
use axum::Router;
use axum::routing::{get, post};
use std::net::SocketAddr;
use std::time::Duration;

fn api_routes() -> Router<app::AppRef> {
    Router::new()
        .route("/healthz", get(ses::healthz))
        .route("/api/health", get(ses::health))
        .route("/api/session", get(ses::session))
        .route("/api/session/admin-mode", post(ses::admin_mode))
        .route("/api/session/login", post(ses::login))
        .route("/api/telemetry/client", post(ses::client_telemetry))
        .route("/api/session/logout", post(ses::logout))
        .route("/api/session/password", post(ses::password))
        .route("/api/session/register", post(ses::register))
        .route("/api/session/resend-verification", post(ses::resend_verification))
        .route("/api/session/verification-discord", post(ses::verification_discord))
        .route("/api/session/verification-webhook", post(ses::verification_webhook))
        .route("/api/session/email", post(ses::email))
        .route("/api/session/verification/verify", post(ses::verify))
        .route("/api/dashboard", get(flt::dashboard))
        .route("/api/swarm-leaderboard", get(flt::swarm_leaderboard))
        .route("/api/bots", get(flt::bots))
        .route("/api/bots/{bot_key}/inventory", get(flt::inventory))
        .route("/api/bots/{bot_key}/control-state", get(flt::control_state))
        .route("/api/bots/control", post(flt::control))
        .route("/api/guilds/{guild_id}/convert-channels", post(flt::convert_channels))
        .route("/api/guilds/{guild_id}/control-matrix", get(flt::control_matrix))
        .route("/api/guilds/{guild_id}/leaderboard", get(flt::guild_leaderboard))
        .route("/api/queues", get(flt::queues_list).post(flt::queues_create))
        .route("/api/queues/{queue_id}/rename", post(flt::queues_rename))
        .route("/api/queues/{queue_id}/delete", post(flt::queues_delete))
        .route("/api/music-intelligence", get(flt::music_intelligence))
        .route("/api/bot-telemetry", get(flt::bot_telemetry))
        .route("/api/alert-rules", get(flt::alert_rules).post(flt::alert_rule_create))
        .route("/api/alert-rules/{rule_id}/update", post(flt::alert_rule_update))
        .route("/api/alert-rules/{rule_id}/delete", post(flt::alert_rule_delete))
        .route("/api/users/me", get(com::me).post(com::me_update))
        .route("/api/users/preferences", get(com::preferences).post(com::preferences_update))
        .route("/api/appearance/presets", get(com::presets))
        .route("/api/users/search", get(com::search))
        .route("/api/users/directory", get(com::directory))
        .route("/api/users/{account_id}/profile", get(com::profile))
        .route("/api/users/{account_id}/follow", post(com::follow))
        .route("/api/users/{account_id}/friend-request", post(com::friend_request))
        .route("/api/friends/requests", get(com::friend_requests))
        .route("/api/friends/requests/{request_id}", post(com::friend_respond))
        .route("/api/me/friends", get(com::my_friends))
        .route("/api/messages/threads", get(com::threads))
        .route("/api/messages/{account_id}", get(com::messages).post(com::send_message))
        .route("/api/notifications", get(com::notifications))
        .route("/api/notifications/unread-count", get(com::unread_count))
        .route("/api/notifications/read-all", post(com::notifications_read_all))
        .route("/api/notifications/{notification_id}/read", post(com::notification_read))
        .route("/api/admin/overview", get(com::admin_overview))
        .route("/api/community/counts", get(com::community_counts))
        .route("/api/swarm-accounts/resend-verification", post(acc::resend_verification))
        .route("/api/swarm-accounts/admin", get(acc::admin_list))
        .route("/api/swarm-accounts/update", post(acc::update))
        .route("/api/swarm-accounts/email-verified", post(acc::email_verified))
        .route("/api/swarm-accounts/reset-password", post(acc::reset_password))
        .route("/api/swarm-accounts/moderator", post(acc::moderator))
        .route("/api/swarm-accounts/delete", post(acc::delete))
        .route("/api/swarm-accounts/bulk-delete", post(acc::bulk_delete))
        .route("/api/swarm-accounts/bulk-verify", post(acc::bulk_verify))
        .route("/api/image-gallery/admin", get(adm::gallery_admin))
        .route("/api/image-gallery/admin/media/export.csv", get(adm::gallery_media_csv))
        .route("/api/image-gallery/admin/users/export.csv", get(adm::gallery_users_csv))
        .route("/api/image-gallery/tables", get(adm::gallery_tables))
        .route("/api/image-gallery/table-data", get(adm::gallery_table_data))
        .route("/api/image-gallery/users/delete", post(adm::gallery_user_delete))
        .route("/api/image-gallery/comments/delete", post(adm::gallery_comment_delete))
        .route("/api/image-gallery/users/reset-password", post(adm::gallery_reset_password))
        .route("/api/image-gallery/users/update", post(adm::gallery_user_update))
        .route("/api/image-gallery/users/email-verified", post(adm::gallery_email_verified))
        .route("/api/image-gallery/users/age-verified", post(adm::gallery_age_verified))
        .route("/api/image-gallery/users/resend-verification", post(adm::gallery_resend_verification))
        .route("/api/image-gallery/media/update", post(adm::gallery_media_update))
        .route("/api/image-gallery/media/delete", post(adm::gallery_media_delete))
        .route("/api/image-gallery/admin/media/bulk-delete", post(adm::gallery_bulk_media))
        .route("/api/image-gallery/admin/comments/bulk-delete", post(adm::gallery_bulk_comments))
        .route("/api/image-gallery/admin/users/bulk-delete", post(adm::gallery_bulk_users))
        .route("/api/image-gallery/reports/status", post(adm::gallery_report_status))
        .route("/api/stability", get(adm::stability))
        .route("/api/metrics", get(adm::metrics))
        .route("/api/metrics/history", get(adm::metrics_history))
        .route("/api/metrics/anomalies", get(adm::metrics_anomalies))
        .route("/api/events", get(adm::events))
        .route("/api/lumisound/admin", get(adm::lumisound_admin))
        .route("/api/lumisound/uploads/delete", post(adm::lumisound_upload_delete))
        .route("/api/lumisound/users/active", post(adm::lumisound_user_active))
        .route("/api/lumisound/bug-reports/status", post(adm::lumisound_bug_status))
        .route("/api/databases", get(adm::databases))
        .route("/api/databases/{schema}/tables", get(adm::database_tables))
        .route("/api/database/truncate-table", post(adm::truncate_table))
        .route("/api/database/truncate-schema", post(adm::truncate_schema))
        .route("/api/database/data", get(adm::database_data))
        .route("/api/exports", get(adm::exports))
        .route("/api/exports/{date}/{filename}", get(adm::export_file))
        .route("/api/audit-log", get(adm::audit_log))
        .route("/api/audit-log/{entry_id}/revert", post(adm::audit_revert))
        .route("/api/projects/lumisound/download", get(adm::lumisound_download))
        .route("/ws", get(ws::handler))
}

fn background(app: &app::AppRef) {
    // Open the bot database pools and fill the dashboard cache before the first visitor.
    let a = app.clone();
    tokio::spawn(async move {
        dashboard::get_dashboard_data(&a).await;
    });
    let a = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        social::ensure_indexes(&a).await;
    });
    let a = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            metrics::ensure_history_schema(&a).await;
            metrics::capture(&a).await;
            metrics::maybe_cleanup_history(&a).await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
    let a = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let ws = a.telemetry.ws_active.load(std::sync::atomic::Ordering::Relaxed);
            let _ = a
                .db
                .exec(
                    "accountlogins",
                    "INSERT INTO swarmpanel_process_stats (id, memory_kb, uptime_seconds, active_websocket_count, requests_last_interval, updated_at) VALUES (1, $1, $2, $3, $4, now()) \
                     ON CONFLICT (id) DO UPDATE SET memory_kb = EXCLUDED.memory_kb, uptime_seconds = EXCLUDED.uptime_seconds, active_websocket_count = EXCLUDED.active_websocket_count, requests_last_interval = EXCLUDED.requests_last_interval, updated_at = now()",
                    &[telemetry::memory_kb().into(), (a.started.elapsed().as_secs() as i64).into(), ws.into(), a.pop_requests().into()],
                )
                .await;
            if rand::random_range(0..40) == 0 {
                let _ = a.db.exec("accountlogins", "DELETE FROM swarmpanel_telemetry_events WHERE created_at < now() - interval '30 days'", &[]).await;
            }
        }
    });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = config::Settings::load()?;
    let addr: SocketAddr = format!("{}:{}", settings.bind_host, settings.port).parse()?;
    let app = app::App::new(settings);
    background(&app);
    let router = api_routes()
        .merge(pages::routes())
        .route_layer(axum::middleware::from_fn(http::tag_pattern))
        .fallback(http::not_found)
        .layer(axum::middleware::from_fn_with_state(app.clone(), http::middleware))
        .with_state(app.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("[swarmpanel] listening on {addr}");
    axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = term.recv() => {},
            }
        })
        .await?;
    Ok(())
}
