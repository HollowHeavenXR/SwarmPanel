//! Lumisound admin (port of lumisound.lua): the iOS app's tables, which live
//! in the database PANEL_DB_ROUTES calls "lumisound".

use crate::app::AppRef;
use crate::db::{J, P};
use serde_json::{Value, json};

fn schema(app: &AppRef) -> String {
    if app.settings.db_routes.contains_key("lumisound") { "lumisound".into() } else { app.settings.db_default_schema.clone() }
}

pub async fn admin_data(app: &AppRef, limit: i64) -> Result<Value, String> {
    let sch = schema(app);
    let db = &app.db;
    let pl: [P; 1] = [limit.clamp(1, 200).into()];
    let mut summary = json!({ "schema": sch });
    let counts = [
        ("users", "SELECT COUNT(*) AS count FROM ios_users"),
        ("active_users", "SELECT COUNT(*) AS count FROM ios_users WHERE is_active = TRUE"),
        ("uploads", "SELECT COUNT(*) AS count FROM ios_user_music_metadata"),
        ("play_history", "SELECT COUNT(*) AS count FROM ios_play_history"),
        ("playlists", "SELECT COUNT(*) AS count FROM ios_user_playlists"),
        ("bug_reports_open", "SELECT COUNT(*) AS count FROM ios_bug_reports WHERE status = 'open'"),
        ("listen_rooms_active", "SELECT COUNT(*) AS count FROM ios_listen_rooms WHERE updated_at > NOW() - INTERVAL '1 hour'"),
        ("listening_now", "SELECT COUNT(*) AS count FROM ios_playback_state WHERE updated_at > NOW() - INTERVAL '15 minutes'"),
        ("plays_24h", "SELECT COUNT(*) AS count FROM ios_play_history WHERE played_at > NOW() - INTERVAL '1 day'"),
        ("uploads_storage_bytes", "SELECT COALESCE(SUM(file_size_bytes), 0) AS count FROM ios_user_music_metadata"),
    ];
    let results = futures_util::future::join_all(counts.iter().map(|(_, q)| db.one(&sch, q, &[]))).await;
    for ((k, _), r) in counts.iter().zip(results) {
        summary[*k] = json!(r?.and_then(|r| r.i("count")).unwrap_or(0));
    }
    let (users, uploads, bugs, rooms, now, plays) = tokio::join!(
        db.query(&sch, "SELECT u.id, u.username, u.display_name, u.email, u.created_at, u.last_login, u.is_active, u.share_listening_activity, COUNT(DISTINCT p.id) AS playlist_count, COUNT(DISTINCT up.id) AS upload_count \
                        FROM ios_users u LEFT JOIN ios_user_playlists p ON p.user_id = u.id LEFT JOIN ios_user_music_metadata up ON up.user_id = u.id GROUP BY u.id ORDER BY u.created_at DESC LIMIT $1", &pl),
        db.query(&sch, "SELECT up.id, up.user_id, up.filename, up.title, up.artist, up.file_size_bytes, up.uploaded_at, u.username FROM ios_user_music_metadata up JOIN ios_users u ON u.id = up.user_id ORDER BY up.uploaded_at DESC LIMIT $1", &pl),
        db.query(&sch, "SELECT b.id, b.user_id, b.category, b.description, b.contact_email, b.app_version, b.device_info, b.status, b.created_at, u.username FROM ios_bug_reports b LEFT JOIN ios_users u ON u.id = b.user_id \
                        ORDER BY CASE b.status WHEN 'open' THEN 0 ELSE 1 END, b.created_at DESC LIMIT $1", &pl),
        db.query(&sch, "SELECT r.id, r.host_user_id, r.room_code, r.title, r.artist, r.is_playing, r.updated_at, u.username AS host_username FROM ios_listen_rooms r JOIN ios_users u ON u.id = r.host_user_id WHERE r.updated_at > NOW() - INTERVAL '1 hour' ORDER BY r.updated_at DESC LIMIT $1", &pl),
        db.query(&sch, "SELECT p.user_id, p.song_id, p.title, p.artist, p.source, p.position_seconds, p.duration_seconds, p.updated_at, u.username FROM ios_playback_state p JOIN ios_users u ON u.id = p.user_id WHERE p.updated_at > NOW() - INTERVAL '15 minutes' ORDER BY p.updated_at DESC LIMIT $1", &pl),
        db.query(&sch, "SELECT h.id, h.user_id, h.title, h.artist, h.played_at, h.listen_seconds, u.username FROM ios_play_history h JOIN ios_users u ON u.id = h.user_id ORDER BY h.played_at DESC LIMIT $1", &pl),
    );
    Ok(json!({ "schema": sch, "summary": summary, "users": users?, "uploads": uploads?, "bug_reports": bugs?, "listen_rooms": rooms?, "now_playing": now?, "recent_plays": plays? }))
}

pub async fn delete_upload(app: &AppRef, id: &P) {
    let _ = app.db.exec(&schema(app), "DELETE FROM ios_user_music_metadata WHERE id = $1", &[id.clone()]).await;
}

pub async fn set_user_active(app: &AppRef, id: &P, active: bool) -> Option<Value> {
    let s = schema(app);
    let _ = app.db.exec(&s, "UPDATE ios_users SET is_active = $1 WHERE id = $2", &[active.into(), id.clone()]).await;
    app.db.opt(&s, "SELECT id, username, display_name, email, is_active FROM ios_users WHERE id = $1", &[id.clone()]).await
}

pub async fn update_bug_status(app: &AppRef, id: &P, status: &str) -> Result<(), String> {
    let st = status.trim().to_lowercase();
    if st != "open" && st != "resolved" {
        return Err("Bug report status must be open or resolved.".into());
    }
    app.db.exec(&schema(app), "UPDATE ios_bug_reports SET status = $1 WHERE id = $2", &[st.into(), id.clone()]).await.map(|_| ())
}
