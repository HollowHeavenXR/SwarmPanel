# SwarmPanel

![Witch Knot site icon](favicon.ico)

SwarmPanel is the React and FastAPI command center for Aria and the 12-node music bot fleet. It focuses on live operational visibility, queue and playback control, owner-safe administration, account profiles, social features, and mobile-friendly monitoring.

**Live panel: <https://swarmpanel.xenusanimations.studio>** — a named Cloudflare tunnel onto the backend. That hostname is the panel's public address for the web UI, the iOS app and the Apple TV app alike; the older `*.github.io/SwarmPanel/` Pages address is no longer the entry point.

## What It Does

- Shows live bot status across all music nodes: track, guild, voice channel, queue depth, backup depth, filters, heartbeat, and drift.
- Sends direct bot controls to the fleet for play, pause, resume, skip, stop, loop, filter, queue, and recovery actions.
- Gives the owner expanded admin mode for database inspection, destructive-action safeguards, bot control, and operational review.
- Tracks Swarm health, stale nodes, Medic warnings, queue recovery candidates, voice failures, and database issues.
- Provides user accounts with profile pages, avatars, display names, bios, links, profile colors, visibility, and online or inactive presence.
- Supports profile discovery, friend requests, follows, and direct messages between panel users.
- Includes appearance controls with real previews so users can see how dashboard, queue, Medic, and database sections will look.
- Sends scoped Telegram operator alerts for important panel health issues without spamming normal logs.
- Serves the panel over a named Cloudflare tunnel at the fixed hostname above. A static GitHub Pages copy of the frontend can still be published as a fallback; it reads `live-config.json` at runtime to find the backend.

## Main Surfaces

The panel is grouped into five sections. The sidebar, mobile menu, breadcrumbs, and the tabs above each screen all come from one definition in `lua/src/nav.lua`. The iOS app uses the same sections on its floating console dock: Fleet, Insights, Community and You, with Controls as the Deck on the dock's centre orb.

- **Fleet:** Dashboard (live fleet status, health summaries, sessions), Controls (per-bot playback and queue orders, channel conversion), Invites.
- **Insights:** Leaderboard (top tracks and listeners) and Learning (what the recommendation engine has learned).
- **Community:** Directory, Friends, and Messages, plus public profile pages.
- **Account:** Profile, Appearance (theme, layout, sidebar style, live previews), Other Projects.
- **Admin** (owner/moderator only): an Overview hub, Diagnostics, Intel, Audit Log, Accounts, Databases, Gallery, and Lumisound.

Server-rendered screens live in one Lua module per section: `pages_fleet.lua`, `pages_insights.lua`, `pages_community.lua`, `pages_account.lua`, and `pages_admin.lua`, with sign-in in `pages_auth.lua`. Shared page plumbing lives in `page_kit.lua`.

## Apple TV

`ios/SwarmPanelTV` is a tvOS app with just the Dashboard: fleet metrics, Aria's card and a live card per bot, updated over the same WebSocket feed as the web panel. It is read-only. There are no Controls (you can't paste a YouTube link with a Siri Remote), and it always signs in with admin mode off without changing the account's saved admin-mode choice on the web or iPhone. Your panel background, accent colour and profile (name, avatar, server) follow your web settings live through the `account` feed. The `Build tvOS app` workflow builds it unsigned. Sign it in Xcode to install on an Apple TV.

## Servers And Data

- Frontend: React and Vite, served by the backend behind the Cloudflare tunnel (and optionally published to GitHub Pages as a fallback).
- Public URL: set `PANEL_PAGES_PUBLIC_URL` (and `PANEL_CLOUDFLARE_PUBLIC_URL`, `PANEL_TRUSTED_HOSTS`) to the tunnel hostname. The backend builds email-verification and operator-alert links from it and hands it to the native apps, so a stale value sends people to an address that no longer serves the panel.
- Backend: FastAPI app served from `app.main`.
- Database: MySQL schemas for panel accounts, Aria telemetry, and each music bot queue.
- Bot network: 12 Discord music bots plus Aria.
- Audio control: Lavalink-backed bots with panel-issued command requests.
- Operator alerts: Telegram bridge for scoped health and database notices.

## Guardrails

- Destructive database actions require explicit owner confirmation.
- Secrets belong in ignored environment files, never in committed code.
- Admin mode is treated as owner-only authority, not a regular user feature.
- Owner elevation requires a verified email that matches `SWARM_PANEL_SITE_OWNER_EMAIL`.
- Guild account registration requires a Discord webhook proof from the target server.
- External bot tokens are not surfaced in the frontend or README.
- Queue and Medic data should be treated as operational telemetry, not decoration.

## Copyright

(c) HollowHeavenXR. Discord: <https://discord.com/users/1304564041863266347>
