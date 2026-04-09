use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: i64,
    pub name: String,
    pub spec_source: String,
    pub selected_endpoints: String,
    pub seed_count: i64,
    pub created_at: String,
    pub shared_pools: String,
    pub quantity_configs: String,
}

pub fn init_recipe_db(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS \"recipes\" (
            \"id\" INTEGER PRIMARY KEY,
            \"name\" TEXT NOT NULL,
            \"spec_source\" TEXT NOT NULL,
            \"selected_endpoints\" TEXT NOT NULL,
            \"seed_count\" INTEGER NOT NULL DEFAULT 10,
            \"created_at\" TEXT NOT NULL
        )",
        [],
    )?;
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"shared_pools\" TEXT NOT NULL DEFAULT '{}'",
        [],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") => {}
        Err(e) => return Err(e),
    }
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"quantity_configs\" TEXT NOT NULL DEFAULT '{}'",
        [],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

pub fn create_recipe(
    conn: &Connection,
    name: &str,
    spec_source: &str,
    selected_endpoints_json: &str,
    seed_count: i64,
    shared_pools: Option<&str>,
    quantity_configs: Option<&str>,
) -> Result<Recipe, rusqlite::Error> {
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let shared_pools = shared_pools.unwrap_or("{}");
    let quantity_configs = quantity_configs.unwrap_or("{}");
    conn.execute(
        "INSERT INTO \"recipes\" (\"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![name, spec_source, selected_endpoints_json, seed_count, created_at, shared_pools, quantity_configs],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Recipe {
        id,
        name: name.to_string(),
        spec_source: spec_source.to_string(),
        selected_endpoints: selected_endpoints_json.to_string(),
        seed_count,
        created_at,
        shared_pools: shared_pools.to_string(),
        quantity_configs: quantity_configs.to_string(),
    })
}

pub fn list_recipes(conn: &Connection) -> Result<Vec<Recipe>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT \"id\", \"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\" FROM \"recipes\" ORDER BY \"id\"",
    )?;
    let recipes = stmt
        .query_map([], |row| {
            Ok(Recipe {
                id: row.get(0)?,
                name: row.get(1)?,
                spec_source: row.get(2)?,
                selected_endpoints: row.get(3)?,
                seed_count: row.get(4)?,
                created_at: row.get(5)?,
                shared_pools: row.get(6)?,
                quantity_configs: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(recipes)
}

pub fn get_recipe(conn: &Connection, id: i64) -> Result<Option<Recipe>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT \"id\", \"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\" FROM \"recipes\" WHERE \"id\" = ?1",
    )?;
    match stmt.query_row([id], |row| {
        Ok(Recipe {
            id: row.get(0)?,
            name: row.get(1)?,
            spec_source: row.get(2)?,
            selected_endpoints: row.get(3)?,
            seed_count: row.get(4)?,
            created_at: row.get(5)?,
            shared_pools: row.get(6)?,
            quantity_configs: row.get(7)?,
        })
    }) {
        Ok(recipe) => Ok(Some(recipe)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn update_recipe_config(
    conn: &Connection,
    id: i64,
    shared_pools: &str,
    quantity_configs: &str,
) -> Result<bool, rusqlite::Error> {
    let changes = conn.execute(
        "UPDATE \"recipes\" SET \"shared_pools\" = ?1, \"quantity_configs\" = ?2 WHERE \"id\" = ?3",
        rusqlite::params![shared_pools, quantity_configs, id],
    )?;
    Ok(changes > 0)
}

pub fn delete_recipe(conn: &Connection, id: i64) -> Result<bool, rusqlite::Error> {
    let changes = conn.execute("DELETE FROM \"recipes\" WHERE \"id\" = ?1", [id])?;
    Ok(changes > 0)
}
