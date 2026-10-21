-- Server-rendered HTML helpers for the Lua-only SwarmPanel UI (replaces the
-- React/Vite frontend). No templating engine is vendored (see the backend
-- audit) so this is plain Lua string-building with an explicit escaping
-- helper -- every value interpolated into HTML from user/DB data MUST go
-- through esc()/attr() or it's an XSS hole.

local M = {}
local colorutil = require("colorutil")

local function cjson_encode_safe(v)
  local ok, cjson = pcall(require, "cjson.safe")
  if not ok then return tostring(v) end
  local encoded = cjson.encode(v)
  return encoded or tostring(v)
end

local ESCAPE_MAP = {
  ["&"] = "&amp;", ["<"] = "&lt;", [">"] = "&gt;",
  ['"'] = "&quot;", ["'"] = "&#39;",
}

-- HTML-escapes a value for use in element text content or (quoted)
-- attribute values. nil/false/number all coerce to string first.
function M.esc(v)
  if v == nil then return "" end
  if type(v) == "boolean" then v = tostring(v) end
  return (tostring(v):gsub('[&<>"\']', ESCAPE_MAP))
end

-- Builds a double-quoted, escaped HTML attribute: attr("href", url) -> ' href="..."'
-- Returns "" (not even a space) when value is nil/false/empty, so optional
-- attributes can be spliced in without conditional string-building at call
-- sites: ("<a" .. attr("href", url) .. ">")
function M.attr(name, value)
  if value == nil or value == false or value == "" then return "" end
  return (" %s=\"%s\""):format(name, M.esc(value))
end

-- Boolean HTML attribute (checked/disabled/open/...): only emitted when truthy.
function M.battr(name, truthy)
  if not truthy then return "" end
  return " " .. name
end

-- Joins an array of HTML-string fragments with no separator -- the common
-- case for building a list of rendered rows/cards.
function M.join(parts)
  return table.concat(parts, "")
end

-- classNames-style helper: html.cls({"nav-item", active and "active"}) -> "nav-item active"
function M.cls(parts)
  local out = {}
  for _, p in ipairs(parts) do
    if p and p ~= "" then out[#out + 1] = p end
  end
  return table.concat(out, " ")
end

-- ---------------------------------------------------------------------------
-- Navigation rendering. The section/page model itself lives in nav.lua;
-- everything here only turns it into markup for the four places it shows
-- up: the desktop sidebar, the mobile bottom bar, the mobile/collapsed
-- drawer, and the per-section tab strip above each screen.
-- ---------------------------------------------------------------------------

local nav = require("nav")
local static_assets = require("static")

-- The mobile bottom bar only has room for ~5 icons + a "More" launcher --
-- rendering every screen there breaks the CSS's fixed 6-column layout into
-- a multi-row block that eats half the viewport.
local MOBILE_PRIMARY = { "/", "/controls", "/users", "/messages", "/profile" }

-- Empty count bubble app.js fills from the community_counts live key.
-- Only rendered for guild-bound accounts (the key needs an account id --
-- same rule as the notifications bell in layout()).
local function badge_slot(name, session)
  if not name or not (session and session.guild_id) then return "" end
  return ('<span class="nav-badge" data-badge="%s" hidden></span>'):format(M.esc(name))
end

local function nav_link(item, pathname, class_name, session)
  -- title= gives the icon-only rail (compact sidebar / tablet widths) a
  -- hover label.
  return ('<a class="%s" href="%s" title="%s"%s><span class="nav-glyph">%s</span><span class="nav-label">%s</span>%s</a>'):format(
    M.cls({ class_name, nav.is_active(pathname, item.to) and "active" or "" }),
    M.esc(item.to), M.esc(item.label),
    nav.is_active(pathname, item.to) and ' aria-current="page"' or "",
    item.glyph, M.esc(item.label), badge_slot(item.badge, session))
end

-- Grouped section list, shared by the desktop sidebar and the drawer.
local function render_sections(session, pathname, link_class)
  local out = {}
  for _, section in ipairs(nav.visible_sections(session)) do
    local links = {}
    for _, item in ipairs(section.items) do links[#links + 1] = nav_link(item, pathname, link_class, session) end
    local current = nav.locate(pathname)
    out[#out + 1] = ([[<div class="%s"><p class="nav-group-label"><span class="nav-glyph">%s</span><span class="nav-label">%s</span>%s</p>%s</div>]]):format(
      M.cls({ "nav-group", (current and current.key == section.key) and "current" or "" }),
      section.glyph, M.esc(section.label), badge_slot(section.badge, session), table.concat(links, ""))
  end
  return table.concat(out, "")
end

local function render_mobile_primary(session, pathname)
  local out = {}
  for _, to in ipairs(MOBILE_PRIMARY) do
    local _, item = nav.locate(to)
    if item and nav.can_see(session, item) then out[#out + 1] = nav_link(item, pathname, "nav-item mobile-primary", session) end
  end
  return table.concat(out, "")
end

-- "Fleet > Controls" -- the section links to its first visible screen. A
-- path deeper than the matched screen (/users/42) gets the page title as a
-- third crumb.
local function render_breadcrumb(session, pathname, title)
  local section, item = nav.locate(pathname)
  if not section or not item or not session.authenticated then return '<div class="crumbs"></div>' end
  local visible = nav.visible_section(session, section.key)
  local section_href = (visible and visible.items[1].to) or item.to
  local crumbs = {
    ('<a href="%s">%s</a>'):format(M.esc(section_href), M.esc(section.label)),
  }
  local canonical = pathname == item.to or (item.to == "/" and pathname == "/dashboard")
  if canonical then
    crumbs[#crumbs + 1] = ('<strong aria-current="page">%s</strong>'):format(M.esc(item.label))
  else
    crumbs[#crumbs + 1] = ('<a href="%s">%s</a>'):format(M.esc(item.to), M.esc(item.label))
    crumbs[#crumbs + 1] = ('<strong aria-current="page">%s</strong>'):format(M.esc(title or ""))
  end
  return '<nav class="crumbs" aria-label="Breadcrumb">' .. table.concat(crumbs, '<span class="crumb-sep" aria-hidden="true">/</span>') .. "</nav>"
end

-- Sibling screens of the current section, shown above the page so related
-- screens (Directory / Friends / Messages) read as one area of the panel.
-- Skipped for single-screen sections.
local function render_section_tabs(session, pathname)
  local section = nav.locate(pathname)
  if not section or not session.authenticated then return "" end
  local visible = nav.visible_section(session, section.key)
  if not visible or #visible.items < 2 then return "" end
  local links = {}
  for _, item in ipairs(visible.items) do links[#links + 1] = nav_link(item, pathname, "nav-item section-tab", session) end
  return ('<nav class="section-tabs" aria-label="%s screens">%s</nav>'):format(M.esc(visible.label), table.concat(links, ""))
end

M.render_section_tabs = render_section_tabs

-- ---------------------------------------------------------------------------
-- Appearance / theming (mirrors frontend/src/config.js DEFAULT_PREFERENCES
-- and utils/control.js panelClassName()/panelStyle()) -- CSS was ported
-- verbatim to static/app.css, so it still expects this exact combinatorial
-- class-name system on the root wrapper.
-- ---------------------------------------------------------------------------

M.DEFAULT_PREFERENCES = {
  theme_mode = "dark", accent_color = "#89b4fa", accent_secondary = "",
  background_mode = "default", layout_mode = "standard", density = "comfortable",
  card_shape = "soft", font_scale = "normal", motion = "standard",
  operator_layout = "command", roster_layout = "cards", tab_style = "rail",
  stream_card_style = "telemetry", dashboard_density = "command",
  card_hover_effect = "lift", notification_position = "br",
  surface_opacity = 0.92, surface_blur = 18,
}

local function choice(prefs, key, fallback)
  local v = prefs and prefs[key]
  if v == nil or v == "" then return fallback end
  return v
end

function M.panel_class(prefs)
  prefs = prefs or M.DEFAULT_PREFERENCES
  local notify_pos = choice(prefs, "notification_position", "br")
  return M.cls({
    "app-shell",
    "panel-theme-" .. choice(prefs, "theme_mode", "dark"),
    "panel-bg-" .. choice(prefs, "background_mode", "default"),
    "panel-layout-" .. choice(prefs, "layout_mode", "standard"),
    "panel-density-" .. choice(prefs, "density", "comfortable"),
    "panel-shape-" .. choice(prefs, "card_shape", "soft"),
    "panel-font-" .. choice(prefs, "font_scale", "normal"),
    "panel-motion-" .. choice(prefs, "motion", "standard"),
    "panel-operator-" .. choice(prefs, "operator_layout", "command"),
    "panel-roster-" .. choice(prefs, "roster_layout", "cards"),
    "panel-tabs-" .. choice(prefs, "tab_style", "rail"),
    "panel-stream-" .. choice(prefs, "stream_card_style", "telemetry"),
    "panel-dashboard-" .. choice(prefs, "dashboard_density", "command"),
    "panel-hover-" .. choice(prefs, "card_hover_effect", "lift"),
    -- App.jsx applied notification_position as a SEPARATE class from
    -- panelClassName()'s output (only added when non-default) -- matches
    -- that exactly rather than folding it into a "panel-notify-br" no-op.
    -- NOTE: every entry here must be a string (never bare `nil`) -- M.cls
    -- iterates with ipairs(), which stops dead at the first hole in the
    -- array, silently dropping every class listed after it. "" is the
    -- correct "omit this one" value; M.cls already filters empty strings.
    (notify_pos ~= "br") and ("panel-notify-" .. notify_pos) or "",
    -- The following were saved/round-tripped by profiles.lua's
    -- clean_panel_preferences and editable on /appearance, but had NO
    -- class wiring anywhere -- not even in the original pre-rewrite React
    -- app (verified via git history) -- so changing them never did
    -- anything. Giving them real, if new, effect per the corresponding
    -- CSS added below.
    "panel-radius-" .. choice(prefs, "panel_radius", "medium"),
    "panel-fontfamily-" .. choice(prefs, "panel_font_family", "system"),
    "panel-nav-" .. choice(prefs, "sidebar_style", "full"),
    "panel-detail-" .. choice(prefs, "bot_card_detail", "full"),
    (prefs.show_bot_uptime == false) and "panel-hide-uptime" or "",
    (prefs.show_queue_pressure == false) and "panel-hide-queue-pressure" or "",
    (prefs.compact_sidebar == true) and "panel-compact-nav" or "",
  })
end

local BACKGROUND_PRESETS = {
  default = "#0d1117", midnight = "#090b12", aurora = "#101821", ember = "#17100d",
}

local function safe_hex(value, fallback)
  if type(value) == "string" and value:match("^#%x%x%x%x%x%x$") then return value end
  return fallback
end

local function clamp_number(value, fallback, min, max)
  local n = tonumber(value)
  if not n then return fallback end
  if n < min then return min end
  if n > max then return max end
  return n
end

local function css_url(value)
  -- Matches utils/control.js's safeImage()/panelStyle() -- only http(s)
  -- URLs are trusted into a CSS url(); escapes quotes/backslashes so a
  -- crafted background_image_url can't break out of the string literal.
  local text = tostring(value or ""):match("^%s*(.-)%s*$")
  if not text:match("^https?://") then return "none" end
  return ('url("%s")'):format(text:gsub('([\\"])', '\\%1'))
end

-- Was only setting --surface-opacity/--surface-blur/--accent -- --bg (the
-- background_mode preset/custom_color) and --panel-bg-image
-- (background_image_url, custom_image mode) are both referenced by
-- app.css's .app-shell::before/.panel-bg-* rules but were never actually
-- set anywhere, so background_mode="custom_image" plus a saved image URL
-- silently did nothing. Mirrors utils/control.js's panelStyle() exactly.
function M.panel_style(prefs)
  prefs = prefs or M.DEFAULT_PREFERENCES
  local mode = choice(prefs, "background_mode", "default")
  local base_bg = (mode == "custom_color") and safe_hex(prefs.background_color, "#0b0e18")
    or (BACKGROUND_PRESETS[mode] or BACKGROUND_PRESETS.default)
  local use_image = (mode == "custom_image") and tostring(prefs.background_image_url or ""):match("^%s*https?://")
  local accent = safe_hex(prefs.accent_color, "#89b4fa")
  local parts = {
    ("--accent:%s"):format(accent),
    ("--bg:%s"):format(base_bg),
    ("--panel-bg-image:%s"):format(use_image and css_url(prefs.background_image_url) or "none"),
    ("--surface-opacity:%s"):format(tostring(clamp_number(prefs.surface_opacity, 0.92, 0.35, 1))),
    ("--surface-blur:%spx"):format(tostring(clamp_number(prefs.surface_blur, 18, 0, 36))),
  }
  -- BUGFIX: accent_contrast_text/accent_gradient were only ever computed by
  -- routes.lua's with_derived_accent() for API JSON responses (GET/POST
  -- /api/users/preferences) -- never for an actual page render, which reads
  -- straight off accounts.get_panel_preferences() and never calls that
  -- helper. --accent-contrast-text (app.css) and --accent-gradient (the
  -- .primary button background above) therefore never had real values on
  -- any rendered page, and "Accent secondary" -- which only feeds the
  -- gradient -- did nothing visible anywhere. Compute both directly here so
  -- every page render gets them regardless of what's stored, mirroring
  -- with_derived_accent's own accent_secondary-or-auto_secondary fallback.
  local secondary = prefs.accent_secondary
  if not secondary or secondary == "" then secondary = colorutil.auto_secondary(accent) end
  parts[#parts + 1] = "--accent-contrast-text:" .. colorutil.contrast_text(accent)
  parts[#parts + 1] = "--accent-gradient:" .. colorutil.gradient(accent, secondary)
  -- BUGFIX: profile_backdrop_image_url/profile_backdrop_strength ("Profile
  -- backdrop" on /appearance) are stored in the VIEWER's own panel
  -- preferences, not on any individual profile -- app.css's
  -- .public-profile-hero/.profile-stat-panel/.form-panel rules already read
  -- --profile-backdrop/--profile-backdrop-strength expecting exactly that
  -- (a personal skin applied to every profile-shaped surface you look at,
  -- yours or someone else's), but nothing ever set those two vars anywhere.
  -- Root-level here (inherits via the .app-shell wrapper to every page,
  -- same as --accent/--surface-opacity above) rather than duplicated into
  -- each page that happens to render a profile card.
  local backdrop_url = tostring(prefs.profile_backdrop_image_url or ""):match("^%s*(.-)%s*$")
  parts[#parts + 1] = "--profile-backdrop:" .. ((backdrop_url ~= "" and backdrop_url:match("^https?://")) and css_url(backdrop_url) or "none")
  parts[#parts + 1] = "--profile-backdrop-strength:" .. tostring(clamp_number(prefs.profile_backdrop_strength, 0.18, 0, 0.55) * 100) .. "%"
  return table.concat(parts, ";")
end

-- ---------------------------------------------------------------------------
-- Page shell (mirrors Shell.jsx's topbar/nav/footer chrome)
-- ---------------------------------------------------------------------------

-- opts: { title, path, session (from get_auth-shaped table or nil),
--         body (HTML string), head_extra (HTML string, optional), token }
-- session.username/admin_mode/site_owner/moderator/image_gallery_owner drive
-- the nav + session-bar exactly like ctx.isAdmin/isOwner/etc in Shell.jsx.
-- `token` is only used to seed window.SWARM_TOKEN for the dashboard
-- WebSocket's existing {type:"auth", token} first-frame protocol (see
-- routes.lua's /ws handler) -- it is never rendered anywhere visible and
-- never sent anywhere except that one same-origin WS connection.
function M.layout(opts)
  local session = opts.session or { authenticated = false }
  local authed = session.authenticated
  local username = session.username or "Operator"
  local initial = username:sub(1, 1):upper()

  -- BUGFIX: gated on `authed` alone -- the bare env-configured admin login
  -- (settings.admin_username/admin_password, no `users` table row, no
  -- guild_id at all -- see routes.lua's /api/login) is `authed` but has no
  -- account_id that /api/notifications/* can ever resolve (they all call
  -- account_id_for_auth(), which requires a.guild_id). That made this bell
  -- poll /api/notifications/unread-count every 30s FOREVER for that account
  -- type, 403ing every single time -- confirmed live, this is what surfaced
  -- as a recurring 403 in the network tab. Notifications, like Friends, are
  -- inherently tied to a guild account; only render/poll for sessions that
  -- have one.
  local notif_bell = authed and session.guild_id and [[
      <div class="notifications-bell">
        <button class="icon-button" type="button" id="notif-bell-btn" title="Notifications" aria-haspopup="true" aria-expanded="false">
          &#128276;<span class="notifications-badge" id="notif-badge" hidden>0</span>
        </button>
        <div class="notifications-backdrop" id="notif-backdrop" hidden></div>
        <div class="notifications-dropdown liquid-glass" id="notif-dropdown" hidden>
          <div class="notifications-dropdown-head">
            <strong>Notifications</strong>
            <button type="button" id="notif-mark-all">Mark all read</button>
          </div>
          <ul class="notifications-list" id="notif-list"></ul>
        </div>
      </div>
    ]] or ""

  local session_bar
  if authed then
    session_bar = ([[
      <span class="mode-pill%s">%s</span>
      %s
      %s
      <a class="profile-link" href="/profile"><span class="profile-link-badge">%s</span><span>%s</span></a>
      <form method="POST" action="/logout" class="logout-form"><button class="icon-button" type="submit" title="Logout">&#8677;</button></form>
    ]]):format(
      session.admin_mode and " admin" or "", session.admin_mode and "Admin" or "User",
      session.site_owner and ([[<button class="mode-toggle%s" type="button" data-admin-toggle="%s">%s</button>]]):format(
        session.admin_mode and " active" or "", session.admin_mode and "0" or "1",
        session.admin_mode and "Admin On" or "Admin Off") or "",
      notif_bell,
      M.esc(initial), M.esc(username))
  elseif opts.path ~= "/login" then
    session_bar = '<a class="button-link primary" href="/login">&#8676; Login</a>'
  else
    session_bar = ""
  end

  -- Shell structure:
  --   topbar   brand | breadcrumb | session controls
  --   body     sidebar (grouped by section) | stage (section tabs + page)
  --   mobile   bottom bar (5 primary screens + More) and a drawer holding
  --            the full grouped menu -- the same drawer doubles as the
  --            desktop menu when sidebar_style is "hidden".
  local sidebar = ""
  local drawer_button = ""
  local mobile_nav = ""
  local section_tabs = ""
  if authed then
    sidebar = ('<aside class="sidebar" aria-label="Main"><nav class="sidebar-nav">%s</nav></aside>'):format(
      render_sections(session, opts.path, "sidebar-link"))
    drawer_button = '<button class="icon-button nav-drawer-button" type="button" title="Menu" aria-label="Open menu" data-mobile-nav-toggle>&#9776;</button>'
    section_tabs = render_section_tabs(session, opts.path)
    mobile_nav = ([[
      <nav class="nav nav-mobile" aria-label="Mobile Main">%s
        <a class="nav-item mobile-nav-launcher" href="#" data-mobile-nav-toggle><span class="nav-glyph">&#9776;</span><span class="nav-label">More</span></a>
      </nav>
      <div class="mobile-nav-backdrop" data-mobile-nav-backdrop></div>
      <aside class="mobile-nav-sheet" data-mobile-nav-sheet aria-label="Menu">
        <div class="mobile-nav-sheet-head">
          <div><strong>SwarmPanel</strong><p>Every screen, by section.</p></div>
          <button type="button" data-mobile-nav-close>Close</button>
        </div>
        <div class="mobile-session-chip"><span class="mode-pill%s">%s</span><span>%s</span></div>
        <nav class="mobile-nav-list" aria-label="All screens">%s</nav>
      </aside>
    ]]):format(render_mobile_primary(session, opts.path),
      session.admin_mode and " admin" or "", session.admin_mode and "Admin" or "User", M.esc(username),
      render_sections(session, opts.path, "nav-item"))
  end

  local prefs = opts.preferences or M.DEFAULT_PREFERENCES

  return ([[<!doctype html>
<html lang="en" data-theme="dark">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>%s // SwarmPanel</title>
<link rel="stylesheet" href="]] .. static_assets.url("/static/app.css") .. [[">
<script>window.SWARM_TOKEN = %s; window.SWARM_SESSION = %s;</script>
<script src="]] .. static_assets.url("/static/app.js") .. [["></script>
%s
</head>
<body>
<!-- Liquid Glass edge-refraction filter, shared by every .liquid-glass
     element via backdrop-filter: ... url(#liquid-glass-edge). A static
     fractal-noise displacement (not tied to any element's live content),
     so this single filter def works for the topbar, notification dropdown,
     and dashboard spotlight card alike -- no per-element filter needed. -->
<svg aria-hidden="true" style="position:absolute;width:0;height:0;overflow:hidden">
  <filter id="liquid-glass-edge" x="-20%%" y="-20%%" width="140%%" height="140%%">
    <feTurbulence type="fractalNoise" baseFrequency="0.008 0.012" numOctaves="2" seed="7" result="noise"/>
    <feGaussianBlur in="noise" stdDeviation="2" result="softNoise"/>
    <feDisplacementMap in="SourceGraphic" in2="softNoise" scale="14" xChannelSelector="R" yChannelSelector="G"/>
  </filter>
</svg>
<div class="%s" style="%s">
<a class="skip-link" href="#main">Skip to content</a>
<header class="topbar liquid-glass">
  <div class="topbar-lead">%s<a class="brand" href="/"><span class="brand-mark">&#10022;</span><span class="brand-copy"><strong>SwarmPanel</strong><small>Fleet Command</small></span></a></div>
  %s
  <div class="session-bar">%s</div>
</header>
<div class="%s">
%s
<main class="stage" id="main">%s%s</main>
</div>
%s
<footer class="site-footer">
  <span>SwarmPanel // HollowHeavenXR</span>
  <a href="https://discord.com/users/1304564041863266347" target="_blank" rel="noreferrer">Discord</a>
</footer>
</div>
<div id="toast" class="toast" role="status" aria-live="polite"></div>
</body>
</html>]]):format(
    M.esc(opts.title or "SwarmPanel"),
    opts.token and ('"' .. M.esc(opts.token):gsub('"', '\\"') .. '"') or "null",
    (authed and "true" or "false"),
    opts.head_extra or "",
    M.esc(M.panel_class(prefs)), M.esc(M.panel_style(prefs)),
    drawer_button,
    render_breadcrumb(session, opts.path or "", opts.title),
    session_bar,
    authed and "shell-body" or "shell-body shell-body-public",
    sidebar, section_tabs, opts.body or "", mobile_nav)
end

-- ---------------------------------------------------------------------------
-- Shared component primitives (mirror components/ui.jsx)
-- ---------------------------------------------------------------------------

-- Page(title, eyebrow, lede, actions_html, body_html)
function M.page(opts)
  return ([[
    <div class="page">
      <div class="page-head">
        <div>
          %s
          <h1>%s</h1>
          %s
        </div>
        %s
      </div>
      %s
    </div>
  ]]):format(
    opts.eyebrow and ("<p>" .. M.esc(opts.eyebrow) .. "</p>") or "",
    M.esc(opts.title or ""),
    opts.lede and ('<span class="page-lede">' .. M.esc(opts.lede) .. "</span>") or "",
    -- .page-actions had layout CSS (flex row, mobile full-width) but every
    -- page.head's actions slot was always called with opts.actions unset --
    -- nothing in the codebase ever passed one, so the slot rendered "".
    opts.actions and ('<div class="page-actions">' .. opts.actions .. "</div>") or "",
    opts.body or "")
end

function M.section_head(title, subtitle)
  return ('<div class="section-head"><h2>%s</h2>%s</div>'):format(
    M.esc(title), subtitle and ("<p>" .. M.esc(subtitle) .. "</p>") or "")
end

-- metrics: array of {label=, value=}
function M.metric_grid(metrics)
  local cells = {}
  for _, m in ipairs(metrics) do
    cells[#cells + 1] = ('<div class="metric"><span class="metric-value">%s</span><span class="metric-label">%s</span></div>'):format(
      M.esc(m.value), M.esc(m.label))
  end
  return '<div class="metric-grid">' .. table.concat(cells, "") .. "</div>"
end

function M.notice(kind, text)
  return ('<div class="notice notice-%s">%s</div>'):format(M.esc(kind or "info"), M.esc(text))
end

function M.empty_state(text)
  return ('<div class="empty-state">%s</div>'):format(M.esc(text))
end

-- Shimmer placeholder grid (skeleton-grid/-card had full CSS -- shimmer
-- keyframe, sizing -- but nothing ever rendered it; every async section
-- placeholder was just empty_state("Loading...") plain text).
function M.skeleton_grid(count)
  local cards = {}
  for _ = 1, (count or 6) do cards[#cards + 1] = '<div class="skeleton-card"></div>' end
  return '<div class="skeleton-grid">' .. table.concat(cards, "") .. "</div>"
end

-- Titled loading placeholder for a whole section (section-loading-* had
-- full CSS -- copy block + a shimmer card grid -- but nothing rendered it;
-- sections either had no placeholder at all before their first fetch
-- resolved, or fell back to the plain skeleton_grid() above).
function M.section_loading(title, note, count)
  local cards = {}
  for _ = 1, (count or 3) do cards[#cards + 1] = '<div class="section-loading-card"></div>' end
  return ([[
    <div class="section-loading-screen">
      <div class="section-loading-copy"><strong>%s</strong><p>%s</p></div>
      <div class="section-loading-grid">%s</div>
    </div>
  ]]):format(M.esc(title), M.esc(note or ""), table.concat(cards, ""))
end

-- Generic auto-column table (mirrors swarm.jsx's DataTable). rows: array of
-- plain tables; columns: optional array of column names to force ordering
-- (defaults to the keys of the first row, capped at 9, matching the React
-- version's column cap).
function M.data_table(rows, columns, opts)
  opts = opts or {}
  if not rows or #rows == 0 then return M.empty_state(opts.empty_text or "No rows.") end
  if not columns then
    columns = {}
    local seen = {}
    for k in pairs(rows[1]) do
      if not seen[k] and k ~= "password_hash" and #columns < 9 then
        seen[k] = true
        columns[#columns + 1] = k
      end
    end
    table.sort(columns)
  end
  local thead = {}
  for _, c in ipairs(columns) do thead[#thead + 1] = "<th>" .. M.esc(c) .. "</th>" end
  local tbody = {}
  for _, row in ipairs(rows) do
    local cells = {}
    for _, c in ipairs(columns) do
      local v = row[c]
      if type(v) == "table" then v = cjson_encode_safe(v) end
      cells[#cells + 1] = "<td>" .. M.esc(v) .. "</td>"
    end
    tbody[#tbody + 1] = "<tr>" .. table.concat(cells, "") .. "</tr>"
  end
  return ('<div class="table-wrap"><table class="data-table"><thead><tr>%s</tr></thead><tbody>%s</tbody></table></div>'):format(
    table.concat(thead, ""), table.concat(tbody, ""))
end

return M
