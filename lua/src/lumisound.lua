-- Lumisound (iOS companion app) admin dashboard data. Port of
-- app/db/lumisound.py. The ios_* tables live in the same Postgres database
-- as the GWS music bot (discord_music_gws) — this is db_default_schema in
-- config.lua terms, since PANEL_DB_DEFAULT_SCHEMA=discord_music_gws in .env.
-- The `botuser` grants gap that used to block every query here (permission
-- denied for every ios_* table except `stash`-owned reads) was already
-- fixed before this pass — verified live below.
local db = require("db")
local admin_browser = require("admin_browser")

local M = {}

-- 2026-10-26: Lumisound's live data is written by the iOS bridge on this PC,
-- while the GWS bot's own data in the same-named database stays on Mel -- so
-- Lumisound gets its own "lumisound" route (PANEL_DB_ROUTES) when configured.
local function schema(settings)
  if settings.db_routes and settings.db_routes.lumisound then return "lumisound" end
  return settings.db_default_schema
end

function M.get_lumisound_admin_data(settings, limit)
  local sch = schema(settings)
  local safe_limit = math.max(1, math.min(tonumber(limit) or 50, 200))
  local summary = { schema = sch }

  local COUNT_QUERIES = {
    { key = "users", sql = "SELECT COUNT(*) AS count FROM ios_users" },
    { key = "active_users", sql = "SELECT COUNT(*) AS count FROM ios_users WHERE is_active = TRUE" },
    { key = "uploads", sql = "SELECT COUNT(*) AS count FROM ios_user_music_metadata" },
    { key = "play_history", sql = "SELECT COUNT(*) AS count FROM ios_play_history" },
    { key = "playlists", sql = "SELECT COUNT(*) AS count FROM ios_user_playlists" },
    { key = "bug_reports_open", sql = "SELECT COUNT(*) AS count FROM ios_bug_reports WHERE status = 'open'" },
    { key = "listen_rooms_active", sql = "SELECT COUNT(*) AS count FROM ios_listen_rooms WHERE updated_at > NOW() - INTERVAL '1 hour'" },
    { key = "listening_now", sql = "SELECT COUNT(*) AS count FROM ios_playback_state WHERE updated_at > NOW() - INTERVAL '15 minutes'" },
    { key = "plays_24h", sql = "SELECT COUNT(*) AS count FROM ios_play_history WHERE played_at > NOW() - INTERVAL '1 day'" },
    { key = "uploads_storage_bytes", sql = "SELECT COALESCE(SUM(file_size_bytes), 0) AS count FROM ios_user_music_metadata" },
  }
  for _, q in ipairs(COUNT_QUERIES) do
    local row = db.fetchone(sch, q.sql)
    summary[q.key] = db.toint(row and row.count, 0)
  end

  local users = db.fetchall(
    sch,
    [[SELECT u.id, u.username, u.display_name, u.email, u.created_at, u.last_login,
             u.is_active, u.share_listening_activity,
             COUNT(DISTINCT p.id) AS playlist_count,
             COUNT(DISTINCT up.id) AS upload_count
      FROM ios_users u
      LEFT JOIN ios_user_playlists p ON p.user_id = u.id
      LEFT JOIN ios_user_music_metadata up ON up.user_id = u.id
      GROUP BY u.id ORDER BY u.created_at DESC LIMIT %s]],
    safe_limit
  )

  local uploads = db.fetchall(
    sch,
    [[SELECT up.id, up.user_id, up.filename, up.title, up.artist, up.file_size_bytes, up.uploaded_at, u.username
      FROM ios_user_music_metadata up JOIN ios_users u ON u.id = up.user_id
      ORDER BY up.uploaded_at DESC LIMIT %s]],
    safe_limit
  )

  local bug_reports = db.fetchall(
    sch,
    [[SELECT b.id, b.user_id, b.category, b.description, b.contact_email, b.app_version,
             b.device_info, b.status, b.created_at, u.username
      FROM ios_bug_reports b LEFT JOIN ios_users u ON u.id = b.user_id
      ORDER BY CASE b.status WHEN 'open' THEN 0 ELSE 1 END, b.created_at DESC LIMIT %s]],
    safe_limit
  )

  local listen_rooms = db.fetchall(
    sch,
    [[SELECT r.id, r.host_user_id, r.room_code, r.title, r.artist, r.is_playing, r.updated_at, u.username AS host_username
      FROM ios_listen_rooms r JOIN ios_users u ON u.id = r.host_user_id
      WHERE r.updated_at > NOW() - INTERVAL '1 hour'
      ORDER BY r.updated_at DESC LIMIT %s]],
    safe_limit
  )

  local now_playing = db.fetchall(
    sch,
    [[SELECT p.user_id, p.song_id, p.title, p.artist, p.source, p.position_seconds, p.duration_seconds, p.updated_at, u.username
      FROM ios_playback_state p JOIN ios_users u ON u.id = p.user_id
      WHERE p.updated_at > NOW() - INTERVAL '15 minutes'
      ORDER BY p.updated_at DESC LIMIT %s]],
    safe_limit
  )

  local recent_plays = db.fetchall(
    sch,
    [[SELECT h.id, h.user_id, h.title, h.artist, h.played_at, h.listen_seconds, u.username
      FROM ios_play_history h JOIN ios_users u ON u.id = h.user_id
      ORDER BY h.played_at DESC LIMIT %s]],
    safe_limit
  )

  return {
    schema = sch, summary = summary, users = users, uploads = uploads,
    bug_reports = bug_reports, listen_rooms = listen_rooms,
    now_playing = now_playing, recent_plays = recent_plays,
  }
end

function M.delete_lumisound_upload(settings, upload_id)
  db.execute(schema(settings), "DELETE FROM ios_user_music_metadata WHERE id = %s", tostring(upload_id))
end

function M.set_lumisound_user_active(settings, user_id, active)
  local sch = schema(settings)
  db.execute(sch, "UPDATE ios_users SET is_active = %s WHERE id = %s", active and true or false, tostring(user_id))
  return db.fetchone(sch, "SELECT id, username, display_name, email, is_active FROM ios_users WHERE id = %s", tostring(user_id))
end

function M.update_lumisound_bug_report_status(settings, report_id, status)
  status = tostring(status or ""):lower():match("^%s*(.-)%s*$")
  if status ~= "open" and status ~= "resolved" then
    error("Bug report status must be open or resolved.", 0)
  end
  db.execute(schema(settings), "UPDATE ios_bug_reports SET status = %s WHERE id = %s", status, tostring(report_id))
end

return M
