//! Admin database browser (port of admin_browser.lua).

use crate::app::AppRef;
use crate::db::J;
use serde_json::{Value, json};

pub fn ident(v: &str, field: &str) -> Result<String, String> {
    let v = v.trim();
    if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("Invalid {field}: {v}"));
    }
    Ok(v.to_string())
}

pub fn list_schemas(app: &AppRef) -> Vec<String> {
    let s = &app.settings;
    let mut out: Vec<String> = s.bots.iter().map(|b| b.db_schema.clone()).chain(["discord_aria", "accountlogins", "image_gallery"].map(String::from)).collect();
    out.sort();
    out.dedup();
    out
}

pub async fn list_tables(app: &AppRef, schema: &str) -> Result<Vec<Value>, String> {
    let schema = ident(schema, "schema")?;
    let rows = app
        .db
        .query(&schema, "SELECT c.relname::text AS table_name, COALESCE(c.reltuples, 0)::bigint AS estimated_rows FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = 'public' AND c.relkind = 'r' ORDER BY c.relname", &[])
        .await?;
    Ok(rows.iter().map(|r| json!({ "table_name": r.s("table_name"), "estimated_rows": r.i("estimated_rows").unwrap_or(0).max(0) })).collect())
}

pub async fn table_data(app: &AppRef, schema: &str, table: &str, limit: i64) -> Result<Value, String> {
    let schema = ident(schema, "schema")?;
    let table = ident(table, "table")?;
    let rows = app.db.query(&schema, &format!("SELECT * FROM \"{table}\" LIMIT $1"), &[limit.clamp(1, 500).into()]).await?;
    Ok(json!({ "schema": schema, "table": table, "count": rows.len(), "rows": rows }))
}

pub async fn truncate_table(app: &AppRef, schema: &str, table: &str) -> Result<(), String> {
    let schema = ident(schema, "schema")?;
    let table = ident(table, "table")?;
    app.db.batch(&schema, &format!("TRUNCATE TABLE \"{table}\" CASCADE")).await
}

pub async fn truncate_schema(app: &AppRef, schema: &str) -> Result<Value, String> {
    let schema = ident(schema, "schema")?;
    let tables = list_tables(app, &schema).await?;
    let names: Vec<String> = tables.iter().filter_map(|t| t.s("table_name")).collect();
    for n in &names {
        app.db.batch(&schema, &format!("TRUNCATE TABLE \"{}\" CASCADE", ident(n, "table")?)).await?;
    }
    Ok(json!({ "schema": schema, "truncated_tables": names.len(), "tables": names }))
}
