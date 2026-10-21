-- Bot Telemetry page (2026-10-12, per operator request): a panel view onto
-- each bot's own <bot>_telemetry_events table (process_queue_started/
-- _skipped_busy/_slow/_error, guild_queue_stuck, voice_link_lost,
-- node_failover, runtime, ... -- whatever categories a given bot actually
-- records; this page doesn't hardcode the list). Distinct from telemetry.lua
-- (that module is SwarmPanel's OWN usage telemetry -- who's using the panel,
-- which routes run) and from /intel (fleet-wide 5-minute aggregate trend
-- sampling, swarm_metrics_history) -- this is the raw per-event stream
-- straight from a single bot's own database, same shape as Leaderboard/
-- Learning in pages_insights.lua.
local httpd = require("httpd")
local html = require("html")
local kit = require("page_kit")

local M = {}

function M.register(cfg)
  local music_bots = cfg.music_bots
  local session_view, page_shell, denied = kit.session_view, kit.page_shell, kit.denied

  httpd.route("GET", "/bot-telemetry", function(req)
    local a, status, headers = cfg.require_auth_page(req)
    if not a then return status, "", headers end
    if not a.admin_mode then return denied(req, a, "/bot-telemetry", "Bot Telemetry") end

    local bot_options = {}
    for _, bot in ipairs(music_bots) do
      bot_options[#bot_options + 1] = ("<option value=\"%s\">%s</option>"):format(html.esc(bot.key), html.esc(bot.display_name))
    end

    local body = html.page({
      title = "Bot Telemetry", eyebrow = "Admin", lede = "Raw diagnostic events straight from a single bot's own database.",
      body = ([[
        <form id="tel-form" class="panel form-panel">
          <label class="field">Bot<select name="bot_key">%s</select></label>
          <label class="field">Category (optional)<input type="text" name="category" placeholder="e.g. runtime"></label>
          <label class="field">Limit<input type="number" name="limit" value="100" min="1" max="1000"></label>
          <button type="submit" class="button-link primary">Load</button>
        </form>
        <div class="metric-grid" id="tel-summary"></div>
        %s
        <div id="tel-results">%s</div>
      ]]):format(html.join(bot_options), html.section_head("Recent Events"),
        html.section_loading("Loading telemetry", "Fetching recent diagnostic events for this bot.", 3)),
    })
    local script = [[
      const telForm = document.getElementById("tel-form");
      function telEsc(s) { return String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c])); }
      async function loadTelemetry() {
        const fd = new FormData(telForm);
        const params = new URLSearchParams();
        params.set("bot_key", fd.get("bot_key"));
        if (fd.get("category")) params.set("category", fd.get("category"));
        if (fd.get("limit")) params.set("limit", fd.get("limit"));
        try {
          const res = await swarmFetch(`/api/bot-telemetry?${params.toString()}`);
          const rows = res.events || [];
          const counts = {};
          rows.forEach((r) => { counts[r.event_name] = (counts[r.event_name] || 0) + 1; });
          const summaryEntries = Object.entries(counts).sort((a, b) => b[1] - a[1]).slice(0, 6);
          document.getElementById("tel-summary").innerHTML = summaryEntries.length
            ? summaryEntries.map(([name, n]) => `<div class="metric"><span class="metric-value">${n}</span><span class="metric-label">${telEsc(name)}</span></div>`).join("")
            : "";
          document.getElementById("tel-results").innerHTML = rows.length
            ? '<div class="table-wrap"><table class="data-table"><thead><tr><th>Time</th><th>Category</th><th>Event</th><th>Guild</th><th>Value</th><th>Detail</th></tr></thead><tbody>'
              + rows.map((r) => `<tr><td>${telEsc(new Date(r.created_at).toLocaleString())}</td><td>${telEsc(r.event_category)}</td><td>${telEsc(r.event_name)}</td><td>${telEsc(r.guild_id || "")}</td><td>${r.numeric_value != null ? Number(r.numeric_value).toFixed(1) : ""}</td><td>${telEsc(r.text_value || "")}</td></tr>`).join("")
              + "</tbody></table></div>"
            : '<div class="empty-state">No telemetry events recorded yet for this bot.</div>';
        } catch (err) { swarmToast(err.message, "error"); }
      }
      telForm.addEventListener("submit", (e) => { e.preventDefault(); loadTelemetry(); });
      loadTelemetry();
    ]]
    return page_shell(req, a, "/bot-telemetry", "Bot Telemetry", body, script)
  end)
end

return M
