use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type FrozenRows = HashMap<String, Vec<serde_json::Value>>;

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
    pub faker_rules: String,
    pub rules: String,
    pub frozen_rows: String,
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
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"faker_rules\" TEXT NOT NULL DEFAULT '{}'",
        [],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") => {}
        Err(e) => return Err(e),
    }
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"rules\" TEXT NOT NULL DEFAULT '[]'",
        [],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") => {}
        Err(e) => return Err(e),
    }
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"frozen_rows\" TEXT NOT NULL DEFAULT '{}'",
        [],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn create_recipe(
    conn: &Connection,
    name: &str,
    spec_source: &str,
    selected_endpoints_json: &str,
    seed_count: i64,
    shared_pools: Option<&str>,
    quantity_configs: Option<&str>,
    faker_rules: Option<&str>,
    rules: Option<&str>,
    frozen_rows: Option<&str>,
) -> Result<Recipe, rusqlite::Error> {
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let shared_pools = shared_pools.unwrap_or("{}");
    let quantity_configs = quantity_configs.unwrap_or("{}");
    let faker_rules = faker_rules.unwrap_or("{}");
    let rules = rules.unwrap_or("[]");
    let frozen_rows = frozen_rows.unwrap_or("{}");
    conn.execute(
        "INSERT INTO \"recipes\" (\"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\", \"faker_rules\", \"rules\", \"frozen_rows\") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![name, spec_source, selected_endpoints_json, seed_count, created_at, shared_pools, quantity_configs, faker_rules, rules, frozen_rows],
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
        faker_rules: faker_rules.to_string(),
        rules: rules.to_string(),
        frozen_rows: frozen_rows.to_string(),
    })
}

pub fn list_recipes(conn: &Connection) -> Result<Vec<Recipe>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT \"id\", \"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\", \"faker_rules\", \"rules\", \"frozen_rows\" FROM \"recipes\" ORDER BY \"id\"",
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
                faker_rules: row.get(8)?,
                rules: row.get(9)?,
                frozen_rows: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(recipes)
}

pub fn get_recipe(conn: &Connection, id: i64) -> Result<Option<Recipe>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT \"id\", \"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\", \"faker_rules\", \"rules\", \"frozen_rows\" FROM \"recipes\" WHERE \"id\" = ?1",
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
            faker_rules: row.get(8)?,
            rules: row.get(9)?,
            frozen_rows: row.get(10)?,
        })
    }) {
        Ok(recipe) => Ok(Some(recipe)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update_recipe_config(
    conn: &Connection,
    id: i64,
    shared_pools: &str,
    quantity_configs: &str,
    faker_rules: &str,
    rules: &str,
    frozen_rows: &str,
) -> Result<bool, rusqlite::Error> {
    let changes = conn.execute(
        "UPDATE \"recipes\" SET \"shared_pools\" = ?1, \"quantity_configs\" = ?2, \"faker_rules\" = ?3, \"rules\" = ?4, \"frozen_rows\" = ?5 WHERE \"id\" = ?6",
        rusqlite::params![shared_pools, quantity_configs, faker_rules, rules, frozen_rows, id],
    )?;
    Ok(changes > 0)
}

#[allow(clippy::too_many_arguments)]
pub fn update_recipe(
    conn: &Connection,
    id: i64,
    name: &str,
    spec_source: &str,
    selected_endpoints_json: &str,
    seed_count: i64,
    shared_pools: &str,
    quantity_configs: &str,
    faker_rules: &str,
    rules: &str,
    frozen_rows: &str,
) -> Result<bool, rusqlite::Error> {
    let changes = conn.execute(
        "UPDATE \"recipes\" SET \"name\" = ?1, \"spec_source\" = ?2, \"selected_endpoints\" = ?3, \"seed_count\" = ?4, \"shared_pools\" = ?5, \"quantity_configs\" = ?6, \"faker_rules\" = ?7, \"rules\" = ?8, \"frozen_rows\" = ?9 WHERE \"id\" = ?10",
        rusqlite::params![name, spec_source, selected_endpoints_json, seed_count, shared_pools, quantity_configs, faker_rules, rules, frozen_rows, id],
    )?;
    Ok(changes > 0)
}

pub fn delete_recipe(conn: &Connection, id: i64) -> Result<bool, rusqlite::Error> {
    let changes = conn.execute("DELETE FROM \"recipes\" WHERE \"id\" = ?1", [id])?;
    Ok(changes > 0)
}

/// Return a unique clone name for the given base recipe name.
///
/// Queries existing recipe names once via `list_recipes`, then checks
/// candidates in-memory:  `"<name> (copy)"`, `"<name> (copy 2)"`, etc.
pub fn find_unique_clone_name(
    conn: &Connection,
    base_name: &str,
) -> Result<String, rusqlite::Error> {
    let recipes = list_recipes(conn)?;
    let existing: std::collections::HashSet<String> = recipes.into_iter().map(|r| r.name).collect();

    let candidate = format!("{base_name} (copy)");
    if !existing.contains(&candidate) {
        return Ok(candidate);
    }

    let mut n = 2u64;
    loop {
        let candidate = format!("{base_name} (copy {n})");
        if !existing.contains(&candidate) {
            return Ok(candidate);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_recipe_db(&conn).unwrap();
        conn
    }

    fn insert_recipe(conn: &Connection, name: &str) {
        create_recipe(conn, name, "spec", "[]", 10, None, None, None, None, None).unwrap();
    }

    #[test]
    fn test_clone_name_no_conflict() {
        let conn = setup_db();
        insert_recipe(&conn, "My Recipe");
        let name = find_unique_clone_name(&conn, "My Recipe").unwrap();
        assert_eq!(name, "My Recipe (copy)");
    }

    #[test]
    fn test_clone_name_first_conflict() {
        let conn = setup_db();
        insert_recipe(&conn, "My Recipe");
        insert_recipe(&conn, "My Recipe (copy)");
        let name = find_unique_clone_name(&conn, "My Recipe").unwrap();
        assert_eq!(name, "My Recipe (copy 2)");
    }

    #[test]
    fn test_clone_name_multiple_conflicts() {
        let conn = setup_db();
        insert_recipe(&conn, "My Recipe");
        insert_recipe(&conn, "My Recipe (copy)");
        insert_recipe(&conn, "My Recipe (copy 2)");
        insert_recipe(&conn, "My Recipe (copy 3)");
        let name = find_unique_clone_name(&conn, "My Recipe").unwrap();
        assert_eq!(name, "My Recipe (copy 4)");
    }

    #[test]
    fn test_clone_name_gap_in_sequence() {
        let conn = setup_db();
        insert_recipe(&conn, "My Recipe");
        insert_recipe(&conn, "My Recipe (copy)");
        // skip (copy 2)
        insert_recipe(&conn, "My Recipe (copy 3)");
        let name = find_unique_clone_name(&conn, "My Recipe").unwrap();
        assert_eq!(name, "My Recipe (copy 2)");
    }

    #[test]
    fn test_clone_name_source_is_already_copy() {
        let conn = setup_db();
        insert_recipe(&conn, "My Recipe (copy)");
        let name = find_unique_clone_name(&conn, "My Recipe (copy)").unwrap();
        assert_eq!(name, "My Recipe (copy) (copy)");
    }
}
