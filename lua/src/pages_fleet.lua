-- Fleet section: Dashboard, Controls, Invites -- live bot status and direct
-- playback control. Pattern shared by every page module: the shell and
-- static structure render server-side via html.lua, live/interactive data
-- is populated by inline JS calling the same JSON API (swarmFetch /
-- swarmLive, from static/app.js).
local httpd = require("httpd")
local html = require("html")
local accounts = require("accounts")
local kit = require("page_kit")
local dashboard = require("dashboard")
local config = require("config")

local M = {}

-- BUGFIX 2026-08-22: the "Audio Nodes: Healthy/Checking" summary badges
-- (boot screen + dashboard spotlight, below) used to check ONLY the
-- "lavalink" (primary) node's status -- accurate back when that was the
-- only real node, but since the 2026-08-17 3-node pool + per-bot
-- node_affinity rotation (see Music/lua-shared/swarmlua/bot.lua/
-- nodepool.lua), many bots' actual preferred node is lavalink2 or
-- lavalink3, not "lavalink". A primary that happens to be down while both
-- pool nodes are fine (the fleet keeps working fine via failover) used to
-- show "Checking" here forever; the reverse -- lavalink2/lavalink3 both
-- down while the primary happens to be fine -- used to show "Healthy" with
-- 2 of 3 real nodes silently degraded. Healthy here now means "the fleet
-- still has at least one working real Lavalink node" -- NodeLink is a
-- last-resort fallback, deliberately not counted toward this summary the
-- same way it isn't counted as one of the "real" nodes in bot.lua's own
-- node_affinity rotation.
local function any_lavalink_node_healthy(node_health)
  node_health = node_health or {}
  for _, name in ipairs({ "lavalink", "lavalink2", "lavalink3" }) do
    if (node_health[name] or {}).status == "healthy" then return true end
  end
  return false
end

local CONTROL_ACTIONS = {
  "PLAY", "SMART_RECOMMEND", "PAUSE", "RESUME", "SKIP", "STOP", "CLEAR",
  "RESET_QUEUE", "SHUFFLE", "LOOP", "FILTER", "LEAVE", "SET_HOME", "RECOVER", "RESTART",
}

function M.register(cfg)
  local settings = cfg.settings
  local music_bots = cfg.music_bots
  local session_view, page_shell, denied = kit.session_view, kit.page_shell, kit.denied

  -- ---------------------------------------------------------------------
  -- Dashboard -- mirrors pages/DashboardPage.jsx
  -- ---------------------------------------------------------------------
  local function render_dashboard(req, path)
    local a, redirect_status, redirect_headers = cfg.require_auth_page(req)
    if not a then return redirect_status, "", redirect_headers end

    -- Scoped to the caller's guild, same as the live push (see
    -- routes.lua's scoped_dashboard_data).
    local data = cfg.scoped_dashboard_data(a)

    -- Mirrors swarm.jsx's bestSession()/playbackBadge(): the featured
    -- session for a card is whichever guild is actually playing, falling
    -- back to the first known session, then the badge reflects that
    -- session's state (or the bot's own heartbeat health when nothing is
    -- playing at all).
    local function best_session(bot)
      for _, s in ipairs(bot.sessions or {}) do
        if s.is_playing then return s end
      end
      return bot.sessions and bot.sessions[1] or nil
    end
    -- Was checking session.is_playing BEFORE offline/stale status, so a bot
    -- that went offline mid-track (its last DB-persisted session row still
    -- has is_playing=true from before it dropped) still showed a "Live"
    -- badge and a playback counter that could never advance again -- which
    -- is what actually produced several of the "duration stuck" bot cards:
    -- not a display bug, a genuinely dead bot whose last-known state was
    -- being trusted as current. Offline/stale is checked first now.
    local function bot_is_offline(bot)
      local status_lower = tostring(bot.status or ""):lower()
      if status_lower == "offline" then return true end
      local age = tonumber(bot.heartbeat_age_seconds)
      if age and age > 120 then return true end
      return false
    end
    -- 2026-10-21 (per operator request: "the online system needs
    -- revamping"): checked before is_playing/is_paused on purpose -- a bot
    -- can be mid-flap (TrackStartEvent firing repeatedly between Discord
    -- kills, confirmed live the same day) and still have session.is_playing
    -- true at the instant this renders, which would otherwise show a
    -- reassuring "Live" pill for a bot that's actually looping. See
    -- dashboard.lua's node_status for what sets status="degraded".
    local function voice_degraded(bot)
      return tostring(bot.status or ""):lower() == "degraded"
    end
    local function playback_badge(session, bot)
      if bot_is_offline(bot) then return "danger", "Offline" end
      if voice_degraded(bot) then return "danger", "Degraded" end
      if session and session.is_playing then return "live", "Live" end
      if session and session.is_paused then return "soft", "Paused" end
      local status_lower = tostring(bot.heartbeat_status or bot.status or ""):lower()
      if status_lower:find("stale") then return "danger", "Stale" end
      return "off", "Idle"
    end


    -- "2h 14m" / "34m" / "45s" -- for Aria's own custom card below (see
    -- BUGFIX 2026-09-14 just below the bot_cards loop). No existing helper
    -- for this anywhere in the codebase; every other "time ago" rendering
    -- on this page is a raw "%ds ago" (see the heartbeat chip further
    -- down), which reads fine for seconds/minutes but not for a
    -- multi-hour process uptime.
    local function format_uptime(seconds)
      seconds = math.floor(tonumber(seconds) or 0)
      if seconds < 60 then return seconds .. "s" end
      local minutes = math.floor(seconds / 60)
      if minutes < 60 then return minutes .. "m" end
      local hours = math.floor(minutes / 60)
      minutes = minutes % 60
      if hours < 24 then return ("%dh %dm"):format(hours, minutes) end
      local days = math.floor(hours / 24)
      hours = hours % 24
      return ("%dd %dh"):format(days, hours)
    end

    local bot_cards = {}
    local all_sessions = {}
    for _, bot in ipairs(data.bots) do
      -- BUGFIX 2026-09-14 (per operator report, screenshot confirmed: "the
      -- panel is still treating Aria's card like it's a music bot"): this
      -- loop rendered every single bot.kind through the identical
      -- now-playing/seek-bar/"X live, X guilds, X queued, X backup" music
      -- card template, Aria included -- dashboard.lua's own fix (removing
      -- her borrowed known_guild_count/active_playing_count numbers, adding
      -- real memory_kb/uptime_seconds) only fixed the DATA; this template
      -- never had a branch to render those real fields differently, so her
      -- card just showed the same layout with zeros instead of wrong
      -- numbers, and her real fields (memory/uptime/medic summary) weren't
      -- displayed anywhere at all. Orchestrator (Aria) now gets her own
      -- card body entirely -- real process stats and medic/interaction
      -- counts, no now-playing thumbnail/seek bar/queue chips that don't
      -- apply to her.
      if bot.kind == "orchestrator" then
        local medic = bot.medic_summary or {}
        local uptime_chip = bot.uptime_seconds and ("up " .. html.esc(format_uptime(bot.uptime_seconds))) or ""
        local memory_chip = bot.memory_kb and (tostring(math.floor(bot.memory_kb / 1024)) .. " MB mem") or ""
        local heartbeat_chip = (tonumber(bot.heartbeat_age_seconds) ~= nil)
          and ("heartbeat " .. math.floor(bot.heartbeat_age_seconds) .. "s ago")
          or ""
        local offline_overlay = bot_is_offline(bot)
          and '<div class="bot-card-offline-overlay"><span class="bot-card-offline-label">Offline</span></div>' or ""
        bot_cards[#bot_cards + 1] = ([[
          <article class="bot-card bot-card-orchestrator%s" data-bot-key="%s" style="--card-accent: %s">
            <div data-bot-offline-overlay>%s</div>
            <div class="bot-head">
              <span class="bot-dot"></span>
              <div class="bot-head-copy">
                <h3>%s</h3>
                <small>Autonomous swarm orchestrator</small>
              </div>
              <span class="data-pill data-pill-%s" data-bot-badge>%s</span>
            </div>
            <div class="bot-now">
              <div class="bot-thumb bot-thumb-empty">&#9881;</div>
              <div class="bot-now-copy">
                <strong data-bot-now-title>%d pending repair%s, %d pending infra task%s</strong>
                <small data-bot-now-sub>%d interaction%s recorded &middot; %d critical, %d recoverable health issue%s</small>
              </div>
            </div>
            <div class="chip-row">
              <span data-chip="uptime">%s</span>
              <span data-chip="memory">%s</span>
              <span data-chip="heartbeat">%s</span>
            </div>
          </article>
        ]]):format(
          offline_overlay ~= "" and " bot-card-offline" or "", html.esc(bot.key), html.esc(config.bot_accents[bot.key] or "#cba6f7"),
          offline_overlay,
          html.esc(bot.display_name),
          bot_is_offline(bot) and "danger" or "live", bot_is_offline(bot) and "Offline" or "Online",
          medic.pending_repairs or 0, (medic.pending_repairs == 1) and "" or "s",
          medic.pending_infra or 0, (medic.pending_infra == 1) and "" or "s",
          bot.recent_interaction_count or 0, (bot.recent_interaction_count == 1) and "" or "s",
          medic.critical_health or 0, medic.recoverable_health or 0, (medic.recoverable_health == 1) and "" or "s",
          uptime_chip, memory_chip, heartbeat_chip)
        goto continue_bot_card
      end

      local session = best_session(bot)
      local tone, label = playback_badge(session, bot)
      local accent = config.bot_accents[bot.key] or "#89b4fa"

      local thumb
      if session and session.thumbnail and session.thumbnail ~= "" then
        thumb = ('<img class="bot-thumb" src="%s" alt="" loading="lazy">'):format(html.esc(session.thumbnail))
      else
        thumb = '<div class="bot-thumb bot-thumb-empty">&#9835;</div>'
      end

      local now_title = (session and session.title and session.title ~= "") and session.title
        or bot.error or bot.schema or "Waiting for live playback."
      local now_sub = (session and (session.media_source_label or session.session_state_label))
        or "Live state will fill in automatically."

      local playback_block = ""
      if session then
        local duration = math.floor(session.duration_seconds or 0)
        local pct = (duration > 0) and math.min(100, math.floor(100 * (session.position_seconds or 0) / duration)) or 0
        -- data-playback-bar must live INSIDE the data-playback-counter
        -- element -- app.js's tickPlaybackCounters() finds the bar via
        -- el.querySelector() scoped to the counter element, so a sibling
        -- bar (as this used to be) is never found and its width just
        -- freezes at whatever the server rendered on page load.
        -- bot-seek-thumb/-times had CSS (a drag handle riding the fill, plus
        -- a split current/duration time row) but the seek bar only ever
        -- rendered the bare track/fill -- no handle, and the single inline
        -- data-playback-label wasn't split the way .bot-seek-times expects.
        playback_block = ([[
          <div class="bot-playback-wrap" data-playback-counter data-position="%s" data-observed-at="%s" data-duration="%s" data-playing="%s">
            <div class="bot-seek-bar" data-seek-bar data-bot-key="%s" data-guild-id="%s" data-duration="%d">
              <div class="bot-seek-track">
                <div class="bot-seek-fill" data-playback-bar style="width:%d%%"></div>
                <div class="bot-seek-thumb" data-seek-thumb style="left:%d%%"></div>
              </div>
            </div>
            <div class="bot-seek-times"><span data-seek-current></span><span data-seek-duration></span></div>
          </div>
        ]]):format(
          tostring(session.position_seconds or 0), tostring(session.position_observed_at or 0),
          tostring(session.duration_seconds or 0), tostring(session.is_playing == true),
          html.esc(bot.key), html.esc(session.guild_id), duration, pct, pct)
      end

      local offline_overlay = bot_is_offline(bot)
        and '<div class="bot-card-offline-overlay"><span class="bot-card-offline-label">Offline</span></div>' or ""

      bot_cards[#bot_cards + 1] = ([[
        <article class="bot-card%s" data-bot-key="%s" style="--card-accent: %s">
          %s
          <div class="bot-head">
            <span class="bot-dot"></span>
            <div class="bot-head-copy">
              <h3>%s</h3>
              <small>%s</small>
            </div>
            <span class="data-pill data-pill-%s">%s</span>
          </div>
          <div class="bot-now">
            %s
            <div class="bot-now-copy">
              <strong>%s</strong>
              <small>%s</small>
            </div>
          </div>
          %s
          <div class="chip-row">
            <span>%d live</span>
            <span>%d guilds</span>
            <span data-queue-pressure>%d queued</span>
            <span data-queue-pressure>%d backup</span>
            %s
          </div>
        </article>
      ]]):format(offline_overlay ~= "" and " bot-card-offline" or "", html.esc(bot.key), html.esc(accent),
        offline_overlay,
        html.esc(bot.display_name), html.esc(bot.heartbeat_status or bot.status or "telemetry ready"),
        tone, label,
        thumb,
        html.esc(now_title), html.esc(now_sub),
        playback_block,
        bot.active_playing_count or 0, bot.known_guild_count or 0,
        bot.queue_depth or 0, bot.backup_queue_depth or 0,
        -- show_bot_uptime ("Show bot uptime" on /appearance): saved but
        -- never had any stat to toggle. heartbeat_age_seconds is real,
        -- live data already flowing through /api/dashboard (used for
        -- offline detection above) -- surfaced here as "last heartbeat"
        -- rather than inventing a fabricated process-uptime number, since
        -- no bot actually persists a process start time anywhere.
        (tonumber(bot.heartbeat_age_seconds) ~= nil)
          and ('<span data-bot-uptime>heartbeat %ds ago</span>'):format(math.floor(bot.heartbeat_age_seconds))
          or "")

      for _, s in ipairs(bot.sessions or {}) do
        s.bot_key = bot.key
        s.bot_display = bot.display_name
        all_sessions[#all_sessions + 1] = s
      end

      ::continue_bot_card::
    end

    local session_rows = {}
    for _, s in ipairs(all_sessions) do
      if s.is_playing or s.is_paused or (s.title and s.title ~= "") then
        -- Read-only compact counter here, not the draggable bot-seek-bar
        -- (that belongs on the bot cards above, matching the original
        -- BotCard/SessionTable split) -- the seek bar has no width of its
        -- own (100% of its container), so dropped into a wide table column
        -- it stretched across nearly the full row. The bare 160px cap here
        -- matches PlaybackCounter's compact rendering in ControlState.
        session_rows[#session_rows + 1] = ([[
          <tr>
            <td>%s</td>
            <td>%s</td>
            <td>%s</td>
            <td style="max-width:160px">
              <div class="bot-playback compact" data-playback-counter data-position="%s" data-observed-at="%s" data-duration="%s" data-playing="%s">
                <div class="bot-playback-bar" aria-hidden="true"><span data-playback-bar style="width:%d%%"></span></div>
                <span data-playback-label></span>
              </div>
            </td>
            <td>%d queued</td>
          </tr>
        ]]):format(
          html.esc(s.bot_display), html.esc(s.title or "—"), html.esc(s.session_state_label or ""),
          tostring(s.position_seconds or 0), tostring(s.position_observed_at or 0), tostring(s.duration_seconds or 0),
          tostring(s.is_playing == true),
          (s.duration_seconds and s.duration_seconds > 0) and math.min(100, math.floor(100 * (s.position_seconds or 0) / s.duration_seconds)) or 0,
          s.queue_count or 0)
      end
    end

    -- Cross-bot Lavalink/NodeLink health (see dashboard.lua's
    -- get_node_health() -- reads the shared Redis scoreboard every bot's
    -- own Lavalink client writes to on every success/failure, so this is
    -- one shared status, not per-bot). "degraded"/"stale" reuse the same
    -- data-pill-danger/data-pill-off tones the bot cards above already use
    -- for offline/idle, so a red pill here reads the same way it does
    -- everywhere else on this page.
    local NODE_HEALTH_TONE = { healthy = "live", degraded = "danger", stale = "off", unknown = "off" }
    local NODE_HEALTH_LABEL = { healthy = "Healthy", degraded = "Degraded", stale = "Stale", unknown = "No data yet" }
    -- BUGFIX 2026-08-22: matches dashboard.lua's get_node_health() fix --
    -- this hardcoded 2-node list independently had the exact same gap
    -- (missing lavalink2/lavalink3, the 2 extra real Lavalink instances
    -- added 2026-08-17 to spread the fleet's voice-session load), so even
    -- with that fix, THIS page still wouldn't have rendered them: a
    -- degraded/down node on 2 of the 3 real Lavalink instances could sit
    -- invisible here indefinitely, same failure mode.
    local NODE_DISPLAY_NAME = {
      lavalink = "Lavalink (primary)", lavalink2 = "Lavalink 2", lavalink3 = "Lavalink 3",
      nodelink = "NodeLink (backup)",
    }
    local node_pills = {}
    for _, node_name in ipairs({ "lavalink", "lavalink2", "lavalink3", "nodelink" }) do
      local h = (data.node_health or {})[node_name] or { status = "unknown" }
      local tone = NODE_HEALTH_TONE[h.status] or "off"
      local label = NODE_HEALTH_LABEL[h.status] or "Unknown"
      local detail
      if h.status == "healthy" and h.last_success_age_seconds then
        detail = ("last success %ds ago"):format(h.last_success_age_seconds)
      elseif h.consecutive_failures and h.consecutive_failures > 0 then
        detail = ("%d consecutive failures"):format(h.consecutive_failures)
      else
        detail = "no recent activity"
      end
      node_pills[#node_pills + 1] = ([[
        <div class="bot-card" style="--card-accent: #89b4fa">
          <div class="bot-head">
            <span class="bot-dot"></span>
            <div class="bot-head-copy">
              <h3>%s</h3>
              <small>%s</small>
            </div>
            <span class="data-pill data-pill-%s">%s</span>
          </div>
        </div>
      ]]):format(html.esc(NODE_DISPLAY_NAME[node_name] or node_name), html.esc(detail), tone, label)
    end

    -- Boot screen (swarm-loading-*) uses real numbers already computed
    -- above, not placeholders -- an operator landing on the dashboard sees
    -- an accurate snapshot for the ~1s the panel is visible, not a fake
    -- progress bar. Fades itself out client-side (fully rendered content is
    -- already behind it since this is a server-rendered page, not an
    -- actual loading gate).
    local online_bots, live_count = 0, 0
    for _, bot in ipairs(data.bots) do
      if not bot_is_offline(bot) then online_bots = online_bots + 1 end
    end
    for _, s in ipairs(all_sessions) do
      if s.is_playing then live_count = live_count + 1 end
    end
    local boot_screen = ([[
      <div class="swarm-loading-screen" id="boot-screen">
        <div class="swarm-loading-backdrop"></div>
        <div class="swarm-loading-panel">
          <div class="swarm-loading-hero">
            <div class="swarm-loading-radar">
              <div class="swarm-loading-ring ring-a"></div>
              <div class="swarm-loading-ring ring-b"></div>
              <div class="swarm-loading-ring ring-c"></div>
              <div class="swarm-loading-sweep"></div>
              <div class="swarm-loading-core"></div>
            </div>
            <div class="swarm-loading-copy">
              <span class="swarm-loading-kicker">Fleet Command</span>
              <strong>SwarmPanel</strong>
              <p>Syncing with the swarm...</p>
            </div>
          </div>
          <div class="swarm-loading-status-grid">
            <article><span>Bots Online</span><strong>%d / %d</strong><small>heartbeat within 120s</small></article>
            <article><span>Live Sessions</span><strong>%d</strong><small>currently playing</small></article>
            <article><span>Audio Nodes</span><strong>%s</strong><small>Lavalink / NodeLink</small></article>
          </div>
          <div class="swarm-loading-progress"><span></span></div>
        </div>
      </div>
      <script>
        (function () {
          var el = document.getElementById("boot-screen");
          if (!el) return;
          setTimeout(function () {
            el.classList.add("is-leaving");
            setTimeout(function () { el.remove(); }, 420);
          }, 650);
        })();
      </script>
    ]]):format(
      online_bots, #data.bots, live_count,
      any_lavalink_node_healthy(data.node_health) and "Healthy" or "Checking"
    )

    -- Fleet Overview spotlight: dashboard-spotlight/-metrics/-mini-metrics/
    -- -queue-leaders/-queue-card all had full CSS with no HTML ever built
    -- for them. Real aggregates from the same data.bots the cards below
    -- already render from, not fabricated numbers.
    local total_queue, total_backup, total_guilds = 0, 0, 0
    local busiest_bot = nil
    for _, bot in ipairs(data.bots) do
      total_queue = total_queue + (bot.queue_depth or 0)
      total_backup = total_backup + (bot.backup_queue_depth or 0)
      total_guilds = total_guilds + (bot.known_guild_count or 0)
      if bot.kind == "music" and (not busiest_bot or (bot.queue_depth or 0) > (busiest_bot.queue_depth or 0)) then
        busiest_bot = bot
      end
    end
    -- Real Now Playing widget for the spotlight card (bot-playback-head/
    -- dashboard-playback CSS existed with no consumer) -- the first session
    -- actually playing right now, not just the deepest queue.
    local spotlight_session = nil
    for _, s in ipairs(all_sessions) do
      if s.is_playing then spotlight_session = s; break end
    end
    local spotlight_playback = ""
    if spotlight_session then
      local duration = math.floor(spotlight_session.duration_seconds or 0)
      local pct = (duration > 0) and math.min(100, math.floor(100 * (spotlight_session.position_seconds or 0) / duration)) or 0
      spotlight_playback = ([[
        <div class="bot-playback dashboard-playback" data-playback-counter data-position="%s" data-observed-at="%s" data-duration="%s" data-playing="true">
          <div class="bot-playback-head"><strong>%s</strong><small>%s</small></div>
          <div class="bot-playback-bar"><span data-playback-bar style="width:%d%%"></span></div>
          <small data-playback-label></small>
        </div>
      ]]):format(
        tostring(spotlight_session.position_seconds or 0), tostring(spotlight_session.position_observed_at or 0), tostring(spotlight_session.duration_seconds or 0),
        html.esc(spotlight_session.title or "Now Playing"), html.esc(spotlight_session.bot_display or ""), pct
      )
    end

    local queue_leaders_bots = {}
    for _, bot in ipairs(data.bots) do
      if bot.kind == "music" and (bot.queue_depth or 0) > 0 then queue_leaders_bots[#queue_leaders_bots + 1] = bot end
    end
    table.sort(queue_leaders_bots, function(x, y) return (x.queue_depth or 0) > (y.queue_depth or 0) end)
    local queue_leader_cards = {}
    for i = 1, math.min(4, #queue_leaders_bots) do
      local bot = queue_leaders_bots[i]
      queue_leader_cards[#queue_leader_cards + 1] = ([[
        <div class="dashboard-queue-card bot-card">
          <div class="bot-head"><span class="bot-dot"></span><div class="bot-head-copy"><h3>%s</h3></div><span class="data-pill data-pill-off">%d queued</span></div>
        </div>
      ]]):format(html.esc(bot.display_name), bot.queue_depth or 0)
    end

    local spotlight = ([[
      <div class="dashboard-brief-panel">
        <div class="dashboard-spotlight liquid-glass">
          <div class="dashboard-spotlight-head">
            <span class="dashboard-eyebrow">Fleet Overview</span>
            <span class="dashboard-state-badge%s">%s</span>
          </div>
          <strong>%s</strong>
          <p>%s</p>
          %s
          <div class="dashboard-spotlight-metrics">
            <article><span>Bots Online</span><strong id="metric-bots-online">%d / %d</strong><small>heartbeat within 120s</small></article>
            <article><span>Live Sessions</span><strong id="metric-live-sessions">%d</strong><small>currently playing</small></article>
            <article><span>Queue Depth</span><strong id="metric-queue-depth">%d</strong><small><span id="metric-queue-backup">%d</span> in backup</small></article>
            <article><span>Guilds Served</span><strong id="metric-guilds-served">%d</strong><small>across the fleet</small></article>
          </div>
        </div>
      </div>
      <div class="dashboard-mini-metrics">
        <article><span>Audio Nodes</span><strong>%s</strong></article>
        <article><span>Aria Orchestrator</span><strong>%s</strong></article>
      </div>
      %s
      <div class="dashboard-queue-leaders">%s</div>
    ]]):format(
      live_count > 0 and " live" or " idle", live_count > 0 and "Active" or "Idle",
      busiest_bot and (busiest_bot.display_name .. (live_count > 0 and " is carrying live playback" or " has the deepest queue")) or "Fleet is quiet right now",
      busiest_bot and ("%d queued, %d live guild(s)"):format(busiest_bot.queue_depth or 0, busiest_bot.active_playing_count or 0) or "No active bot to highlight.",
      spotlight_playback,
      online_bots, #data.bots, live_count, total_queue, total_backup, total_guilds,
      any_lavalink_node_healthy(data.node_health) and "Healthy" or "Checking",
      (function()
        for _, bot in ipairs(data.bots) do if bot.key == "aria" then return bot.status or "Unknown" end end
        return "Unknown"
      end)(),
      #queue_leader_cards > 0 and html.section_head("Queue Leaders") or "",
      html.join(queue_leader_cards)
    )

    local body = html.page({
      title = "Dashboard",
      eyebrow = "Fleet",
      lede = "Live status across the swarm.",
      body = ([[
        %s
        %s
        <div class="bot-grid">%s</div>
        %s
        <div class="bot-grid" id="bot-cards">%s</div>
        %s
        <div class="table-wrap">
          <table class="data-table" id="sessions-table">
            <thead><tr><th>Bot</th><th>Track</th><th>State</th><th>Position</th><th>Queue</th></tr></thead>
            <tbody>%s</tbody>
          </table>
        </div>
        %s
      ]]):format(
        spotlight,
        html.section_head("Audio Nodes"), html.join(node_pills),
        html.section_head("Bots"), html.join(bot_cards),
        html.section_head("Live Sessions"),
        #session_rows > 0 and html.join(session_rows) or ('<tr><td colspan="5">' .. html.esc("Nothing playing right now.") .. "</td></tr>"),
        -- Incident feed (admin only -- the "events" live key and
        -- /api/events are both admin-gated): the newest bot errors and
        -- Aria Medic events, so trouble shows up on the Dashboard without
        -- a trip to Intel.
        a.admin_mode and ([[
          <div class="section-head"><div><h2>Incidents</h2><p>Latest bot errors and Aria Medic events.</p></div><a class="button-link" href="/intel">Open Intel</a></div>
          <div class="event-list" id="dash-events">%s</div>
        ]]):format(html.empty_state("Waiting for the live feed...")) or ""),
    })

    body = boot_screen .. body .. [[
      <script>
        // BUGFIX: this handler used to do nothing at all -- confirmed live
        // via Playwright, the page never updated without a manual refresh
        // despite its own lede claiming "Live status across the swarm."
        // A first attempt at fixing this reloaded the whole page on every
        // message, but the snapshot's per-session position_seconds/
        // position_observed_at tick essentially every broadcast (~2s,
        // ensure_broadcast_loop in routes.lua), so the server's digest
        // basically always differs while anything anywhere is playing --
        // that caused a reload storm (confirmed: 8 reloads in 15s even
        // throttled), which is worse than the original do-nothing bug, not
        // better. Patching just the 4 spotlight numbers directly from the
        // payload avoided re-deriving the rest of the page, but left every
        // bot card and the session table on whatever position/duration/
        // title/badge the page happened to render at load -- once a track's
        // tickPlaybackCounters() (app.js) clamped its counter to the
        // track's own duration, that card was frozen at "5:36 / 5:36"
        // forever (or stuck showing a track that had already ended and a
        // new one started) until a manual reload. Fixed the same way this
        // page already fixes the spotlight numbers -- patch the DOM directly
        // from the payload instead of reloading -- just extended to the bot
        // cards and session rows below. Mirrors pages_fleet.lua's own
        // bot_is_offline/best_session/playback_badge functions (Lua) so a
        // card looks identical whether it was server-rendered on load or
        // live-patched afterward; keep the two in sync if that logic ever
        // changes.
        function patchDashboardMetrics(data) {
          const bots = (data && data.bots) || [];
          let online = 0, live = 0, queue = 0, backup = 0, guilds = 0;
          for (const bot of bots) {
            const age = Number(bot.heartbeat_age_seconds);
            const offline = String(bot.status || "").toLowerCase() === "offline" || (Number.isFinite(age) && age > 120);
            if (!offline) online++;
            queue += bot.queue_depth || 0;
            backup += bot.backup_queue_depth || 0;
            guilds += bot.known_guild_count || 0;
            for (const s of bot.sessions || []) { if (s.is_playing) live++; }
          }
          const setText = (id, text) => { const el = document.getElementById(id); if (el) el.textContent = text; };
          setText("metric-bots-online", online + " / " + bots.length);
          setText("metric-live-sessions", String(live));
          setText("metric-queue-depth", String(queue));
          setText("metric-queue-backup", String(backup));
          setText("metric-guilds-served", String(guilds));
        }
        function escHtml(s) { return String(s == null ? "" : s).replace(/</g, "&lt;"); }
        function botIsOffline(bot) {
          const statusLower = String(bot.status || "").toLowerCase();
          if (statusLower === "offline") return true;
          const age = Number(bot.heartbeat_age_seconds);
          return Number.isFinite(age) && age > 120;
        }
        function bestSession(bot) {
          for (const s of bot.sessions || []) { if (s.is_playing) return s; }
          return (bot.sessions && bot.sessions[0]) || null;
        }
        function voiceDegraded(bot) {
          return String(bot.status || "").toLowerCase() === "degraded";
        }
        function playbackBadge(session, bot) {
          if (botIsOffline(bot)) return ["danger", "Offline"];
          if (voiceDegraded(bot)) return ["danger", "Degraded"];
          if (session && session.is_playing) return ["live", "Live"];
          if (session && session.is_paused) return ["soft", "Paused"];
          const statusLower = String(bot.heartbeat_status || bot.status || "").toLowerCase();
          if (statusLower.indexOf("stale") !== -1) return ["danger", "Stale"];
          return ["off", "Idle"];
        }
        function renderPlaybackWrap(session, botKey) {
          if (!session) return "";
          const duration = Math.floor(session.duration_seconds || 0);
          const pos = session.position_seconds || 0;
          const pct = duration > 0 ? Math.min(100, Math.floor((100 * pos) / duration)) : 0;
          return `
            <div class="bot-playback-wrap" data-playback-counter data-position="${escHtml(pos)}" data-observed-at="${escHtml(session.position_observed_at || 0)}" data-duration="${escHtml(session.duration_seconds || 0)}" data-playing="${session.is_playing === true}">
              <div class="bot-seek-bar" data-seek-bar data-bot-key="${escHtml(botKey)}" data-guild-id="${escHtml(session.guild_id)}" data-duration="${duration}">
                <div class="bot-seek-track">
                  <div class="bot-seek-fill" data-playback-bar style="width:${pct}%"></div>
                  <div class="bot-seek-thumb" data-seek-thumb style="left:${pct}%"></div>
                </div>
              </div>
              <div class="bot-seek-times"><span data-seek-current></span><span data-seek-duration></span></div>
            </div>`;
        }
        function renderBotCardInner(bot) {
          const session = bestSession(bot);
          const [tone, label] = playbackBadge(session, bot);
          const thumb = (session && session.thumbnail)
            ? `<img class="bot-thumb" src="${escHtml(session.thumbnail)}" alt="" loading="lazy">`
            : '<div class="bot-thumb bot-thumb-empty">&#9835;</div>';
          const nowTitle = (session && session.title) || bot.error || bot.schema || "Waiting for live playback.";
          const nowSub = (session && (session.media_source_label || session.session_state_label)) || "Live state will fill in automatically.";
          const offline = botIsOffline(bot);
          const offlineOverlay = offline ? '<div class="bot-card-offline-overlay"><span class="bot-card-offline-label">Offline</span></div>' : "";
          // Text is filled in by patchHeartbeat(): it changes on every push,
          // and keeping it out of the markup is what lets patchBotCards()
          // skip rebuilding a card whose real content hasn't changed.
          const uptimeSpan = Number.isFinite(Number(bot.heartbeat_age_seconds)) ? "<span data-bot-uptime></span>" : "";
          return `
            ${offlineOverlay}
            <div class="bot-head">
              <span class="bot-dot"></span>
              <div class="bot-head-copy">
                <h3>${escHtml(bot.display_name)}</h3>
                <small>${escHtml(bot.heartbeat_status || bot.status || "telemetry ready")}</small>
              </div>
              <span class="data-pill data-pill-${tone}">${escHtml(label)}</span>
            </div>
            <div class="bot-now">
              ${thumb}
              <div class="bot-now-copy">
                <strong>${escHtml(nowTitle)}</strong>
                <small>${escHtml(nowSub)}</small>
              </div>
            </div>
            ${renderPlaybackWrap(session, bot.key)}
            <div class="chip-row">
              <span>${bot.active_playing_count || 0} live</span>
              <span>${bot.known_guild_count || 0} guilds</span>
              <span data-queue-pressure>${bot.queue_depth || 0} queued</span>
              <span data-queue-pressure>${bot.backup_queue_depth || 0} backup</span>
              <span data-bot-ping></span>
              ${uptimeSpan}
            </div>`;
        }
        // 2026-10-21 (per operator request: cards need "a actively always
        // accurate tracked ms ping"): same live-patch pattern as
        // patchHeartbeat below -- ping_ms comes from dashboard.lua's direct
        // Lavalink query, refreshed every ~2s broadcast tick, and needs to
        // update without a full card rebuild for the same stutter reason
        // patchHeartbeat already exists. Sourced from the bot's currently
        // playing session; a bot with no active session (nothing actually
        // connected to voice right now) shows no ping chip at all, rather
        // than a stale or fabricated number.
        function patchPing(card, bot) {
          const el = card.querySelector("[data-bot-ping]");
          if (!el) return;
          const session = bestSession(bot);
          const ping = session ? Number(session.ping_ms) : NaN;
          if (!Number.isFinite(ping)) { el.textContent = ""; el.removeAttribute("data-ping-tone"); return; }
          const tone = ping < 100 ? "good" : ping < 300 ? "warn" : "bad";
          el.textContent = ping + "ms ping";
          el.setAttribute("data-ping-tone", tone);
        }
        function formatUptime(seconds) {
          seconds = Math.floor(Number(seconds) || 0);
          if (seconds < 60) return seconds + "s";
          let minutes = Math.floor(seconds / 60);
          if (minutes < 60) return minutes + "m";
          let hours = Math.floor(minutes / 60);
          minutes = minutes % 60;
          if (hours < 24) return hours + "h " + minutes + "m";
          const days = Math.floor(hours / 24);
          hours = hours % 24;
          return days + "d " + hours + "h";
        }
        function plural(n, word) { return n + " " + word + (n === 1 ? "" : "s"); }
        // Aria's orchestrator card (mirrors the server-rendered one above).
        // The live patch used to push her through renderBotCardInner(), so
        // two seconds after load her card turned back into an empty
        // music-bot card with a "0 live / 0 guilds" chip row.
        function renderAriaCardInner(bot) {
          const medic = bot.medic_summary || {};
          const offline = botIsOffline(bot);
          const repairs = Number(medic.pending_repairs || 0);
          const infra = Number(medic.pending_infra || 0);
          const interactions = Number(bot.recent_interaction_count || 0);
          const critical = Number(medic.critical_health || 0);
          const recoverable = Number(medic.recoverable_health || 0);
          const uptime = bot.uptime_seconds != null ? "up " + formatUptime(bot.uptime_seconds) : "";
          const memory = bot.memory_kb != null ? Math.floor(bot.memory_kb / 1024) + " MB mem" : "";
          return `
            <div data-bot-offline-overlay>${offline ? '<div class="bot-card-offline-overlay"><span class="bot-card-offline-label">Offline</span></div>' : ""}</div>
            <div class="bot-head">
              <span class="bot-dot"></span>
              <div class="bot-head-copy">
                <h3>${escHtml(bot.display_name)}</h3>
                <small>Autonomous swarm orchestrator</small>
              </div>
              <span class="data-pill data-pill-${offline ? "danger" : "live"}" data-bot-badge>${offline ? "Offline" : "Online"}</span>
            </div>
            <div class="bot-now">
              <div class="bot-thumb bot-thumb-empty">&#9881;</div>
              <div class="bot-now-copy">
                <strong data-bot-now-title>${plural(repairs, "pending repair")}, ${plural(infra, "pending infra task")}</strong>
                <small data-bot-now-sub>${plural(interactions, "interaction")} recorded &middot; ${critical} critical, ${plural(recoverable, "recoverable health issue")}</small>
              </div>
            </div>
            <div class="chip-row">
              <span data-chip="uptime">${escHtml(uptime)}</span>
              <span data-chip="memory">${escHtml(memory)}</span>
              <span data-chip="heartbeat" data-bot-uptime></span>
            </div>`;
        }
        function patchHeartbeat(card, bot) {
          const el = card.querySelector("[data-bot-uptime]");
          if (!el) return;
          const age = Number(bot.heartbeat_age_seconds);
          const text = (bot.heartbeat_age_seconds != null && Number.isFinite(age)) ? "heartbeat " + Math.floor(age) + "s ago" : "";
          if (el.textContent !== text) el.textContent = text;
        }
        // Last markup written per card. Cards are only rebuilt when their
        // content actually changed -- rebuilding all of them on every push
        // re-decoded every thumbnail, reset hover state and forced a full
        // grid relayout every two seconds, which is what made scrolling the
        // dashboard stutter.
        const botCardMarkup = new WeakMap();
        function patchBotCards(data) {
          for (const bot of (data && data.bots) || []) {
            const card = document.querySelector(`[data-bot-key="${window.CSS && CSS.escape ? CSS.escape(bot.key) : bot.key}"]`);
            if (!card) continue;
            // Never swap a card out from under an in-progress seek drag.
            if (card.querySelector(".bot-seek-seeking")) continue;
            card.classList.toggle("bot-card-offline", botIsOffline(bot));
            const markup = bot.kind === "orchestrator" ? renderAriaCardInner(bot) : renderBotCardInner(bot);
            if (botCardMarkup.get(card) !== markup) {
              card.innerHTML = markup;
              botCardMarkup.set(card, markup);
            }
            patchHeartbeat(card, bot);
            patchPing(card, bot);
          }
        }
        function renderSessionRow(s) {
          const duration = s.duration_seconds || 0;
          const pct = duration > 0 ? Math.min(100, Math.floor((100 * (s.position_seconds || 0)) / duration)) : 0;
          return `
            <tr>
              <td>${escHtml(s.bot_display || s.bot_name)}</td>
              <td>${escHtml(s.title || "—")}</td>
              <td>${escHtml(s.session_state_label || "")}</td>
              <td style="max-width:160px">
                <div class="bot-playback compact" data-playback-counter data-position="${escHtml(s.position_seconds || 0)}" data-observed-at="${escHtml(s.position_observed_at || 0)}" data-duration="${escHtml(s.duration_seconds || 0)}" data-playing="${s.is_playing === true}">
                  <div class="bot-playback-bar" aria-hidden="true"><span data-playback-bar style="width:${pct}%"></span></div>
                  <span data-playback-label></span>
                </div>
              </td>
              <td>${s.queue_count || 0} queued</td>
            </tr>`;
        }
        let lastSessionsMarkup = null;
        function patchSessionsTable(data) {
          const tbody = document.querySelector("#sessions-table tbody");
          if (!tbody) return;
          const rows = ((data && data.sessions) || []).filter((s) => s.is_playing || s.is_paused || (s.title && s.title !== ""));
          const markup = rows.length
            ? rows.map(renderSessionRow).join("")
            : `<tr><td colspan="5">${escHtml("Nothing playing right now.")}</td></tr>`;
          if (markup === lastSessionsMarkup) return;
          lastSessionsMarkup = markup;
          tbody.innerHTML = markup;
          tickPlaybackCounters();
        }
        // Pinned bots: a star on each card floats that bot to the front of
        // the grid (CSS order, so live patching never has to reshuffle the
        // DOM). Stored per browser; the iOS app keeps its own pins.
        const PINNED_BOTS_KEY = "swarmpanel.pinnedBots";
        let pinnedBots = new Set();
        try { pinnedBots = new Set(JSON.parse(localStorage.getItem(PINNED_BOTS_KEY) || "[]")); } catch { /* storage unavailable */ }
        function savePinnedBots() {
          try { localStorage.setItem(PINNED_BOTS_KEY, JSON.stringify(Array.from(pinnedBots))); } catch { /* storage unavailable */ }
        }
        // patchBotCards() rewrites each card's innerHTML, so the button is
        // re-attached after every patch rather than baked into the markup.
        function decoratePinnedBots() {
          document.querySelectorAll("#bot-cards [data-bot-key]").forEach((card) => {
            const pinned = pinnedBots.has(card.getAttribute("data-bot-key"));
            card.classList.toggle("bot-card-pinned", pinned);
            let btn = card.querySelector("[data-pin-toggle]");
            if (!btn) {
              btn = document.createElement("button");
              btn.type = "button";
              btn.className = "bot-pin";
              btn.setAttribute("data-pin-toggle", "");
              card.appendChild(btn);
            }
            btn.textContent = pinned ? "\u2605" : "\u2606";
            btn.title = pinned ? "Unpin" : "Pin to top";
            btn.setAttribute("aria-label", btn.title);
            btn.setAttribute("aria-pressed", pinned ? "true" : "false");
          });
        }
        const botCardsEl = document.getElementById("bot-cards");
        if (botCardsEl) {
          botCardsEl.addEventListener("click", (e) => {
            const btn = e.target.closest("[data-pin-toggle]");
            if (!btn) return;
            const key = btn.closest("[data-bot-key]").getAttribute("data-bot-key");
            if (pinnedBots.has(key)) pinnedBots.delete(key); else pinnedBots.add(key);
            savePinnedBots();
            decoratePinnedBots();
          });
        }
        decoratePinnedBots();

        // Pushes are applied on the next animation frame, latest one wins:
        // a burst of snapshots (reconnect, tab returning from background)
        // costs one DOM pass instead of one per message, and a hidden tab
        // does no DOM work at all until it's shown again.
        let pendingDashboard = null;
        let dashboardFrame = 0;
        function applyDashboard() {
          dashboardFrame = 0;
          const data = pendingDashboard;
          pendingDashboard = null;
          if (!data) return;
          patchDashboardMetrics(data);
          patchBotCards(data);
          decoratePinnedBots();
          patchSessionsTable(data);
          tickPlaybackCounters();
        }
        window.swarmLive.watch("dashboard", (msg) => {
          if (msg.type !== "snapshot") return;
          pendingDashboard = msg.data;
          if (!dashboardFrame) dashboardFrame = requestAnimationFrame(applyDashboard);
        });

        const dashEvents = document.getElementById("dash-events");
        if (dashEvents) {
          window.swarmLive.watch("events", (msg) => {
            if (msg.type === "snapshot_error") {
              dashEvents.innerHTML = '<div class="empty-state">Incident feed unavailable.</div>';
              return;
            }
            if (msg.type !== "snapshot") return;
            // The feed arrives oldest-first; show the newest 8.
            const latest = ((msg.data && msg.data.events) || []).slice(-8).reverse();
            dashEvents.innerHTML = latest.length
              ? latest.map(swarmEventCard).join("")
              : '<div class="empty-state">No incidents. The fleet is quiet.</div>';
          });
        }
      </script>
    ]]

    local prefs = a and accounts.get_panel_preferences(a.username, a.guild_id) or nil
    return 200, html.layout({ title = "Dashboard", path = path, session = session_view(a), token = req.cookies and req.cookies.swarm_session, body = body, preferences = prefs }),
      { ["Content-Type"] = "text/html; charset=utf-8" }
  end

  httpd.route("GET", "/", function(req) return render_dashboard(req, "/") end)
  httpd.route("GET", "/dashboard", function(req) return render_dashboard(req, "/dashboard") end)

  -- -----------------------------------------------------------------
  -- Controls
  -- -----------------------------------------------------------------
  httpd.route("GET", "/controls", function(req)
    local a, status, headers = cfg.require_auth_page(req)
    if not a then return status, "", headers end

    -- Only bots this account can actually send orders to (a guild account
    -- gets its own guild's bots; POST /api/bots/control rejects the rest).
    local data = cfg.scoped_dashboard_data(a)
    -- 2026-10-21 (per operator request: "all controls need to take into
    -- account if the selected bot is in local or streaming mode"): the
    -- control form issues the same PLAY/SKIP/etc. commands regardless of
    -- mode (no command's actual behavior differs by mode -- checked
    -- control.lua's full action list), so there's nothing to disable here.
    -- What was actually missing is visibility: an operator picking a bot
    -- from this dropdown had no way to tell its mode before sending a
    -- command. data-mode carries it through to updateCommandPreview()
    -- below without a second lookup.
    local bot_options = {}
    for _, bot in ipairs(data.bots) do
      local mode = bot.playback_mode
      local mode_suffix = mode and (" (" .. mode:sub(1, 1):upper() .. mode:sub(2) .. ")") or ""
      bot_options[#bot_options + 1] = ("<option value=\"%s\" data-mode=\"%s\" data-status=\"%s\">%s%s</option>"):format(
        html.esc(bot.key), html.esc(mode or ""), html.esc(tostring(bot.status or "")), html.esc(bot.display_name), html.esc(mode_suffix))
    end
    local action_options = {}
    for _, act in ipairs(CONTROL_ACTIONS) do
      action_options[#action_options + 1] = ("<option value=\"%s\">%s</option>"):format(html.esc(act), html.esc(act))
    end

    -- Mirrors ControlsPage.jsx's form.guild_id default: a guild-scoped
    -- account's own registered guild (ctx.session.guild_id), read-only --
    -- POST /api/bots/control now rejects any other guild_id for a scoped
    -- account anyway (require_bot_guild_access), so letting them type an
    -- arbitrary one here was never actually usable, just confusing. Admins
    -- get it pre-filled from the first live session (also matching the
    -- React fallback: dash.sessions?.[0]?.guild_id) but still editable,
    -- since they can legitimately target any guild.
    local own_guild_id = (not a.admin_mode) and a.guild_id or nil
    local default_guild_id = own_guild_id
    if not default_guild_id then
      for _, bot in ipairs(data.bots) do
        local first = bot.sessions and bot.sessions[1]
        if first and first.guild_id then default_guild_id = first.guild_id; break end
      end
    end
    -- Shows the guild's real NAME (resolved client-side from the bot's
    -- Discord inventory, same data the voice/text channel pickers already
    -- use) instead of a bare numeric ID -- the ID is still exactly what
    -- gets submitted (this <select>'s value), just never what's displayed.
    -- A scoped account gets exactly one <option> (its own guild) -- enabled,
    -- not disabled, since a disabled field is excluded from FormData.
    local guild_field = own_guild_id
      and ('<label class="field field-inline">Guild<select name="guild_id" id="control-guild-id" data-scoped="1"><option value="%s">Guild %s</option></select><button type="button" class="field-inline-action" data-copy-target="#control-guild-id">Copy ID</button></label>'):format(html.esc(own_guild_id), html.esc(own_guild_id))
      or ('<label class="field field-inline">Guild<select name="guild_id" id="control-guild-id" required><option value="%s">%s</option></select><button type="button" class="field-inline-action" data-copy-target="#control-guild-id">Copy ID</button></label>'):format(html.esc(default_guild_id or ""), default_guild_id and ("Guild " .. html.esc(default_guild_id)) or "Choose a guild")

    local body = html.page({
      title = "Controls", eyebrow = "Fleet", lede = "Send a direct order to any bot in any guild.",
      body = ([[
        <div class="control-layout">
        <form id="control-form" class="panel form-panel">
          <label class="field">Bot<select name="bot_key" required>%s</select></label>
          %s
          <label class="field">Action<select name="action">%s</select></label>
          <label class="field">Source URL / search (PLAY only)<input type="text" name="source_url" placeholder="https://... or search terms"></label>
          <label class="field">Voice channel<select name="voice_channel_id"><option value="">Choose channel</option></select></label>
          <label class="field">Text channel<select name="text_channel_id"><option value="">None</option></select></label>
          <label class="field">Loop mode (LOOP only)<select name="loop_mode"><option value="off">off</option><option value="song">song</option><option value="queue">queue</option></select></label>
          <label class="field">Filter mode (FILTER only)<select name="filter_mode">
            <option value="none">None</option>
            <option value="nightcore">Nightcore</option>
            <option value="bassboost">Bassboost</option>
            <option value="vaporwave">Vaporwave</option>
            <option value="8d">8D</option>
            <option value="karaoke">Karaoke</option>
            <option value="tremolo">Tremolo</option>
            <option value="vibrato">Vibrato</option>
            <option value="lowpass">Low Pass</option>
            <option value="lofi">Lo-fi</option>
            <option value="electronic">Electronic</option>
            <option value="party">Party</option>
            <option value="radio">Radio</option>
            <option value="cinema">Cinema</option>
          </select></label>
          <div class="command-preview-grid">
            <div class="preview-card"><span>Bot</span><strong id="cmd-preview-bot">--</strong></div>
            <div class="preview-card"><span>Mode</span><strong id="cmd-preview-mode">--</strong></div>
            <div class="preview-card"><span>Action</span><strong id="cmd-preview-action">--</strong></div>
            <div class="preview-card"><span>Guild</span><strong id="cmd-preview-guild">--</strong></div>
            <div class="preview-card command-preview-primary" id="cmd-preview-summary">Choose a bot, action, and guild above to preview the order before sending.</div>
          </div>
          <button type="submit" class="button-link primary">Send</button>
        </form>
        <div>
        <div id="control-result"></div>
        %s
        <div id="control-state" class="control-state"></div>
        %s
        <div class="panel guild-overview">
          <div class="actions-row">
            <p>Every bot's state in the selected guild at a glance, with quick transport controls.</p>
            <button type="button" id="guild-overview-btn" class="button-link">Load overview</button>
          </div>
          <div id="guild-overview"></div>
        </div>
        %s
        <div id="saved-queues"></div>
        %s
        %s
        </div>
        </div>
      ]]):format(html.join(bot_options), guild_field, html.join(action_options),
        html.section_head("Control State"), html.section_head("Guild Overview"), html.section_head("Saved Queues"),
        a.admin_mode and ([[
          %s
          <div class="panel">
            <p>Sends RECOVER to every bot/guild session fleet-wide currently sitting in "Recovery Pending".</p>
            <button type="button" id="recover-all-btn" class="button-link primary">Recover All Stale Sessions</button>
            <div id="recover-all-result"></div>
          </div>
        ]]):format(html.section_head("Fleet Recovery")) or "",
        -- Voice<->stage conversion: visible to any account with access to
        -- this page (a guild-scoped account for its own guild, an admin for
        -- whichever guild is selected above) -- unlike Fleet Recovery this
        -- isn't a site-wide/admin-only action, it operates on exactly the
        -- one guild currently selected in the form. Every bot already knows
        -- how to play correctly from a stage channel (auto-unsuppress,
        -- request-to-speak fallback, stage topics -- see channel_convert.lua's
        -- own header); this only flips the underlying Discord channel type
        -- for every bot's home channel in that guild, keeping everything
        -- else (name, position, permissions) exactly as the owner had it.
        ([[
          %s
          <div class="panel">
            <p>Converts every home-channel-connected bot's voice channel in the selected guild to a stage channel, or back -- names and every other channel setting are left untouched either way.</p>
            <div class="actions-row">
              <button type="button" id="convert-to-stage-btn" class="button-link primary">Convert to Stage Channels</button>
              <button type="button" id="convert-to-voice-btn" class="button-link">Convert to Voice Channels</button>
            </div>
            <div id="convert-channels-result"></div>
          </div>
        ]]):format(html.section_head("Stage / Voice Channel Conversion"))),
    })

    local script = [[
      const PAYLOAD_FIELDS = {
        PLAY: ["source_url", "voice_channel_id", "text_channel_id"],
        SMART_RECOMMEND: ["voice_channel_id", "text_channel_id"],
        SET_HOME: ["voice_channel_id"],
        LOOP: ["loop_mode"],
        FILTER: ["filter_mode"],
      };
      const form = document.getElementById("control-form");
      // command-preview-grid/-primary had CSS but nothing rendered it -- a
      // plain-language readout of what the form will actually send, kept in
      // sync with every field change so it's accurate right up to submit.
      function updateCommandPreview() {
        const selectedBotOpt = form.bot_key.selectedOptions[0];
        const botLabel = selectedBotOpt ? selectedBotOpt.textContent : "--";
        const botMode = selectedBotOpt ? selectedBotOpt.dataset.mode : "";
        const botStatus = selectedBotOpt ? (selectedBotOpt.dataset.status || "").toLowerCase() : "";
        const action = form.action.value || "--";
        const guildLabel = form.guild_id.selectedOptions[0] ? form.guild_id.selectedOptions[0].textContent : "--";
        document.getElementById("cmd-preview-bot").textContent = botLabel;
        document.getElementById("cmd-preview-mode").textContent = botMode ? (botMode[0].toUpperCase() + botMode.slice(1)) : "--";
        document.getElementById("cmd-preview-action").textContent = action;
        document.getElementById("cmd-preview-guild").textContent = guildLabel;
        const summary = document.getElementById("cmd-preview-summary");
        if (form.bot_key.value && form.guild_id.value) {
          let detail = "";
          if (action === "PLAY" && form.source_url.value) detail = ` with "${form.source_url.value}"`;
          else if (action === "LOOP") detail = ` (${form.loop_mode.value})`;
          else if (action === "FILTER") detail = ` (${form.filter_mode.value})`;
          let warning = "";
          // 2026-10-21 (per operator request, controls need mode/health
          // awareness): the command still queues normally either way --
          // control.lua has no health gate -- but an operator sending PLAY/
          // SKIP/etc. to a bot whose voice link is actively degraded or
          // offline should know the order may sit unapplied until it
          // recovers, not silently assume it landed.
          if (botStatus === "degraded") warning = " Heads up: this bot's voice link is currently degraded -- the order will queue but may not apply until it recovers.";
          else if (botStatus === "offline" || botStatus === "stale") warning = " Heads up: this bot looks offline right now -- the order will queue but won't run until it reconnects.";
          summary.textContent = `${botLabel} will run ${action}${detail} in ${guildLabel}.${warning}`;
        } else {
          summary.textContent = "Choose a bot, action, and guild above to preview the order before sending.";
        }
      }
      form.addEventListener("input", updateCommandPreview);
      form.addEventListener("change", updateCommandPreview);
      updateCommandPreview();
      form.addEventListener("submit", async (e) => {
        e.preventDefault();
        const fd = new FormData(form);
        const action = fd.get("action");
        const fields = PAYLOAD_FIELDS[action] || [];
        const payload = {};
        for (const f of fields) payload[f] = fd.get(f);
        const box = document.getElementById("control-result");
        try {
          const res = await swarmFetch("/api/bots/control", {
            method: "POST",
            body: JSON.stringify({ bot_key: fd.get("bot_key"), guild_id: fd.get("guild_id"), action, payload }),
          });
          box.innerHTML = '<div class="notice notice-success">' + (res.message || "Order sent.") + "</div>";
          swarmToast("Order sent.", "success");
          refreshControlState();
        } catch (err) {
          box.innerHTML = '<div class="notice notice-error">' + err.message + "</div>";
        }
      });

      // BUGFIX (live-push migration): was swarmFetch on a 4s
      // swarmLiveRefresh poll. Split into a pure renderer (applyControlState,
      // called from the "control_state" live-push handler below) and a
      // resubscribeControlState() that re-issues the watch with the
      // currently-selected bot_key/guild_id whenever either changes --
      // routes.lua's SNAPSHOT_BUILDERS.control_state builds the identical
      // payload GET /api/bots/{key}/control-state used to (same
      // dashboard.get_bot_control_state + enrich_control_state_with_discord
      // call), just pushed instead of polled, and the server recomputes it
      // every 2s regardless of whether any client asked, so an actual state
      // change (someone else's PLAY/SKIP, a bot restart) now reaches this
      // page without waiting up to 4s for the next poll tick.
      function applyControlState(state) {
        const stateBox = document.getElementById("control-state");
        stateBox.innerHTML = '<pre class="json-panel">' + JSON.stringify(state, null, 2).replace(/</g, "&lt;") + "</pre>";
        // .state-recovering already existed in CSS (recovering-pulse
        // keyframe) but nothing ever applied it -- an operator watching
        // this panel had no visual cue that a session was mid-recovery.
        stateBox.classList.toggle("state-recovering", !!(state && state.session && state.session.session_state === "recovering"));
        // Mirrors ControlsPage.jsx's controlState effect: voice/text channel
        // are one-time defaults (only fill an empty field -- never clobber
        // what the operator is mid-typing for a PLAY/SET_HOME order), while
        // loop_mode/filter_mode always reflect the bot's actual live
        // setting, since those aren't per-order inputs, they're "what is
        // this guild currently configured to do" (was previously stuck on
        // the form's hardcoded "off"/"none" defaults regardless of what
        // the bot was really set to -- e.g. every bot defaults to
        // loop_mode=queue, but the form never showed that).
        const session = state && state.session;
        if (session) {
          // voice/text channel selects are populated asynchronously by
          // loadChannels() below (real channel NAMES, not raw IDs -- a
          // bare numeric snowflake told you nothing about which channel
          // you were about to target), so the desired id is stashed here
          // and applied once loadChannels() has real <option>s to match
          // against, instead of writing straight to .value (which is a
          // silent no-op on a <select> with no matching <option> yet).
          if (!form.voice_channel_id.value) {
            pendingVoiceChannelId = session.home_channel_id || session.channel_id || "";
            applyPendingChannelValue(form.voice_channel_id, pendingVoiceChannelId);
          }
          if (!form.text_channel_id.value) {
            pendingTextChannelId = session.feedback_channel_id || "";
            applyPendingChannelValue(form.text_channel_id, pendingTextChannelId);
          }
          if (session.loop_mode) form.loop_mode.value = session.loop_mode;
          if (session.filter_mode) form.filter_mode.value = session.filter_mode;
        }
      }
      window.swarmLive.watch("control_state", (msg) => {
        if (msg.type === "snapshot") applyControlState(msg.data);
        // BUGFIX: an initial watch (no bot_key/guild_id yet -- see the
        // eager subscribe below) or a stale/invalid pair pushes back
        // snapshot_error, which this handler used to silently drop --
        // #control-state just stayed on its empty server-rendered markup
        // forever with no indication anything had gone wrong.
        else if (msg.type === "snapshot_error") {
          document.getElementById("control-state").innerHTML =
            `<div class="empty-state">Couldn't load control state: ${(msg.error || "unknown error").replace(/</g, "&lt;")}</div>`;
        }
      });
      function resubscribeControlState() {
        const botKey = form.bot_key.value, guildId = form.guild_id.value;
        if (!botKey || !guildId) return;
        window.swarmLive.resubscribe("control_state", { bot_key: botKey, guild_id: guildId });
      }
      // Kept as the async-function name refreshControlState() below (rather
      // than renaming every call site) so the rest of this file -- and the
      // race-condition fix's own comment just below, which specifically
      // documents awaiting this before it reads guild_id -- didn't need
      // touching beyond swapping its body from a fetch to a resubscribe.
      async function refreshControlState() {
        resubscribeControlState();
      }
      // Switching bots left guild_id pointed at whatever guild the PREVIOUS
      // bot defaulted to -- if the new bot isn't even in that guild,
      // control-state/inventory still return 200 (guild_id is valid, just
      // not one this bot serves) with an empty/idle session, so the page
      // looked like nothing happened. Re-pick a guild the newly selected
      // bot actually has a live session in (falls back to its first known
      // guild) whenever guild_id is editable, mirroring the page-load
      // default-guild logic in the route handler above.
      async function pickGuildForBot(botKey) {
        try {
          const dash = await swarmFetch("/api/dashboard");
          const bot = (dash.bots || []).find((b) => b.key === botKey);
          if (!bot) return null;
          const sessions = bot.sessions || [];
          const live = sessions.find((s) => s.is_playing) || sessions[0];
          if (live && live.guild_id) return String(live.guild_id);
        } catch { /* fall through */ }
        return null;
      }
      let pendingGuildId = "";
      // BUGFIX: refreshControlState() and loadChannels() were both fired
      // here without awaiting either, as if they were independent. They
      // aren't: loadChannels() is the ONLY one of the two that actually
      // resolves pendingGuildId into a real value on guild_id.value (it
      // awaits /api/bots/{key}/inventory then calls fillGuildSelect()).
      // refreshControlState() reads form.guild_id.value directly and has no
      // knowledge of pendingGuildId, so firing it in parallel made it race
      // against loadChannels() and lose almost every time -- it read the
      // PREVIOUS bot's guild_id (still unchanged at that point), queried
      // control-state for a bot/guild pair that often has no session at
      // all, and populated loop_mode/filter_mode and the pending voice/text
      // channel from that wrong answer. Nothing corrected it afterwards
      // either: programmatically setting a <select>'s .value (what
      // fillGuildSelect() does) never fires a native "change" event, so
      // guild_id's own change listener below never re-ran refreshControlState()
      // -- confirmed live (Playwright): after switching bots, the guild
      // select and voice channel stayed on the OLD bot's values for
      // 4-6+ seconds, only catching up once the unrelated 4s
      // swarmLiveRefresh(refreshControlState, ...) poll happened to land
      // after loadChannels() had finally caught up. Awaiting loadChannels()
      // first guarantees guild_id is already correct before
      // refreshControlState() ever reads it.
      // BUGFIX: applyControlState() above only ever fills voice/text channel
      // when the select is EMPTY (so it doesn't clobber an operator mid-
      // picking a channel for a PLAY/SET_HOME order). That guard has no
      // memory of WHICH bot it was last filled for, so once it was set for
      // bot A it stayed permanently non-empty -- switching to bot B still
      // passed loadChannels() bot B's own channel list (so the dropdown
      // wasn't literally frozen), but since the old value usually matched a
      // real option in bot B's guild too (channels are guild-scoped, and
      // most bots here share one guild), the select just silently kept
      // showing bot A's home/feedback channel forever, and applyControlState
      // never got a chance to apply bot B's actual home_channel_id /
      // feedback_channel_id. Clearing both selects on an actual bot/guild
      // switch (not on every keystroke -- this is the "change" listener, so
      // an in-progress PLAY/SET_HOME pick on the SAME bot is untouched)
      // re-opens that guard so the next control_state push fills in the
      // newly selected bot's real channels instead of the previous one's.
      form.bot_key.addEventListener("change", async () => {
        form.voice_channel_id.value = "";
        form.text_channel_id.value = "";
        pendingVoiceChannelId = ""; pendingTextChannelId = "";
        if (form.guild_id.dataset.scoped !== "1") {
          // Setting .value directly here would silently no-op -- the new
          // bot's guild options (real names) haven't loaded yet, so there's
          // no matching <option> to select. Stash it; loadChannels() below
          // applies it once fillGuildSelect() has real options to match
          // against, same pattern the voice/text channel selects already use.
          pendingGuildId = (await pickGuildForBot(form.bot_key.value)) || "";
        }
        await loadChannels();
        refreshControlState();
        resubscribeQueues();
      });
      form.guild_id.addEventListener("change", () => {
        form.voice_channel_id.value = "";
        form.text_channel_id.value = "";
        pendingVoiceChannelId = ""; pendingTextChannelId = "";
        refreshControlState(); resubscribeQueues(); loadChannels();
      });

      let pendingVoiceChannelId = "", pendingTextChannelId = "";
      let channelsRequestId = 0;
      const VOICE_TYPES = new Set([2, 13]);
      const TEXT_TYPES = new Set([0, 5, 10, 11, 12]);
      function fillChannelSelect(select, channels, keepValue) {
        const current = keepValue || select.value;
        select.innerHTML = '<option value="">' + (select.name === "text_channel_id" ? "None" : "Choose channel") + "</option>"
          + channels.map((c) => `<option value="${c.id}">${(c.name || c.id).replace(/</g, "&lt;")}</option>`).join("");
        if (current && channels.some((c) => String(c.id) === String(current))) select.value = current;
      }
      // refreshControlState() polls independently of loadChannels() -- if a
      // session's home channel arrives after the select is already
      // populated, nothing would otherwise re-apply it until the next
      // loadChannels() call, so try to select it immediately too.
      function applyPendingChannelValue(select, value) {
        if (value && [...select.options].some((o) => o.value === String(value))) select.value = value;
      }
      // Real guild NAMES (from the bot's Discord inventory) instead of bare
      // IDs -- the <select>'s value stays the ID either way, only the
      // visible <option> text changes. A scoped account's select only ever
      // has its own single guild, so it's left alone here (repopulating it
      // from the bot's full guild list would leak other guilds' names into
      // an account that's not supposed to see them) -- just its one
      // option's label gets the real name once inventory has it.
      function fillGuildSelect(guilds) {
        const select = form.guild_id;
        if (select.dataset.scoped === "1") {
          const own = guilds.find((g) => String(g.id) === select.value);
          if (own && select.options[0]) select.options[0].textContent = own.name || select.options[0].textContent;
          return;
        }
        const current = pendingGuildId || select.value;
        select.innerHTML = guilds.map((g) => `<option value="${g.id}">${(g.name || g.id).toString().replace(/</g, "&lt;")}</option>`).join("")
          || `<option value="">No guilds found</option>`;
        if (current && guilds.some((g) => String(g.id) === String(current))) select.value = current;
        pendingGuildId = "";
      }
      async function loadChannels() {
        const botKey = form.bot_key.value, guildId = form.guild_id.value;
        if (!botKey || !guildId) return;
        // bot_key/guild_id changes and the initial call can overlap in
        // flight; only the response to the MOST RECENT request may write
        // to the selects, or a slow stale fetch can clobber a faster
        // newer one and silently leave the wrong guild's channels showing.
        const requestId = ++channelsRequestId;
        try {
          const inv = await swarmFetch(`/api/bots/${botKey}/inventory`);
          if (requestId !== channelsRequestId) return;
          fillGuildSelect(inv.guilds || []);
          const resolvedGuildId = form.guild_id.value;
          const guild = (inv.guilds || []).find((g) => String(g.id) === String(resolvedGuildId));
          const channels = (guild && guild.channels) || [];
          fillChannelSelect(form.voice_channel_id, channels.filter((c) => VOICE_TYPES.has(Number(c.type))), pendingVoiceChannelId);
          fillChannelSelect(form.text_channel_id, channels.filter((c) => TEXT_TYPES.has(Number(c.type))), pendingTextChannelId);
          pendingVoiceChannelId = ""; pendingTextChannelId = "";
          updateCommandPreview();
        } catch { /* bot token/inventory unavailable -- selects just stay empty */ }
      }
      // BUGFIX: the pre-selected bot/guild's control_state and queues were
      // never subscribed to at page load -- both watches only ever get
      // real bot_key/guild_id params from the change listeners above, which
      // never fire on their own (the <select>s already show their correct
      // default value from the server-rendered markup, so nothing ever
      // changes it). The panel looked blank/idle until the user picked a
      // DIFFERENT bot at least once. Mirrors the bot_key change listener's
      // own await-then-subscribe order (see its BUGFIX comment above) so
      // guild_id is guaranteed resolved first.
      (async () => {
        await loadChannels();
        refreshControlState();
        resubscribeQueues();
      })();

      // BUGFIX (live-push migration): was swarmFetch on a 30s
      // swarmLiveRefresh poll. applyQueues() is the pure renderer, fed by
      // BOTH the "queues" live-push watch (passive updates -- someone else
      // saves/renames/deletes a queue) AND loadQueues()'s own direct fetch
      // (kept for the user's own delete/rename/save actions below, so THOSE
      // get instant feedback rather than waiting for the next push).
      let savedQueues = [];
      function applyQueues(data) {
        savedQueues = (data && data.queues) || [];
        const rows = savedQueues.map((q) => `
          <div class="collection-row" data-load-queue="${q.id}">
            <span><strong>${(q.name || "").replace(/</g, "&lt;")}</strong><small>${(q.items || []).length} track${(q.items || []).length === 1 ? "" : "s"}</small></span>
            <button type="button" class="icon-button" data-rename-queue="${q.id}" title="Rename">&#9998;</button>
            <button type="button" class="icon-button" data-delete-queue="${q.id}" title="Delete">&times;</button>
          </div>`
        ).join("");
        document.getElementById("saved-queues").innerHTML = `<div class="list-panel">${rows || '<div class="empty-state">No saved queues.</div>'}</div>`;
      }
      window.swarmLive.watch("queues", (msg) => {
        if (msg.type === "snapshot") applyQueues(msg.data);
        // Same visibility fix as the control_state watch above.
        else if (msg.type === "snapshot_error") {
          document.getElementById("saved-queues").innerHTML =
            `<div class="empty-state">Couldn't load saved queues: ${(msg.error || "unknown error").replace(/</g, "&lt;")}</div>`;
        }
      });
      function resubscribeQueues() {
        const botKey = form.bot_key.value, guildId = form.guild_id.value;
        if (!botKey || !guildId) return;
        window.swarmLive.resubscribe("queues", { bot_key: botKey, guild_id: guildId });
      }
      async function loadQueues() {
        const botKey = form.bot_key.value, guildId = form.guild_id.value;
        if (!botKey || !guildId) return;
        try {
          applyQueues(await swarmFetch(`/api/queues?guild_id=${encodeURIComponent(guildId)}&bot_key=${botKey}`));
        } catch { /* ignore */ }
      }
      document.getElementById("saved-queues").addEventListener("click", async (e) => {
        const del = e.target.getAttribute("data-delete-queue");
        if (del) {
          e.stopPropagation();
          await swarmFetch(`/api/queues/${del}/delete`, { method: "POST" }).catch(() => {});
          loadQueues();
          return;
        }
        // Inline rename: swaps the row's display for a mini-form
        // (mini-row/mini-input CSS existed, unused) instead of a full page
        // or a native prompt(), matching the rest of this codebase's
        // no-prompt() convention.
        const renameId = e.target.getAttribute("data-rename-queue");
        if (renameId) {
          e.stopPropagation();
          const row = e.target.closest("[data-load-queue]");
          const queue = savedQueues.find((q) => String(q.id) === renameId);
          if (!row || !queue) return;
          row.innerHTML = `
            <form class="mini-form" data-rename-form="${renameId}">
              <div class="mini-row">
                <input class="mini-input" name="name" value="${(queue.name || "").replace(/"/g, "&quot;")}" maxlength="120" required>
                <button type="submit">Save</button>
                <button type="button" data-cancel-rename>Cancel</button>
              </div>
            </form>`;
          row.querySelector("input").focus();
          return;
        }
        if (e.target.hasAttribute("data-cancel-rename")) { e.stopPropagation(); loadQueues(); return; }
        if (e.target.closest("[data-rename-form]")) return;
        // "Load" a saved queue: previously a dead button (data-load-queue
        // rendered but never listened for) -- replays every saved track by
        // sending each as its own PLAY order in sequence, same as manually
        // re-queueing them one at a time from the form above.
        const row = e.target.closest("[data-load-queue]");
        if (!row) return;
        const queue = savedQueues.find((q) => String(q.id) === row.getAttribute("data-load-queue"));
        if (!queue || !(queue.items || []).length) return;
        const botKey = form.bot_key.value, guildId = form.guild_id.value;
        let queued = 0;
        for (const item of queue.items) {
          try {
            await swarmFetch("/api/bots/control", {
              method: "POST",
              body: JSON.stringify({ bot_key: botKey, guild_id: guildId, action: "PLAY", payload: { source_url: item.video_url } }),
            });
            queued++;
          } catch { /* keep going through the rest of the queue */ }
        }
        swarmToast(`Queued ${queued}/${queue.items.length} track(s) from "${queue.name}".`, queued ? "success" : "error");
        refreshControlState();
      });
      document.getElementById("saved-queues").addEventListener("submit", async (e) => {
        const renameForm = e.target.closest("[data-rename-form]");
        if (!renameForm) return;
        e.preventDefault();
        const id = renameForm.getAttribute("data-rename-form");
        const name = renameForm.elements.name.value.trim();
        if (!name) return;
        try {
          await swarmFetch(`/api/queues/${id}/rename`, { method: "POST", body: JSON.stringify({ guild_id: form.guild_id.value, name }) });
          swarmToast("Renamed.", "success");
          loadQueues();
        } catch (err) { swarmToast(err.message, "error"); }
      });
    ]] .. [[
      const recoverAllBtn = document.getElementById("recover-all-btn");
      if (recoverAllBtn) {
        recoverAllBtn.addEventListener("click", async () => {
          const resultBox = document.getElementById("recover-all-result");
          recoverAllBtn.disabled = true;
          resultBox.innerHTML = "";
          try {
            const dash = await swarmFetch("/api/dashboard");
            const targets = [];
            for (const bot of (dash.bots || [])) {
              for (const session of (bot.sessions || [])) {
                if (session.session_state === "recovering" && session.guild_id) {
                  targets.push({ bot_key: bot.key, guild_id: String(session.guild_id) });
                }
              }
            }
            if (targets.length === 0) {
              resultBox.innerHTML = '<div class="notice notice-success">Nothing to recover — no sessions pending.</div>';
              return;
            }
            let succeeded = 0, failed = 0;
            for (const t of targets) {
              try {
                await swarmFetch("/api/bots/control", {
                  method: "POST",
                  body: JSON.stringify({ bot_key: t.bot_key, guild_id: t.guild_id, action: "RECOVER" }),
                });
                succeeded++;
              } catch { failed++; }
            }
            resultBox.innerHTML = `<div class="notice notice-${failed ? "error" : "success"}">Recovered ${succeeded}/${targets.length} session(s)${failed ? `, ${failed} failed` : ""}.</div>`;
            swarmToast(`Recovery sent to ${succeeded}/${targets.length} session(s).`, failed ? "error" : "success");
            refreshControlState();
          } catch (err) {
            resultBox.innerHTML = '<div class="notice notice-error">' + err.message + "</div>";
          } finally {
            recoverAllBtn.disabled = false;
          }
        });
      }
    ]] .. [[
      // Stage/voice channel conversion -- operates on whatever guild is
      // currently selected in the form above (locked to the account's own
      // guild for a scoped account, per guild_field's data-scoped attr).
      async function convertChannels(direction, btn) {
        const guildId = form.guild_id.value;
        if (!guildId) { swarmToast("Choose a guild first.", "error"); return; }
        const label = direction === "stage" ? "stage channels" : "voice channels";
        // Discord doesn't support an in-place type change (confirmed live,
        // see discord_service.lua's recreate_channel_as_type) -- this
        // actually deletes and recreates each channel, so the confirm
        // prompt says so plainly rather than implying a seamless swap.
        if (!confirm(`Convert every home-channel-connected bot's channel in this guild to ${label}?\n\nDiscord doesn't support converting a channel in place, so this deletes each channel and recreates it as a ${direction === "stage" ? "stage" : "voice"} channel with the same name, category, position, and permissions. Anyone currently connected will be briefly disconnected, and any in-channel chat history on that channel will be lost.`)) return;
        const resultBox = document.getElementById("convert-channels-result");
        const otherBtn = direction === "stage" ? document.getElementById("convert-to-voice-btn") : document.getElementById("convert-to-stage-btn");
        btn.disabled = true;
        if (otherBtn) otherBtn.disabled = true;
        resultBox.innerHTML = "";
        try {
          const res = await swarmFetch(`/api/guilds/${guildId}/convert-channels`, {
            method: "POST",
            body: JSON.stringify({ direction }),
          });
          const converted = res.converted || [];
          const failedN = (res.failed || []).length;
          const warnings = converted.filter(c => c.warning);
          let msg = `Converted ${converted.length} channel(s) to ${label}`;
          if (res.skipped_no_home_channel) msg += ` (${res.skipped_no_home_channel} bot(s) had no home channel set)`;
          if (warnings.length) msg += ` -- ${warnings.length} with a warning: ` + warnings.map(c => c.warning).join("; ");
          if (failedN) msg += ` -- ${failedN} failed: ` + res.failed.map(f => f.error).join("; ");
          const hasIssue = failedN > 0 || warnings.length > 0;
          resultBox.innerHTML = `<div class="notice notice-${hasIssue ? "error" : "success"}">${msg}.</div>`;
          swarmToast(hasIssue ? `Converted ${converted.length}, but check the details below.` : `Converted ${converted.length} channel(s) to ${label}.`, hasIssue ? "error" : "success");
          refreshControlState();
        } catch (err) {
          resultBox.innerHTML = '<div class="notice notice-error">' + err.message + "</div>";
        } finally {
          btn.disabled = false;
          if (otherBtn) otherBtn.disabled = false;
        }
      }
      const convertToStageBtn = document.getElementById("convert-to-stage-btn");
      const convertToVoiceBtn = document.getElementById("convert-to-voice-btn");
      if (convertToStageBtn) convertToStageBtn.addEventListener("click", () => convertChannels("stage", convertToStageBtn));
      if (convertToVoiceBtn) convertToVoiceBtn.addEventListener("click", () => convertChannels("voice", convertToVoiceBtn));

      // Guild overview: one row per music bot for the selected guild, from
      // GET /api/guilds/:guild_id/control-matrix. Loaded on demand rather
      // than live -- each load resolves channel names through Discord for
      // every bot, which is too heavy to repeat on a broadcast tick.
      const overviewBox = document.getElementById("guild-overview");
      const overviewBtn = document.getElementById("guild-overview-btn");
      let overviewGuild = null;
      function overviewRow(bot) {
        const s = bot.session || {};
        const key = bot.key || bot.bot_key || s.bot_key || "";
        const name = bot.display_name || bot.bot_display || s.bot_display || key;
        if (bot.error) {
          return `<tr><td><strong>${swarmEsc(name)}</strong></td><td><span class="data-pill data-pill-danger">Unavailable</span></td><td colspan="2">${swarmEsc(bot.error)}</td></tr>`;
        }
        const playing = s.is_playing === true, paused = s.is_paused === true;
        const channel = s.channel_name ? `#${s.channel_name}` : (s.channel_id ? `Channel ${s.channel_id}` : "Not connected");
        const tone = playing ? "live" : paused ? "soft" : "off";
        const actions = (playing || paused) ? `
          <button type="button" data-overview-action="${paused ? "RESUME" : "PAUSE"}" data-bot="${swarmEsc(key)}">${paused ? "Resume" : "Pause"}</button>
          <button type="button" data-overview-action="SKIP" data-bot="${swarmEsc(key)}">Skip</button>` : "";
        return `<tr>
          <td><strong>${swarmEsc(name)}</strong><br><small>${swarmEsc(channel)}</small></td>
          <td><span class="data-pill data-pill-${tone}">${swarmEsc(s.session_state_label || (playing ? "Playing" : "Idle"))}</span></td>
          <td>${swarmEsc(s.title || "—")}<br><small>${Number(s.queue_count || 0)} queued</small></td>
          <td><div class="table-actions">${actions}</div></td>
        </tr>`;
      }
      async function loadGuildOverview() {
        const guildId = form.guild_id.value;
        if (!guildId) { swarmToast("Choose a guild first.", "error"); return; }
        overviewGuild = guildId;
        overviewBtn.disabled = true;
        overviewBox.innerHTML = '<div class="empty-state">Loading every bot in this guild...</div>';
        try {
          const res = await swarmFetch(`/api/guilds/${encodeURIComponent(guildId)}/control-matrix`);
          const bots = res.bots || [];
          const active = bots.filter((b) => b.session && (b.session.is_playing || b.session.is_paused)).length;
          overviewBox.innerHTML = bots.length ? `
            <p class="muted">${active} of ${bots.length} bots active in this guild. Loaded ${swarmEsc(new Date().toLocaleTimeString())}.</p>
            <div class="table-wrap"><table class="data-table">
              <thead><tr><th>Bot</th><th>State</th><th>Now playing</th><th></th></tr></thead>
              <tbody>${bots.map(overviewRow).join("")}</tbody>
            </table></div>` : '<div class="empty-state">No music bots found.</div>';
        } catch (err) {
          overviewBox.innerHTML = '<div class="notice notice-error">' + swarmEsc(err.message) + "</div>";
        } finally {
          overviewBtn.disabled = false;
        }
      }
      if (overviewBtn) overviewBtn.addEventListener("click", loadGuildOverview);
      if (overviewBox) {
        overviewBox.addEventListener("click", async (e) => {
          const btn = e.target.closest("[data-overview-action]");
          if (!btn || !overviewGuild) return;
          btn.disabled = true;
          try {
            await swarmFetch("/api/bots/control", {
              method: "POST",
              body: JSON.stringify({ bot_key: btn.getAttribute("data-bot"), guild_id: overviewGuild, action: btn.getAttribute("data-overview-action"), payload: {} }),
            });
            swarmToast("Order sent.", "success");
            loadGuildOverview();
          } catch (err) {
            swarmToast(err.message, "error");
            btn.disabled = false;
          }
        });
      }
    ]]

    return page_shell(req, a, "/controls", "Controls", body, script)
  end)

  -- -----------------------------------------------------------------
  -- Invites
  -- -----------------------------------------------------------------
  httpd.route("GET", "/invites", function(req)
    local a, status, headers = cfg.require_auth_page(req)
    if not a then return status, "", headers end
    local body = html.page({
      title = "Invites", eyebrow = "Fleet", lede = "Invite links for every bot in the swarm.",
      body = '<div id="invite-cards" class="invite-grid">' .. html.empty_state("Loading...") .. "</div>",
    })
    local script = [[
      function applyInvites(res) {
        const cards = (res.invite_bots || res.bots || []).map((b) => {
          const avatar = b.icon_url
            ? `<img class="avatar invite-avatar" src="${b.icon_url}" alt="">`
            : `<span class="avatar invite-avatar avatar-fallback">${(b.identity_name || b.name || b.display_name || "?").slice(0, 1).toUpperCase()}</span>`;
          return `
          <div class="invite-card" style="--card-accent: ${b.accent || "#89b4fa"}">
            <div class="invite-card-head">
              ${avatar}
              <div class="invite-card-copy">
                <strong>${b.name || b.display_name}</strong>
                <small>${(b.capability_summary || "").replace(/</g, "&lt;")}</small>
              </div>
              ${b.connected_to_session_guild ? '<span class="data-pill data-pill-live">In your guild</span>' : ""}
            </div>
            ${b.identity_error ? `<p class="notice notice-error">${b.identity_error.replace(/</g, "&lt;")}</p>` : ""}
            <p>
              ${b.invite_url ? `<a class="button-link primary" href="${b.invite_url}" target="_blank" rel="noreferrer">Invite</a>` : "<span>No invite available</span>"}
            </p>
          </div>`;
        }).join("");
        document.getElementById("invite-cards").innerHTML = cards || "<p>No bots found.</p>";
      }
      window.swarmLive.watch("invites", (msg) => {
        if (msg.type === "snapshot_error") { swarmToast("Failed to load bots.", "error"); return; }
        if (msg.type === "snapshot") applyInvites(msg.data);
      });
    ]]
    return page_shell(req, a, "/invites", "Invites", body, script)
  end)
end

return M
