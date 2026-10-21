-- SwarmPanel backend (Lua rewrite) entrypoint.
package.path = "./lib/?.lua;./lib/?/init.lua;./src/?.lua;" .. package.path

local config = require("config")
local db = require("db")
local httpd = require("httpd")
local routes = require("routes")
-- Server-rendered screens, one module per nav.lua section.
local page_modules = {
  require("pages_auth"),
  require("pages_fleet"),
  require("pages_insights"),
  require("pages_telemetry"),
  require("pages_community"),
  require("pages_account"),
  require("pages_admin"),
}
local static = require("static")
local copas = require("copas")
local metrics = require("metrics")
local telemetry = require("telemetry")
local socket = require("socket")

local settings = config.load()
db.init(settings)
telemetry.init()

httpd.cors.allowed_origins = settings.cors_allowed_origins
-- Settings ship a Python `re` pattern for preview-tunnel origins, e.g.
--   https://[a-zA-Z0-9-]+\.(trycloudflare\.com|ngrok-free\.dev|ngrok\.io)
-- Lua patterns have no alternation operator, so a literal regex translation
-- is not possible. Instead, pull the suffix alternatives out of the `(a|b|c)`
-- group and check "https://<subdomain>.<suffix>" directly. This covers the
-- one shape actually used in .env; anything more exotic than that falls back
-- to exact-origin matching only (cors_allowed_origins).
local function build_origin_suffix_matcher(pcre)
  if not pcre or pcre == "" then return nil end
  local alternation = pcre:match("%(([^%)]+)%)")
  if not alternation then return nil end
  local suffixes = {}
  for part in alternation:gmatch("[^|]+") do
    suffixes[#suffixes + 1] = part:gsub("\\%.", ".")
  end
  return function(origin)
    if not origin then return false end
    local subdomain = origin:match("^https://([%w%-]+)%.")
    if not subdomain then return false end
    for _, suffix in ipairs(suffixes) do
      if origin == "https://" .. subdomain .. "." .. suffix then return true end
    end
    return false
  end
end
httpd.cors.origin_suffix_matcher = build_origin_suffix_matcher(settings.cors_allow_origin_regex)

routes.register({
  settings = settings,
  music_bots = config.music_bots,
  aria_bot = config.aria_bot,
  bot_index = config.bot_index,
  bot_tokens = config.bot_tokens,
  bot_accents = config.bot_accents,
})
local pages_cfg = {
  settings = settings,
  music_bots = config.music_bots,
  aria_bot = config.aria_bot,
  get_auth = routes.get_auth,
  require_auth_page = routes.require_auth_page,
  scoped_dashboard_data = routes.scoped_dashboard_data,
  session_cookie_header = routes.session_cookie_header,
  clear_session_cookie_header = routes.clear_session_cookie_header,
}
for _, mod in ipairs(page_modules) do mod.register(pages_cfg) end
static.register()

-- Port of app/main.py's _metrics_history_capture_loop(): samples fleet
-- totals into swarm_metrics_history every 5 minutes so the Intel page's
-- trend charts/anomaly detection have data newer than the Python app's
-- retirement. This is the one background task the Lua rewrite's read-only
-- metrics.lua port explicitly left out for lack of a scheduler -- copas
-- (already used for the dashboard WebSocket broadcast loop) works fine as
-- one.
-- Social-table indexes (see social.lua's ensure_indexes): run once, off the
-- request path, shortly after startup so the listener is already up.
copas.addthread(function()
  copas.sleep(5)
  local ok, err = pcall(require("social").ensure_indexes)
  if not ok then print("[swarmpanel-lua] social index setup failed: " .. tostring(err)) end
end)

local METRICS_HISTORY_CAPTURE_INTERVAL_SECONDS = 300
copas.addthread(function()
  copas.sleep(30)
  while true do
    -- Index + retention for swarm_metrics_history; see metrics.lua's own
    -- comment for the measured cost of having had neither.
    pcall(metrics.ensure_history_schema)
    local ok, err = pcall(metrics.capture_metrics_snapshot, config.music_bots)
    if not ok then print("[swarmpanel-lua] metrics history capture failed: " .. tostring(err)) end
    pcall(metrics.maybe_cleanup_history)
    copas.sleep(METRICS_HISTORY_CAPTURE_INTERVAL_SECONDS)
  end
end)

-- Panel-wide telemetry/analytics (2026-09-14, per operator request): a
-- structured, queryable event log (swarmpanel_telemetry_events, fed by
-- httpd.lua's per-request recorder + audit.lua's admin-action fan-out +
-- routes.lua's login analytics) plus a periodic process resource/activity
-- snapshot (swarmpanel_process_stats), matching the same pattern already
-- deployed fleet-wide across the 13 music bots + Aria.
local PROCESS_START_TIME = socket.gettime()
local TELEMETRY_SNAPSHOT_INTERVAL_SECONDS = 30
copas.addthread(function()
  while true do
    copas.sleep(TELEMETRY_SNAPSHOT_INTERVAL_SECONDS)
    local ok, err = pcall(function()
      telemetry.snapshot({
        memory_kb = collectgarbage("count"),
        uptime_seconds = math.floor(socket.gettime() - PROCESS_START_TIME),
        active_websocket_count = telemetry.active_websocket_count(),
        requests_last_interval = telemetry.pop_request_count(),
      })
      telemetry.maybe_cleanup(30, 40)
    end)
    if not ok then print("[swarmpanel-lua] telemetry snapshot failed: " .. tostring(err)) end
  end
end)

httpd.listen(settings.bind_host, settings.port)
httpd.run()
