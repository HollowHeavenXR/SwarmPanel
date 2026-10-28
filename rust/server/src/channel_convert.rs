//! Convert every home-channel-connected bot's channel in a guild between
//! voice and stage (port of channel_convert.lua).

use crate::app::AppRef;
use crate::db::J;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub async fn convert_guild_channels(app: &AppRef, gid: &str, direction: &str) -> Result<Value, String> {
    let ctype = match direction {
        "stage" => crate::discord::STAGE,
        "voice" => crate::discord::VOICE,
        other => return Err(format!("direction must be 'stage' or 'voice', got: {other}")),
    };
    let mut by: BTreeMap<String, Vec<&crate::config::Bot>> = BTreeMap::new();
    let mut skipped = 0;
    for bot in &app.settings.bots {
        let _ = app.db.batch(&bot.db_schema, &format!("CREATE TABLE IF NOT EXISTS {}_bot_home_channels (guild_id BIGINT, bot_name VARCHAR(50), home_vc_id BIGINT, PRIMARY KEY (guild_id, bot_name))", bot.key)).await;
        let ch = app.db.opt(&bot.db_schema, &format!("SELECT home_vc_id FROM {}_bot_home_channels WHERE guild_id = $1 AND bot_name = $2", bot.key), &[gid.into(), bot.key.into()]).await.and_then(|r| r.s("home_vc_id"));
        match ch.filter(|c| !c.is_empty() && c != "0") {
            Some(c) => by.entry(c).or_default().push(bot),
            None => skipped += 1,
        }
    }
    let (mut converted, mut failed) = (Vec::new(), Vec::new());
    for (ch, bots) in by {
        let keys: Vec<&str> = bots.iter().map(|b| b.key).collect();
        let token = app.settings.token(bots[0].key);
        if token.is_empty() {
            failed.push(json!({ "channel_id": ch, "bots": keys, "error": "No Discord token configured for this bot" }));
            continue;
        }
        match crate::discord::recreate_channel_as_type(app, &token, &ch, ctype).await {
            Err(e) => failed.push(json!({ "channel_id": ch, "bots": keys, "error": e })),
            Ok((new_id, warning)) => {
                for bot in &bots {
                    let _ = app.db.exec(&bot.db_schema, &format!("UPDATE {}_bot_home_channels SET home_vc_id = $1 WHERE guild_id = $2 AND bot_name = $3", bot.key), &[new_id.clone().into(), gid.into(), bot.key.into()]).await;
                    let _ = crate::control::control_bot(app, bot, gid, "RECOVER", &json!({ "voice_channel_id": new_id }), "channel_convert").await;
                }
                converted.push(json!({ "old_channel_id": ch, "new_channel_id": new_id, "bots": keys, "warning": warning }));
            }
        }
    }
    Ok(json!({ "converted": converted, "failed": failed, "skipped_no_home_channel": skipped }))
}
