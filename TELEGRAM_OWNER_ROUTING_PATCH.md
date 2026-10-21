# SwarmPanel Telegram Owner Routing Patch

## Goal
Harden the Telegram bridge and route SwarmPanel Telegram updates to the owner account `@HollowHeavenXR`.

## Changes

- Added `PANEL_TELEGRAM_ALLOWED_USERNAMES=@HollowHeavenXR` to `.env` and `.env.example`.
- Added username-based authorization in `app/telegram.py`.
- Added runtime learning of the real numeric Telegram chat ID after the allowed account sends the bot any message such as `/start` or `/id`.
- Learned recipients are stored in `.runtime/telegram_recipients.json`.
- Alerts now send to configured chat IDs plus learned owner chat IDs.
- If no chat ID has been learned yet, alerts are skipped with a clear log warning instead of silently failing.
- Rejected unauthorized Telegram chats even when `PANEL_TELEGRAM_ALLOWED_CHAT_IDS` is empty.
- Sanitized Telegram handler error replies so internal exceptions are not sent to chat.
- Added Telegram API retry handling for 429/rate-limit and transient 5xx/network errors.
- Avoided exposing the bot token in Telegram API errors.

## Important Telegram behavior
Telegram bots generally cannot reliably start a private DM to a user account by username alone. The owner account must message the bot first so SwarmPanel can learn the numeric chat ID. After that, updates are sent to the learned chat ID.

## Owner setup

1. Open Telegram as `@HollowHeavenXR`.
2. Open the SwarmPanel bot.
3. Send `/start` or `/id`.
4. Restart SwarmPanel once if it was already running before this patch.

