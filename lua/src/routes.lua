-- Route wiring. Only the Bearer-token auth path is implemented (see auth.lua
-- and the final report): frontend/src/api.js always fetches with
-- `credentials: "omit"` and stores the token itself, so the cookie-session
-- half of app/auth.py's model is dead code for the deployed SPA and is
-- intentionally not ported yet.
local httpd = require("httpd")
local auth = require("auth")
local db = require("db")
local accounts = require("accounts")
local dashboard = require("dashboard")
local control = require("control")
local channel_convert = require("channel_convert")
local social = require("social")
local profiles = require("profiles")
local audit = require("audit")
local telemetry = require("telemetry")
local queues = require("queues")
local alerts = require("alerts")
local admin_browser = require("admin_browser")
local gallery = require("gallery")
local csv_export = require("csv_export")
local lumisound = require("lumisound")
local metrics = require("metrics")
local discord_service = require("discord_service")
local looks = require("looks")
local colorutil = require("colorutil")

-- Attaches computed --accent-contrast-text / --accent-gradient / a derived
-- accent_secondary (when the caller hasn't picked one) onto a preferences-
-- or profile-shaped table that has an accent_color/theme_accent field.
-- Shared by the panel-preferences and profile-preferences response paths so
-- the frontend never has to guess at readable text color again.
local function with_derived_accent(prefs, accent_key, secondary_key)
  accent_key = accent_key or "accent_color"
  secondary_key = secondary_key or "accent_secondary"
  local accent = prefs[accent_key]
  if not accent or accent == "" then return prefs end
  local secondary = prefs[secondary_key]
  if not secondary or secondary == "" then secondary = colorutil.auto_secondary(accent) end
  prefs.accent_contrast_text = colorutil.contrast_text(accent)
  prefs.accent_gradient = colorutil.gradient(accent, secondary)
  return prefs
end
local notify = require("notify")
local ratelimit = require("ratelimit")
local copas = require("copas")
local socket = require("socket")
local cjson = require("cjson.safe")

local M = {}

function M.register(cfg)
  local settings = cfg.settings
  local music_bots = cfg.music_bots
  local bot_index = cfg.bot_index

  local function json_error(status, detail)
    return status, { detail = detail }
  end

  -- Server-rendered pages (plain <a href> navigation, no fetch()) can't send
  -- an Authorization header, so they authenticate via the swarm_session
  -- cookie instead -- same opaque token, just carried a different way. The
  -- JSON API's Bearer-header path (frontend/src/api.js, and any future
  -- fetch()-based JS on the rendered pages) is unchanged and checked first.
  local SESSION_COOKIE = "swarm_session"

  -- Device sessions ("remember this device", native apps only): a longer
  -- token that each refresh re-issues with the same lifetime. See
  -- config.lua's device_token_ttl_seconds.
  local function token_ttl(device)
    if device then return settings.device_token_ttl_seconds or 2592000 end
    return settings.api_token_ttl_seconds
  end
  local KNOWN_CLIENTS = { web = true, ios = true, tvos = true, widget = true, linux = true }
  local function client_name(value)
    local c = tostring(value or ""):lower()
    return KNOWN_CLIENTS[c] and c or nil
  end

  -- A presented-but-rejected token (expired, bad signature, ...) is what a
  -- "why did I get logged out" report comes down to, and was invisible
  -- before. Throttled per IP+reason so a client stuck retrying a dead token
  -- can't flood the telemetry table.
  local token_reject_last = {}
  local function record_token_rejected(req, reason, via)
    local key = tostring(req.client_ip) .. ":" .. tostring(reason)
    local now = os.time()
    if (token_reject_last[key] or 0) > now - 60 then return end
    token_reject_last[key] = now
    if next(token_reject_last) and math.random(50) == 1 then
      for k, t in pairs(token_reject_last) do
        if t <= now - 60 then token_reject_last[k] = nil end
      end
    end
    pcall(telemetry.record, "auth", "token_rejected", {
      path = req.path, client_ip = req.client_ip, text_value = reason,
      metadata = { reason = reason, via = via },
    })
  end

  local function get_auth(req)
    local token = auth.extract_bearer_token(req.headers["authorization"])
    local via = "bearer"
    if not token and req.cookies then token, via = req.cookies[SESSION_COOKIE], "cookie" end
    if not token then return nil end
    local a, reject_reason = auth.verify_api_token(settings.session_secret, token)
    if not a then record_token_rejected(req, reject_reason, via) end
    -- Side effect, not just a return value: httpd.lua's request-telemetry
    -- recorder (see its own comment) reads req.auth AFTER the handler
    -- returns to attribute the request to a username/guild -- never the
    -- token itself, just the already-verified identity this call already
    -- computed, so no second lookup/cost is paid for it.
    if a then req.auth = a end
    return a
  end

  -- Set-Cookie value for a freshly-issued token. HttpOnly (no client JS
  -- needs to read it -- the page-rendered UI never touches the token
  -- directly) + SameSite=Lax (survives normal top-level navigation, blocks
  -- cross-site POST) + Max-Age matching the token's own embedded expiry.
  -- BUGFIX: was missing Secure -- see config.lua's cookie_secure for why.
  local COOKIE_SECURE_ATTR = settings.cookie_secure and "; Secure" or ""
  local function session_cookie_header(token)
    return ("%s=%s; Path=/; HttpOnly; SameSite=Lax%s; Max-Age=%d"):format(
      SESSION_COOKIE, token, COOKIE_SECURE_ATTR, settings.api_token_ttl_seconds or 43200)
  end
  local function clear_session_cookie_header()
    return SESSION_COOKIE .. "=; Path=/; HttpOnly; SameSite=Lax" .. COOKIE_SECURE_ATTR .. "; Max-Age=0"
  end

  -- Port of auth_deps.py's _touch_request_presence: every authenticated
  -- request (API or page) refreshes last_seen_at, throttled so it's one
  -- UPDATE per ~30s of activity rather than per-request. accounts.lua's
  -- touch_account_seen() existed already (ported from db/accounts.py) but
  -- was never actually called from anywhere, so last_seen_at only ever got
  -- set once at login -- every account showed "Inactive" a few minutes
  -- into a session no matter how actively they were using the panel.
  local PRESENCE_TOUCH_INTERVAL_SECONDS = 30
  -- touch_account_seen's UPDATE is already a no-op inside the interval, but
  -- it still cost a DB round trip on every authenticated request. Remember
  -- when each account was last touched and skip the query until it's due.
  local last_presence_touch = {} -- "username\0guild" -> socket.gettime()
  local function touch_presence(a)
    if not a then return end
    local username = tostring(a.username or ""):match("^%s*(.-)%s*$")
    local guild_id = a.guild_id
    if username == "" or not guild_id or guild_id == "" then return end
    local touch_key = username:lower() .. "\0" .. tostring(guild_id)
    local now = socket.gettime()
    local last = last_presence_touch[touch_key]
    if last and now - last < PRESENCE_TOUCH_INTERVAL_SECONDS then return end
    last_presence_touch[touch_key] = now
    local ok, err = pcall(accounts.touch_account_seen, username, guild_id, PRESENCE_TOUCH_INTERVAL_SECONDS)
    if not ok then
      print("[swarmpanel-lua] presence touch failed for " .. username .. ": " .. tostring(err))
    end
  end

  local function require_auth(req)
    local a = get_auth(req)
    if not a then return nil, json_error(401, "Authentication required") end
    touch_presence(a)
    return a
  end

  -- Page-route counterpart to require_auth: a plain <a href> GET can't do
  -- anything with a 401 JSON body, so unauthenticated page requests get a
  -- 303 redirect to /login instead (mirrors App.jsx's Protected component
  -- rendering <Navigate to="/login">).
  local function require_auth_page(req)
    local a = get_auth(req)
    if not a then return nil, 303, { Location = "/login?next=" .. httpd.url_encode(req.path) } end
    touch_presence(a)
    return a
  end

  -- Exposed so the pages_*.lua modules (server-rendered HTML routes, registered separately
  -- from main.lua) can share the exact same auth/cookie logic rather than
  -- reimplementing it -- both modules authenticate against the one
  -- SESSION_COOKIE convention defined above.
  M.get_auth = get_auth
  M.require_auth_page = require_auth_page
  M.session_cookie_header = session_cookie_header
  M.clear_session_cookie_header = clear_session_cookie_header

  local function is_site_owner(a) return a and a.site_owner == true end
  local function is_admin_auth(a)
    if not a or not is_site_owner(a) then return false end
    if a.admin_mode ~= nil then return a.admin_mode end
    return tostring(a.role or "admin"):lower() == "admin" and not a.guild_id
  end

  -- Port of auth_deps.py's _scoped_guild_id(): nil means "unrestricted /
  -- admin", OWNER_SCOPE_SENTINEL means "authenticated but with no guild
  -- binding at all" (sees nothing), any other string is the one guild this
  -- caller may see.
  local OWNER_SCOPE_SENTINEL = "__site_owner_only__"
  local function scoped_guild_id(a)
    if a and not is_site_owner(a) and (a.guild_id == nil or a.guild_id == "") then
      return OWNER_SCOPE_SENTINEL
    end
    if is_admin_auth(a) then return nil end
    local gid = a and a.guild_id
    if gid ~= nil and gid ~= "" then return tostring(gid) end
    return nil
  end

  local function require_admin(req)
    local a, err_status, err_body = require_auth(req)
    if not a then return nil, err_status, err_body end
    if not is_admin_auth(a) then return nil, 403, { detail = "Admin access required" } end
    return a
  end

  local function is_moderator_auth(a) return a and a.moderator == true end
  local function require_admin_or_moderator(req)
    local a, err_status, err_body = require_auth(req)
    if not a then return nil, err_status, err_body end
    if not (is_admin_auth(a) or is_moderator_auth(a)) then return nil, 403, { detail = "Admin or moderator access required" } end
    return a
  end

  local function is_image_gallery_owner_auth(a) return is_admin_auth(a) and is_site_owner(a) end
  local function require_image_gallery_owner(req)
    local a, err_status, err_body = require_auth(req)
    if not a then return nil, err_status, err_body end
    if not is_image_gallery_owner_auth(a) then return nil, 403, { detail = "Image Gallery owner access required" } end
    return a
  end
  local function require_gallery_admin_or_moderator(req)
    local a, err_status, err_body = require_auth(req)
    if not a then return nil, err_status, err_body end
    if not (is_image_gallery_owner_auth(a) or is_moderator_auth(a)) then return nil, 403, { detail = "Image Gallery admin or moderator access required" } end
    return a
  end

  -- Port of auth_deps.py's _account_guild_id()/_account_id_for_auth(). NOTE:
  -- unlike the Python original's _hydrate_site_owner_auth(), this does not
  -- re-check the DB on every request for a site-owner promotion that
  -- happened after token issuance — the token already carries site_owner/
  -- admin_mode computed at login/session-refresh time (see /api/session).
  -- Accepted simplification given the 12h token TTL; see final report.
  local function account_guild_id(a)
    local gid = a and a.guild_id
    if gid ~= nil and gid ~= "" then return tostring(gid) end
    return nil
  end

  local function account_id_for_auth(a)
    local username = tostring((a and a.username) or ""):match("^%s*(.-)%s*$")
    local gid = account_guild_id(a)
    if username == "" or not gid then return nil, "Guild account access required" end
    local ok, profile = pcall(accounts.get_account_profile, username, gid)
    if not ok or not profile then return nil, "Account profile not found" end
    return db.toint(profile.id)
  end

  -- Port of auth_deps.py's _require_guild_scope(): returns an error tuple if
  -- this caller is restricted to one guild and it isn't the one requested.
  local function require_guild_scope(req, a, guild_id)
    local scoped = scoped_guild_id(a)
    if scoped and tostring(guild_id) ~= scoped then
      return 403, { detail = "This account can only control its registered guild" }
    end
    return nil
  end

  -- Port of dashboard.py's _bot_has_registered_guild()/_require_bot_guild_access().
  -- Reuses music_bot_snapshot's already-tested known_guild_ids computation
  -- rather than a second bespoke query, then falls back to a live Discord
  -- guild-membership check exactly like the Python original.
  local function bot_has_registered_guild(bot, guild_id)
    if not bot or bot.kind ~= "music" then return false end
    local ok, snapshot = pcall(dashboard.music_bot_snapshot, bot)
    if ok and snapshot and snapshot.known_guild_ids then
      for _, gid in ipairs(snapshot.known_guild_ids) do
        if gid == tostring(guild_id) then return true end
      end
    end
    local token = cfg.bot_tokens[bot.key]
    if not token or token == "" then return false end
    local ok2, guilds = pcall(discord_service.fetch_guilds, token)
    if not ok2 or not guilds then return false end
    for _, g in ipairs(guilds) do
      if tostring(g.id) == tostring(guild_id) then return true end
    end
    return false
  end

  local function require_bot_guild_access(req, a, bot_key, guild_id)
    local gerr_status, gerr_body = require_guild_scope(req, a, guild_id)
    if gerr_status then return gerr_status, gerr_body end
    local scoped = scoped_guild_id(a)
    if scoped and not bot_has_registered_guild(bot_index[bot_key], scoped) then
      return 403, { detail = "This bot is not registered with your guild" }
    end
    return nil
  end

  -- Port of app/routers/bots.py's _enrich_control_state_with_discord() +
  -- _control_state_error_payload()'s nested shape. CRITICAL SHAPE BUG found
  -- while wiring this up: dashboard.get_bot_control_state() returns a FLAT
  -- table (bot_key, bot_display, guild_id, channel_id, ... all top-level),
  -- but ControlsPage.jsx / components/swarm.jsx's ControlState always read
  -- `state.session.channel_name` / `state.session.home_channel_name` etc
  -- (matching the Python original's nested {key, display_name, guild_id,
  -- db, discord, session:{...}} response shape). The already-shipped
  -- /api/guilds/:guild_id/control-matrix route returned the flat shape
  -- directly, so `bot.session` was always undefined on the frontend --
  -- THIS, not just the missing routes, is why guild/voice/text channel
  -- names never rendered ("not found"/blank) even when the bot/guild data
  -- was actually available. Fixed here by reshaping flat -> nested before
  -- Discord enrichment, for both this route and control-matrix below.
  --
  -- NOTE: feedback_channel_id/feedback_channel_name aren't in this Lua
  -- port's session shape at all yet (see dashboard.lua's music_bot_snapshot
  -- -- feedback_channel_id was never queried), so only guild/channel/home
  -- are enriched here; a real follow-up, not silently worked around.
  local function enrich_control_state_with_discord(flat)
    local session = {}
    for k, v in pairs(flat) do session[k] = v end
    local state = {
      key = flat.bot_key, display_name = flat.bot_display, guild_id = flat.guild_id,
      db = { status = flat.status, reachable = flat.status == "online", message = flat.heartbeat_status },
      session = session,
    }

    local token = cfg.bot_tokens[state.key] or ""
    local guild_id = tostring(state.guild_id)
    if token == "" then
      state.discord = { status = "missing", reachable = false, message = "Panel token is not configured for Discord inventory access.", token_configured = false }
      return state
    end

    local ok, err = pcall(function()
      local guild = discord_service.fetch_guild(token, guild_id)
      local placements = { { guild_id = guild_id, channel_id = nil } }
      if session.channel_id then placements[#placements + 1] = { guild_id = guild_id, channel_id = session.channel_id } end
      if session.home_channel_id then placements[#placements + 1] = { guild_id = guild_id, channel_id = session.home_channel_id } end

      local name_map = discord_service.resolve_guild_channel_names(token, placements)
      local guild_meta = name_map[guild_id .. "\0"] or {}
      session.guild_name = guild_meta.guild_name or guild.name or ("Guild " .. guild_id)

      if session.channel_id then
        local m = name_map[guild_id .. "\0" .. tostring(session.channel_id)]
        session.channel_name = m and m.channel_name or nil
      end
      if session.home_channel_id then
        local m = name_map[guild_id .. "\0" .. tostring(session.home_channel_id)]
        session.home_channel_name = m and m.channel_name or nil
      end
    end)
    if ok then
      state.discord = { status = "online", reachable = true, message = "Live Discord route is valid in " .. tostring(session.guild_name) .. ".", token_configured = true }
    else
      state.discord = { status = "error", reachable = false, message = tostring(err):sub(1, 240), token_configured = true }
    end
    return state
  end

  -- ---------------------------------------------------------------- health
  httpd.route("GET", "/healthz", function(req)
    local ok = pcall(db.fetchone, "accountlogins", "SELECT 1 AS ok")
    if ok then return 200, { ok = true, db = "ok" } end
    return 503, { ok = false, db = "unavailable" }
  end)

  httpd.route("GET", "/api/health", function(req)
    local a = get_auth(req)
    if not a then return 401, { detail = "Authentication required" } end
    local db_ok = pcall(db.fetchone, "accountlogins", "SELECT 1 AS ok")
    return 200, {
      ok = db_ok, db = db_ok and "ok" or "unavailable",
      api_max_rows = settings.api_max_rows,
      bot_control_source_max_chars = settings.bot_control_source_max_chars,
    }
  end)

  -- --------------------------------------------------------------- session
  local function resolve_account_guild_id(a)
    if a.guild_id and a.guild_id ~= "" then return a.guild_id end
    if a.username then
      local ok, gid = pcall(accounts.get_account_guild_id_for_username, a.username)
      if ok then return gid end
    end
    return nil
  end

  httpd.route("GET", "/api/session", function(req)
    local a = get_auth(req)
    if not a then return 200, { authenticated = false, pages_public_url = settings.pages_public_url } end
    local username = a.username or settings.admin_username
    local role = a.role or "admin"
    local account_guild_id = resolve_account_guild_id(a)
    local site_owner = is_site_owner(a)
    local admin_mode = is_admin_auth(a)
    -- A device session stays a device session across refreshes (same
    -- long TTL, same client tag); a normal session keeps the normal TTL.
    local device = a.device == true
    local fresh_token = auth.issue_api_token(settings.session_secret, username, {
      role = role, guild_id = account_guild_id or a.guild_id, admin_mode = admin_mode,
      site_owner = site_owner, moderator = a.moderator, ttl_seconds = token_ttl(device),
      device = device, client = a.client,
    })
    if a.client and a.client ~= "web" then
      pcall(telemetry.record, "auth", "session_refresh", {
        username = username, guild_id = account_guild_id, client_ip = req.client_ip,
        numeric_value = a.exp and (a.exp - os.time()) or nil,
        metadata = { client = a.client, device = device },
      })
    end
    return 200, {
      authenticated = true,
      mode = "token",
      username = username,
      role = role,
      guild_id = (a.guild_id or account_guild_id) and tostring(a.guild_id or account_guild_id) or nil,
      account_guild_id = account_guild_id,
      site_owner = site_owner,
      admin_mode = admin_mode,
      moderator = a.moderator == true,
      image_gallery_owner = admin_mode and site_owner,
      token = fresh_token,
      pages_public_url = settings.pages_public_url,
      expires_in = token_ttl(device),
    }, { ["Set-Cookie"] = session_cookie_header(fresh_token) }
  end)

  -- Port of app/routers/session.py's POST /api/session/admin-mode: lets a
  -- verified site-owner account flip between its normal guild-scoped view
  -- and the unrestricted admin view, re-issuing the token with the new flag.
  httpd.route("POST", "/api/session/admin-mode", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    if not is_site_owner(a) then return 403, { detail = "Admin mode is locked to the verified owner account." } end
    local username = tostring(a.username or settings.admin_username)
    local linked_guild_id = resolve_account_guild_id(a)
    if not linked_guild_id then return 400, { detail = "This owner account is not linked to a registered guild." } end
    local body = req.json or {}
    local admin_mode = body.enabled == true
    -- Remember the choice so the next login starts in the same mode.
    -- Best-effort: failing to save it must not block the switch itself.
    local saved_ok, saved = pcall(accounts.set_admin_mode_preference, username, linked_guild_id, admin_mode)
    if not saved_ok or not saved then
      print("[swarmpanel-lua] could not save admin-mode preference for " .. username .. ": " .. tostring(saved))
    end
    local role = a.role or "account"
    local fresh_token = auth.issue_api_token(settings.session_secret, username, {
      role = role, guild_id = linked_guild_id, admin_mode = admin_mode,
      site_owner = true, moderator = a.moderator, ttl_seconds = token_ttl(a.device == true),
      device = a.device == true, client = a.client,
    })
    return 200, {
      ok = true, username = username, role = role,
      guild_id = linked_guild_id, account_guild_id = linked_guild_id,
      site_owner = true, admin_mode = admin_mode,
      image_gallery_owner = admin_mode and true or false,
      token = fresh_token,
      pages_public_url = settings.pages_public_url,
      expires_in = token_ttl(a.device == true),
    }, { ["Set-Cookie"] = session_cookie_header(fresh_token) }
  end)

  httpd.route("POST", "/api/session/login", function(req)
    local body = req.json or {}
    local username = tostring(body.username or "")
    local password = tostring(body.password or "")
    local guild_id = body.guild_id

    local rl_status, rl_body = ratelimit.check(
      ("login-api:%s:%s"):format(req.client_ip or "unknown", username:lower():sub(1, 80)), 15, 900)
    if rl_status then return rl_status, rl_body end
    -- Second, IP-independent cap keyed on username alone: the ip:username
    -- bucket above stops one IP from hammering a username, but a
    -- distributed/rotating-IP attacker can still brute-force a fixed
    -- username (e.g. the PANEL_ADMIN_USERNAME account) at 15 attempts/900s
    -- PER IP indefinitely across many IPs. This bucket is global across all
    -- IPs for the same username, with a more generous window/count so
    -- legitimate multi-IP admin use (phone + laptop + office) isn't blocked.
    local rl2_status, rl2_body = ratelimit.check(
      ("login-api-username:%s"):format(username:lower():sub(1, 80)), 30, 900)
    if rl2_status then return rl2_status, rl2_body end

    local auth_result = nil
    if auth.verify_credentials(username, password, settings.admin_username, settings.admin_password) then
      auth_result = { username = username, role = "admin", guild_id = nil, site_owner = true, admin_mode = true, moderator = false }
    else
      local secret = password ~= "" and password or (guild_id and tostring(guild_id) or nil)
      if secret then
        local ok, account = pcall(accounts.authenticate_account_login, username, secret)
        if ok and account then
          -- site_owner requires the configured owner email on this account,
          -- optionally requiring verification — mirrors auth_deps.py's
          -- _is_site_owner_account(). Verification-tracking columns are
          -- ported (accounts.lua), so this check is real, not a stub.
          local site_owner = false
          if account.email and settings.site_owner_email ~= "" and account.email:lower() == settings.site_owner_email then
            site_owner = (not settings.owner_email_requires_verification) or account.email_verified
          end
          -- The owner starts in admin mode unless they switched it off
          -- last time (accounts.get_admin_mode_preference); a lookup
          -- failure falls back to the old default of admin mode on.
          local admin_mode = site_owner
          if site_owner then
            local pref_ok, pref = pcall(accounts.get_admin_mode_preference, account.username, account.guild_id)
            if pref_ok and pref == false then admin_mode = false end
          end
          auth_result = {
            username = account.username, role = "account", guild_id = account.guild_id,
            site_owner = site_owner, moderator = account.panel_role == "moderator", admin_mode = admin_mode,
          }
        end
      end
    end

    if not auth_result then
      -- Login analytics: the attempted username (never the password) so a
      -- brute-force/credential-stuffing pattern against one account is
      -- visible -- generic HTTP telemetry alone (see httpd.lua) only shows
      -- "POST /api/session/login -> 401", with no way to tell one repeatedly
      -- failing username apart from many different ones failing once each.
      pcall(telemetry.record, "auth", "login_failed", {
        username = username, client_ip = req.client_ip, metadata = { client = client_name(body.client) or "web" },
      })
      return 401, { detail = "Invalid username or password" }
    end
    -- Native apps ask to be remembered so closing the app doesn't mean
    -- signing in again (see token_ttl). Only honored for a known native
    -- client tag; browsers keep the normal cookie-length session.
    local client = client_name(body.client) or "web"
    local device = body.remember_device == true and client ~= "web"
    pcall(telemetry.record, "auth", "login_success", {
      username = auth_result.username, guild_id = auth_result.guild_id, client_ip = req.client_ip,
      metadata = { role = auth_result.role, site_owner = auth_result.site_owner, client = client, device = device },
    })

    -- Clients that must never run in admin mode (the tvOS dashboard) ask
    -- for a non-admin session here. This only shapes THIS token -- it does
    -- not touch the owner's saved admin-mode preference, so signing in on
    -- the TV doesn't switch admin mode off on the web. GET /api/session
    -- refreshes keep the token's own admin_mode, so it stays off.
    if body.disable_admin_mode == true then auth_result.admin_mode = false end

    local token = auth.issue_api_token(settings.session_secret, auth_result.username, {
      role = auth_result.role, guild_id = auth_result.guild_id, admin_mode = auth_result.admin_mode,
      site_owner = auth_result.site_owner, moderator = auth_result.moderator, ttl_seconds = token_ttl(device),
      device = device, client = client,
    })
    return 200, {
      ok = true, token = token, username = auth_result.username, role = auth_result.role,
      guild_id = auth_result.guild_id, account_guild_id = auth_result.guild_id,
      site_owner = auth_result.site_owner, admin_mode = auth_result.admin_mode, moderator = auth_result.moderator,
      image_gallery_owner = auth_result.admin_mode and auth_result.site_owner,
      pages_public_url = settings.pages_public_url, expires_in = token_ttl(device),
    }, { ["Set-Cookie"] = session_cookie_header(token) }
  end)

  -- ----------------------------------------------------- client telemetry
  -- Batched events from the native apps (tvOS today): launches, session
  -- restores/expiries, socket drops, load latency, ... -- the things the
  -- server can't see for itself. Stored in the same swarmpanel_telemetry_events
  -- table under category "client", with the app's platform/version/launch id
  -- folded into metadata. telemetry.record() already drops credential-looking
  -- keys and redacts token-shaped text.
  --
  -- Auth is optional and this route never answers 401: the apps sign the
  -- user out on any 401, and a telemetry upload must never be the thing
  -- that does that (an expired token here just means the batch is recorded
  -- without a username).
  local CLIENT_TELEMETRY_MAX_BYTES = 65536
  local CLIENT_TELEMETRY_MAX_EVENTS = 50
  local function clean_event_name(v)
    local name = tostring(v or ""):gsub("[^%w_%.%-]", ""):sub(1, 80)
    return name ~= "" and name or nil
  end
  local function clean_meta_string(v, max_len)
    if v == nil then return nil end
    return tostring(v):sub(1, max_len or 120)
  end
  httpd.route("POST", "/api/telemetry/client", function(req)
    if #(req.raw_body or "") > CLIENT_TELEMETRY_MAX_BYTES then
      return 413, { detail = "Telemetry batch too large." }
    end
    local a = get_auth(req)
    local rl_key = "client-telemetry:" .. tostring((a and a.username) or req.client_ip or "unknown"):lower()
    if not ratelimit.allow(rl_key, 10, 60) then
      return 429, { detail = "Too many telemetry batches. Try again later." }
    end
    local body = req.json or {}
    local events = type(body.events) == "table" and body.events or {}
    local client = client_name(body.client) or "unknown"
    local base_meta = {
      client = client,
      app_version = clean_meta_string(body.app_version, 40),
      os_version = clean_meta_string(body.os_version, 40),
      device_model = clean_meta_string(body.device_model, 60),
      launch_id = clean_meta_string(body.launch_id, 64),
    }
    local accepted = 0
    for i = 1, math.min(#events, CLIENT_TELEMETRY_MAX_EVENTS) do
      local e = events[i]
      local name = type(e) == "table" and clean_event_name(e.name) or nil
      if name then
        local meta = {}
        for k, v in pairs(base_meta) do meta[k] = v end
        if type(e.metadata) == "table" then
          local n = 0
          for k, v in pairs(e.metadata) do
            n = n + 1
            if n > 20 then break end
            meta[tostring(k):sub(1, 40)] = (type(v) == "number" or type(v) == "boolean") and v or clean_meta_string(v, 200)
          end
        end
        meta.client_ts = tonumber(e.at)
        pcall(telemetry.record, "client", client .. "." .. name, {
          username = a and a.username or nil,
          guild_id = a and a.guild_id or nil,
          client_ip = req.client_ip,
          numeric_value = tonumber(e.value),
          text_value = e.text ~= nil and clean_meta_string(e.text, 500) or nil,
          metadata = meta,
        })
        accepted = accepted + 1
      end
    end
    return 200, { ok = true, accepted = accepted }
  end)

  httpd.route("POST", "/api/session/logout", function(req)
    -- Stateless bearer tokens: nothing server-side to invalidate (matches
    -- the "not implemented" cookie-session note above; a real revocation
    -- list is future work if that's ever needed). The cookie itself is
    -- cleared client-side so a server-rendered page stops being "logged in".
    return 200, { ok = true }, { ["Set-Cookie"] = clear_session_cookie_header() }
  end)

  -- Port of app/routers/session.py's POST /api/session/password: self-service
  -- password change, verifying current_password before writing new_password.
  httpd.route("POST", "/api/session/password", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required" } end
    local rl_status, rl_body = ratelimit.check(("password-change:%s"):format(username:lower()), 3, 600)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    local ok, profile_or_err = pcall(
      accounts.update_account_password, username, scoped_gid,
      body.current_password, body.new_password
    )
    if not ok then return 400, { detail = tostring(profile_or_err):gsub("^.-:%d+:%s*", "") } end
    if not profile_or_err then return 404, { detail = "Account profile not found" } end
    return 200, { ok = true, profile = profile_or_err }
  end)

  -- ------------------------------------------------------------- dashboard
  -- Port of dashboard.py's _build_dashboard_payload(). Factored into a
  -- standalone function (rather than inlined in the route below, as before)
  -- because the WS /ws live-feed handler further down needs the exact same
  -- guild-scoping + snapshot-building logic on a ~2s broadcast loop, with
  -- enrich_discord=false (mirrors websocket.py's own enrich_discord=False on
  -- its broadcast loop — Discord REST enrichment is too expensive to redo
  -- for every connected client every 2 seconds, so the HTTP route is the
  -- only caller that gets bot.discord identity/name enrichment).
  local function build_dashboard_payload(a, enrich_discord)
    if enrich_discord == nil then enrich_discord = true end
    local data = dashboard.get_dashboard_data(music_bots) -- pcall'd by callers

    local scoped = scoped_guild_id(a)
    local bots = data.bots or {}

    if scoped then
      local scoped_bots = {}
      for _, bot in ipairs(bots) do
        local known_ids = bot.known_guild_ids or {}
        local visible = false
        if bot.kind == "music" and scoped ~= OWNER_SCOPE_SENTINEL then
          for _, gid in ipairs(known_ids) do
            if gid == scoped then visible = true break end
          end
        end
        if visible then
          local sessions = {}
          local active_playing, queue_depth, backup_depth = 0, 0, 0
          for _, s in ipairs(bot.sessions or {}) do
            if tostring(s.guild_id) == scoped then
              sessions[#sessions + 1] = s
              if s.is_playing then active_playing = active_playing + 1 end
              queue_depth = queue_depth + (s.queue_count or 0)
              backup_depth = backup_depth + (s.backup_queue_count or 0)
            end
          end
          bot.sessions = sessions
          bot.known_guild_count = 1
          bot.guild_count = 1
          bot.active_playing_count = active_playing
          bot.queue_depth = queue_depth
          bot.backup_queue_depth = backup_depth
          scoped_bots[#scoped_bots + 1] = bot
        end
      end
      bots = scoped_bots
      data.bots = bots
    end

    -- Flatten + dedupe sessions across bots (dashboard.py's flattened_sessions
    -- / seen_session_keys), and fill per-bot/per-session defaults expected by
    -- the frontend (bot.name, bot.guild_count, session.bot_key/bot_name/bot_display).
    local flattened, seen = {}, {}
    for _, bot in ipairs(bots) do
      bot.name = bot.name or bot.display_name
      bot.guild_count = bot.guild_count or bot.known_guild_count or 0
      bot.known_guild_ids = nil -- internal-only field, not part of the API contract
      for _, s in ipairs(bot.sessions or {}) do
        s.bot_key = s.bot_key or bot.key
        s.bot_name = s.bot_name or bot.display_name
        s.bot_display = s.bot_display or bot.display_name
        local key = tostring(s.bot_key or bot.key) .. "\0" .. tostring(s.guild_id or "0")
        if not seen[key] then
          seen[key] = true
          flattened[#flattened + 1] = s
        end
      end
    end
    data.sessions = flattened

    -- Discord identity/name enrichment (dashboard.py's enrich_discord=True
    -- path). Best-effort per bot: a slow/unreachable Discord API for one
    -- bot must not break the rest of the dashboard, so every lookup is
    -- pcall-wrapped and errors surface as bot.discord.error /
    -- bot.discord.name_resolution_error instead of failing the request.
    for _, bot in ipairs(bots) do
      local token = cfg.bot_tokens[bot.key]
      bot.name = bot.name or bot.display_name
      if not enrich_discord then
        bot.discord = { token_configured = token ~= nil and token ~= "" }
      elseif token and token ~= "" then
        bot.discord = { token_configured = true }
        local ok, identity_or_err = pcall(discord_service.fetch_identity, token)
        if ok and identity_or_err then
          bot.discord.identity = identity_or_err
        else
          bot.discord.error = tostring(identity_or_err or "Discord identity lookup failed.")
        end

        local placements = {}
        for _, s in ipairs(bot.sessions or {}) do
          if s.channel_id then placements[#placements + 1] = { guild_id = s.guild_id, channel_id = s.channel_id } end
          if s.home_channel_id then placements[#placements + 1] = { guild_id = s.guild_id, channel_id = s.home_channel_id } end
        end
        if #placements > 0 then
          local ok2, name_map = pcall(discord_service.resolve_guild_channel_names, token, placements)
          if ok2 and name_map then
            for _, s in ipairs(bot.sessions or {}) do
              local gid = tostring(s.guild_id)
              local channel_entry = s.channel_id and name_map[gid .. "\0" .. tostring(s.channel_id)]
              if channel_entry then
                s.guild_name = channel_entry.guild_name
                s.channel_name = channel_entry.channel_name
              end
              local home_entry = s.home_channel_id and name_map[gid .. "\0" .. tostring(s.home_channel_id)]
              if home_entry then
                s.guild_name = s.guild_name or home_entry.guild_name
                s.home_channel_name = home_entry.channel_name
              end
            end
          else
            bot.discord.name_resolution_error = tostring(name_map or "Discord channel lookup failed.")
          end
        end
      else
        bot.discord = { token_configured = false }
      end
    end

    return data
  end

  -- Server-rendered pages (pages_fleet.lua's Dashboard/Controls) use this
  -- instead of the raw fleet snapshot, so the first paint is scoped exactly
  -- like the live "dashboard" push that replaces it: a guild account only
  -- ever sees the bots/sessions in its own guild. Rendering the unscoped
  -- snapshot showed every guild's bots, tracks and guild names until the
  -- first push arrived (and left cards the push never touches, like Aria's,
  -- on screen for good).
  M.scoped_dashboard_data = function(a) return build_dashboard_payload(a, false) end

  -- DoS backstop: every require_auth-only (non-admin) endpoint below gets a
  -- modest per-account rate limit, same ratelimit.lua mechanism the auth
  -- routes above use, keyed on the account's username alone (any authorized
  -- guild account, so no IP scoping needed) rather than being left
  -- completely unbounded like upload/avatar/2fa routes already are limited
  -- but these plain reads previously weren't. Admin-gated routes are left
  -- alone (lower risk, already gated) as are routes that already carry
  -- their own rate limit.
  httpd.route("GET", "/api/dashboard", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local ok, data = pcall(build_dashboard_payload, a, true)
    if not ok then return 503, { detail = "Dashboard unavailable: " .. tostring(data) } end
    return 200, data
  end)

  -- Factored out of GET /api/bots below so the "invites" live-push builder
  -- (SNAPSHOT_BUILDERS, defined further down) can call the identical logic
  -- directly instead of duplicating it -- same pattern as
  -- build_dashboard_payload above. Must be defined before SNAPSHOT_BUILDERS
  -- (Lua locals aren't hoisted), which is why this moved up here rather
  -- than staying next to the route handler that also uses it.
  local function build_bots_payload(a)
    local visible_bots = {}
    for _, bot in ipairs(music_bots) do visible_bots[#visible_bots + 1] = bot end
    visible_bots[#visible_bots + 1] = cfg.aria_bot

    local bots_out = {}
    for _, bot in ipairs(visible_bots) do
      bots_out[#bots_out + 1] = {
        key = bot.key, display_name = bot.display_name, name = bot.display_name,
        kind = bot.kind, schema = bot.db_schema,
        token_configured = (cfg.bot_tokens[bot.key] or "") ~= "",
      }
    end

    -- Real invite cards: OAuth invite URL (needs only the bot's client id,
    -- derived locally from the token — no Discord call) plus a best-effort
    -- live identity/avatar lookup. A slow/unreachable Discord API for one
    -- bot degrades that bot's card (identity_error set) rather than failing
    -- the whole roster response.
    local scoped = scoped_guild_id(a)
    local public_scoped = (scoped ~= OWNER_SCOPE_SENTINEL) and scoped or nil

    -- Mirrors _visible_music_bots_for_auth(): only worth computing when this
    -- caller IS guild-scoped (an unscoped/admin caller's Python equivalent
    -- short-circuits `scoped_guild_id and ...` straight to false too).
    local visible_keys = {}
    if public_scoped then
      local ok, dash_data = pcall(dashboard.get_dashboard_data, music_bots)
      if ok then
        for _, bot in ipairs(dash_data.bots or {}) do
          for _, gid in ipairs(bot.known_guild_ids or {}) do
            if gid == public_scoped then visible_keys[bot.key] = true break end
          end
        end
      end
    end

    local invite_bots = {}
    for _, bot in ipairs(visible_bots) do
      local token = cfg.bot_tokens[bot.key] or ""
      local client_id = token ~= "" and discord_service.client_id_from_token(token) or nil
      local identity, identity_error = {}, nil
      if token ~= "" then
        local ok, result = pcall(discord_service.fetch_identity, token)
        if ok and result then
          identity = result
          client_id = identity.id or client_id
        else
          identity_error = tostring(result or "Discord identity lookup failed.")
        end
      end
      local perms = (bot.kind == "orchestrator") and discord_service.ARIA_BOT_PERMISSIONS or discord_service.MUSIC_BOT_PERMISSIONS
      local permission_integer = discord_service.permission_value(perms)
      invite_bots[#invite_bots + 1] = {
        key = bot.key, display_name = bot.display_name, name = bot.display_name,
        kind = bot.kind, schema = bot.db_schema, token_configured = token ~= "",
        client_id = client_id,
        invite_url = client_id and discord_service.invite_url_for_bot(client_id, permission_integer, public_scoped) or nil,
        permission_integer = string.format("%.0f", permission_integer),
        permissions = perms,
        capability_summary = discord_service.BOT_CAPABILITY_SUMMARIES[bot.kind] or "Discord bot with slash commands and server tools.",
        accent = cfg.bot_accents and cfg.bot_accents[bot.key] or "#89b4fa",
        connected_to_session_guild = (public_scoped ~= nil) and (visible_keys[bot.key] == true),
        icon_url = identity.avatar_url,
        identity_name = identity.global_name or identity.username,
        identity_error = identity_error,
      }
    end

    return { bots = bots_out, invite_bots = invite_bots, scoped_guild_id = public_scoped }
  end

  -- Factored out of GET /api/exports below for the same reason as
  -- build_bots_payload above -- the "exports" live-push builder needs it.
  local EXPORTS_DIR = (settings.scheduled_exports_dir and settings.scheduled_exports_dir ~= "")
    and settings.scheduled_exports_dir or "./.runtime/exports"

  local function build_exports_payload()
    local snapshots = {}
    local p = io.popen and io.popen('ls -1 "' .. EXPORTS_DIR:gsub('"', '') .. '" 2>/dev/null')
    if p then
      for date_dir in p:lines() do
        if date_dir:match("^%d%d%d%d%-%d%d%-%d%d$") then
          local files = {}
          local p2 = io.popen('ls -1 "' .. EXPORTS_DIR:gsub('"', '') .. "/" .. date_dir:gsub('"', '') .. '" 2>/dev/null')
          if p2 then
            for fname in p2:lines() do
              if fname:match("^[%w_.%-]+%.csv$") then
                local f = io.open(EXPORTS_DIR .. "/" .. date_dir .. "/" .. fname, "rb")
                local size = 0
                if f then size = f:seek("end") or 0; f:close() end
                files[#files + 1] = { name = fname, size_bytes = size }
              end
            end
            p2:close()
          end
          if #files > 0 then snapshots[#snapshots + 1] = { date = date_dir, files = files } end
        end
      end
      p:close()
    end
    table.sort(snapshots, function(x, y) return x.date > y.date end)
    return { ok = true, enabled = settings.scheduled_exports_enabled or false, snapshots = snapshots }
  end

  -- Swarm-wide "top tracks across all N bots this N days" leaderboard, admin
  -- only (fans out to every bot's own Postgres database, so it's a heavier
  -- query than anything guild-scoped -- see dashboard.lua's
  -- get_swarm_leaderboard for why this needed the Postgres migration to be
  -- possible at all). ?days=7&limit=20 by default.
  httpd.route("GET", "/api/swarm-leaderboard", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    if not is_admin_auth(a) then return 403, { detail = "Swarm-wide leaderboard is admin-only (it queries every bot's database)." } end
    local ok, data = pcall(dashboard.get_swarm_leaderboard, music_bots, { days = req.query.days, limit = req.query.limit })
    if not ok then return 503, { detail = "Swarm leaderboard unavailable: " .. tostring(data) } end
    return 200, data
  end)

  -- ------------------------------------------------------------------- ws
  -- Port of app/routers/websocket.py's /ws live dashboard feed. Auth is
  -- bearer-token-only in this rewrite (the session-cookie branch in the
  -- Python original is dead code here — see this file's own header comment:
  -- the deployed SPA always fetches with credentials:"omit" and carries its
  -- own token, and frontend/src/hooks/useDashboardStream.js confirms the
  -- browser really does send `{"type":"auth","token":...}` as the first WS
  -- frame rather than relying on a cookie). Mirrors the Python original's
  -- shape exactly: dashboard_snapshot / dashboard_snapshot_error / ping
  -- messages out, pong (ignored) in, one shared ~2s broadcast loop across
  -- every connected client cached per guild/admin scope so N clients in the
  -- same scope share one build_dashboard_payload() call per tick.
  local active_ws_connections = {} -- list of {conn=, auth=, watches={}, last_digests={}, next_due={}}
  local broadcast_loop_running = false

  local function dashboard_scope_cache_key(a)
    local scoped = scoped_guild_id(a)
    if scoped then return "guild:" .. scoped end
    if is_admin_auth(a) then return "admin" end
    return "session:" .. tostring((a and a.username) or "default")
  end

  local function remove_ws_connection(entry)
    for i, c in ipairs(active_ws_connections) do
      if c == entry then table.remove(active_ws_connections, i) return end
    end
  end

  local function iso_now()
    return os.date("!%Y-%m-%dT%H:%M:%SZ")
  end

  -- ---------------------------------------------------------------------
  -- Generalized live-push registry: was a single hardcoded dashboard_snapshot
  -- broadcast, extended to every resource the panel used to poll for on a
  -- setInterval (Controls' control-state/queues, the notifications bell,
  -- Friends, Messages threads/active-thread). Each entry:
  --   interval   -- minimum seconds between recomputes for this key (keeps
  --                 the original per-resource cadence -- e.g. queues stays
  --                 5s-ish, not upgraded to the 2s tick rate, to avoid
  --                 turning "no more client polling" into "much more
  --                 server/DB load instead").
  --   scope_key  -- (auth, params) -> cache-sharing key. Two connections
  --                 watching the identical thing (same guild's dashboard,
  --                 same bot+guild's control-state, ...) share ONE build per
  --                 tick instead of paying for it twice, same principle the
  --                 original dashboard-only version already used.
  --   build      -- (auth, params) -> ok, data_or_error
  -- params come from the client's {type:"watch", key, ...} message (e.g.
  -- bot_key/guild_id for control_state, account_id for thread_messages) --
  -- per-CONNECTION state, not scope-wide, since two operators can have two
  -- different bots selected in Controls at the same time.
  --
  -- BUGFIX: params starts life as the boolean `true` (a client's initial
  -- {type:"watch", key} with no params yet -- e.g. Controls registering its
  -- handler before the first bot/guild is known -- decodes to `entry.
  -- watches[key] = true`, not a table) and only becomes a real params table
  -- once resubscribe() sends one. `params and params.bot_key` blows up with
  -- "attempt to index a boolean value" the instant Lua evaluates the second
  -- operand against `true` -- confirmed live: this took the ENTIRE broadcast
  -- loop coroutine down, silently, for every connected client, not just the
  -- one that triggered it (uncaught inside copas.addthread, so the `while`
  -- loop died without resetting broadcast_loop_running, permanently
  -- blocking ensure_broadcast_loop() from ever starting a new one). Always
  -- go through this helper instead of `params and params.x`.
  local function watch_param(params, field)
    return (type(params) == "table") and params[field] or nil
  end
  -- Fold the caller's admin/moderator tier into a builder's scope_key() so
  -- build_cache (shared across every connection watching the same key on a
  -- given tick) never lets one caller's auth outcome leak to another -- see
  -- the SNAPSHOT_BUILDERS admin-gated entries below for why this matters.
  local function admin_scope(a) return is_admin_auth(a) and "admin" or "denied" end
  local function admin_or_mod_scope(a)
    return (is_admin_auth(a) or is_moderator_auth(a)) and "admin_or_mod" or "denied"
  end
  -- Defined here, ahead of SNAPSHOT_BUILDERS, because the thread_messages
  -- live key below needs it. It used to be declared further down with the
  -- profile routes, so from inside the builder the name resolved to an
  -- undefined global and every live thread_messages push failed (clients
  -- silently fell back to REST).
  local function social_permissions(profile)
    local status = tostring((profile and profile.friend_status) or "none"):lower()
    local mode = tostring((profile and profile.profile_social_mode) or "open"):lower()
    if mode ~= "open" and mode ~= "friends" and mode ~= "quiet" then mode = "open" end
    local public_profile = profile and profile.public_profile or false
    local is_friend = status == "friends"

    local function can_access(action)
      if status == "self" then return true end
      if not public_profile then return is_friend and (action == "message" or action == "view_friends") end
      if mode == "quiet" then return false end
      if mode == "friends" then
        if action == "friend_request" then
          return not (status == "friends" or status == "pending_out" or status == "pending_in" or status == "self")
        end
        return is_friend
      end
      return true
    end

    return {
      can_follow = can_access("follow") and status ~= "self" and status ~= "pending_out",
      can_friend = can_access("friend_request"),
      can_message = can_access("message") and status ~= "self",
      can_view_friends = can_access("view_friends"),
    }
  end

  local THREAD_PERMISSION_TTL_SECONDS = 20
  local thread_permission_cache = {} -- "actor:target" -> {expires=, allowed=}
  -- Pruned from the broadcast loop so conversations that were opened once
  -- don't leave entries behind for the life of the process.
  local function prune_thread_permission_cache(now)
    for k, v in pairs(thread_permission_cache) do
      if v.expires <= now then thread_permission_cache[k] = nil end
    end
  end

  -- ---------------------------------------------------------------------
  local SNAPSHOT_BUILDERS = {
    dashboard = {
      interval = 2,
      scope_key = function(a) return dashboard_scope_cache_key(a) end,
      build = function(a)
        local ok, snapshot = pcall(build_dashboard_payload, a, false)
        if not ok then return false, tostring(snapshot) end
        return true, snapshot
      end,
    },
    notifications = {
      interval = 5,
      scope_key = function(a)
        local actor_id = select(1, account_id_for_auth(a))
        return "account:" .. tostring(actor_id or "none")
      end,
      build = function(a)
        local actor_id, aerr = account_id_for_auth(a)
        if not actor_id then return false, aerr end
        return true, { unread_count = social.unread_notification_count(actor_id), notifications = social.list_notifications(actor_id, 30) }
      end,
    },
    friends = {
      interval = 5,
      scope_key = function(a)
        local actor_id = select(1, account_id_for_auth(a))
        return "account:" .. tostring(actor_id or "none")
      end,
      -- Bundles /api/me/friends with /api/friends/requests (incoming +
      -- outgoing) in one push -- the Friends page's loadFriends() always
      -- fetches all three together, so splitting them into separate watch
      -- keys would just mean two round trips of latency instead of one.
      build = function(a)
        local actor_id, aerr = account_id_for_auth(a)
        if not actor_id then return false, aerr end
        return true, {
          friends = social.list_account_friends(actor_id),
          incoming = social.list_account_friend_requests(actor_id, "incoming"),
          outgoing = social.list_account_friend_requests(actor_id, "outgoing"),
        }
      end,
    },
    -- The signed-in account's own profile summary + panel preferences
    -- (background, accent, ...), so another device -- the tvOS dashboard --
    -- picks up an Appearance or profile change made on the web within
    -- seconds instead of only at its next launch. Same data as GET
    -- /api/users/me + /api/users/preferences, trimmed to display fields.
    account = {
      interval = 10,
      scope_key = function(a)
        return "acct:" .. tostring(a and a.username or "") .. ":" .. tostring(account_guild_id(a))
      end,
      build = function(a)
        local username = tostring((a and a.username) or settings.admin_username or "")
        local gid = account_guild_id(a)
        local defaults = profiles.default_panel_preferences()
        local summary = { username = username, display_name = username, guild_id = gid }
        local prefs = defaults
        if gid and username ~= "" then
          local ok, profile = pcall(accounts.get_account_profile, username, gid)
          if ok and profile then
            for _, field in ipairs({ "display_name", "avatar_url", "server_name", "server_icon_url",
                                     "profile_headline", "favorite_bot", "theme_accent" }) do
              if profile[field] ~= nil then summary[field] = profile[field] end
            end
            local stored = (type(profile.panel_preferences) == "table") and profile.panel_preferences or nil
            local okp, cleaned = pcall(profiles.clean_panel_preferences, stored, defaults)
            if okp then prefs = cleaned end
          end
        end
        -- Internal flag, not a display preference.
        prefs.admin_mode_enabled = nil
        summary.role = a and a.role or nil
        summary.admin_mode = is_admin_auth(a)
        return true, { profile = summary, preferences = with_derived_accent(prefs) }
      end,
    },
    -- Community badge counts (sidebar/mobile nav/section tabs on every
    -- page, and the iOS Community tab). Same per-account scope as threads.
    community_counts = {
      interval = 10,
      scope_key = function(a)
        local actor_id = select(1, account_id_for_auth(a))
        return "account:" .. tostring(actor_id or "none")
      end,
      build = function(a)
        local actor_id, aerr = account_id_for_auth(a)
        if not actor_id then return false, aerr end
        local ok, counts = pcall(social.community_counts, actor_id)
        if not ok then return false, tostring(counts) end
        return true, counts
      end,
    },
    threads = {
      interval = 5,
      scope_key = function(a)
        local actor_id = select(1, account_id_for_auth(a))
        return "account:" .. tostring(actor_id or "none")
      end,
      build = function(a)
        local actor_id, aerr = account_id_for_auth(a)
        if not actor_id then return false, aerr end
        return true, { threads = social.list_account_message_threads(actor_id) }
      end,
    },
    thread_messages = {
      interval = 2,
      scope_key = function(a, params)
        local actor_id = select(1, account_id_for_auth(a))
        return "thread:" .. tostring(actor_id or "none") .. ":" .. tostring(watch_param(params, "account_id"))
      end,
      build = function(a, params)
        local actor_id, aerr = account_id_for_auth(a)
        if not actor_id then return false, aerr end
        local target_id = watch_param(params, "account_id")
        if not target_id then return false, "account_id is required" end
        -- The permission check (profile + social snapshot, ~3 queries) only
        -- changes when someone edits their profile or friendship, but this
        -- key rebuilds every 2s per open conversation -- remember the
        -- outcome briefly instead of redoing it every tick.
        local perm_key = tostring(actor_id) .. ":" .. tostring(target_id)
        local now = socket.gettime()
        local cached_perm = thread_permission_cache[perm_key]
        local allowed
        if cached_perm and cached_perm.expires > now then
          allowed = cached_perm.allowed
        else
          local target = accounts.get_account_by_id(target_id)
          if target then
            local snap = social.get_account_social_snapshot(target.id, actor_id)
            for k, v in pairs(snap) do target[k] = v end
          end
          allowed = target ~= nil and social_permissions(target).can_message
          thread_permission_cache[perm_key] = { expires = now + THREAD_PERMISSION_TTL_SECONDS, allowed = allowed }
        end
        if not allowed then
          return false, "This profile is not accepting direct messages."
        end
        local ok, messages = pcall(social.list_account_messages, actor_id, target_id, 80)
        if not ok then return false, tostring(messages) end
        return true, { messages = messages }
      end,
    },
    control_state = {
      interval = 2,
      -- Fold the caller's guild-scope identity into the cache key -- see the
      -- admin/moderator-gated builders' scope_key()s below (and
      -- dashboard_scope_cache_key above) for why: build_cache is shared
      -- across every connection watching the same key THIS tick, so without
      -- this, one operator's access check (require_bot_guild_access inside
      -- build()) could get run once and its cached result reused for a
      -- DIFFERENT connection watching the identical bot_key/guild_id whose
      -- own auth would not actually pass that check.
      scope_key = function(a, params) return "cs:" .. tostring(scoped_guild_id(a)) .. ":" .. tostring(watch_param(params, "bot_key")) .. ":" .. tostring(watch_param(params, "guild_id")) end,
      build = function(a, params)
        local bot_key, gid = watch_param(params, "bot_key"), watch_param(params, "guild_id")
        if not bot_key or not gid or gid == "" then return false, "bot_key/guild_id required" end
        local gerr_status, gerr_body = require_bot_guild_access(nil, a, bot_key, gid)
        if gerr_status then return false, (gerr_body and gerr_body.detail) or "forbidden" end
        local bot = bot_index[bot_key]
        if not bot or bot.kind ~= "music" then return false, "Unknown music bot key" end
        local ok, state = pcall(dashboard.get_bot_control_state, bot, gid)
        if not ok then return false, tostring(state) end
        local ok2, enriched = pcall(enrich_control_state_with_discord, state)
        return true, ok2 and enriched or state
      end,
    },
    queues = {
      interval = 5,
      -- Same fix as control_state above: fold the caller's guild-scope
      -- identity in so require_guild_scope's per-connection outcome inside
      -- build() can't be cached under a key that's identical for every
      -- caller regardless of what guild(s) they're actually allowed to see.
      scope_key = function(a, params) return "q:" .. tostring(scoped_guild_id(a)) .. ":" .. tostring(watch_param(params, "guild_id")) .. ":" .. tostring(watch_param(params, "bot_key")) end,
      build = function(a, params)
        local gid, bot_key = watch_param(params, "guild_id"), watch_param(params, "bot_key")
        if not gid or gid == "" then return false, "guild_id is required" end
        local gerr_status, gerr_body = require_guild_scope(nil, a, gid)
        if gerr_status then return false, (gerr_body and gerr_body.detail) or "forbidden" end
        local ok, result = pcall(queues.list_saved_queues, gid, bot_key)
        if not ok then return false, tostring(result) end
        return true, { queues = result }
      end,
    },
    -- Discord API calls (one fetch_identity per bot) are the actual work
    -- here, not a DB query -- a much longer interval than the other keys to
    -- avoid turning "no more client polling" into "hammering Discord's API
    -- on a fast broadcast tick for every connected operator". Bot identity/
    -- avatar essentially never changes minute to minute anyway.
    invites = {
      interval = 60,
      scope_key = function(a) return dashboard_scope_cache_key(a) end,
      build = function(a) return true, build_bots_payload(a) end,
    },
    -- Every admin/moderator-gated key below double-checks the CONNECTION's
    -- own auth inside build() (not just at watch-time) -- entry.auth is
    -- fixed for the socket's lifetime, but this mirrors each key's REST
    -- equivalent exactly rather than trusting the client to only ever send
    -- a "watch" for keys its UI happens to expose. scope_key() also folds
    -- the caller's admin/moderator tier into the cache key: build_cache is
    -- shared across every connection watching the same key THIS TICK, so if
    -- it only ever returned a constant like "admin", the first caller's
    -- pass/fail outcome (e.g. a rando's "denied") would get reused for a
    -- real admin's connection processing the same key right after.
    lumisound_admin = {
      interval = 15,
      scope_key = function(a) return admin_or_mod_scope(a) end,
      build = function(a)
        if not (is_admin_auth(a) or is_moderator_auth(a)) then return false, "Admin or moderator access required" end
        local ok, data = pcall(lumisound.get_lumisound_admin_data, settings, 50)
        if not ok then return false, tostring(data) end
        return true, { data = data }
      end,
    },
    -- stability/metrics/events power BOTH Diagnostics and Intel (they show
    -- the identical fleet-wide data, just arranged differently) -- one
    -- cache-shared build per tick regardless of which page(s) are watching.
    -- Unlike the others below, /api/stability itself only ever required
    -- plain auth (any signed-in operator), so no extra gating here.
    stability = {
      interval = 5,
      scope_key = function() return "global" end,
      build = function()
        local ok, data = pcall(metrics.get_stability_snapshot, music_bots)
        if not ok then return false, tostring(data) end
        return true, data
      end,
    },
    metrics_snapshot = {
      interval = 5,
      scope_key = function(a) return admin_scope(a) end,
      build = function(a)
        if not is_admin_auth(a) then return false, "Admin access required" end
        local ok, data = pcall(metrics.get_metrics_snapshot, music_bots)
        if not ok then return false, tostring(data) end
        return true, data
      end,
    },
    events = {
      interval = 5,
      scope_key = function(a) return admin_scope(a) end,
      build = function(a)
        if not is_admin_auth(a) then return false, "Admin access required" end
        -- Mirrors GET /api/events exactly (sort by timestamp, dedup by
        -- content key, trim to the requested limit) -- the Intel page reads
        -- events.events the same way whether it came via REST or here.
        local bounded_limit = 80
        local ok1, bot_error_events = pcall(metrics.get_recent_bot_error_events, music_bots, bounded_limit)
        if not ok1 then bot_error_events = {} end
        local ok2, aria_medic_events = pcall(metrics.get_recent_aria_medic_events, math.max(5, math.floor(bounded_limit / 2)))
        if not ok2 then aria_medic_events = {} end
        local combined = {}
        for _, e in ipairs(bot_error_events) do combined[#combined + 1] = e end
        for _, e in ipairs(aria_medic_events) do combined[#combined + 1] = e end
        table.sort(combined, function(x, y) return tostring(x.timestamp) < tostring(y.timestamp) end)
        local seen, deduped = {}, {}
        for _, event in ipairs(combined) do
          local key = table.concat({
            tostring(event.timestamp or ""), tostring(event.source or ""),
            tostring(event.title or ""), tostring(event.description or ""), tostring(event.type or "feed_event"),
          }, "\0")
          if not seen[key] then
            seen[key] = true
            deduped[#deduped + 1] = event
          end
        end
        local out = {}
        local start_idx = math.max(1, #deduped - bounded_limit + 1)
        for i = start_idx, #deduped do out[#out + 1] = deduped[i] end
        return true, { events = out }
      end,
    },
    -- Intel's two 24h trend charts (queued_tracks, active_bots) -- bundled
    -- into one key since the page always renders both together.
    trends = {
      interval = 60,
      scope_key = function(a) return admin_scope(a) end,
      build = function(a)
        if not is_admin_auth(a) then return false, "Admin access required" end
        local out = {}
        for _, metric in ipairs({ "queued_tracks", "active_bots" }) do
          local ok1, points = pcall(metrics.get_metrics_history, metric, 24)
          local ok2, anomalies = pcall(metrics.get_metric_anomalies, metric, 24)
          out[metric] = { points = ok1 and points or {}, anomalies = ok2 and anomalies or {} }
        end
        return true, out
      end,
    },
    audit_log = {
      interval = 15,
      scope_key = function(a) return admin_or_mod_scope(a) end,
      build = function(a)
        if not (is_admin_auth(a) or is_moderator_auth(a)) then return false, "Admin or moderator access required" end
        local ok, data = pcall(audit.list_audit_log, 200, 0, nil)
        if not ok then return false, tostring(data) end
        return true, { data = data }
      end,
    },
    alert_rules = {
      interval = 30,
      scope_key = function(a) return admin_or_mod_scope(a) end,
      build = function(a)
        if not (is_admin_auth(a) or is_moderator_auth(a)) then return false, "Admin or moderator access required" end
        local ok, rules = pcall(alerts.list_alert_rules)
        if not ok then return false, tostring(rules) end
        return true, { rules = rules }
      end,
    },
    exports = {
      interval = 60,
      scope_key = function(a) return admin_scope(a) end,
      build = function(a)
        if not is_admin_auth(a) then return false, "Admin access required" end
        local ok, data = pcall(build_exports_payload)
        if not ok then return false, tostring(data) end
        return true, data
      end,
    },
  }

  local function ensure_broadcast_loop()
    if broadcast_loop_running then return end
    broadcast_loop_running = true
    copas.addthread(function()
      while #active_ws_connections > 0 do
        local now = os.time()
        local build_cache = {} -- "key:scope" -> serialized snapshot string
        local dead = {}
        for _, entry in ipairs(active_ws_connections) do
          -- Snapshot the watched keys before iterating rather than looping
          -- `pairs(entry.watches)` directly: entry.conn:send() below can
          -- yield (copas wraps the socket cooperatively), and if this same
          -- connection's OWN receive loop (a separate copas thread) handles
          -- an inbound watch/unwatch during that yield, it mutates
          -- entry.watches while this pairs() iteration is still open on it
          -- -- undefined in Lua, and unlike the per-key pcall above, "table
          -- changed during iteration" is a fault in `next()` itself, not
          -- something a pcall around the loop BODY catches for the NEXT
          -- `next()` call the `for` statement makes internally.
          local keys = {}
          for key in pairs(entry.watches) do keys[#keys + 1] = key end
          for _, key in ipairs(keys) do
            local params = entry.watches[key]
            local builder = params ~= nil and SNAPSHOT_BUILDERS[key]
            if builder then
              local due_at = entry.next_due[key] or 0
              if now >= due_at then
                entry.next_due[key] = now + builder.interval
                -- BUGFIX: the whole per-key body (scope_key(), cjson.encode,
                -- conn:send -- not just builder.build() below) is now inside
                -- one pcall. A single uncaught error anywhere in here used
                -- to propagate straight out of this copas.addthread and kill
                -- the ENTIRE broadcast loop coroutine permanently (silently
                -- -- the `while` loop just stopped running, for every
                -- connected client, not only the one that triggered it, and
                -- broadcast_loop_running stayed true forever so
                -- ensure_broadcast_loop() never restarted it). Confirmed
                -- live: a boolean `params` (see watch_param()'s own comment)
                -- reaching scope_key()'s unguarded `params.bot_key` did
                -- exactly that. One bad key for one connection should skip
                -- that key this tick, nothing more.
                local pok, perr = pcall(function()
                  local cache_key = key .. ":" .. builder.scope_key(entry.auth, params)
                  local cached = build_cache[cache_key]
                  local serialized, digest
                  if cached then
                    serialized, digest = cached[1], cached[2]
                  else
                    -- builder.build() returns (ok, data_or_error) as normal
                    -- multi-return, not by throwing -- this inner pcall is
                    -- only a backstop against an unexpected runtime error
                    -- inside one builder. On a pcall failure, r1 IS the
                    -- error message (pcall's own (false, errmsg) shape), not
                    -- the builder's ok flag.
                    local bok, r1, r2 = pcall(builder.build, entry.auth, params)
                    local ok, data
                    if bok then ok, data = r1, r2 else ok, data = false, r1 end
                    local payload, data_json
                    if ok then
                      -- The change digest covers the data only, never the
                      -- per-build generated_at stamp: with the timestamp in
                      -- it, every key looked "changed" on every interval, so
                      -- unchanged notifications/friends/threads/badges were
                      -- re-sent (and re-rendered client-side) every few
                      -- seconds forever.
                      -- Encoded once and spliced into the frame below, so
                      -- the digest doesn't cost a second full encode of
                      -- large payloads like the fleet dashboard.
                      data_json = cjson.encode(data)
                      if not data_json then
                        payload = { type = "snapshot_error", key = key, error = "snapshot could not be encoded", generated_at = iso_now() }
                      end
                      digest = "s" .. (data_json or "!")
                    else
                      digest = "e" .. tostring(data)
                      payload = { type = "snapshot_error", key = key, error = tostring(data), generated_at = iso_now() }
                      -- In-depth telemetry (per operator request): a
                      -- snapshot builder failing is invisible outside this
                      -- print() otherwise -- the client just silently gets
                      -- a snapshot_error frame. Best-effort/pcall'd like
                      -- every other telemetry call site; this already runs
                      -- inside the outer pcall below too, so a throw here
                      -- can't take the broadcast loop down either way.
                      if entry.last_digests[key] ~= digest then
                        pcall(telemetry.record, "websocket", "snapshot_error", {
                          username = entry.auth and entry.auth.username, text_value = tostring(data),
                          metadata = { key = key },
                        })
                      end
                    end
                    if data_json then
                      serialized = '{"type":"snapshot","key":' .. cjson.encode(key)
                        .. ',"data":' .. data_json
                        .. ',"generated_at":' .. cjson.encode(iso_now()) .. '}'
                    else
                      serialized = cjson.encode(payload)
                    end
                    build_cache[cache_key] = { serialized, digest }
                  end
                  if entry.last_digests[key] ~= digest then
                    local sent = entry.conn:send(serialized)
                    if sent then
                      entry.last_digests[key] = digest
                    else
                      dead[#dead + 1] = entry
                    end
                  end
                end)
                if not pok then
                  print("[swarmpanel-lua] broadcast loop: key '" .. tostring(key) .. "' failed: " .. tostring(perr))
                  pcall(telemetry.record, "websocket", "broadcast_error", {
                    username = entry.auth and entry.auth.username, text_value = tostring(perr),
                    metadata = { key = key },
                  })
                end
              end
            end
          end
        end
        for _, entry in ipairs(dead) do remove_ws_connection(entry) end
        prune_thread_permission_cache(socket.gettime())
        copas.sleep(1)
      end
      broadcast_loop_running = false
    end)
  end

  -- Same algorithm as the installed lua-websockets rock's websocket.sync
  -- receive() (frame reassembly across CONTINUATION frames, CLOSE
  -- echo/cleanup), EXCEPT a socket-read timeout is returned as a distinct
  -- ("timeout") sentinel instead of being treated as a fatal disconnect —
  -- websocket.sync.receive() closes the connection on ANY sock_receive
  -- error, which would turn this server's idle keepalive-ping timer into an
  -- unwanted forced disconnect every WS_RECEIVE_TIMEOUT_SECONDS. Real
  -- disconnects/protocol errors still close exactly like the library
  -- function does. conn:send()/conn:close() (unmodified library functions,
  -- from websocket.sync.extend() in httpd.lua) are unaffected by this.
  local WS_PRESENCE_TOUCH_SECONDS = 60
  local ws_frame = require("websocket.frame")
  local function ws_receive_with_timeout(conn)
    if conn.state ~= "OPEN" and not conn.is_closing then
      return nil, nil, false, 1006, "wrong state"
    end
    local first_opcode, frames
    local bytes, encoded = 3, ""
    local function clean(was_clean, code, reason)
      conn.state = "CLOSED"
      conn:sock_close()
      return nil, nil, was_clean, code, reason or "closed"
    end
    while true do
      local chunk, err = conn:sock_receive(bytes)
      if err then
        if err == "timeout" then return nil, "timeout" end
        return clean(false, 1006, err)
      end
      encoded = encoded .. chunk
      local decoded, fin, opcode = ws_frame.decode(encoded)
      if decoded then
        if opcode == ws_frame.CLOSE then
          if not conn.is_closing then
            local code, reason = ws_frame.decode_close(decoded)
            local close_msg = ws_frame.encode_close(code)
            local close_encoded = ws_frame.encode(close_msg, ws_frame.CLOSE, not conn.is_server)
            local n = conn:sock_send(close_encoded)
            return clean(n == #close_encoded, code, reason)
          else
            return decoded, opcode
          end
        end
        if not first_opcode then first_opcode = opcode end
        if not fin then
          frames = frames or {}
          bytes, encoded = 3, ""
          table.insert(frames, decoded)
        elseif not frames then
          return decoded, first_opcode
        else
          table.insert(frames, decoded)
          return table.concat(frames), first_opcode
        end
      else
        bytes = fin -- frame.decode's 2nd return doubles as "more bytes needed" when decode fails
      end
    end
  end

  httpd.ws_route("/ws", function(conn, req, sock)
    copas.settimeout(sock, 10)
    local raw = ws_receive_with_timeout(conn)
    local a = nil
    if raw then
      local ok, msg = pcall(cjson.decode, raw)
      if ok and type(msg) == "table" and msg.type == "auth" then
        local reason
        a, reason = auth.verify_api_token(settings.session_secret, tostring(msg.token or ""))
        if not a then
          record_token_rejected({ path = req.path, client_ip = req.headers and (req.headers["x-forwarded-for"] or ""):match("^[^,]*") }, reason, "websocket")
        end
      end
    end
    if not a then
      pcall(function() copas.settimeout(sock, 3); conn:close(4401) end)
      return
    end

    -- dashboard is always-on (every page renders the topbar/shell that used
    -- to rely on it, and it's cheap/cached per-scope) -- everything else is
    -- opt-in per page via {type:"watch", key, ...params} so a Diagnostics
    -- tab doesn't pay for control-state builds nobody's looking at, and a
    -- Controls tab's bot_key/guild_id selection (per-CONNECTION, not
    -- scope-wide -- two operators can have two different bots picked at
    -- once) has somewhere to live.
    local entry = { conn = conn, auth = a, watches = { dashboard = true }, last_digests = {}, next_due = {} }
    active_ws_connections[#active_ws_connections + 1] = entry
    ensure_broadcast_loop()

    -- Presence: an operator sitting on a page with the live socket open
    -- makes no HTTP requests at all (everything arrives by push), so
    -- last_seen_at -- only refreshed by require_auth -- went stale and they
    -- showed as "Inactive" in the directory/profiles after 3 minutes while
    -- actively watching the panel. Any inbound frame (pong, watch, or the
    -- client's own keepalive) now counts as activity, throttled per socket
    -- so it costs at most one UPDATE a minute.
    local last_presence_touch = socket.gettime()
    local function note_activity()
      local now = socket.gettime()
      if now - last_presence_touch >= WS_PRESENCE_TOUCH_SECONDS then
        last_presence_touch = now
        touch_presence(a)
      end
    end

    copas.settimeout(sock, 45)
    local keep_going = true
    while keep_going do
      local message, info = ws_receive_with_timeout(conn)
      if message ~= nil then note_activity() end
      if message == nil and info == "timeout" then
        local sent = conn:send(cjson.encode({ type = "ping", timestamp = iso_now() }))
        if not sent then keep_going = false end
      elseif message == nil then
        keep_going = false -- real disconnect/close/protocol error; conn already cleaned up
      elseif #message > 512 then
        pcall(function() copas.settimeout(sock, 3); conn:close(4409) end)
        keep_going = false
      else
        local ok, msg = pcall(cjson.decode, message)
        if ok and type(msg) == "table" then
          if msg.type == "watch" and type(msg.key) == "string" and SNAPSHOT_BUILDERS[msg.key] then
            entry.watches[msg.key] = (type(msg.params) == "table") and msg.params or true
            entry.next_due[msg.key] = nil -- force an immediate rebuild+send on the next tick
            entry.last_digests[msg.key] = nil -- params may have changed (e.g. new bot_key) -- don't skip-as-duplicate against the old params' last payload
          elseif msg.type == "unwatch" and type(msg.key) == "string" then
            entry.watches[msg.key] = nil
            entry.last_digests[msg.key] = nil
            entry.next_due[msg.key] = nil
          end
          -- else: pong or unrecognized -- no action needed, receiving it
          -- already reset the idle window for the next loop iteration.
        end
      end
    end
    remove_ws_connection(entry)
    pcall(function() conn:close() end)
  end)

  -- ------------------------------------------------------------------ bots
  httpd.route("GET", "/api/bots", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    return 200, build_bots_payload(a)
  end)

  -- Port of app/routers/bots.py's GET /api/bots/{bot_key}/inventory: the
  -- guild+channel tree the Controls page's guild/voice/text-channel pickers
  -- populate from. Was entirely missing before (see discord_service.lua's
  -- fetch_inventory doc comment) -- ControlsPage.jsx's fetch against this
  -- path 404'd against httpd's generic handler, which is what surfaced to
  -- users as "not found" errors for guilds/voice/text channels.
  httpd.route("GET", "/api/bots/:bot_key/inventory", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local bot = bot_index[req.params.bot_key]
    if not bot then return 404, { detail = "Unknown bot key" } end
    local scoped = scoped_guild_id(a)
    if scoped then
      local gerr_status, gerr_body = require_bot_guild_access(req, a, req.params.bot_key, scoped)
      if gerr_status then return gerr_status, gerr_body end
    end

    local token = cfg.bot_tokens[bot.key] or ""
    if token == "" then return 400, { detail = "Missing token env for " .. bot.display_name } end

    local guild_hints = {}
    if bot.kind == "music" then
      local ok, snapshot = pcall(dashboard.music_bot_snapshot, bot)
      if ok and snapshot and snapshot.known_guild_ids then guild_hints = snapshot.known_guild_ids end
    end
    local include_channels = req.query.include_channels ~= "false"
    local ok, inventory = pcall(discord_service.fetch_inventory, token, {
      include_channels = include_channels,
      guild_hints = scoped and { scoped } or guild_hints,
    })
    if not ok then return 503, { detail = "Discord inventory unavailable: " .. tostring(inventory) } end
    if scoped then
      local filtered = {}
      for _, guild in ipairs(inventory.guilds or {}) do
        if tostring(guild.id) == scoped then filtered[#filtered + 1] = guild end
      end
      inventory.guilds = filtered
    end
    inventory.bot = { key = bot.key, display_name = bot.display_name, kind = bot.kind }
    return 200, inventory
  end)

  -- Port of app/routers/bots.py's GET /api/bots/{bot_key}/control-state:
  -- single bot+guild control panel state, Discord-name-enriched. Also
  -- previously missing (only the multi-bot /control-matrix variant below was
  -- ported) -- ControlsPage.jsx calls this one directly when a single bot is
  -- selected, so it also 404'd.
  httpd.route("GET", "/api/bots/:bot_key/control-state", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local gid = req.query.guild_id
    if not gid or gid == "" then return 400, { detail = "guild_id is required" } end
    local gerr_status, gerr_body = require_bot_guild_access(req, a, req.params.bot_key, gid)
    if gerr_status then return gerr_status, gerr_body end
    local bot = bot_index[req.params.bot_key]
    if not bot or bot.kind ~= "music" then return 404, { detail = "Unknown music bot key" } end

    local ok, state = pcall(dashboard.get_bot_control_state, bot, gid)
    if not ok then return 503, { detail = "Control state unavailable: " .. tostring(state) } end
    local ok2, enriched = pcall(enrich_control_state_with_discord, state)
    return 200, ok2 and enriched or state
  end)

  httpd.route("POST", "/api/bots/control", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    local action = tostring(body.action or body.command or ""):upper()
    if not control.VALID_ACTIONS[action] then return 400, { detail = "Unsupported action: " .. action } end
    if action == "RESTART" and not is_admin_auth(a) then
      return 403, { detail = "Restart is admin-only because it affects the whole bot node" }
    end
    local bot_key = tostring(body.bot_key or "")
    local bot = bot_index[bot_key]
    if not bot or bot.kind ~= "music" then return 404, { detail = "Unknown bot key" } end

    local gid = action == "RESTART" and "0" or tostring(body.guild_id or "")
    if action ~= "RESTART" and gid == "" then return 400, { detail = "guild_id is required" } end

    -- This endpoint checked auth (any valid session) but never checked
    -- guild SCOPE -- a guild-bound account could PLAY/SKIP/STOP/etc any
    -- bot in any guild by just typing a different guild_id, not only its
    -- own registered one. Every other guild-scoped endpoint in this file
    -- (control-state, control-matrix, queues, ...) already calls
    -- require_bot_guild_access/require_guild_scope; this one was missed.
    if action ~= "RESTART" then
      local gerr_status, gerr_body = require_bot_guild_access(req, a, bot_key, gid)
      if gerr_status then return gerr_status, gerr_body end
    end

    -- Every control action (not just destructive account/admin actions) is
    -- now audit-logged, payload included -- previously nothing recorded
    -- what was actually sent (e.g. a PLAY's source_url), and swarm_
    -- direct_orders/overrides rows get deleted right after processing, so
    -- once a command succeeded there was no way to answer "what URL did I
    -- send to which bot" after the fact.
    local ok, result_or_err, maybe_err = pcall(control.control_bot, bot, gid, action, body.payload, a.username)
    if not ok then
      audit.record_audit_log(a.username, "swarm_bot_control", {
        target_type = "music_bot", target_id = bot_key,
        details = cjson.encode({ action = action, guild_id = gid, payload = body.payload, ok = false, error = tostring(result_or_err) }),
      })
      return 500, { detail = "Bot control failed: " .. tostring(result_or_err) }
    end
    local result, cerr = result_or_err, maybe_err
    audit.record_audit_log(a.username, "swarm_bot_control", {
      target_type = "music_bot", target_id = bot_key,
      details = cjson.encode({ action = action, guild_id = gid, payload = body.payload, ok = result ~= nil, error = (not result) and tostring(cerr) or nil }),
    })
    if not result then return 400, { detail = tostring(cerr) } end
    result.ok = true
    return 200, result
  end)

  -- Converts every home-channel-connected bot's channel in one guild
  -- between voice and stage, in a single click (see channel_convert.lua's
  -- own header for why this is safe/scoped the way it is -- playback
  -- itself is untouched, only the underlying Discord channel object's
  -- type). direction: "stage" or "voice".
  httpd.route("POST", "/api/guilds/:guild_id/convert-channels", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-write:%s"):format(tostring(a.username or "unknown"):lower()), 10, 60)
    if rl_status then return rl_status, rl_body end
    local gid = req.params.guild_id
    local gerr_status, gerr_body = require_guild_scope(req, a, gid)
    if gerr_status then return gerr_status, gerr_body end
    local direction = tostring((req.json or {}).direction or ""):lower()
    if direction ~= "stage" and direction ~= "voice" then
      return 400, { detail = "direction must be 'stage' or 'voice'" }
    end
    local ok, result = pcall(channel_convert.convert_guild_channels, cfg, gid, direction)
    audit.record_audit_log(a.username, "swarm_convert_channels", {
      target_type = "guild", target_id = tostring(gid),
      details = cjson.encode({ direction = direction, ok = ok, result = ok and result or tostring(result) }),
    })
    if not ok then return 500, { detail = "Channel conversion failed: " .. tostring(result) } end
    result.ok = true
    return 200, result
  end)

  -- ------------------------------------------------------- session/register
  -- Port of app/routers/session.py's JSON /api/session/register, including
  -- Discord-webhook verification enrollment (notify.lua + accounts.lua's
  -- update this pass added): if a verification_webhook_url/
  -- registration_proof_url is supplied, its Discord-webhook proof is
  -- verified against the guild being registered (notify.verify_guild_registration_proof,
  -- same public GET /webhooks/{id}/{token} lookup as the Python original —
  -- no bot token required), then a fresh code is issued and sent through it.
  httpd.route("POST", "/api/session/register", function(req)
    local rl_status, rl_body = ratelimit.check(("register-api:%s"):format(req.client_ip or "unknown"), 10, 3600)
    if rl_status then return rl_status, rl_body end

    local body = req.json or {}
    local proof_url = body.verification_webhook_url or body.registration_proof_url
    local verification_code = auth.verification_code()

    local proof = {}
    if proof_url and tostring(proof_url) ~= "" then
      local ok_proof, proof_or_err = pcall(notify.verify_guild_registration_proof, body.guild_id, proof_url)
      if not ok_proof then return 400, { detail = tostring(proof_or_err):gsub("^.-:%d+:%s*", "") } end
      proof = proof_or_err
    end

    local ok, account = pcall(
      accounts.register_account_login, body.username, body.guild_id, body.password, body.email,
      proof_url, proof.channel_id, proof.webhook_name, verification_code
    )
    if not ok then return 400, { detail = tostring(account):gsub("^.-:%d+:%s*", "") } end

    local verification_sent = false
    if proof_url and tostring(proof_url) ~= "" then
      verification_sent = notify.send_verification_webhook_code(proof_url, verification_code, account.username, account.guild_id)
    end

    -- Straight-to-account-owner alternative to the webhook proof above: if
    -- no webhook was supplied but a Discord User ID was, verify via DM
    -- instead -- mutually exclusive with the webhook path (a fresh account
    -- only needs one proof of ownership), same as /api/session/verification-*
    -- treats them post-registration.
    local discord_user_id = tostring(body.discord_user_id or ""):match("^%s*(.-)%s*$")
    if not verification_sent and discord_user_id ~= "" then
      local dm_ok = pcall(accounts.update_account_discord_user_id, account.username, account.guild_id, discord_user_id)
      if dm_ok then
        accounts.issue_account_discord_dm_verification_code(account.username, account.guild_id, verification_code)
        verification_sent = notify.send_discord_dm(
          settings.discord_bot_token, discord_user_id,
          string.format("SwarmPanel verification code: **%s**\nEnter this code in SwarmPanel to verify your account. It expires in %d minutes.",
            verification_code, math.floor(settings.discord_dm_verification_ttl_seconds / 60))
        ) and true or false
      end
    end

    local reg_client = client_name(body.client) or "web"
    local reg_device = body.remember_device == true and reg_client ~= "web"
    local token = auth.issue_api_token(settings.session_secret, account.username, {
      role = "account", guild_id = account.guild_id, admin_mode = false, site_owner = false, moderator = false,
      ttl_seconds = token_ttl(reg_device), device = reg_device, client = reg_client,
    })
    return 200, {
      ok = true, token = token, username = account.username, role = "account",
      guild_id = account.guild_id, account_guild_id = account.guild_id,
      site_owner = false, admin_mode = false, moderator = false, image_gallery_owner = false,
      verification_sent = verification_sent,
      pages_public_url = settings.pages_public_url, expires_in = token_ttl(reg_device),
    }
  end)

  -- --------------------------------------------- session/verification-*
  -- Port of session.py's /api/session/resend-verification,
  -- /api/session/verification-webhook, /api/session/verification/verify.
  -- Rate limiting (_rate_limit_auth in the Python original) is now ported
  -- via ratelimit.lua, with the same per-route keys/limits as session.py.
  local function verification_is_complete(profile)
    return profile ~= nil and (profile.webhook_verified == true or profile.email_verified == true or profile.discord_dm_verified == true)
  end

  httpd.route("POST", "/api/session/resend-verification", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required" } end
    local rl_status, rl_body = ratelimit.check(
      ("session-verification-resend:%s"):format(username:lower()), 8, 3600)
    if rl_status then return rl_status, rl_body end

    local profile = accounts.get_account_profile(username, scoped_gid)
    if not profile then return 404, { detail = "Account profile not found" } end
    local has_webhook = profile.verification_webhook_url and profile.verification_webhook_url ~= ""
    local has_discord = profile.discord_user_id and profile.discord_user_id ~= ""
    if not has_webhook and not has_discord then
      return 400, { detail = "Set a Discord verification webhook or Discord User ID before requesting a code." }
    end
    if verification_is_complete(profile) then
      return 200, { ok = true, verification_sent = false, already_verified = true }
    end
    local code = auth.verification_code()
    local sent
    if has_webhook then
      profile = accounts.issue_account_webhook_verification_code(username, scoped_gid, code)
      sent = profile ~= nil and notify.send_verification_webhook_code(profile.verification_webhook_url, code, username, scoped_gid)
    else
      profile = accounts.issue_account_discord_dm_verification_code(username, scoped_gid, code)
      local dm_ok = profile ~= nil and notify.send_discord_dm(
        settings.discord_bot_token, profile.discord_user_id,
        string.format("SwarmPanel verification code: **%s**\nEnter this code in SwarmPanel to verify your account. It expires in %d minutes.",
          code, math.floor(settings.discord_dm_verification_ttl_seconds / 60))
      )
      sent = dm_ok and true or false
    end
    return 200, { ok = sent and true or false, verification_sent = sent and true or false, already_verified = false }
  end)

  -- Discord DM verification enrollment: sets/clears the account's
  -- discord_user_id and (if non-empty) immediately sends a fresh code via
  -- notify.send_discord_dm -- direct counterpart to
  -- /api/session/verification-webhook just below, for operators who'd
  -- rather prove ownership of their own Discord account than stand up a
  -- guild webhook.
  httpd.route("POST", "/api/session/verification-discord", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required" } end
    local rl_status, rl_body = ratelimit.check(
      ("session-verification-discord:%s"):format(username:lower()), 8, 3600)
    if rl_status then return rl_status, rl_body end

    local body = req.json or {}
    local discord_user_id = tostring(body.discord_user_id or ""):match("^%s*(.-)%s*$")

    local ok, profile = pcall(accounts.update_account_discord_user_id, username, scoped_gid, discord_user_id)
    if not ok then return 400, { detail = tostring(profile):gsub("^.-:%d+:%s*", "") } end
    if not profile then return 404, { detail = "Account profile not found" } end
    if discord_user_id == "" then
      return 200, { ok = true, profile = profile, verification_sent = false }
    end

    local code = auth.verification_code()
    profile = accounts.issue_account_discord_dm_verification_code(username, scoped_gid, code)
    local sent, send_err = notify.send_discord_dm(
      settings.discord_bot_token, discord_user_id,
      string.format("SwarmPanel verification code: **%s**\nEnter this code in SwarmPanel to verify your account. It expires in %d minutes.",
        code, math.floor(settings.discord_dm_verification_ttl_seconds / 60))
    )
    if not sent then return 400, { detail = send_err or "Could not send the DM." } end
    return 200, { ok = true, profile = profile, verification_sent = true }
  end)

  httpd.route("POST", "/api/session/verification-webhook", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required" } end
    local rl_status, rl_body = ratelimit.check(
      ("session-verification-webhook:%s"):format(username:lower()), 8, 3600)
    if rl_status then return rl_status, rl_body end

    local body = req.json or {}
    local webhook_url = tostring(body.verification_webhook_url or ""):match("^%s*(.-)%s*$")
    local profile = accounts.get_account_profile(username, scoped_gid)
    if not profile then return 404, { detail = "Account profile not found" } end

    if webhook_url == "" then
      profile = accounts.update_account_verification_webhook(username, scoped_gid, nil, nil, nil)
      return 200, { ok = true, profile = profile, verification_sent = false }
    end

    local ok_proof, proof_or_err = pcall(notify.verify_guild_registration_proof, scoped_gid, webhook_url)
    if not ok_proof then return 400, { detail = tostring(proof_or_err):gsub("^.-:%d+:%s*", "") } end
    profile = accounts.update_account_verification_webhook(username, scoped_gid, webhook_url, proof_or_err.channel_id, proof_or_err.webhook_name)

    local code = auth.verification_code()
    profile = accounts.issue_account_webhook_verification_code(username, scoped_gid, code)
    local sent = profile ~= nil and notify.send_verification_webhook_code(webhook_url, code, username, scoped_gid)
    return 200, { ok = true, profile = profile, verification_sent = sent and true or false }
  end)

  -- Port of session.py's /api/session/email (identity.email field on the
  -- frontend's ProfilePage save flow) — email address management is
  -- independent of the webhook-verification code paths above but the
  -- frontend calls it from the same save button, so it's wired here too.
  httpd.route("POST", "/api/session/email", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required" } end
    local rl_status, rl_body = ratelimit.check(("session-email-update:%s"):format(username:lower()), 8, 3600)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    local ok, profile = pcall(accounts.update_account_email, username, scoped_gid, body.email)
    if not ok then return 400, { detail = tostring(profile):gsub("^.-:%d+:%s*", "") } end
    if not profile then return 404, { detail = "Account profile not found" } end
    return 200, { ok = true, profile = profile }
  end)

  httpd.route("POST", "/api/session/verification/verify", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required" } end
    local rl_status, rl_body = ratelimit.check(("session-verification-verify:%s"):format(username:lower()), 20, 3600)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    local code = tostring(body.code or ""):gsub("%D+", ""):sub(1, 16)
    if code == "" then return 400, { detail = "Enter the verification code you received." } end
    -- Tries both pending-code stores -- a given account only ever has one
    -- actually populated at a time (issuing a webhook code never touches
    -- the discord_dm_* columns and vice versa), so trying both here lets
    -- this stay the single "enter your code" endpoint for either method.
    local profile = accounts.verify_account_webhook_code(username, scoped_gid, code, settings.email_verification_ttl_seconds)
    if not profile then
      -- Best-effort cosmetic username lookup ("Verified as @handle") done
      -- BEFORE the actual verify call, since verify_account_discord_dm_code
      -- clears discord_user_id's pending state as soon as it succeeds --
      -- looking it up after would have nothing left to look up against on
      -- a fresh get_account_profile, and a failed lookup here must never
      -- block the verification itself succeeding.
      local pending = accounts.get_account_profile(username, scoped_gid)
      local handle = pending and pending.discord_user_id
        and notify.get_discord_username(settings.discord_bot_token, pending.discord_user_id) or nil
      profile = accounts.verify_account_discord_dm_code(username, scoped_gid, code, settings.discord_dm_verification_ttl_seconds, handle)
    end
    if not profile then return 400, { detail = "Invalid verification code." } end
    return 200, { ok = true, profile = profile }
  end)

  -- -------------------------------------------------------------- users/me
  local function favorite_bot_options()
    local out = {}
    for _, bot in ipairs(music_bots) do out[#out + 1] = { key = bot.key, display_name = bot.display_name, kind = bot.kind } end
    out[#out + 1] = { key = cfg.aria_bot.key, display_name = cfg.aria_bot.display_name, kind = cfg.aria_bot.kind }
    return out
  end

  httpd.route("GET", "/api/users/me", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or settings.admin_username)
    if not scoped_gid then
      return 200, {
        editable = false,
        profile = {
          username = username, display_name = username, guild_id = nil, public_profile = false, has_password = false,
          panel_preferences = profiles.default_panel_preferences(),
          bio = "Built-in admin sessions can browse the user directory; registered accounts own public profiles.",
        },
        favorite_bot_options = favorite_bot_options(),
      }
    end
    local profile = accounts.get_account_profile(username, scoped_gid)
    if not profile then return 404, { detail = "Account profile not found" } end
    local snap = social.get_account_social_snapshot(profile.id, profile.id)
    for k, v in pairs(snap) do profile[k] = v end
    local summaries = dashboard.get_music_activity_summary_for_guilds(music_bots, { scoped_gid })
    profile.activity = summaries[scoped_gid] or dashboard.empty_music_activity_summary()
    return 200, { editable = true, profile = with_derived_accent(profile, "theme_accent"), favorite_bot_options = favorite_bot_options() }
  end)

  httpd.route("POST", "/api/users/me", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required to edit a public profile" } end
    local ok, updates_or_err = pcall(profiles.clean_profile_updates, req.json or {}, bot_index)
    if not ok then return 400, { detail = tostring(updates_or_err):gsub("^.-:%d+:%s*", "") } end
    local profile = accounts.update_account_profile(username, scoped_gid, updates_or_err)
    if not profile then return 404, { detail = "Account profile not found" } end
    -- Profile-customization analytics (per operator request) -- distinct
    -- from panel_preferences above (that's dashboard/UI-wide theme; this is
    -- the user's own profile card look: card style/shape/hover effect,
    -- accent, bio, links). updates_or_err already IS just the changed
    -- fields (clean_profile_updates only returns what the request
    -- supplied), so no separate before/after diff is needed here.
    pcall(telemetry.record, "customization", "profile_update", { username = username, guild_id = scoped_gid, metadata = updates_or_err })
    local summaries = dashboard.get_music_activity_summary_for_guilds(music_bots, { scoped_gid })
    profile.activity = summaries[scoped_gid] or dashboard.empty_music_activity_summary()
    return 200, { ok = true, profile = with_derived_accent(profile, "theme_accent") }
  end)

  httpd.route("GET", "/api/users/preferences", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    local defaults = profiles.default_panel_preferences()
    if not scoped_gid or username == "" then return 200, { editable = false, preferences = with_derived_accent(defaults) } end
    local profile = accounts.get_account_profile(username, scoped_gid)
    if not profile then return 404, { detail = "Account profile not found" } end
    local stored = (type(profile.panel_preferences) == "table") and profile.panel_preferences or nil
    local ok, prefs = pcall(profiles.clean_panel_preferences, stored, defaults)
    if not ok then prefs = defaults end
    return 200, { editable = true, preferences = with_derived_accent(prefs) }
  end)

  httpd.route("POST", "/api/users/preferences", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local scoped_gid = account_guild_id(a)
    local username = tostring(a.username or "")
    if not scoped_gid or username == "" then return 403, { detail = "Guild account access required to save panel preferences" } end
    local current_profile = accounts.get_account_profile(username, scoped_gid)
    if not current_profile then return 404, { detail = "Account profile not found" } end
    local defaults = profiles.default_panel_preferences()
    local stored = (type(current_profile.panel_preferences) == "table") and current_profile.panel_preferences or nil
    local ok1, base_prefs = pcall(profiles.clean_panel_preferences, stored, defaults)
    if not ok1 then base_prefs = defaults end
    -- admin_mode_enabled is owned by POST /api/session/admin-mode (which
    -- checks site-owner status); never accept it from an Appearance save.
    local incoming = {}
    for k, v in pairs(req.json or {}) do
      if k ~= "admin_mode_enabled" then incoming[k] = v end
    end
    local ok2, prefs = pcall(profiles.clean_panel_preferences, incoming, base_prefs)
    if not ok2 then return 400, { detail = tostring(prefs):gsub("^.-:%d+:%s*", "") } end
    local profile = accounts.update_account_panel_preferences(username, scoped_gid, prefs)
    if not profile then return 404, { detail = "Account profile not found" } end
    -- Customization/appearance analytics (per operator request): a diff of
    -- what actually changed (theme, accent, density, card style, hover
    -- effect, ...), not the full preferences blob every time -- reuses
    -- audit.lua's diff_details() (same before/after JSON shape it already
    -- produces for the admin audit log) rather than reimplementing table
    -- diffing here.
    local diff = audit.diff_details(base_prefs, prefs)
    if diff ~= "" then
      pcall(telemetry.record, "customization", "panel_preferences_update", { username = username, guild_id = scoped_gid, text_value = diff })
    end
    return 200, { ok = true, preferences = with_derived_accent((type(profile.panel_preferences) == "table" and profile.panel_preferences) or prefs) }
  end)

  -- Server-owned appearance presets (see looks.lua): lets new panel/profile
  -- look bundles ship without a frontend redeploy. Auth-gated like the rest
  -- of /api/users/* even though the content isn't guild-scoped, for
  -- consistency with every other authenticated-only route in this file.
  httpd.route("GET", "/api/appearance/presets", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    return 200, { panel = looks.PANEL_LOOKS, profile = looks.PROFILE_LOOKS }
  end)

  httpd.route("GET", "/api/users/search", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local q = tostring(req.query.q or ""):gsub("%s+", " "):sub(1, 80)
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 24, 50))
    local viewer_id = select(1, account_id_for_auth(a))
    local users = accounts.search_account_profiles(q, limit, viewer_id, nil)
    return 200, { ok = true, query = q, users = users, limit = limit }
  end)

  httpd.route("GET", "/api/users/directory", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local q = tostring(req.query.q or ""):gsub("%s+", " "):sub(1, 80)
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 24, 50))
    local viewer_id = select(1, account_id_for_auth(a))
    -- Cross-server roster: not guild-scoped, gated only by public_profile=1
    -- in search_account_profiles (mirrors users.py's directory route note).
    -- ?online=1 limits to operators seen in the last 3 minutes.
    local online_only = req.query.online == "1" or req.query.online == "true"
    local users = accounts.search_account_profiles(q, limit, viewer_id, nil, online_only)
    return 200, { ok = true, query = q, users = users, limit = limit, online_only = online_only }
  end)

  httpd.route("GET", "/api/users/:account_id/profile", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local viewer_id = select(1, account_id_for_auth(a))
    local ok, profile = pcall(social.get_public_account_profile, req.params.account_id, viewer_id)
    if not ok or not profile then return 404, { detail = "Profile not found" } end
    local permissions = social_permissions(profile)
    local friends = {}
    if permissions.can_view_friends then
      local all_friends = social.list_account_friends(db.toint(profile.id))
      for i = 1, math.min(12, #all_friends) do friends[i] = all_friends[i] end
    end
    return 200, { ok = true, profile = profile, friends = friends, social_permissions = permissions, favorite_bot_options = favorite_bot_options() }
  end)

  httpd.route("POST", "/api/users/:account_id/follow", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local following = (req.json or {}).following and true or false
    local ok, target = pcall(social.get_public_account_profile, req.params.account_id, actor_id)
    if not ok or not target then
      -- get_public_account_profile hides non-public/non-self profiles; fall
      -- back to a permission-agnostic lookup so the can_follow gate below
      -- still sees real social state (mirrors _load_social_target_profile).
      target = accounts.get_account_by_id(req.params.account_id)
      if target then
        local snap = social.get_account_social_snapshot(target.id, actor_id)
        for k, v in pairs(snap) do target[k] = v end
      end
    end
    if not target then return 404, { detail = "Profile not found" } end
    if following and not social_permissions(target).can_follow then
      return 403, { detail = "This profile is not accepting follows." }
    end
    local ok2, result = pcall(social.set_account_follow, actor_id, req.params.account_id, following)
    if not ok2 then return 400, { detail = tostring(result):gsub("^.-:%d+:%s*", "") } end
    if following then
      social.create_notification(req.params.account_id, "follow", tostring(a.username) .. " started following you", nil, "/users/" .. tostring(actor_id))
    end
    result.ok = true
    return 200, result
  end)

  httpd.route("POST", "/api/users/:account_id/friend-request", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local target = accounts.get_account_by_id(req.params.account_id)
    if target then
      local snap = social.get_account_social_snapshot(target.id, actor_id)
      for k, v in pairs(snap) do target[k] = v end
    end
    if not target then return 404, { detail = "Profile not found" } end
    if not social_permissions(target).can_friend then return 403, { detail = "This profile is not accepting friend requests." } end
    local ok, result = pcall(social.send_account_friend_request, actor_id, req.params.account_id)
    if not ok then return 400, { detail = tostring(result):gsub("^.-:%d+:%s*", "") } end
    if result.status == "pending_out" then
      social.create_notification(req.params.account_id, "friend_request", tostring(a.username) .. " sent you a friend request", nil, "/friends")
    elseif result.status == "friends" then
      social.create_notification(req.params.account_id, "friend_accept", "You and " .. tostring(a.username) .. " are now friends", nil, "/friends")
    end
    result.ok = true
    return 200, result
  end)

  httpd.route("GET", "/api/friends/requests", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    return 200, {
      ok = true,
      incoming = social.list_account_friend_requests(actor_id, "incoming"),
      outgoing = social.list_account_friend_requests(actor_id, "outgoing"),
    }
  end)

  httpd.route("POST", "/api/friends/requests/:request_id", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local ok, result = pcall(social.respond_account_friend_request, actor_id, req.params.request_id, (req.json or {}).action)
    if not ok then return 400, { detail = tostring(result):gsub("^.-:%d+:%s*", "") } end
    if not result then return 404, { detail = "Friend request not found" } end
    if result.status == "accepted" then
      local requester_id = db.toint(result.requester_account_id)
      if requester_id and requester_id ~= actor_id then
        social.create_notification(requester_id, "friend_accept", tostring(a.username) .. " accepted your friend request", nil, "/friends")
      end
    end
    return 200, { ok = true, request = result }
  end)

  httpd.route("GET", "/api/me/friends", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    return 200, { ok = true, friends = social.list_account_friends(actor_id) }
  end)

  httpd.route("GET", "/api/messages/threads", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    return 200, { ok = true, threads = social.list_account_message_threads(actor_id) }
  end)

  httpd.route("GET", "/api/messages/:account_id", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local target = accounts.get_account_by_id(req.params.account_id)
    if target then
      local snap = social.get_account_social_snapshot(target.id, actor_id)
      for k, v in pairs(snap) do target[k] = v end
    end
    if not target or not social_permissions(target).can_message then
      return 403, { detail = "This profile is not accepting direct messages." }
    end
    local limit = tonumber(req.query.limit) or 80
    local ok, messages = pcall(social.list_account_messages, actor_id, req.params.account_id, limit)
    if not ok then return 400, { detail = tostring(messages):gsub("^.-:%d+:%s*", "") } end
    return 200, { ok = true, messages = messages }
  end)

  httpd.route("POST", "/api/messages/:account_id", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local target = accounts.get_account_by_id(req.params.account_id)
    if target then
      local snap = social.get_account_social_snapshot(target.id, actor_id)
      for k, v in pairs(snap) do target[k] = v end
    end
    if not target or not social_permissions(target).can_message then
      return 403, { detail = "This profile is not accepting direct messages." }
    end
    local body_text = tostring((req.json or {}).body or "")
    local ok, message = pcall(social.send_account_message, actor_id, req.params.account_id, body_text)
    if not ok then return 400, { detail = tostring(message):gsub("^.-:%d+:%s*", "") } end
    social.create_notification(req.params.account_id, "message", "New message from " .. tostring(a.username), body_text:sub(1, 200), "/messages/" .. tostring(actor_id))
    return 200, { ok = true, message = message }
  end)

  httpd.route("GET", "/api/notifications", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 30, 100))
    return 200, { ok = true, notifications = social.list_notifications(actor_id, limit) }
  end)

  httpd.route("GET", "/api/notifications/unread-count", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    return 200, { ok = true, unread_count = social.unread_notification_count(actor_id) }
  end)

  -- Admin overview stats (web /admin, iOS Admin hub). Staff-only; each
  -- stat is computed independently and left out (nil) if its source is
  -- unavailable or the caller's tier can't see it -- open gallery reports
  -- are admin-only, matching who can open the Gallery tools.
  httpd.route("GET", "/api/admin/overview", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local admin = is_admin_auth(a)
    if not (admin or is_moderator_auth(a)) then return 403, { detail = "Admin or moderator access required" } end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local out = { ok = true, generated_at = os.date("!%Y-%m-%dT%H:%M:%SZ") }
    local ok_dash, data = pcall(dashboard.get_dashboard_data, music_bots)
    if ok_dash and data then
      local total, online = 0, 0
      for _, bot in ipairs(data.bots or {}) do
        if bot.kind == nil or bot.kind == "music" then
          total = total + 1
          -- Same offline rule as the Dashboard's bot cards.
          local age = tonumber(bot.heartbeat_age_seconds)
          if tostring(bot.status or ""):lower() ~= "offline" and not (age and age > 120) then online = online + 1 end
        end
      end
      out.bots_total, out.bots_online = total, online
    end
    local ok_rules, rules = pcall(alerts.list_enabled_alert_rules)
    if ok_rules and type(rules) == "table" then out.alert_rules_enabled = #rules end
    local ok_audit, audit_count = pcall(audit.count_recent, 24)
    if ok_audit then out.audit_entries_recent = audit_count end
    if admin then
      local ok_reports, reports = pcall(gallery.count_open_reports)
      if ok_reports then out.open_reports = reports end
    end
    return 200, out
  end)

  -- REST twin of the community_counts live key (iOS fallback / first paint).
  httpd.route("GET", "/api/community/counts", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    local ok, counts = pcall(social.community_counts, actor_id)
    if not ok then return 503, { detail = "Community counts unavailable: " .. tostring(counts) } end
    counts.ok = true
    return 200, counts
  end)

  httpd.route("POST", "/api/notifications/:notification_id/read", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    social.mark_notification_read(req.params.notification_id, actor_id)
    return 200, { ok = true }
  end)

  httpd.route("POST", "/api/notifications/read-all", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local actor_id, aerr = account_id_for_auth(a)
    if not actor_id then return 403, { detail = aerr } end
    social.mark_all_notifications_read(actor_id)
    return 200, { ok = true }
  end)

  -- ------------------------------------------------------- swarm-accounts
  -- Admin CRUD over registered SwarmPanel accounts. Port of
  -- app/routers/swarm_accounts.py, now including /resend-verification (see
  -- notify.lua for the Discord-webhook client this needed).
  httpd.route("POST", "/api/swarm-accounts/resend-verification", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local account = accounts.get_account_admin(body.account_id)
    if not account then return 404, { detail = "SwarmPanel account not found" } end
    if not account.verification_webhook_url or account.verification_webhook_url == "" then
      return 400, { detail = "This SwarmPanel account does not have a Discord verification webhook configured." }
    end
    if account.webhook_verified == true or account.email_verified == true then
      return 200, { ok = true, verification_sent = false, already_verified = true }
    end
    local code = auth.verification_code()
    account = accounts.issue_account_webhook_verification_code_by_id(body.account_id, code)
    local sent = account ~= nil and notify.send_verification_webhook_code(account.verification_webhook_url, code, account.username, account.guild_id)
    audit.record_audit_log(a.username, "swarm_account_resend_verification", { target_type = "account", target_id = body.account_id, details = "sent=" .. tostring(sent and true or false) })
    return 200, { ok = sent and true or false, verification_sent = sent and true or false, already_verified = false }
  end)

  httpd.route("GET", "/api/swarm-accounts/admin", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 100, 500))
    local q = tostring(req.query.query or ""):gsub("%s+", " "):sub(1, 120)
    local ok, data = pcall(accounts.get_account_admin_data, q, limit)
    if not ok then return 503, { detail = "SwarmPanel account database unavailable: " .. tostring(data) } end
    return 200, { ok = true, data = data }
  end)

  httpd.route("POST", "/api/swarm-accounts/update", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local account_id = body.account_id
    if not account_id then return 400, { detail = "account_id is required" } end
    local updates = {}
    for _, key in ipairs({ "username", "guild_id", "email", "display_name", "public_profile", "server_name" }) do
      if body[key] ~= nil then updates[key] = body[key] end
    end
    local before = accounts.get_account_admin(account_id)
    local account, err = accounts.update_account_admin(account_id, updates)
    if err then return 400, { detail = err } end
    if not account then return 404, { detail = "SwarmPanel account not found" } end
    local keys = {}
    for k in pairs(updates) do keys[#keys + 1] = k end
    table.sort(keys)
    local details = before and audit.diff_details(before, account) or table.concat(keys, ",")
    audit.record_audit_log(a.username, "swarm_account_update", { target_type = "account", target_id = account_id, details = (details ~= "" and details) or nil })
    return 200, { ok = true, account = account }
  end)

  httpd.route("POST", "/api/swarm-accounts/email-verified", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local account = accounts.set_account_webhook_verified_admin(body.account_id, body.verified and true or false)
    if not account then return 404, { detail = "SwarmPanel account not found" } end
    return 200, { ok = true, account = account }
  end)

  httpd.route("POST", "/api/swarm-accounts/reset-password", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local account, err = accounts.reset_account_password_admin(body.account_id, body.new_password)
    if err then return 400, { detail = err } end
    if not account then return 404, { detail = "SwarmPanel account not found" } end
    audit.record_audit_log(a.username, "swarm_account_reset_password", { target_type = "account", target_id = body.account_id })
    return 200, { ok = true, account = account }
  end)

  httpd.route("POST", "/api/swarm-accounts/moderator", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local account = accounts.set_account_panel_role(body.account_id, body.moderator and true or false)
    if not account then return 404, { detail = "SwarmPanel account not found" } end
    audit.record_audit_log(a.username, "swarm_account_set_moderator", { target_type = "account", target_id = body.account_id, details = "moderator=" .. tostring(body.moderator and true or false) })
    return 200, { ok = true, account = account }
  end)

  httpd.route("POST", "/api/swarm-accounts/delete", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local ok, err = accounts.delete_account_admin(body.account_id)
    if not ok then return 400, { detail = err } end
    audit.record_audit_log(a.username, "swarm_account_delete", { target_type = "account", target_id = body.account_id })
    return 200, { ok = true }
  end)

  local MAX_BULK_IDS = 200
  local function run_bulk_op(ids, run_one)
    local seen, unique = {}, {}
    for _, id in ipairs(ids or {}) do
      local key = tostring(id)
      if not seen[key] and #unique < MAX_BULK_IDS then
        seen[key] = true
        unique[#unique + 1] = id
      end
    end
    local succeeded, failed = {}, {}
    for _, id in ipairs(unique) do
      local ok, err = pcall(run_one, id)
      if ok and err ~= false then
        succeeded[#succeeded + 1] = id
      else
        failed[#failed + 1] = { id = id, error = tostring(err):sub(1, 240) }
      end
    end
    return { succeeded = succeeded, failed = failed }
  end

  httpd.route("POST", "/api/swarm-accounts/bulk-delete", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local result = run_bulk_op(body.ids, function(id)
      local ok, err = accounts.delete_account_admin(id)
      if not ok then error(err, 0) end
    end)
    audit.record_audit_log(a.username, "swarm_account_bulk_delete", { target_type = "account", details = string.format("succeeded_ids=%d failed=%d", #result.succeeded, #result.failed) })
    result.ok = true
    return 200, result
  end)

  httpd.route("POST", "/api/swarm-accounts/bulk-verify", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local verified = body.verified and true or false
    local result = run_bulk_op(body.ids, function(id)
      local account = accounts.set_account_webhook_verified_admin(id, verified)
      if not account then error("Account not found", 0) end
    end)
    audit.record_audit_log(a.username, "swarm_account_bulk_verify", { target_type = "account", details = string.format("verified=%s succeeded_ids=%d failed=%d", tostring(verified), #result.succeeded, #result.failed) })
    result.ok = true
    return 200, result
  end)

  -- ------------------------------------------------------------- queues
  httpd.route("GET", "/api/queues", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local gid = req.query.guild_id
    if not gid or gid == "" then return 400, { detail = "guild_id is required" } end
    local gerr_status, gerr_body = require_guild_scope(req, a, gid)
    if gerr_status then return gerr_status, gerr_body end
    local ok, result = pcall(queues.list_saved_queues, gid, req.query.bot_key)
    if not ok then return 503, { detail = "Saved queues unavailable: " .. tostring(result) } end
    return 200, { ok = true, queues = result }
  end)

  httpd.route("POST", "/api/queues", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    if not body.guild_id or tostring(body.guild_id) == "" then return 400, { detail = "guild_id is required" } end
    local gerr_status, gerr_body = require_guild_scope(req, a, body.guild_id)
    if gerr_status then return gerr_status, gerr_body end
    local ok, queue = pcall(queues.create_saved_queue, body.guild_id, body.bot_key, body.name, a.username, body.items)
    if not ok then return 400, { detail = tostring(queue):gsub("^.-:%d+:%s*", "") } end
    audit.record_audit_log(a.username, "saved_queue_create", {
      target_type = "saved_queue", target_id = queue.id,
      details = string.format("guild=%s bot=%s name=%s", tostring(body.guild_id), tostring(body.bot_key), tostring(body.name)),
    })
    return 200, { ok = true, queue = queue }
  end)

  httpd.route("POST", "/api/queues/:queue_id/rename", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    if not body.guild_id or tostring(body.guild_id) == "" then return 400, { detail = "guild_id is required" } end
    local gerr_status, gerr_body = require_guild_scope(req, a, body.guild_id)
    if gerr_status then return gerr_status, gerr_body end
    local ok, queue = pcall(queues.rename_saved_queue, req.params.queue_id, body.guild_id, body.name)
    if not ok then return 400, { detail = tostring(queue):gsub("^.-:%d+:%s*", "") } end
    audit.record_audit_log(a.username, "saved_queue_rename", { target_type = "saved_queue", target_id = req.params.queue_id, details = "name=" .. tostring(body.name) })
    return 200, { ok = true, queue = queue }
  end)

  httpd.route("POST", "/api/queues/:queue_id/delete", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local body = req.json or {}
    if not body.guild_id or tostring(body.guild_id) == "" then return 400, { detail = "guild_id is required" } end
    local gerr_status, gerr_body = require_guild_scope(req, a, body.guild_id)
    if gerr_status then return gerr_status, gerr_body end
    queues.delete_saved_queue(req.params.queue_id, body.guild_id)
    audit.record_audit_log(a.username, "saved_queue_delete", { target_type = "saved_queue", target_id = req.params.queue_id })
    return 200, { ok = true }
  end)

  -- --------------------------------------------------- guild control/intel
  -- Port of app/routers/bots.py's GET /api/guilds/{guild_id}/control-matrix:
  -- every music bot's live control state for one guild.
  httpd.route("GET", "/api/guilds/:guild_id/control-matrix", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local gid = req.params.guild_id
    local gerr_status, gerr_body = require_guild_scope(req, a, gid)
    if gerr_status then return gerr_status, gerr_body end
    local bots = {}
    for _, bot in ipairs(music_bots) do
      if bot.kind == "music" then
        local ok, state = pcall(dashboard.get_bot_control_state, bot, gid)
        if ok then
          local ok2, enriched = pcall(enrich_control_state_with_discord, state)
          bots[#bots + 1] = ok2 and enriched or state
        else
          bots[#bots + 1] = { bot_key = bot.key, bot_display = bot.display_name, error = tostring(state) }
        end
      end
    end
    return 200, { generated_at = os.date("!%Y-%m-%dT%H:%M:%SZ"), guild_id = tostring(gid), bots = bots }
  end)

  -- Port of GET /api/guilds/{guild_id}/leaderboard.
  httpd.route("GET", "/api/guilds/:guild_id/leaderboard", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local gid = req.params.guild_id
    local gerr_status, gerr_body = require_guild_scope(req, a, gid)
    if gerr_status then return gerr_status, gerr_body end
    local bot_key = req.query.bot_key
    local bot = bot_key and bot_index[bot_key]
    if not bot or bot.kind ~= "music" then return 400, { detail = "This action is only supported for music bots" } end
    local ok, result = pcall(dashboard.get_guild_leaderboard, bot, gid, tonumber(req.query.limit))
    if not ok then return 503, { detail = "Leaderboard unavailable: " .. tostring(result) } end
    return 200, { ok = true, data = result }
  end)

  -- Port of GET /api/music-intelligence.
  httpd.route("GET", "/api/music-intelligence", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local gid = req.query.guild_id
    local scoped = scoped_guild_id(a)
    if scoped and scoped ~= OWNER_SCOPE_SENTINEL then
      gid = scoped
    elseif gid and gid ~= "" then
      local gerr_status, gerr_body = require_guild_scope(req, a, gid)
      if gerr_status then return gerr_status, gerr_body end
    elseif not is_admin_auth(a) then
      return 403, { detail = "Admin access required" }
    end
    local ok, result = pcall(dashboard.get_music_intelligence_summary, music_bots, gid, req.query.bot_key, tonumber(req.query.limit))
    if not ok then return 503, { detail = "Music intelligence unavailable: " .. tostring(result) } end
    return 200, { ok = true, data = result }
  end)

  -- ------------------------------------------------------- bot-telemetry
  -- Raw per-event stream from a single bot's own <bot>_telemetry_events
  -- table (see pages_telemetry.lua's header comment for how this differs
  -- from telemetry.lua's panel-usage tracking and /intel's fleet-wide
  -- aggregate trends). Admin/moderator-only -- this is internal
  -- diagnostic detail, not something a guild-scoped account needs.
  httpd.route("GET", "/api/bot-telemetry", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local bot_key = req.query.bot_key
    local bot = bot_key and bot_index[bot_key]
    if not bot or bot.kind ~= "music" then return json_error(400, "unknown or unsupported bot_key") end
    local limit = math.max(1, math.min(1000, tonumber(req.query.limit) or 100))
    local category = req.query.category
    local table_name = bot.table_prefix .. "_telemetry_events"
    local ok, exists = pcall(db.table_exists, bot.db_schema, table_name)
    if not ok or not exists then return 200, { ok = true, events = {} } end
    local sql, args
    if category and category ~= "" then
      sql = ("SELECT event_category, event_name, guild_id, numeric_value, text_value, created_at FROM %s " ..
        "WHERE event_category = %%s ORDER BY created_at DESC LIMIT %%s"):format(table_name)
      args = { category, limit }
    else
      sql = ("SELECT event_category, event_name, guild_id, numeric_value, text_value, created_at FROM %s " ..
        "ORDER BY created_at DESC LIMIT %%s"):format(table_name)
      args = { limit }
    end
    local rok, rows = pcall(db.fetchall, bot.db_schema, sql, unpack(args))
    if not rok then return 503, { detail = "Telemetry query failed: " .. tostring(rows) } end
    return 200, { ok = true, events = rows or {} }
  end)

  -- ---------------------------------------------------------- alert-rules
  httpd.route("GET", "/api/alert-rules", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local ok, rules = pcall(alerts.list_alert_rules)
    if not ok then return 503, { detail = "Alert rules unavailable: " .. tostring(rules) } end
    return 200, { ok = true, rules = rules }
  end)

  httpd.route("POST", "/api/alert-rules", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local ok, rule = pcall(alerts.create_alert_rule, body.rule_type, body.threshold_minutes, body.enabled == nil or body.enabled, body.escalation_minutes, body.escalate_email)
    if not ok then return 400, { detail = tostring(rule):gsub("^.-:%d+:%s*", "") } end
    audit.record_audit_log(a.username, "alert_rule_create", { target_type = "alert_rule", target_id = rule.id })
    return 200, { ok = true, rule = rule }
  end)

  httpd.route("POST", "/api/alert-rules/:rule_id/update", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local before = alerts.get_alert_rule(req.params.rule_id)
    local ok, rule = pcall(alerts.update_alert_rule, req.params.rule_id, body)
    if not ok then return 400, { detail = tostring(rule):gsub("^.-:%d+:%s*", "") } end
    if not rule then return 404, { detail = "Alert rule not found" } end
    local keys = {}
    for k in pairs(body) do keys[#keys + 1] = k end
    table.sort(keys)
    local details = before and audit.diff_details(before, rule) or table.concat(keys, ",")
    audit.record_audit_log(a.username, "alert_rule_update", { target_type = "alert_rule", target_id = req.params.rule_id, details = (details ~= "" and details) or nil })
    return 200, { ok = true, rule = rule }
  end)

  httpd.route("POST", "/api/alert-rules/:rule_id/delete", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    alerts.delete_alert_rule(req.params.rule_id)
    audit.record_audit_log(a.username, "alert_rule_delete", { target_type = "alert_rule", target_id = req.params.rule_id })
    return 200, { ok = true }
  end)

  -- --------------------------------------------------------- image-gallery
  httpd.route("GET", "/api/image-gallery/admin", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 50, 200))
    local ok, data = pcall(gallery.get_image_gallery_admin_data, limit)
    if not ok then return 503, { detail = "Image Gallery database unavailable: " .. tostring(data) } end
    return 200, { ok = true, data = data }
  end)

  httpd.route("GET", "/api/image-gallery/admin/media/export.csv", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 200, 500))
    local ok, data = pcall(gallery.get_image_gallery_admin_data, limit)
    if not ok then return 503, { detail = "Image Gallery database unavailable: " .. tostring(data) } end
    audit.record_audit_log(a.username, "image_gallery_export_media_csv", { target_type = "gallery_media" })
    return csv_export.csv_response(data.media, "image_gallery_media")
  end)

  httpd.route("GET", "/api/image-gallery/admin/users/export.csv", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 200, 500))
    local ok, data = pcall(gallery.get_image_gallery_admin_data, limit)
    if not ok then return 503, { detail = "Image Gallery database unavailable: " .. tostring(data) } end
    audit.record_audit_log(a.username, "image_gallery_export_users_csv", { target_type = "gallery_user" })
    -- csv_export.lua's rows_to_csv_text already strips any *password* column,
    -- matching the extra explicit filter the Python route applies on top.
    return csv_export.csv_response(data.users, "image_gallery_users")
  end)

  httpd.route("GET", "/api/image-gallery/tables", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local ok, tables = pcall(admin_browser.list_tables, "image_gallery")
    if not ok then return 503, { detail = "Image Gallery database unavailable: " .. tostring(tables) } end
    return 200, { ok = true, schema = "image_gallery", tables = tables }
  end)

  httpd.route("GET", "/api/image-gallery/table-data", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 100, 500))
    local ok, data = pcall(admin_browser.get_table_data, "image_gallery", req.query.table_name, limit)
    if not ok then return 400, { detail = tostring(data):gsub("^.-:%d+:%s*", "") } end
    return 200, { ok = true, data = data, rows = data.rows, count = data.count }
  end)

  httpd.route("POST", "/api/image-gallery/users/delete", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    gallery.delete_image_gallery_user(body.user_id)
    audit.record_audit_log(a.username, "image_gallery_delete_user", { target_type = "gallery_user", target_id = body.user_id })
    return 200, { ok = true }
  end)

  httpd.route("POST", "/api/image-gallery/comments/delete", function(req)
    local a, status, err_body = require_gallery_admin_or_moderator(req)
    if not a then return status, err_body end
    local body = req.json or {}
    gallery.delete_image_gallery_comment(body.comment_id)
    audit.record_audit_log(a.username, "image_gallery_delete_comment", { target_type = "gallery_comment", target_id = body.comment_id })
    return 200, { ok = true }
  end)

  httpd.route("POST", "/api/image-gallery/users/reset-password", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local ok, err = pcall(gallery.reset_image_gallery_user_password, body.user_id, body.new_password)
    if not ok then return 400, { detail = tostring(err):gsub("^.-:%d+:%s*", "") } end
    return 200, { ok = true }
  end)

  httpd.route("POST", "/api/image-gallery/users/update", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local updates = {}
    for _, key in ipairs({ "username", "display_name", "email", "public_profile", "show_liked_count", "adult_content_consent" }) do
      if body[key] ~= nil then updates[key] = body[key] end
    end
    local ok, user = pcall(gallery.update_image_gallery_user, body.user_id, updates)
    if not ok then return 400, { detail = tostring(user):gsub("^.-:%d+:%s*", "") } end
    return 200, { ok = true, user = user }
  end)

  httpd.route("POST", "/api/image-gallery/users/email-verified", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local user = gallery.set_image_gallery_email_verified(body.user_id, body.verified and true or false)
    return 200, { ok = true, user = user }
  end)

  httpd.route("POST", "/api/image-gallery/users/age-verified", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local user = gallery.set_image_gallery_age_verified(body.user_id, body.verified and true or false)
    return 200, { ok = true, user = user }
  end)

  httpd.route("POST", "/api/image-gallery/users/resend-verification", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local user = gallery.get_image_gallery_user_admin(body.user_id)
    if not user then return 404, { detail = "Image Gallery user not found" } end
    if not user.email then return 400, { detail = "This Image Gallery user does not have an email address." } end
    if user.email_verified_at then return 200, { ok = true, email_verification_sent = false, already_verified = true } end
    -- Real end-to-end send now (notify.lua's hand-rolled STARTTLS SMTP
    -- client — see its doc comment for why the bundled socket.smtp module
    -- couldn't be reused as-is). The raw token is used both as the ?token=
    -- link parameter and the displayed code, matching gallery.py's
    -- email_token reuse; only its hash is ever persisted.
    local email_token = auth.verification_code()
    gallery.issue_image_gallery_email_verification_token(body.user_id, auth.verification_token_hash(email_token))
    local verify_url = notify.image_gallery_verification_url(settings, email_token)
    local sent, send_err = notify.send_image_gallery_verification_email(settings, user.email, verify_url, email_token)
    if not sent then
      print("[swarmpanel-lua] image gallery verification email send failed: " .. tostring(send_err))
    end
    return 200, { ok = sent and true or false, email_verification_sent = sent and true or false, already_verified = false }
  end)

  httpd.route("POST", "/api/image-gallery/media/update", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local updates = {}
    for _, key in ipairs({ "title", "is_adult", "moderation_status", "moderation_reason" }) do
      if body[key] ~= nil then updates[key] = body[key] end
    end
    local ok, media = pcall(gallery.update_image_gallery_media, body.media_id, updates)
    if not ok then return 400, { detail = tostring(media):gsub("^.-:%d+:%s*", "") } end
    return 200, { ok = true, media = media }
  end)

  httpd.route("POST", "/api/image-gallery/media/delete", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    gallery.delete_image_gallery_media(body.media_id)
    audit.record_audit_log(a.username, "image_gallery_delete_media", { target_type = "gallery_media", target_id = body.media_id })
    return 200, { ok = true }
  end)

  httpd.route("POST", "/api/image-gallery/admin/media/bulk-delete", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local result = run_bulk_op(body.ids, function(id) gallery.delete_image_gallery_media(id) end)
    audit.record_audit_log(a.username, "image_gallery_bulk_delete_media", { target_type = "gallery_media", details = string.format("succeeded_ids=%d failed=%d", #result.succeeded, #result.failed) })
    result.ok = true
    return 200, result
  end)

  httpd.route("POST", "/api/image-gallery/admin/comments/bulk-delete", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local result = run_bulk_op(body.ids, function(id) gallery.delete_image_gallery_comment(id) end)
    audit.record_audit_log(a.username, "image_gallery_bulk_delete_comments", { target_type = "gallery_comment", details = string.format("succeeded_ids=%d failed=%d", #result.succeeded, #result.failed) })
    result.ok = true
    return 200, result
  end)

  httpd.route("POST", "/api/image-gallery/admin/users/bulk-delete", function(req)
    local a, status, err_body = require_image_gallery_owner(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local result = run_bulk_op(body.ids, function(id) gallery.delete_image_gallery_user(id) end)
    audit.record_audit_log(a.username, "image_gallery_bulk_delete_users", { target_type = "gallery_user", details = string.format("succeeded_ids=%d failed=%d", #result.succeeded, #result.failed) })
    result.ok = true
    return 200, result
  end)

  httpd.route("POST", "/api/image-gallery/reports/status", function(req)
    local a, status, err_body = require_gallery_admin_or_moderator(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local ok, err = pcall(gallery.update_image_gallery_report_status, body.report_id, body.status)
    if not ok then return 400, { detail = tostring(err):gsub("^.-:%d+:%s*", "") } end
    return 200, { ok = true }
  end)

  -- ------------------------------------------------------------ monitoring
  -- NOTE: /api/events below only surfaces the two DB-backed event sources
  -- (per-bot *_error_events tables + Aria medic/infra history). The Python
  -- original also merges an in-process `recent_feed_events` ring buffer fed
  -- by push_feed_event() calls scattered across every router's exception
  -- handlers — wiring that throughout this whole rewrite was out of scope
  -- for this pass (see final report).
  httpd.route("GET", "/api/stability", function(req)
    local a, status, err_body = require_auth(req)
    if not a then return status, err_body end
    local rl_status, rl_body = ratelimit.check(("api-read:%s"):format(tostring(a.username or "unknown"):lower()), 60, 60)
    if rl_status then return rl_status, rl_body end
    local ok, data = pcall(metrics.get_stability_snapshot, music_bots)
    if not ok then return 500, { detail = "Stability snapshot unavailable: " .. tostring(data) } end
    return 200, data
  end)

  httpd.route("GET", "/api/metrics", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local ok, data = pcall(metrics.get_metrics_snapshot, music_bots)
    if not ok then
      return 503, { generated_at = os.date("!%Y-%m-%dT%H:%M:%SZ"), db_error = tostring(data), totals = {}, bots = {} }
    end
    return 200, data
  end)

  httpd.route("GET", "/api/metrics/history", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local metric = req.query.metric or "queued_tracks"
    local hours = math.max(1, math.min(tonumber(req.query.hours) or 24, 24 * 30))
    local ok, points = pcall(metrics.get_metrics_history, metric, hours)
    if not ok then return 503, { detail = "Metrics history unavailable: " .. tostring(points) } end
    return 200, { ok = true, metric = metric, hours = hours, points = points }
  end)

  httpd.route("GET", "/api/metrics/anomalies", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local metric = req.query.metric or "queued_tracks"
    local hours = math.max(1, math.min(tonumber(req.query.hours) or 24, 24 * 30))
    local ok, anomalies = pcall(metrics.get_metric_anomalies, metric, hours)
    if not ok then return 503, { detail = "Metric anomalies unavailable: " .. tostring(anomalies) } end
    return 200, { ok = true, metric = metric, hours = hours, anomalies = anomalies }
  end)

  httpd.route("GET", "/api/events", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local bounded_limit = math.max(1, math.min(tonumber(req.query.limit) or 50, 100))
    local ok1, bot_error_events = pcall(metrics.get_recent_bot_error_events, music_bots, bounded_limit)
    if not ok1 then bot_error_events = {} end
    local ok2, aria_medic_events = pcall(metrics.get_recent_aria_medic_events, math.max(5, math.floor(bounded_limit / 2)))
    if not ok2 then aria_medic_events = {} end
    local combined = {}
    for _, e in ipairs(bot_error_events) do combined[#combined + 1] = e end
    for _, e in ipairs(aria_medic_events) do combined[#combined + 1] = e end
    table.sort(combined, function(x, y) return tostring(x.timestamp) < tostring(y.timestamp) end)
    local seen, deduped = {}, {}
    for _, event in ipairs(combined) do
      local key = table.concat({
        tostring(event.timestamp or ""), tostring(event.source or ""),
        tostring(event.title or ""), tostring(event.description or ""), tostring(event.type or "feed_event"),
      }, "\0")
      if not seen[key] then
        seen[key] = true
        deduped[#deduped + 1] = event
      end
    end
    local out = {}
    local start_idx = math.max(1, #deduped - bounded_limit + 1)
    for i = start_idx, #deduped do out[#out + 1] = deduped[i] end
    return 200, { events = out }
  end)

  -- ------------------------------------------------------------ lumisound
  httpd.route("GET", "/api/lumisound/admin", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 50, 200))
    local ok, data = pcall(lumisound.get_lumisound_admin_data, settings, limit)
    if not ok then return 503, { detail = "Lumisound database unavailable: " .. tostring(data) } end
    return 200, { ok = true, data = data }
  end)

  httpd.route("POST", "/api/lumisound/uploads/delete", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local body = req.json or {}
    lumisound.delete_lumisound_upload(settings, body.upload_id)
    audit.record_audit_log(a.username, "lumisound_delete_upload", { target_type = "lumisound_upload", target_id = body.upload_id })
    return 200, { ok = true }
  end)

  httpd.route("POST", "/api/lumisound/users/active", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local user = lumisound.set_lumisound_user_active(settings, body.user_id, body.active and true or false)
    if not user then return 404, { detail = "Lumisound user not found" } end
    audit.record_audit_log(a.username, body.active and "lumisound_user_active" or "lumisound_user_suspend", { target_type = "lumisound_user", target_id = body.user_id })
    return 200, { ok = true, user = user }
  end)

  httpd.route("POST", "/api/lumisound/bug-reports/status", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local ok, err = pcall(lumisound.update_lumisound_bug_report_status, settings, body.report_id, body.status)
    if not ok then return 400, { detail = tostring(err):gsub("^.-:%d+:%s*", "") } end
    audit.record_audit_log(a.username, "lumisound_bug_report_status", { target_type = "lumisound_bug_report", target_id = body.report_id, details = body.status })
    return 200, { ok = true }
  end)

  -- ----------------------------------------------------------- databases
  httpd.route("GET", "/api/databases", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local ok, schemas = pcall(admin_browser.list_schemas)
    if not ok then return 503, { detail = "Database unavailable: " .. tostring(schemas) } end
    if tostring(req.query.include_tables or "false"):lower() ~= "true" then
      return 200, { schemas = schemas }
    end
    local output = {}
    for _, schema in ipairs(schemas) do
      local ok2, tables = pcall(admin_browser.list_tables, schema)
      output[#output + 1] = { schema = schema, tables = ok2 and tables or {} }
    end
    return 200, { schemas = output }
  end)

  httpd.route("GET", "/api/databases/:schema/tables", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local ok, tables = pcall(admin_browser.list_tables, req.params.schema)
    if not ok then return 503, { detail = "Database unavailable: " .. tostring(tables) } end
    return 200, { schema = req.params.schema, tables = tables }
  end)

  httpd.route("POST", "/api/database/truncate-table", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local expected = string.format("TRUNCATE %s.%s", tostring(body.schema_name), tostring(body.table_name))
    if tostring(body.confirm_text or ""):match("^%s*(.-)%s*$") ~= expected then
      return 400, { detail = "Confirmation mismatch. Expected exact text: " .. expected }
    end
    local expected_owner = tostring(settings.destructive_confirmation_phrase or ""):match("^%s*(.-)%s*$")
    if expected_owner ~= "" and tostring(body.owner_confirm_text or ""):match("^%s*(.-)%s*$") ~= expected_owner then
      return 400, { detail = "Owner confirmation mismatch. Expected exact text: " .. expected_owner }
    end
    local ok, err = pcall(admin_browser.truncate_table, body.schema_name, body.table_name)
    if not ok then return 503, { detail = "Database unavailable: " .. tostring(err) } end
    audit.record_audit_log(a.username, "truncate_table", { target_type = "table", target_id = tostring(body.schema_name) .. "." .. tostring(body.table_name) })
    return 200, { ok = true, message = "Truncated " .. tostring(body.schema_name) .. "." .. tostring(body.table_name) }
  end)

  httpd.route("POST", "/api/database/truncate-schema", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local body = req.json or {}
    local expected = "TRUNCATE ALL " .. tostring(body.schema_name)
    if tostring(body.confirm_text or ""):match("^%s*(.-)%s*$") ~= expected then
      return 400, { detail = "Confirmation mismatch. Expected exact text: " .. expected }
    end
    local expected_owner = tostring(settings.destructive_confirmation_phrase or ""):match("^%s*(.-)%s*$")
    if expected_owner ~= "" and tostring(body.owner_confirm_text or ""):match("^%s*(.-)%s*$") ~= expected_owner then
      return 400, { detail = "Owner confirmation mismatch. Expected exact text: " .. expected_owner }
    end
    local ok, result = pcall(admin_browser.truncate_schema, body.schema_name)
    if not ok then return 503, { detail = "Database unavailable: " .. tostring(result) } end
    audit.record_audit_log(a.username, "truncate_schema", { target_type = "schema", target_id = body.schema_name, details = "truncated_tables=" .. tostring(result.truncated_tables) })
    result.ok = true
    return 200, result
  end)

  httpd.route("GET", "/api/database/data", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 100, 500))
    local ok, data = pcall(admin_browser.get_table_data, req.query.schema_name, req.query.table_name, limit)
    if not ok then return 400, { detail = tostring(data):gsub("^.-:%d+:%s*", "") } end
    if tostring(req.query.format or "json"):lower() == "csv" then
      audit.record_audit_log(a.username, "database_export_csv", { target_type = "table", target_id = tostring(req.query.schema_name) .. "." .. tostring(req.query.table_name) })
      return csv_export.csv_response(data.rows, tostring(req.query.schema_name) .. "_" .. tostring(req.query.table_name))
    end
    return 200, { ok = true, data = data, rows = data.rows, count = data.count }
  end)

  -- ------------------------------------------------------------- exports
  -- Port of app/routers/exports.py's read side only — the writer
  -- (app/main.py's _scheduled_export_loop background task) is not ported,
  -- so this will report `enabled` from settings but the directory will
  -- normally be empty unless something else populates it. Path-traversal
  -- guards (date/filename regex + resolved-path containment check) are
  -- ported as-is since that's the actual security-relevant part.
  httpd.route("GET", "/api/exports", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    return 200, build_exports_payload()
  end)

  httpd.route("GET", "/api/exports/:date/:filename", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local date, filename = req.params.date, req.params.filename
    if not date:match("^%d%d%d%d%-%d%d%-%d%d$") or not filename:match("^[%w_.%-]+%.csv$") then
      return 400, { detail = "Invalid export path" }
    end
    local path = EXPORTS_DIR .. "/" .. date .. "/" .. filename
    local f = io.open(path, "rb")
    if not f then return 404, { detail = "Export file not found" } end
    local content = f:read("*a")
    f:close()
    return 200, content, {
      ["Content-Type"] = "text/csv",
      ["Content-Disposition"] = string.format('attachment; filename="%s_%s"', date, filename),
    }
  end)

  -- ------------------------------------------------------------ audit-log
  httpd.route("GET", "/api/audit-log", function(req)
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end
    local limit = math.max(1, math.min(tonumber(req.query.limit) or 100, 500))
    local offset = math.max(0, tonumber(req.query.offset) or 0)
    local action_filter = tostring(req.query.action or ""):sub(1, 80)
    if action_filter == "" then action_filter = nil end
    local ok, data = pcall(audit.list_audit_log, limit, offset, action_filter)
    if not ok then return 503, { detail = "Audit log unavailable: " .. tostring(data) } end
    return 200, { ok = true, data = data }
  end)

  local REVERTIBLE_ACTIONS = {
    swarm_account_update = function(target_id, before)
      local filtered = {}
      for _, key in ipairs({ "username", "guild_id", "email", "display_name", "public_profile", "server_name" }) do
        if before[key] ~= nil then filtered[key] = before[key] end
      end
      return accounts.update_account_admin(target_id, filtered)
    end,
    alert_rule_update = function(target_id, before)
      local filtered = {}
      for _, key in ipairs({ "rule_type", "threshold_minutes", "enabled", "escalation_minutes", "escalate_email" }) do
        if before[key] ~= nil then filtered[key] = before[key] end
      end
      return alerts.update_alert_rule(target_id, filtered)
    end,
  }

  httpd.route("POST", "/api/audit-log/:entry_id/revert", function(req)
    local a, status, err_body = require_admin(req)
    if not a then return status, err_body end
    local entry = audit.get_audit_log_entry(req.params.entry_id)
    if not entry then return 404, { detail = "Audit log entry not found" } end
    local action = tostring(entry.action or "")
    local revert_fn = REVERTIBLE_ACTIONS[action]
    if not revert_fn then return 400, { detail = "Action '" .. action .. "' cannot be reverted." } end
    local cjson_safe = require("cjson.safe")
    local decoded = entry.details and cjson_safe.decode(entry.details)
    local before = decoded and decoded.before
    if not before then return 400, { detail = "This entry has no recorded before-state to revert to." } end
    local ok, result, err = pcall(revert_fn, entry.target_id, before)
    if not ok then return 400, { detail = tostring(result):gsub("^.-:%d+:%s*", "") } end
    if err then return 400, { detail = err } end
    if not result then return 404, { detail = "Revert target no longer exists." } end
    audit.record_audit_log(a.username, action .. "_revert", { target_type = entry.target_type, target_id = entry.target_id, details = "reverted_from_entry=" .. tostring(entry.id) })
    return 200, { ok = true, result = result }
  end)

  -- ---------------------------------------------------------------------
  -- "My Other Projects" tab: Lumisound's latest .ipa, proxied from its
  -- private GitHub repo. Browsers can't hit a private repo's release asset
  -- directly (needs an Authorization header GitHub's redirect won't carry),
  -- so this shells out to `gh`, which is already authenticated as this
  -- box's owner -- the token itself is never read/stored/handled here.
  -- Same detached-background-job + bounded-poll shape as other long-running
  -- work in this file, so a slow GitHub fetch yields instead of blocking
  -- the single-threaded server.
  -- ---------------------------------------------------------------------
  local LUMISOUND_REPO = "HeavenlyXenusVR/Lumisound"
  local LUMISOUND_POLL_SECONDS = 20
  local LUMISOUND_CACHE_DIR = "./.runtime/project_downloads"

  local function shq(s)
    return "'" .. tostring(s):gsub("'", "'\\''") .. "'"
  end

  local function lumisound_latest_tag()
    local handle = io.popen(
      "env -i HOME=" .. shq(os.getenv("HOME") or "") .. " PATH=" .. shq(os.getenv("PATH") or "")
        .. " GH_TOKEN=" .. shq(os.getenv("GH_TOKEN") or "")
        .. " gh release list --repo " .. shq(LUMISOUND_REPO)
        .. " --limit 1 --json tagName -q '.[0].tagName' 2>/dev/null"
    )
    if not handle then return nil end
    local tag = handle:read("*a")
    handle:close()
    tag = tag and tag:gsub("%s+$", "") or ""
    return tag ~= "" and tag or nil
  end

  local function lumisound_safe_tag(tag)
    return (tag:gsub("[^%w%.%-]", "_"))
  end

  httpd.route("GET", "/api/projects/lumisound/download", function(req)
    -- Gated to admin/moderator (not just any registered guild account):
    -- this triggers a server-side `gh release download` against a private
    -- repo using this box's GH_TOKEN, and the only prior anti-abuse control
    -- was a weak filesystem ".pending"-marker dedup with no real rate limit
    -- and no privilege check -- any registered account could repeatedly
    -- trigger GitHub API/bandwidth usage. Rate-limited too, same
    -- ratelimit.lua pattern used by the auth routes above.
    local a, status, err_body = require_admin_or_moderator(req)
    if not a then return status, err_body end

    local rl_status, rl_body = ratelimit.check(
      ("lumisound-download:%s"):format(tostring(a.username or "unknown"):lower()), 5, 3600)
    if rl_status then return rl_status, rl_body end

    local tag = lumisound_latest_tag()
    if not tag then return 502, { detail = "Could not reach GitHub to check the latest Lumisound release. Try again shortly." } end

    os.execute("mkdir -p " .. shq(LUMISOUND_CACHE_DIR))
    local safe_tag = lumisound_safe_tag(tag)
    local final_path = LUMISOUND_CACHE_DIR .. "/lumisound_" .. safe_tag .. ".ipa"
    local filename = "Lumisound-" .. safe_tag .. ".ipa"

    local function serve(path)
      local f = io.open(path, "rb")
      if not f then return nil end
      local content = f:read("*a")
      f:close()
      return content
    end

    local existing = serve(final_path)
    if existing then
      return 200, existing, {
        ["Content-Type"] = "application/octet-stream",
        ["Content-Disposition"] = 'attachment; filename="' .. filename .. '"',
        ["Cache-Control"] = "private, max-age=0, no-cache",
      }
    end

    -- Filesystem-based ".pending" dedup: a concurrent second visitor hitting
    -- this while the first fetch is still in flight waits on the same
    -- fetch rather than kicking off a redundant one.
    local pending_marker = final_path .. ".pending"
    local pf = io.open(pending_marker, "rb")
    local pending_age = pf and tonumber(pf:read("*a")) or nil
    if pf then pf:close() end
    local still_fetching = pending_age and (os.time() - pending_age) < LUMISOUND_POLL_SECONDS

    if not still_fetching then
      local marker = assert(io.open(pending_marker, "wb"))
      marker:write(tostring(os.time()))
      marker:close()

      local tmp_path = final_path .. ".tmp." .. tostring(math.random(100000, 999999))
      local cmd = string.format(
        "( env -i HOME=%s PATH=%s GH_TOKEN=%s gh release download %s --repo %s --pattern '*.ipa' -O %s --clobber "
          .. "&& mv -f %s %s; rm -f %s %s ) </dev/null >/dev/null 2>&1 &",
        shq(os.getenv("HOME") or ""), shq(os.getenv("PATH") or ""), shq(os.getenv("GH_TOKEN") or ""),
        shq(tag), shq(LUMISOUND_REPO), shq(tmp_path),
        shq(tmp_path), shq(final_path),
        shq(tmp_path), shq(pending_marker)
      )
      os.execute(cmd)
    end

    local waited = 0
    while waited < LUMISOUND_POLL_SECONDS do
      local content = serve(final_path)
      if content then
        return 200, content, {
          ["Content-Type"] = "application/octet-stream",
          ["Content-Disposition"] = 'attachment; filename="' .. filename .. '"',
          ["Cache-Control"] = "private, max-age=0, no-cache",
        }
      end
      copas.sleep(0.5)
      waited = waited + 0.5
    end
    return 503, { detail = "Still fetching the latest Lumisound build from GitHub -- try again in a moment." }
  end)
end

return M
