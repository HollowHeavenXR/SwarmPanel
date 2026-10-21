-- Port of app/db/bots.py's _music_bot_snapshot() + get_dashboard_data() and
-- app/db/helpers.py's _derive_session_state()/_detect_media_source().
-- This is the core "GET /api/dashboard" data source.
local db = require("db")
local config = require("config")
local Redis = require("swarmlua.redis")
local telemetry = require("telemetry")
local socket_http = require("socket.http")
local ltn12 = require("ltn12")
local cjson = require("cjson")
local copas_http_ok, copas_http = pcall(require, "copas.http")

local M = {}

-- 2026-10-21 BUGFIX: each bot persists its OWN view of its Lavalink host
-- (see rhythm.lua's swarm_health writeup), which is "127.0.0.1" for every
-- bot regardless of which physical machine it runs on -- correct from
-- that bot's own perspective (it really does reach Lavalink over its own
-- loopback), meaningless from SwarmPanel's, which needs the real reachable
-- address. Confirmed live: gws/nexus had real, current session data
-- persisted and still returned no ping, because SwarmPanel was dialing
-- ITS OWN loopback instead of Mel's. This mirrors the exact split this
-- session's fleet migration left things in (see mel-migration-architecture
-- notes) -- update alongside any future bot move between hosts.
local MEL_HOSTED_BOTS = {
  gws = true, tunestream = true, melodic = true, sapphire = true, maestro = true, nexus = true,
}
local function resolve_lavalink_host(bot_key, reported_host)
  if reported_host == "127.0.0.1" and MEL_HOSTED_BOTS[bot_key] then return "192.168.1.87" end
  return reported_host
end

-- 2026-10-21 (per operator request: cards need a live, accurate voice
-- ping): queries this bot's own Lavalink node directly for every guild's
-- current state.ping in one call -- /v4/sessions/{id}/players returns every
-- player on that session, not just one, so this is one REST call per bot
-- per dashboard refresh, not one per guild. Lavalink has no "list every
-- session" endpoint (confirmed live: GET /v4/sessions 404s on 4.2.2), which
-- is exactly why session_id has to be persisted by the bot itself (see
-- rhythm.lua's swarm_health writeup) rather than discovered here.
-- Short timeout (2s): this runs on the ~2s dashboard broadcast loop, and a
-- slow/unreachable node (the whole point of checking) must degrade this one
-- widget, not stall the broadcast for every connected operator.
local LAVALINK_PING_TIMEOUT_SECONDS = 2
local function fetch_lavalink_pings(host, port, session_id)
  if not (host and port and session_id and session_id ~= "") then return {} end
  local url = ("http://%s:%s/v4/sessions/%s/players"):format(host, tostring(port), session_id)
  local response_body = {}
  -- 2026-10-21 BUGFIX: http.request's table form returns (1, code, headers,
  -- status) on success -- the FIRST value is just a literal 1, not the
  -- body or status. Confirmed live: the earlier version checked that bare
  -- `1` against 200 and silently discarded every real ping (always fell
  -- through to the empty-table return) despite the request actually
  -- succeeding every time.
  local pcall_ok, _, code = pcall(function()
    local request = {
      url = url, method = "GET", sink = ltn12.sink.table(response_body), redirect = false,
      headers = { ["Authorization"] = config.settings.lavalink_password },
      timeout = LAVALINK_PING_TIMEOUT_SECONDS,
    }
    if copas_http_ok and coroutine.isyieldable and coroutine.isyieldable() then
      return copas_http.request(request)
    end
    socket_http.TIMEOUT = LAVALINK_PING_TIMEOUT_SECONDS
    return socket_http.request(request)
  end)
  if not pcall_ok or code ~= 200 then return {} end
  local decode_ok, players = pcall(cjson.decode, table.concat(response_body))
  if not decode_ok or type(players) ~= "table" then return {} end
  local pings = {}
  for _, p in ipairs(players) do
    if p.guildId and p.state and p.state.ping then
      pings[tostring(p.guildId)] = db.toint(p.state.ping, nil)
    end
  end
  return pings
end

-- Persists across calls within this process (get_dashboard_data runs on
-- both the ~2s WS broadcast loop and every /api/dashboard request) so the
-- edge-triggered telemetry below (see get_dashboard_data's Aria section)
-- fires once per actual state change, not once per call.
local last_known_aria_online = nil

-- Cross-bot node-health scoreboard (see Music/lua-shared/swarmlua/
-- nodepool.lua) -- one shared Redis hash per Lavalink-protocol node
-- ("swarm:node_health:lavalink" / "...:nodelink"), written by whichever of
-- the 13 music bots most recently noticed a node degrade or recover. The
-- panel only ever reads this (never writes) -- it's a dashboard widget, not
-- a participant in node selection. Connects lazily and reconnects on
-- demand (Redis.new() doesn't open a socket until first use), so a Redis
-- outage degrades this one widget rather than the whole dashboard route
-- (every call site below is pcall-wrapped).
-- BUGFIX 2026-08-22: this widget only ever checked "lavalink" and
-- "nodelink" (the original single-primary+backup shape) -- confirmed live
-- via `redis-cli KEYS "swarm:node_health:*"` that the real scoreboard also
-- has "lavalink2" and "lavalink3" (Music/lua-shared/swarmlua/bot.lua's
-- config.lavalink_pool, added 2026-08-17 to spread the 13-bot fleet's DAVE/
-- MLS voice-session load across 3 real Lavalink instances instead of
-- piling onto one -- see nodepool.lua). Those two nodes' health has been
-- completely invisible on this dashboard since the day they were added --
-- a real degraded/down node 2 of the 3 real nodes could sit unnoticed here
-- indefinitely. Matches bot.lua's own "lavalink" / "lavalink" .. (i+1)
-- naming scheme exactly, plus "nodelink" for the NodeLink last-resort
-- fallback.
local NODE_NAMES = { "lavalink", "lavalink2", "lavalink3", "nodelink" }
local _redis_client = nil
local function redis_client()
  if not _redis_client then
    -- REDIS_HOST/REDIS_PORT (Music/.env) take priority; falls back to
    -- parsing the pre-existing REDIS_URL (redis://host:port/db) so this
    -- still works even against an older Music/.env that predates the
    -- discrete host/port vars.
    local host = config.env("REDIS_HOST", "")
    local port = tonumber(config.env("REDIS_PORT", ""))
    if host == "" or not port then
      local url = config.env("REDIS_URL", "")
      local url_host, url_port = url:match("^redis://([^:/]+):?(%d*)")
      host = (host ~= "" and host) or url_host or "127.0.0.1"
      port = port or tonumber(url_port) or 6379
    end
    _redis_client = Redis.new({ host = host, port = port })
  end
  return _redis_client
end

-- {lavalink = {status=, consecutive_failures=, last_success_age_seconds=,
--              last_failure_age_seconds=, last_error=}, nodelink = {...}}
-- status is "healthy" (a success within the last 5 minutes and no failure
-- streak), "degraded" (>=3 consecutive failures within the last minute --
-- matches nodepool.lua's own pick_best() threshold, so this label always
-- agrees with which node the bots are actually preferring), or "unknown"
-- (no data yet -- Redis unreachable, or this node has never been used).
function M.get_node_health()
  local out = {}
  local ok, client = pcall(redis_client)
  if not ok then
    for _, name in ipairs(NODE_NAMES) do out[name] = { status = "unknown" } end
    return out
  end
  local now = os.time()
  for _, name in ipairs(NODE_NAMES) do
    local hok, h = pcall(function() return client:hgetall("swarm:node_health:" .. name) end)
    if not hok or not h or next(h) == nil then
      out[name] = { status = "unknown" }
    else
      local failures = tonumber(h.consecutive_failures) or 0
      local last_success_at = tonumber(h.last_success_at)
      local last_failure_at = tonumber(h.last_failure_at)
      local recent_failure = last_failure_at and (now - last_failure_at) < 60
      local status = "unknown"
      if recent_failure and failures >= 3 then
        status = "degraded"
      elseif last_success_at and (now - last_success_at) < 300 then
        status = "healthy"
      elseif last_success_at or last_failure_at then
        status = "stale"
      end
      out[name] = {
        status = status,
        consecutive_failures = failures,
        last_success_age_seconds = last_success_at and (now - last_success_at) or nil,
        last_failure_age_seconds = last_failure_at and (now - last_failure_at) or nil,
        last_error = h.last_error,
        last_latency_ms = tonumber(h.last_latency_ms),
      }
    end
  end
  return out
end

-- home_channel_id is a snowflake and must stay a string end-to-end (see the
-- precision note further down) — "truthy" here means non-nil/non-empty/not
-- the literal zero-string, matching Python's `if home_channel_id:` on an int.
local function has_channel_value(v)
  return v ~= nil and v ~= "" and v ~= "0"
end

local function derive_session_state(playback, queue_count, has_settings, home_channel_id, backup_queue_count)
  backup_queue_count = backup_queue_count or 0
  local is_playing = db.tobool(playback.is_playing)
  local is_paused = db.tobool(playback.is_paused)
  local has_track = (playback.title ~= nil and playback.title ~= "") or (playback.video_url ~= nil and playback.video_url ~= "")
  local has_channel = playback.channel_id ~= nil
  local has_recovery_path = has_channel_value(home_channel_id) and (has_track or queue_count > 0 or backup_queue_count > 0)

  if is_paused and has_track and has_channel then return "paused", "Paused" end
  if is_playing and has_track and has_channel then return "playing", "Playing" end
  if has_track and has_channel then return "paused", "Paused" end
  if has_recovery_path and (queue_count > 0 or backup_queue_count > 0 or has_track) then return "recovering", "Recovery Pending" end
  if queue_count > 0 then return "queued", "Queued" end
  if has_settings or has_channel_value(home_channel_id) then return "configured", "Configured" end
  return "idle", "Idle"
end

local YOUTUBE_HOSTS = {
  ["youtube.com"] = true, ["www.youtube.com"] = true, ["m.youtube.com"] = true,
  ["music.youtube.com"] = true, ["youtu.be"] = true, ["www.youtu.be"] = true,
}

-- Port of app/db/helpers.py's _extract_youtube_video_id() + _derive_thumbnail_url().
-- Was never ported to this Lua rewrite at all -- sessions never got a
-- thumbnail/thumbnail_url field, so BotCard's `session?.thumbnail ||
-- session?.thumbnail_url` was always empty and every bot card fell back to
-- the plain Music2 icon instead of a real video thumbnail.
local function extract_youtube_video_id(video_url)
  video_url = tostring(video_url or ""):match("^%s*(.-)%s*$")
  if video_url == "" then return nil end
  local host, path_and_query = video_url:match("^https?://([^/]+)(/?.*)$")
  if not host or not YOUTUBE_HOSTS[host:lower()] then return nil end
  local path, query = path_and_query:match("^([^?]*)%??(.*)$")
  if host:lower():match("youtu%.be$") then
    return path:match("^/([^/]+)") or nil
  end
  if path == "/watch" then
    local v = query:match("[?&]?v=([^&]+)")
    return v ~= "" and v or nil
  end
  local first, second = path:match("^/([^/]+)/([^/]+)")
  if first == "shorts" or first == "embed" or first == "live" then
    return second ~= "" and second or nil
  end
  return nil
end

local function derive_thumbnail_url(video_url)
  local video_id = extract_youtube_video_id(video_url)
  if not video_id then return nil end
  return "https://i.ytimg.com/vi/" .. video_id .. "/hqdefault.jpg"
end

-- BUGFIX 2026-09-14 (per operator report: the panel claims bots are
-- "playing from local cache" when they aren't): this used to guess via
-- `ls /audiocache/<video_id>.*` (see docker-compose.yml's read-only
-- /mnt/fastssd/audiocache mount) -- i.e. "does ANY cached file for this
-- video ID exist anywhere, ever". That's a fundamentally wrong question:
-- lib/swarmlua/lavalink.lua's own header confirms bots stream directly by
-- default and only opt INTO the local/cached path (force_local) after
-- direct streaming has already failed a few times for that specific play --
-- so a file existing on disk from some PAST play (the cache has been
-- accumulating since June, 3000+ files) says nothing about whether THIS
-- CURRENT session actually used it. Every bot already records the real,
-- per-play ground truth itself: <prefix>_playback_events.forced_local, set
-- from the exact same `force_local` value passed into that resolve (see
-- e.g. strife.lua's log_playback_event call right after update_player
-- succeeds). detect_media_source now takes that real flag directly instead
-- of re-deriving a guess from the filesystem -- see forced_local_map below,
-- built from that same table.
-- 2026-10-21 (per operator request): relabeled now that PLAYBACK_MODE=local
-- is the fleet-wide default, not a fallback a track escalates into after
-- streaming fails -- "Cached (local)"/"YouTube" read like "normally
-- streams, this one happened to fall back," which stopped being true the
-- moment the whole fleet switched. The underlying forced_local signal
-- (is_forced_local) is unchanged and already correct -- see this
-- function's original header above forced_local_map in music_bot_snapshot
-- for why it's real per-play ground truth, not a filesystem guess. Only
-- the wording changed, to plainly say "downloaded" vs "streaming."
local function detect_media_source(video_url, is_forced_local)
  video_url = video_url or ""
  if video_url == "" then return { key = "unknown", label = "Unknown" } end
  local host = video_url:match("^https?://([^/]+)")
  if host and YOUTUBE_HOSTS[host:lower()] then
    if is_forced_local then
      return { key = "local_download", label = "Local Download" }
    end
    return { key = "youtube", label = "Streaming (YouTube)" }
  end
  if host and host:lower():match("soundcloud%.com$") then return { key = "soundcloud", label = "Streaming (SoundCloud)" } end
  if host then return { key = "link", label = "Streaming (Direct Link)" } end
  return { key = "search", label = "Search" }
end

local function music_bot_snapshot(bot)
  local schema = bot.db_schema
  local prefix = bot.table_prefix
  local names = {
    playback = prefix .. "_playback_state",
    settings = prefix .. "_guild_settings",
    queue = prefix .. "_queue",
    backup = prefix .. "_queue_backup",
    home = prefix .. "_bot_home_channels",
    heartbeat = "swarm_health",
    metrics = prefix .. "_metrics",
    intelligence = prefix .. "_track_intelligence",
    recommendations = prefix .. "_smart_recommendations",
    playback_events = prefix .. "_playback_events",
    error_events = prefix .. "_error_events",
  }
  local exists = db.batch_table_exists(schema, {
    names.playback, names.settings, names.queue, names.backup,
    names.home, names.heartbeat, names.metrics, names.intelligence, names.recommendations,
    names.playback_events, names.error_events,
  })

  -- IMPORTANT: guild_id/channel_id are Discord snowflakes (bigint, routinely
  -- >2^53). pg.lua forces bigint columns to come back as Lua STRINGS to
  -- preserve full precision (see pg.lua's set_type_deserializer comment) —
  -- so every map below is keyed by the STRING form of guild_id. Never run
  -- guild_id/channel_id through db.toint()/tonumber(): converting a 17-19
  -- digit snowflake to a Lua double (IEEE754, ~15-17 significant digits)
  -- silently corrupts it, e.g. 781797192058013696 -> 7.8179719205801e+17.
  -- (This exact class of bug is called out in pg.lua's own comments and in
  -- the swarm-wide migration notes — caught it here during dashboard testing
  -- against real data, see final report.)
  local known_guilds = {}
  local playback_map, filter_map, queue_map, backup_map, home_map, metrics_map, intel_map, reco_map = {}, {}, {}, {}, {}, {}, {}, {}

  local function gid_key(v)
    if v == nil then return nil end
    return tostring(v)
  end

  if exists[names.playback] then
    local rows = db.fetchall(schema, "SELECT * FROM " .. names.playback .. " ORDER BY guild_id LIMIT 500")
    for _, row in ipairs(rows) do
      local gid = gid_key(row.guild_id)
      if gid then
        playback_map[gid] = row
        known_guilds[gid] = true
      end
    end
  end

  -- Real "is this specific track actually playing from the local cache"
  -- data -- see detect_media_source's own header. One batched query per
  -- bot (not per guild): every currently-tracked track_uid's most recent
  -- track_start event tells us the real force_local value that resolve
  -- actually used.
  local forced_local_map = {}
  if exists[names.playback_events] then
    local uids, seen = {}, {}
    for _, row in pairs(playback_map) do
      if row.track_uid and row.track_uid ~= "" and not seen[row.track_uid] then
        seen[row.track_uid] = true
        uids[#uids + 1] = row.track_uid
      end
    end
    if #uids > 0 then
      local marks = {}
      for i = 1, #uids do marks[i] = "%s" end
      local ok, rows = pcall(db.fetchall, schema, string.format(
        [[SELECT track_uid, forced_local FROM %s WHERE event_type = 'track_start' AND track_uid IN (%s) ORDER BY created_at DESC]],
        names.playback_events, table.concat(marks, ", ")
      ), unpack(uids))
      if ok then
        for _, row in ipairs(rows) do
          -- ORDER BY created_at DESC above -- first row seen per track_uid
          -- is the most recent track_start, which is what we want.
          if forced_local_map[row.track_uid] == nil then
            forced_local_map[row.track_uid] = db.tobool(row.forced_local)
          end
        end
      end
    end
  end

  if exists[names.settings] then
    local rows = db.fetchall(
      schema,
      "SELECT guild_id, filter_mode, loop_mode, transition_mode, fade_seconds, fade_curve FROM " .. names.settings .. " LIMIT 500"
    )
    for _, row in ipairs(rows) do
      local gid = gid_key(row.guild_id)
      -- BUGFIX: this loop (unlike playback_map above it) assigned straight
      -- into filter_map[gid] with no nil check -- t[nil] = v is a hard Lua
      -- runtime error, so a NULL guild_id here would abort this bot's
      -- entire dashboard snapshot instead of just skipping one row.
      if gid then
        filter_map[gid] = {
          filter_mode = row.filter_mode or "none",
          loop_mode = row.loop_mode or "queue",
          transition_mode = row.transition_mode or "off",
          fade_seconds = tonumber(row.fade_seconds) or 5.0,
          fade_curve = row.fade_curve or "linear",
        }
        known_guilds[gid] = true
      end
    end
  end

  if exists[names.queue] then
    local rows = db.fetchall(
      schema, "SELECT guild_id, COUNT(*) AS queue_len FROM " .. names.queue .. " WHERE bot_name = %s GROUP BY guild_id", bot.key
    )
    for _, row in ipairs(rows) do
      local gid = gid_key(row.guild_id)
      if gid then
        queue_map[gid] = db.toint(row.queue_len, 0)
        known_guilds[gid] = true
      end
    end
  end

  if exists[names.backup] then
    local rows = db.fetchall(
      schema, "SELECT guild_id, COUNT(*) AS backup_len FROM " .. names.backup .. " WHERE bot_name = %s GROUP BY guild_id", bot.key
    )
    for _, row in ipairs(rows) do
      -- BUGFIX: this loop never set known_guilds[gid] -- a guild whose only
      -- surviving state is a stale backup queue (playback_state/
      -- guild_settings rows already cleared after the bot left voice) never
      -- got added to sorted_guild_ids below, so its row silently never
      -- rendered anywhere on the dashboard despite backup_map holding real
      -- data for it.
      local gid = gid_key(row.guild_id)
      if gid then
        backup_map[gid] = db.toint(row.backup_len, 0)
        known_guilds[gid] = true
      end
    end
  end

  if exists[names.home] then
    local rows = db.fetchall(schema, "SELECT guild_id, home_vc_id FROM " .. names.home .. " WHERE bot_name = %s LIMIT 500", bot.key)
    for _, row in ipairs(rows) do
      local gid = gid_key(row.guild_id)
      if gid then
        home_map[gid] = row.home_vc_id and gid_key(row.home_vc_id) or nil
        known_guilds[gid] = true
      end
    end
  end

  if exists[names.metrics] then
    local rows = db.fetchall(
      schema,
      "SELECT *, EXTRACT(EPOCH FROM (NOW() - updated_at))::int AS metric_age_seconds FROM " .. names.metrics .. " WHERE bot_name = %s LIMIT 500",
      bot.key
    )
    for _, row in ipairs(rows) do
      local gid = gid_key(row.guild_id)
      if gid then
        metrics_map[gid] = row
        known_guilds[gid] = true
      end
    end
  end

  if exists[names.intelligence] then
    local rows = db.fetchall(
      schema,
      [[SELECT guild_id, COUNT(*) AS learned_tracks,
               COALESCE(SUM(play_count), 0) AS plays,
               COALESCE(SUM(like_count), 0) AS likes,
               COALESCE(SUM(dislike_count), 0) AS dislikes
        FROM ]] .. names.intelligence .. [[ GROUP BY guild_id LIMIT 500]]
    )
    for _, row in ipairs(rows) do
      local gid = gid_key(row.guild_id)
      if gid then
        intel_map[gid] = {
          learned_tracks = db.toint(row.learned_tracks, 0),
          plays = db.toint(row.plays, 0),
          likes = db.toint(row.likes, 0),
          dislikes = db.toint(row.dislikes, 0),
        }
        known_guilds[gid] = true
      end
    end
  end

  if exists[names.recommendations] then
    local rows = db.fetchall(schema, "SELECT guild_id, COUNT(*) AS recommendations FROM " .. names.recommendations .. " GROUP BY guild_id LIMIT 500")
    for _, row in ipairs(rows) do
      -- BUGFIX: same known_guilds gap as the backup loop above.
      local gid = gid_key(row.guild_id)
      if gid then
        reco_map[gid] = db.toint(row.recommendations, 0)
        known_guilds[gid] = true
      end
    end
  end

  local sorted_guild_ids = {}
  for gid in pairs(known_guilds) do sorted_guild_ids[#sorted_guild_ids + 1] = gid end
  -- Numeric-safe string sort (snowflakes are decimal strings of varying
  -- length) without ever parsing them into a lossy Lua number: shorter
  -- numeric strings sort first, equal-length strings compare lexically.
  table.sort(sorted_guild_ids, function(a, b)
    if #a ~= #b then return #a < #b end
    return a < b
  end)

  local sessions = {}
  local active_playing_count = 0
  for _, gid in ipairs(sorted_guild_ids) do
    local playback = playback_map[gid] or {}
    local metric = metrics_map[gid] or {}
    local settings = filter_map[gid] or {}
    local queue_count = queue_map[gid] or 0
    local backup_queue_count = backup_map[gid] or 0
    local home_channel_id = home_map[gid]
    local smart = { learned_tracks = 0, plays = 0, likes = 0, dislikes = 0 }
    for k, v in pairs(intel_map[gid] or {}) do smart[k] = v end
    smart.recommendations = reco_map[gid] or 0

    local source_info = detect_media_source(playback.video_url, playback.track_uid and forced_local_map[playback.track_uid])
    local metric_age = db.toint(metric.metric_age_seconds, 999999)
    local metric_fresh = next(metric) ~= nil and metric_age <= 90
    local is_playing = metric_fresh and db.tobool(metric.player_playing) or db.tobool(playback.is_playing)
    local is_paused = metric_fresh and db.tobool(metric.player_paused) or db.tobool(playback.is_paused)
    local effective_channel_id = (metric_fresh and metric.connected_channel_id) or playback.channel_id
    local effective_position = db.toint(metric.position_seconds or playback.position_seconds, 0)
    local effective_duration = db.toint(
      metric.duration_seconds or metric.track_length_seconds or playback.duration_seconds
        or playback.length_seconds or playback.track_length_seconds, 0
    )
    local observed_at = (metric_fresh and metric.updated_at) or playback.updated_at

    local effective_playback = {}
    for k, v in pairs(playback) do effective_playback[k] = v end
    effective_playback.is_playing = is_playing
    effective_playback.is_paused = is_paused
    effective_playback.channel_id = effective_channel_id

    local session_state, session_state_label = derive_session_state(
      effective_playback, queue_count, filter_map[gid] ~= nil, home_channel_id, backup_queue_count
    )
    if is_playing then active_playing_count = active_playing_count + 1 end

    sessions[#sessions + 1] = {
      guild_id = tostring(gid),
      channel_id = effective_channel_id and tostring(effective_channel_id) or nil,
      title = playback.title,
      video_url = playback.video_url,
      media_source = source_info.key,
      media_source_label = source_info.label,
      thumbnail = derive_thumbnail_url(playback.video_url),
      position_seconds = effective_position,
      duration_seconds = effective_duration,
      position_observed_at = observed_at,
      is_playing = is_playing,
      is_paused = is_paused,
      metric_age_seconds = next(metric) ~= nil and metric_age or nil,
      session_state = session_state,
      session_state_label = session_state_label,
      filter_mode = settings.filter_mode or "none",
      loop_mode = settings.loop_mode or "queue",
      transition_mode = settings.transition_mode or "off",
      fade_seconds = settings.fade_seconds or 5.0,
      fade_curve = settings.fade_curve or "linear",
      queue_count = queue_count,
      backup_queue_count = backup_queue_count,
      backup_restore_ready = backup_queue_count > 0
        and (session_state == "recovering" or session_state == "queued" or session_state == "configured" or session_state == "idle" or session_state == "paused"),
      home_channel_id = home_channel_id and tostring(home_channel_id) or nil,
      -- 2026-10-21: filled in below, after the heartbeat query gives us
      -- this bot's current Lavalink node/session -- see fetch_lavalink_pings.
      ping_ms = nil,
    }
  end

  local heartbeat_age, heartbeat_status = nil, "unknown"
  -- 2026-10-21 (per operator request: controls need to know local vs
  -- streaming mode): playback_mode lives on swarm_health, written by each
  -- bot's own heartbeat tick (see rhythm.lua's identical writeup) -- read
  -- it from the same row/query as the heartbeat rather than a second query.
  local playback_mode = nil
  if exists[names.heartbeat] then
    local row = db.fetchone(
      schema,
      "SELECT status, playback_mode, lavalink_node_host, lavalink_node_port, lavalink_session_id, " ..
        "EXTRACT(EPOCH FROM (NOW() - last_pulse))::int AS heartbeat_age FROM " .. names.heartbeat .. " WHERE bot_name = %s LIMIT 1",
      bot.key
    )
    if row then
      heartbeat_age = db.toint(row.heartbeat_age, 0)
      heartbeat_status = row.status or "unknown"
      playback_mode = row.playback_mode
      -- 2026-10-21: see fetch_lavalink_pings' own header. One REST call
      -- per bot (not per guild), merged into every session this bot has
      -- by guild_id below.
      local pings = fetch_lavalink_pings(resolve_lavalink_host(bot.key, row.lavalink_node_host), row.lavalink_node_port, row.lavalink_session_id)
      for _, s in ipairs(sessions) do
        s.ping_ms = pings[s.guild_id]
      end
    end
  end

  -- BUGFIX 2026-10-21 (per operator request: "the online system needs
  -- revamping"): a bare heartbeat only proves the PROCESS is alive --
  -- confirmed live this same day, a bot can sit heartbeat-"online" for
  -- hours while two duplicate instances fight over the same voice session
  -- (repeated Discord "Session is no longer valid"/"Call terminated"
  -- closes) and the dashboard would have shown nothing wrong the whole
  -- time. voice_hard_invalidation (Discord explicitly killed a session,
  -- see rhythm.lua's WebSocketClosedEvent handler) and
  -- voice_recovery_giveup (this bot gave up reconnecting after repeated
  -- attempts) are both real, already-persisted signals of exactly that --
  -- a recent one of either means voice is actively unhealthy for this bot
  -- right now, regardless of what the heartbeat says. Window matches the
  -- longer of the two cooldowns those events themselves impose (10min
  -- giveup cooldown) plus margin, so a currently-stuck guild stays flagged
  -- for the duration it's actually stuck, not just the instant the event fired.
  local voice_degraded = false
  if exists[names.error_events] then
    local row = db.fetchone(
      schema,
      "SELECT COUNT(*) AS n FROM " .. names.error_events ..
        " WHERE error_type IN ('voice_hard_invalidation', 'voice_recovery_giveup') AND created_at > NOW() - INTERVAL '12 minutes'"
    )
    voice_degraded = row ~= nil and db.toint(row.n, 0) > 0
  end

  local node_status = "unknown"
  if heartbeat_age ~= nil then
    if heartbeat_age > 120 then
      node_status = "stale"
    elseif voice_degraded then
      node_status = "degraded"
    else
      node_status = "online"
    end
  end

  local queue_depth, backup_depth = 0, 0
  for _, s in ipairs(sessions) do
    queue_depth = queue_depth + s.queue_count
    backup_depth = backup_depth + s.backup_queue_count
  end

  local known_guild_count = 0
  for _ in pairs(known_guilds) do known_guild_count = known_guild_count + 1 end

  return {
    key = bot.key,
    display_name = bot.display_name,
    kind = bot.kind,
    schema = schema,
    status = node_status,
    heartbeat_age_seconds = heartbeat_age,
    heartbeat_status = heartbeat_status,
    playback_mode = playback_mode,
    active_playing_count = active_playing_count,
    known_guild_count = known_guild_count,
    queue_depth = queue_depth,
    backup_queue_depth = backup_depth,
    sessions = sessions,
    -- Not part of the frontend contract (stripped/overwritten in routes.lua's
    -- guild-scoping pass) — carries the raw known-guild id list so
    -- _visible_music_bots_for_auth-equivalent filtering doesn't need a second
    -- round of queries. Mirrors app/db.get_known_guild_ids(bot_key).
    known_guild_ids = sorted_guild_ids,
  }
end

M.music_bot_snapshot = music_bot_snapshot

-- Port of app/db/bots.py's get_bot_control_state(): single-guild control
-- panel state for one bot. Rather than duplicate music_bot_snapshot's ~9
-- table queries, just take its per-guild session out of the batch -- the
-- Python original queried per-guild directly, but this schema is small
-- enough (a handful of guilds per bot) that the extra rows fetched here are
-- negligible, and reusing tested logic beats a second, divergent code path.
function M.get_bot_control_state(bot, guild_id)
  local snap = music_bot_snapshot(bot)
  local gid = tostring(guild_id)
  for _, session in ipairs(snap.sessions or {}) do
    if session.guild_id == gid then
      local state = {}
      for k, v in pairs(session) do state[k] = v end
      state.bot_key = bot.key
      state.bot_display = bot.display_name
      state.status = snap.status
      state.heartbeat_age_seconds = snap.heartbeat_age_seconds
      state.heartbeat_status = snap.heartbeat_status
      return state
    end
  end
  -- Never seen this guild in any of the bot's tables -- an idle/unconfigured
  -- default, matching Python's all-zero fallback rather than 404ing (a music
  -- bot simply not yet used in this guild is a normal, common case).
  return {
    bot_key = bot.key, bot_display = bot.display_name, guild_id = gid,
    status = snap.status, heartbeat_age_seconds = snap.heartbeat_age_seconds, heartbeat_status = snap.heartbeat_status,
    session_state = "idle", session_state_label = "Idle", is_playing = false, is_paused = false,
    queue_count = 0, backup_queue_count = 0, backup_restore_ready = false,
    filter_mode = "none", loop_mode = "queue", transition_mode = "off", fade_seconds = 5.0, fade_curve = "linear",
  }
end

-- Port of app/db/bots.py's get_guild_leaderboard(): top tracks/listeners for
-- one bot+guild, built from the existing track_intelligence/
-- user_track_affinity tables (no new tables, matching the Python original).
function M.get_guild_leaderboard(bot, guild_id, limit)
  local schema = bot.db_schema
  local prefix = bot.table_prefix
  local intelligence_table = prefix .. "_track_intelligence"
  local affinity_table = prefix .. "_user_track_affinity"
  local safe_limit = math.max(1, math.min(tonumber(limit) or 10, 50))
  local exists = db.batch_table_exists(schema, { intelligence_table, affinity_table })

  local top_tracks = {}
  if exists[intelligence_table] then
    top_tracks = db.fetchall(
      schema,
      [[SELECT title, video_url, play_count, finish_count, skip_count, like_count, dislike_count,
               ((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5)) AS smart_score
        FROM ]] .. intelligence_table .. [[
        WHERE guild_id = %s
        ORDER BY smart_score DESC, play_count DESC
        LIMIT %s]],
      guild_id, safe_limit
    )
  end

  local top_listeners = {}
  if exists[affinity_table] then
    top_listeners = db.fetchall(
      schema,
      [[SELECT user_id, COUNT(*) AS track_count, COALESCE(SUM(play_count), 0) AS play_count,
               COALESCE(SUM(score), 0) AS taste_score
        FROM ]] .. affinity_table .. [[
        WHERE guild_id = %s
        GROUP BY user_id
        ORDER BY taste_score DESC, play_count DESC
        LIMIT %s]],
      guild_id, safe_limit
    )
  end

  return { guild_id = tostring(guild_id), bot_key = bot.key, top_tracks = top_tracks, top_listeners = top_listeners }
end

-- Port of app/db/bots.py's get_music_intelligence_summary(), simplified to
-- the single-optional-guild_id shape routes.lua's /api/music-intelligence
-- actually receives (the Python version's list-of-guild_ids parameter is
-- only ever called with zero or one guild in this codebase).
function M.get_music_intelligence_summary(music_bots, guild_id, bot_key, limit)
  local safe_limit = math.max(1, math.min(tonumber(limit) or 8, 25))
  local bots = music_bots
  if bot_key and bot_key ~= "" then
    bots = {}
    for _, bot in ipairs(music_bots) do
      if bot.key == bot_key then bots[1] = bot end
    end
    if #bots == 0 then error("Unknown bot key", 0) end
  end

  local totals = { learned_tracks = 0, plays = 0, finishes = 0, skips = 0, likes = 0, dislikes = 0, recommendations = 0 }
  local result_bots = {}

  for _, bot in ipairs(bots) do
    if bot.kind == "music" and bot.db_schema and bot.table_prefix then
      local schema, prefix = bot.db_schema, bot.table_prefix
      local intelligence_table = prefix .. "_track_intelligence"
      local affinity_table = prefix .. "_user_track_affinity"
      local recommendations_table = prefix .. "_smart_recommendations"
      local exists = db.batch_table_exists(schema, { intelligence_table, affinity_table, recommendations_table })

      if exists[intelligence_table] then
        local where, param = "", nil
        if guild_id and tostring(guild_id) ~= "" then
          where = "WHERE guild_id = %s"
          param = guild_id
        end

        local summary
        if param then
          summary = db.fetchone(
            schema,
            [[SELECT COUNT(*) AS learned_tracks, COALESCE(SUM(play_count), 0) AS plays,
                     COALESCE(SUM(finish_count), 0) AS finishes, COALESCE(SUM(skip_count), 0) AS skips,
                     COALESCE(SUM(like_count), 0) AS likes, COALESCE(SUM(dislike_count), 0) AS dislikes
              FROM ]] .. intelligence_table .. " " .. where,
            param
          ) or {}
        else
          summary = db.fetchone(
            schema,
            [[SELECT COUNT(*) AS learned_tracks, COALESCE(SUM(play_count), 0) AS plays,
                     COALESCE(SUM(finish_count), 0) AS finishes, COALESCE(SUM(skip_count), 0) AS skips,
                     COALESCE(SUM(like_count), 0) AS likes, COALESCE(SUM(dislike_count), 0) AS dislikes
              FROM ]] .. intelligence_table
          ) or {}
        end

        local top_tracks
        if param then
          top_tracks = db.fetchall(
            schema,
            [[SELECT guild_id, title, video_url, play_count, finish_count, skip_count, like_count, dislike_count,
                     ((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5)) AS smart_score,
                     updated_at
              FROM ]] .. intelligence_table .. " " .. where .. [[
              ORDER BY smart_score DESC, updated_at DESC
              LIMIT %s]],
            param, safe_limit
          )
        else
          top_tracks = db.fetchall(
            schema,
            [[SELECT guild_id, title, video_url, play_count, finish_count, skip_count, like_count, dislike_count,
                     ((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5)) AS smart_score,
                     updated_at
              FROM ]] .. intelligence_table .. [[
              ORDER BY smart_score DESC, updated_at DESC
              LIMIT %s]],
            safe_limit
          )
        end

        local recommendation_count = 0
        if exists[recommendations_table] then
          local rec_row
          if param then
            rec_row = db.fetchone(schema, "SELECT COUNT(*) AS recommendations FROM " .. recommendations_table .. " " .. where, param) or {}
          else
            rec_row = db.fetchone(schema, "SELECT COUNT(*) AS recommendations FROM " .. recommendations_table) or {}
          end
          recommendation_count = db.toint(rec_row.recommendations, 0)
        end

        local learned_tracks = db.toint(summary.learned_tracks, 0)
        totals.learned_tracks = totals.learned_tracks + learned_tracks
        totals.plays = totals.plays + db.toint(summary.plays, 0)
        totals.finishes = totals.finishes + db.toint(summary.finishes, 0)
        totals.skips = totals.skips + db.toint(summary.skips, 0)
        totals.likes = totals.likes + db.toint(summary.likes, 0)
        totals.dislikes = totals.dislikes + db.toint(summary.dislikes, 0)
        totals.recommendations = totals.recommendations + recommendation_count

        result_bots[#result_bots + 1] = {
          bot_key = bot.key, bot_display = bot.display_name, schema = schema,
          learned_tracks = learned_tracks, plays = db.toint(summary.plays, 0),
          finishes = db.toint(summary.finishes, 0), skips = db.toint(summary.skips, 0),
          likes = db.toint(summary.likes, 0), dislikes = db.toint(summary.dislikes, 0),
          recommendations = recommendation_count, top_tracks = top_tracks,
        }
      end
    end
  end

  return { totals = totals, bots = result_bots }
end

-- The three aria_* "recent activity" widgets below are all
-- ORDER BY created_at DESC, id DESC LIMIT <6, and this whole function runs
-- on the WS broadcast loop's 2-second dashboard tick -- so their cost is
-- paid continuously, not once per page view.
--
-- Measured live 2026-09-20, none of them had an index matching that sort:
-- aria_swarm_events had no created_at index at all (full parallel sort of
-- 304,822 rows, 12,660 buffers read off a Postgres volume that lives on a
-- slow USB disk) and took 4001ms to return 6 rows. aria_operator_decisions
-- had created_at alone (503ms) and aria_infra_history had it only as the
-- second column of (target_name, created_at), unusable for a global sort
-- (271ms). ~4.8s per dashboard build, against a 2s tick -- the loop could
-- never keep up, so it simply ran back-to-back forever. With these three
-- indexes the same queries are 2.1ms / 1.5ms / 1.6ms.
--
-- Created from here rather than in Aria's own schema bootstrap because this
-- is the consumer that needs them and the one whose latency they explain --
-- same precedent as control.lua, which already creates tables it depends on
-- inside the music bots' schemas. IF NOT EXISTS makes it a no-op after the
-- first run, and the whole thing is pcall'd: a panel that cannot create an
-- index must still render the dashboard, just slower.
local aria_indexes_ready = false
local function ensure_aria_dashboard_indexes()
  if aria_indexes_ready then return end
  aria_indexes_ready = true
  for _, stmt in ipairs({
    "CREATE INDEX IF NOT EXISTS aria_swarm_events_created_id_idx ON aria_swarm_events (created_at DESC, id DESC)",
    "CREATE INDEX IF NOT EXISTS aria_operator_decisions_created_id_idx ON aria_operator_decisions (created_at DESC, id DESC)",
    "CREATE INDEX IF NOT EXISTS aria_infra_history_created_id_idx ON aria_infra_history (created_at DESC, id DESC)",
  }) do
    pcall(db.execute, "discord_aria", stmt)
  end
end

local function build_dashboard_data(music_bots)
  ensure_aria_dashboard_indexes()
  local bots = {}
  local total_active = 0
  for _, bot in ipairs(music_bots) do
    local ok, snap = pcall(music_bot_snapshot, bot)
    if ok then
      bots[#bots + 1] = snap
      total_active = total_active + (snap.active_playing_count or 0)
    else
      bots[#bots + 1] = {
        key = bot.key, display_name = bot.display_name, kind = bot.kind, schema = bot.db_schema,
        status = "error", error = tostring(snap),
        heartbeat_age_seconds = nil, heartbeat_status = "unknown",
        active_playing_count = 0, known_guild_count = 0, sessions = {},
      }
    end
  end

  local aria_heartbeat_age, aria_memory_kb, aria_uptime_seconds = nil, nil, nil
  local aria_recent_interactions, aria_recent_interaction_count = {}, 0
  local aria_medic_summary = {
    pending_repairs = 0, pending_infra = 0, critical_health = 0, recoverable_health = 0,
    recent_operator_decisions = {}, recent_infra_history = {}, recent_swarm_events = {},
  }

  local ok = pcall(function()
    -- BUGFIX 2026-09-14 (per operator report: Aria's card shows garbage --
    -- see also the known_guild_count/active_playing_count removal below):
    -- this used to read discord_aria.swarm_health, a MariaDB-era leftover
    -- table the current Lua Aria rewrite has never once written to (see
    -- aria.lua's own comment on this near its `health` command) -- it held
    -- exactly one row, frozen since 2026-08-23, always reporting
    -- status='online' regardless of whether the process was actually up.
    -- aria_process_heartbeat is Aria's own real liveness signal (the same
    -- one aria_watchdog trusts, upserted every 15s) -- use that instead.
    local row = db.fetchone("discord_aria", "SELECT EXTRACT(EPOCH FROM (NOW() - updated_at))::int AS age FROM aria_process_heartbeat WHERE id = 1")
    if row then aria_heartbeat_age = db.toint(row.age) end
    -- Real per-process stats (memory/uptime) -- see lib/swarmlua/telemetry.lua's
    -- 30s snapshot loop in aria.lua. Genuinely Aria-specific data for her
    -- own card, not borrowed/summed from the music bots.
    local stats_row = db.fetchone("discord_aria", "SELECT memory_kb, uptime_seconds FROM aria_process_stats WHERE bot_name = 'aria'")
    if stats_row then
      aria_memory_kb = db.toint(stats_row.memory_kb)
      aria_uptime_seconds = db.toint(stats_row.uptime_seconds)
    end
    local count_row = db.fetchone("discord_aria", "SELECT COUNT(*) AS total FROM aria_interactions")
    aria_recent_interaction_count = db.toint(count_row and count_row.total, 0)
    aria_recent_interactions = db.fetchall(
      "discord_aria",
      [[SELECT guild_id, channel_id, user_id, user_name, interaction_type, prompt_text, response_text, created_at
        FROM aria_interactions ORDER BY created_at DESC, id DESC LIMIT 6]]
    )
    local function count_where(sql)
      local r = db.fetchone("discord_aria", sql)
      return db.toint(r and r.total, 0)
    end
    aria_medic_summary.pending_repairs = count_where("SELECT COUNT(*) AS total FROM aria_repair_tasks WHERE status='pending'")
    aria_medic_summary.pending_infra = count_where("SELECT COUNT(*) AS total FROM aria_infra_tasks WHERE status='pending'")
    aria_medic_summary.critical_health = count_where("SELECT COUNT(*) AS total FROM aria_swarm_health WHERE status_label='critical'")
    aria_medic_summary.recoverable_health = count_where("SELECT COUNT(*) AS total FROM aria_swarm_health WHERE status_label IN ('recoverable','degraded')")
    aria_medic_summary.recent_operator_decisions = db.fetchall(
      "discord_aria",
      "SELECT issue_type, bot_name, guild_id, priority_score, urgency_label, created_at FROM aria_operator_decisions ORDER BY created_at DESC, id DESC LIMIT 5"
    )
    aria_medic_summary.recent_infra_history = db.fetchall(
      "discord_aria",
      "SELECT target_name, action_name, issue_type, success, execution_mode, result_text, created_at FROM aria_infra_history ORDER BY created_at DESC, id DESC LIMIT 5"
    )
    aria_medic_summary.recent_swarm_events = db.fetchall(
      "discord_aria",
      "SELECT event_type, bot_name, guild_id, severity, created_at FROM aria_swarm_events ORDER BY created_at DESC, id DESC LIMIT 6"
    )
  end)

  -- BUGFIX 2026-09-14: heartbeat_status used to be copied verbatim from the
  -- dead swarm_health row above (always "online", regardless of reality) --
  -- a second field that could (and did) directly contradict `status`. One
  -- real status, derived from one real source, used for both.
  local aria_online = aria_heartbeat_age ~= nil and aria_heartbeat_age < 120
  local aria_status_real = aria_online and "ONLINE" or "OFFLINE"

  -- BUGFIX 2026-09-14 (per operator report: "the panel treats her as a
  -- music bot" -- her card showed known_guild_count/active_playing_count
  -- that were actually the SUM of every music bot's own guild/playing
  -- counts, mis-attributed onto Aria's entry instead of describing her at
  -- all -- e.g. "22 guilds" and "16 playing" for an orchestrator with no
  -- voice sessions). Aria gets her own custom card shape below: real
  -- process stats (memory/uptime, from aria_process_stats -- see
  -- lib/swarmlua/telemetry.lua's snapshot loop in aria.lua) and her real
  -- medic/interaction data, with NO music-bot-shaped fields that don't
  -- apply to her (no sessions/known_guild_count/active_playing_count --
  -- omitted, not zeroed, so a consuming UI can tell "not applicable" apart
  -- from "zero of these right now").
  bots[#bots + 1] = {
    key = "aria", display_name = "Aria", kind = "orchestrator", schema = "discord_aria",
    status = aria_status_real,
    heartbeat_age_seconds = aria_heartbeat_age,
    memory_kb = aria_memory_kb,
    uptime_seconds = aria_uptime_seconds,
    recent_interactions = aria_recent_interactions,
    recent_interaction_count = aria_recent_interaction_count,
    medic_summary = aria_medic_summary,
  }

  -- Telemetry (per operator request): edge-triggered, not per-call -- this
  -- function runs on the ~2s WS broadcast loop, so recording every call
  -- would flood the table for a signal that's only actually interesting on
  -- a state change. last_known_aria_online is a module-level local (see top
  -- of file) so this correctly fires once per real transition regardless of
  -- how many times get_dashboard_data runs per second.
  if last_known_aria_online ~= nil and last_known_aria_online ~= aria_online then
    pcall(telemetry.record, "aria", aria_online and "online" or "offline", {
      numeric_value = aria_heartbeat_age,
      metadata = { memory_kb = aria_memory_kb, uptime_seconds = aria_uptime_seconds },
    })
  end
  last_known_aria_online = aria_online

  -- pcall'd: Redis being unreachable must degrade to node_health.*=unknown,
  -- not take the whole dashboard route down (get_node_health() already
  -- pcalls its own Redis calls internally, but wrapping the call itself too
  -- guards against a require-time/connection-construction failure).
  local node_health_ok, node_health = pcall(M.get_node_health)

  return {
    generated_at = os.date("!%Y-%m-%dT%H:%M:%SZ"),
    bots = bots,
    -- BUGFIX: this fallback predates the 2026-08-17 lavalink2/lavalink3
    -- nodes (NODE_NAMES above) -- harmless today since pages_fleet.lua defaults
    -- any missing key to {status="unknown"} anyway, but keep it in sync
    -- with NODE_NAMES so a future reader doesn't have to know that.
    node_health = node_health_ok and node_health or { lavalink = { status = "unknown" }, lavalink2 = { status = "unknown" }, lavalink3 = { status = "unknown" }, nodelink = { status = "unknown" } },
  }
end

-- Short-lived shared snapshot. get_dashboard_data() is called by the WS
-- broadcast loop once per distinct scope per tick (every guild-scoped
-- account, admin, ...), by /api/dashboard, by every Dashboard/Controls page
-- render, and by the invites roster -- and each call is ~10 blocking
-- queries per bot across 12 bot databases plus Aria's. Every one of those
-- calls stalls the whole copas loop, so with a few scopes connected the
-- server spent most of each 2s tick rebuilding the identical fleet state.
-- All callers now share one build per SNAPSHOT_TTL_SECONDS. Callers mutate
-- the result (scoping filters bots/sessions, enrichment adds fields), so
-- each one gets its own deep copy rather than the cached table itself.
local socket = require("socket")
local SNAPSHOT_TTL_SECONDS = 1.5
local snapshot_cache = nil -- { expires = <gettime>, bots_key = <music_bots table>, data = <table> }

local function deep_copy(v)
  if type(v) ~= "table" then return v end
  local out = {}
  for k, val in pairs(v) do out[k] = deep_copy(val) end
  return setmetatable(out, getmetatable(v))
end

function M.get_dashboard_data(music_bots)
  local now = socket.gettime()
  local hit = snapshot_cache
  if hit and hit.bots_key == music_bots and hit.expires > now then
    return deep_copy(hit.data)
  end
  local data = build_dashboard_data(music_bots)
  snapshot_cache = { expires = socket.gettime() + SNAPSHOT_TTL_SECONDS, bots_key = music_bots, data = data }
  return deep_copy(data)
end

-- Port of app/db/bots.py's _empty_music_activity_summary() /
-- get_music_activity_summary_for_guilds(). Used by users.lua/accounts.lua for
-- the account-profile and directory-search "activity" field. Simplified
-- relative to the Python original: computed directly per call (no TTL cache,
-- no separate get_music_intelligence_summary indirection) since profile/search
-- traffic here is low-volume; functionally equivalent output shape.
function M.empty_music_activity_summary()
  return {
    top_tracks = {}, top_bots = {}, active_sessions = {}, total_plays = 0,
    learned_tracks = 0, smart_likes = 0, smart_dislikes = 0,
    smart_recommendations = 0, top_smart_tracks = {},
  }
end

local function in_clause_and_params(guild_ids)
  local marks, params = {}, {}
  for i, gid in ipairs(guild_ids) do
    marks[i] = "%s"
    params[i] = tostring(gid)
  end
  return table.concat(marks, ", "), params
end

function M.get_music_activity_summary_for_guilds(music_bots, guild_ids)
  local seen, normalized = {}, {}
  for _, gid in ipairs(guild_ids or {}) do
    local g = tostring(gid or "")
    if g ~= "" and not seen[g] then
      seen[g] = true
      normalized[#normalized + 1] = g
    end
  end
  local summaries = {}
  for _, gid in ipairs(normalized) do summaries[gid] = M.empty_music_activity_summary() end
  if #normalized == 0 then return summaries end

  local bot_index = {}
  for _, bot in ipairs(music_bots) do bot_index[bot.key] = bot end

  local in_sql, in_params = in_clause_and_params(normalized)
  local per_guild_tracks, per_guild_bots, per_guild_active = {}, {}, {}
  for _, gid in ipairs(normalized) do
    per_guild_tracks[gid] = {}
    per_guild_bots[gid] = {}
    per_guild_active[gid] = {}
  end

  for _, bot in ipairs(music_bots) do
    local ok = pcall(function()
      local schema, prefix = bot.db_schema, bot.table_prefix
      local history_table = prefix .. "_history"
      local playback_table = prefix .. "_playback_state"
      local intel_table = prefix .. "_track_intelligence"
      local reco_table = prefix .. "_smart_recommendations"
      local exists = db.batch_table_exists(schema, { history_table, playback_table, intel_table, reco_table })

      if exists[history_table] then
        local rows = db.fetchall(
          schema,
          string.format(
            [[SELECT guild_id, title, video_url, COUNT(*) AS plays, MAX(played_at) AS last_played_at
              FROM %s WHERE guild_id IN (%s) AND title IS NOT NULL AND title != ''
              GROUP BY guild_id, title, video_url ORDER BY plays DESC, last_played_at DESC LIMIT 300]],
            history_table, in_sql
          ),
          unpack(in_params)
        )
        for _, row in ipairs(rows) do
          local gid = tostring(row.guild_id)
          local title = tostring(row.title or "Unknown Track"):match("^%s*(.-)%s*$")
          local plays = db.toint(row.plays, 0)
          local key = title:lower()
          local tracks = per_guild_tracks[gid]
          if tracks then
            local existing = tracks[key]
            if not existing then
              existing = { title = title, video_url = row.video_url, plays = 0 }
              tracks[key] = existing
            end
            existing.plays = existing.plays + plays
            per_guild_bots[gid][bot.key] = (per_guild_bots[gid][bot.key] or 0) + plays
          end
        end
      end

      if exists[playback_table] then
        local rows = db.fetchall(
          schema,
          string.format(
            [[SELECT guild_id, title, video_url, is_playing, is_paused FROM %s
              WHERE guild_id IN (%s) AND (is_playing = TRUE OR is_paused = TRUE)]],
            playback_table, in_sql
          ),
          unpack(in_params)
        )
        for _, row in ipairs(rows) do
          local gid = tostring(row.guild_id)
          if per_guild_active[gid] then
            table.insert(per_guild_active[gid], {
              bot_key = bot.key, bot_display = bot.display_name,
              title = row.title or "Unknown Track", video_url = row.video_url,
              is_playing = db.tobool(row.is_playing), is_paused = db.tobool(row.is_paused),
            })
          end
        end
      end

      if exists[intel_table] then
        local rows = db.fetchall(
          schema,
          string.format(
            [[SELECT guild_id, COUNT(*) AS learned_tracks, COALESCE(SUM(play_count),0) AS plays,
                     COALESCE(SUM(like_count),0) AS likes, COALESCE(SUM(dislike_count),0) AS dislikes
              FROM %s WHERE guild_id IN (%s) GROUP BY guild_id]],
            intel_table, in_sql
          ),
          unpack(in_params)
        )
        for _, row in ipairs(rows) do
          local gid = tostring(row.guild_id)
          local s = summaries[gid]
          if s then
            s.learned_tracks = s.learned_tracks + db.toint(row.learned_tracks, 0)
            s.smart_likes = s.smart_likes + db.toint(row.likes, 0)
            s.smart_dislikes = s.smart_dislikes + db.toint(row.dislikes, 0)
          end
        end

        local top_rows = db.fetchall(
          schema,
          string.format(
            [[SELECT guild_id, title,
                     ((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5)) AS smart_score
              FROM %s WHERE guild_id IN (%s) ORDER BY smart_score DESC LIMIT 20]],
            intel_table, in_sql
          ),
          unpack(in_params)
        )
        for _, row in ipairs(top_rows) do
          local gid = tostring(row.guild_id)
          local s = summaries[gid]
          if s then
            table.insert(s.top_smart_tracks, { bot_key = bot.key, bot_display = bot.display_name, title = row.title, smart_score = tonumber(row.smart_score) or 0 })
          end
        end
      end

      if exists[reco_table] then
        local rows = db.fetchall(
          schema,
          string.format("SELECT guild_id, COUNT(*) AS recommendations FROM %s WHERE guild_id IN (%s) GROUP BY guild_id", reco_table, in_sql),
          unpack(in_params)
        )
        for _, row in ipairs(rows) do
          local s = summaries[tostring(row.guild_id)]
          if s then s.smart_recommendations = s.smart_recommendations + db.toint(row.recommendations, 0) end
        end
      end
    end)
    -- A single misbehaving bot schema must not blow up the whole summary —
    -- matches the Python version's per-bot exception swallowing (logger.debug).
  end

  for _, gid in ipairs(normalized) do
    local tracks_list = {}
    for _, t in pairs(per_guild_tracks[gid]) do tracks_list[#tracks_list + 1] = t end
    table.sort(tracks_list, function(a, b)
      if a.plays ~= b.plays then return a.plays > b.plays end
      return a.title:lower() < b.title:lower()
    end)
    local bots_list = {}
    for bot_key, plays in pairs(per_guild_bots[gid]) do bots_list[#bots_list + 1] = { bot_key = bot_key, plays = plays } end
    table.sort(bots_list, function(a, b)
      if a.plays ~= b.plays then return a.plays > b.plays end
      return a.bot_key < b.bot_key
    end)

    local s = summaries[gid]
    local total_plays = 0
    for _, b in ipairs(bots_list) do total_plays = total_plays + b.plays end
    s.total_plays = total_plays
    for i = 1, math.min(3, #tracks_list) do s.top_tracks[i] = tracks_list[i] end
    for i = 1, math.min(3, #bots_list) do
      local bot_def = bot_index[bots_list[i].bot_key]
      s.top_bots[i] = { bot_key = bots_list[i].bot_key, bot_display = (bot_def and bot_def.display_name) or bots_list[i].bot_key, plays = bots_list[i].plays }
    end
    for i = 1, math.min(4, #per_guild_active[gid]) do s.active_sessions[i] = per_guild_active[gid][i] end
    table.sort(s.top_smart_tracks, function(a, b) return (a.smart_score or 0) > (b.smart_score or 0) end)
    local trimmed = {}
    for i = 1, math.min(3, #s.top_smart_tracks) do trimmed[i] = s.top_smart_tracks[i] end
    s.top_smart_tracks = trimmed
  end

  return summaries
end

-- Swarm-wide leaderboard ("most played track across all N bots this week"):
-- item #2 from the Postgres-migration feature brainstorm. Every bot got its
-- own dedicated Postgres database in the MariaDB->Postgres migration this
-- panel just got fully wired up for (see config.lua/control.lua), so this
-- was previously impossible without either a shared MariaDB schema (item #2
-- calls out the old per-bot-MariaDB-silo problem directly) or 13 manual
-- per-bot queries -- now it's 13 same-shaped Postgres queries fanned out and
-- merged in application code (Postgres connections are per-database, see
-- db.lua's own header comment, so a single cross-bot SQL query still isn't
-- possible -- this is the "combined in application code" approach that
-- comment already anticipated). Deliberately does NOT try to de-duplicate
-- the same song across different bots/guilds (title/url_key formatting
-- isn't guaranteed consistent swarm-wide) -- each row is tagged with which
-- bot it came from instead, which is more honest than a merge that might be
-- silently wrong.
function M.get_swarm_leaderboard(music_bots, opts)
  opts = opts or {}
  local days = math.max(1, math.min(math.floor(tonumber(opts.days) or 7), 365))
  local limit = math.max(1, math.min(math.floor(tonumber(opts.limit) or 20), 100))
  local per_bot_fetch = math.min(limit, 50) -- cap what we pull per bot before the final merge/sort

  local rows = {}
  local bots_queried, bots_errored = 0, 0
  for _, bot in ipairs(music_bots) do
    if bot.kind == "music" then
      local schema = bot.db_schema
      local prefix = bot.table_prefix
      local table_name = prefix .. "_track_intelligence"
      local ok_exists, exists = pcall(db.table_exists, schema, table_name)
      if ok_exists and exists then
        bots_queried = bots_queried + 1
        local ok_rows, bot_rows = pcall(
          db.fetchall,
          schema,
          [[SELECT title, video_url, play_count, finish_count, skip_count, like_count, dislike_count, last_played,
                   ((finish_count * 3) + (like_count * 5) + play_count - (skip_count * 2) - (dislike_count * 5)) AS smart_score
            FROM ]] .. table_name .. [[
            WHERE last_played IS NOT NULL AND last_played >= NOW() - (%s || ' days')::interval
            ORDER BY play_count DESC
            LIMIT %s]],
          tostring(days), per_bot_fetch
        )
        if ok_rows and bot_rows then
          for _, r in ipairs(bot_rows) do
            r.bot_key = bot.key
            r.bot_display = bot.display_name
            r.play_count = db.toint(r.play_count, 0)
            r.finish_count = db.toint(r.finish_count, 0)
            r.skip_count = db.toint(r.skip_count, 0)
            r.like_count = db.toint(r.like_count, 0)
            r.dislike_count = db.toint(r.dislike_count, 0)
            r.smart_score = db.toint(r.smart_score, 0)
            rows[#rows + 1] = r
          end
        else
          bots_errored = bots_errored + 1
        end
      end
    end
  end

  table.sort(rows, function(a, b)
    if a.play_count ~= b.play_count then return a.play_count > b.play_count end
    return (a.smart_score or 0) > (b.smart_score or 0)
  end)

  local tracks = {}
  for i = 1, math.min(limit, #rows) do tracks[i] = rows[i] end

  return {
    window_days = days,
    generated_at = os.date("!%Y-%m-%dT%H:%M:%SZ"),
    bots_queried = bots_queried,
    bots_errored = bots_errored,
    tracks = tracks,
  }
end

return M
