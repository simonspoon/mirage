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
    pub quantity_configs: String,
    pub faker_rules: String,
    pub rules: String,
    pub frozen_rows: String,
    pub custom_lists: String,
    /// Per-table seed counts. JSON-encoded `{def_name: usize}` map. Empty `{}`
    /// means no per-table overrides stored — activation falls back to
    /// `seed_count` scalar. Populated by the Configure step's per-table rows
    /// inputs (task mqvu). Migration of pre-existing recipes from scalar
    /// `seed_count` to this map is sibling task gtjj.
    pub seed_counts: String,
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
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"custom_lists\" TEXT NOT NULL DEFAULT '{}'",
        [],
    ) {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
            if msg.contains("duplicate column") => {}
        Err(e) => return Err(e),
    }
    match conn.execute(
        "ALTER TABLE \"recipes\" ADD COLUMN \"seed_counts\" TEXT NOT NULL DEFAULT '{}'",
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
    quantity_configs: Option<&str>,
    faker_rules: Option<&str>,
    rules: Option<&str>,
    frozen_rows: Option<&str>,
    custom_lists: Option<&str>,
    seed_counts: Option<&str>,
) -> Result<Recipe, rusqlite::Error> {
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let quantity_configs = quantity_configs.unwrap_or("{}");
    let faker_rules = faker_rules.unwrap_or("{}");
    let rules = rules.unwrap_or("[]");
    let frozen_rows = frozen_rows.unwrap_or("{}");
    let custom_lists = custom_lists.unwrap_or("{}");
    let seed_counts = seed_counts.unwrap_or("{}");
    // "shared_pools" column retained for schema back-compat; we write the
    // default empty-object literal so old deployments do not break, but the
    // value is never read back out.
    conn.execute(
        "INSERT INTO \"recipes\" (\"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"shared_pools\", \"quantity_configs\", \"faker_rules\", \"rules\", \"frozen_rows\", \"custom_lists\", \"seed_counts\") VALUES (?1, ?2, ?3, ?4, ?5, '{}', ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![name, spec_source, selected_endpoints_json, seed_count, created_at, quantity_configs, faker_rules, rules, frozen_rows, custom_lists, seed_counts],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Recipe {
        id,
        name: name.to_string(),
        spec_source: spec_source.to_string(),
        selected_endpoints: selected_endpoints_json.to_string(),
        seed_count,
        created_at,
        quantity_configs: quantity_configs.to_string(),
        faker_rules: faker_rules.to_string(),
        rules: rules.to_string(),
        frozen_rows: frozen_rows.to_string(),
        custom_lists: custom_lists.to_string(),
        seed_counts: seed_counts.to_string(),
    })
}

pub fn list_recipes(conn: &Connection) -> Result<Vec<Recipe>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT \"id\", \"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"quantity_configs\", \"faker_rules\", \"rules\", \"frozen_rows\", \"custom_lists\", \"seed_counts\" FROM \"recipes\" ORDER BY \"id\"",
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
                quantity_configs: row.get(6)?,
                faker_rules: row.get(7)?,
                rules: row.get(8)?,
                frozen_rows: row.get(9)?,
                custom_lists: row.get(10)?,
                seed_counts: row.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(recipes)
}

pub fn get_recipe(conn: &Connection, id: i64) -> Result<Option<Recipe>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT \"id\", \"name\", \"spec_source\", \"selected_endpoints\", \"seed_count\", \"created_at\", \"quantity_configs\", \"faker_rules\", \"rules\", \"frozen_rows\", \"custom_lists\", \"seed_counts\" FROM \"recipes\" WHERE \"id\" = ?1",
    )?;
    match stmt.query_row([id], |row| {
        Ok(Recipe {
            id: row.get(0)?,
            name: row.get(1)?,
            spec_source: row.get(2)?,
            selected_endpoints: row.get(3)?,
            seed_count: row.get(4)?,
            created_at: row.get(5)?,
            quantity_configs: row.get(6)?,
            faker_rules: row.get(7)?,
            rules: row.get(8)?,
            frozen_rows: row.get(9)?,
            custom_lists: row.get(10)?,
            seed_counts: row.get(11)?,
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
    quantity_configs: &str,
    faker_rules: &str,
    rules: &str,
    frozen_rows: &str,
    custom_lists: &str,
) -> Result<bool, rusqlite::Error> {
    // "shared_pools" column retained for schema back-compat; the column is
    // deliberately not updated here so old rows keep whatever they had.
    let changes = conn.execute(
        "UPDATE \"recipes\" SET \"quantity_configs\" = ?1, \"faker_rules\" = ?2, \"rules\" = ?3, \"frozen_rows\" = ?4, \"custom_lists\" = ?5 WHERE \"id\" = ?6",
        rusqlite::params![quantity_configs, faker_rules, rules, frozen_rows, custom_lists, id],
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
    quantity_configs: &str,
    faker_rules: &str,
    rules: &str,
    frozen_rows: &str,
    custom_lists: &str,
    seed_counts: &str,
) -> Result<bool, rusqlite::Error> {
    // "shared_pools" column retained for schema back-compat; the column is
    // deliberately not updated here so old rows keep whatever they had.
    let changes = conn.execute(
        "UPDATE \"recipes\" SET \"name\" = ?1, \"spec_source\" = ?2, \"selected_endpoints\" = ?3, \"seed_count\" = ?4, \"quantity_configs\" = ?5, \"faker_rules\" = ?6, \"rules\" = ?7, \"frozen_rows\" = ?8, \"custom_lists\" = ?9, \"seed_counts\" = ?10 WHERE \"id\" = ?11",
        rusqlite::params![name, spec_source, selected_endpoints_json, seed_count, quantity_configs, faker_rules, rules, frozen_rows, custom_lists, seed_counts, id],
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
        create_recipe(
            conn, name, "spec", "[]", 10, None, None, None, None, None, None,
        )
        .unwrap();
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

    #[test]
    fn test_seed_counts_roundtrip_create_and_update() {
        let conn = setup_db();
        let payload = r#"{"AccessDto":7,"AddressDto":3}"#;
        let recipe = create_recipe(
            &conn,
            "My Recipe",
            "spec",
            "[]",
            10,
            None,
            None,
            None,
            None,
            None,
            Some(payload),
        )
        .unwrap();
        assert_eq!(recipe.seed_counts, payload);
        let fetched = get_recipe(&conn, recipe.id).unwrap().unwrap();
        assert_eq!(fetched.seed_counts, payload);
        let listed = list_recipes(&conn).unwrap();
        assert_eq!(listed[0].seed_counts, payload);

        let updated_payload = r#"{"AccessDto":12}"#;
        assert!(update_recipe(
            &conn,
            recipe.id,
            "My Recipe",
            "spec",
            "[]",
            10,
            "{}",
            "{}",
            "[]",
            "{}",
            "{}",
            updated_payload,
        )
        .unwrap());
        let after = get_recipe(&conn, recipe.id).unwrap().unwrap();
        assert_eq!(after.seed_counts, updated_payload);
    }

    #[test]
    fn test_seed_counts_defaults_to_empty_object() {
        let conn = setup_db();
        let recipe = create_recipe(
            &conn, "R", "spec", "[]", 10, None, None, None, None, None, None,
        )
        .unwrap();
        assert_eq!(recipe.seed_counts, "{}");
        let fetched = get_recipe(&conn, recipe.id).unwrap().unwrap();
        assert_eq!(fetched.seed_counts, "{}");
    }
}
