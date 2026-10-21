-- Panel information architecture: every screen belongs to exactly one
-- section, and the sidebar, mobile drawer, breadcrumb, per-section tab
-- strip, and the Admin overview page are all derived from this one table.
-- Adding a screen means adding one entry here -- nothing else needs to
-- know the list.
--
-- `badge` names a live count (app.js fills [data-badge] elements from the
-- community_counts socket key): "messages", "friends", or "community" for
-- their sum on the section heading.
--
-- `when` gates visibility on the session_view() shape (page_kit.lua):
--   nil       -> any signed-in session
--   "admin"   -> admin mode on
--   "mod"     -> admin mode on, or a moderator
--   "gallery" -> image-gallery owner (admin mode + site owner)
--   "staff"   -> any of the above (used for the Admin section's overview)

local M = {}

M.SECTIONS = {
  {
    key = "fleet", label = "Fleet", glyph = "◧",
    blurb = "Live bot status and direct playback control.",
    items = {
      { to = "/", label = "Dashboard", glyph = "◧", blurb = "Live status across the swarm." },
      { to = "/controls", label = "Controls", glyph = "▶", blurb = "Send orders to any bot in any guild." },
      { to = "/invites", label = "Invites", glyph = "🔗", blurb = "Invite links for every bot." },
    },
  },
  {
    key = "insights", label = "Insights", glyph = "📈",
    blurb = "What the swarm is playing and learning.",
    items = {
      { to = "/leaderboard", label = "Leaderboard", glyph = "🏆", blurb = "Top tracks and listeners." },
      { to = "/learning", label = "Learning", glyph = "🧠", blurb = "The recommendation engine's memory." },
    },
  },
  {
    key = "community", label = "Community", glyph = "👥", badge = "community",
    blurb = "Other operators, friends, and direct messages.",
    items = {
      { to = "/users", label = "Directory", glyph = "👥", blurb = "Find other operators." },
      { to = "/friends", label = "Friends", glyph = "🙂", badge = "friends", blurb = "Requests and confirmed friends." },
      { to = "/messages", label = "Messages", glyph = "✉", badge = "messages", blurb = "Direct messages." },
    },
  },
  {
    key = "account", label = "Account", glyph = "◑",
    blurb = "Your profile and how the panel looks to you.",
    items = {
      { to = "/profile", label = "Profile", glyph = "◑", blurb = "How you appear across the panel." },
      { to = "/appearance", label = "Appearance", glyph = "◈", blurb = "Theme, layout, and motion." },
      { to = "/other-projects", label = "Other Projects", glyph = "🚀", blurb = "Other things I've built." },
    },
  },
  {
    key = "admin", label = "Admin", glyph = "⚙",
    blurb = "Owner and moderator tooling.",
    items = {
      { to = "/admin", label = "Overview", glyph = "⚙", when = "staff", blurb = "Every admin tool in one place." },
      { to = "/diagnostics", label = "Diagnostics", glyph = "♥", when = "admin", blurb = "Stability, metrics, alert rules, and exports." },
      { to = "/intel", label = "Intel", glyph = "⚠", when = "admin", blurb = "Trends, anomalies, and raw events." },
      { to = "/bot-telemetry", label = "Bot Telemetry", glyph = "📡", when = "admin", blurb = "Raw diagnostic events from a single bot's own database." },
      { to = "/audit-log", label = "Audit Log", glyph = "☰", when = "mod", blurb = "Every recorded admin action." },
      { to = "/accounts", label = "Accounts", glyph = "☺", when = "admin", blurb = "Recover and manage swarm accounts." },
      { to = "/databases", label = "Databases", glyph = "▤", when = "admin", blurb = "Browse raw schema tables." },
      { to = "/gallery-admin", label = "Gallery", glyph = "▦", when = "gallery", blurb = "Image Gallery users, media, and reports." },
      { to = "/lumisound-admin", label = "Lumisound", glyph = "♪", when = "mod", blurb = "Lumisound iOS app accounts and library." },
    },
  },
}

-- Paths that belong to a section without being a nav entry of their own
-- (the breadcrumb and active-state logic still need to place them).
local ALIASES = {
  ["/dashboard"] = "/",
}

function M.can_see(session, item)
  if not session or not session.authenticated then return false end
  local w = item.when
  if w == nil then return true end
  local admin, mod, gallery = session.admin_mode, session.moderator, session.image_gallery_owner
  if w == "admin" then return admin == true end
  if w == "mod" then return admin == true or mod == true end
  if w == "gallery" then return gallery == true end
  if w == "staff" then return admin == true or mod == true or gallery == true end
  return false
end

-- Sections with only the items this session may see; sections left empty
-- are dropped entirely (a regular user never sees an "Admin" heading).
function M.visible_sections(session)
  local out = {}
  for _, section in ipairs(M.SECTIONS) do
    local items = {}
    for _, item in ipairs(section.items) do
      if M.can_see(session, item) then items[#items + 1] = item end
    end
    if #items > 0 then
      out[#out + 1] = { key = section.key, label = section.label, glyph = section.glyph, badge = section.badge, blurb = section.blurb, items = items }
    end
  end
  return out
end

function M.is_active(pathname, to)
  pathname = ALIASES[pathname] or pathname
  if to == "/" then return pathname == "/" end
  return pathname == to or pathname:sub(1, #to + 1) == to .. "/"
end

-- Returns section, item for a request path (longest matching prefix wins,
-- so /users/42 lands on Directory). nil when the path isn't part of the
-- panel's nav (login, 404s).
function M.locate(pathname)
  if not pathname then return nil end
  local best_section, best_item, best_len = nil, nil, -1
  for _, section in ipairs(M.SECTIONS) do
    for _, item in ipairs(section.items) do
      if M.is_active(pathname, item.to) and #item.to > best_len then
        best_section, best_item, best_len = section, item, #item.to
      end
    end
  end
  return best_section, best_item
end

-- The same section filtered to what this session can see.
function M.visible_section(session, key)
  for _, section in ipairs(M.visible_sections(session)) do
    if section.key == key then return section end
  end
  return nil
end

return M
