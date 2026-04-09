// Mock data seeder
#![allow(dead_code)]

use crate::parser::{SchemaObject, SwaggerSpec};
use rand::RngExt;
use rusqlite::Connection;

const NAMES: &[&str] = &[
    "Alice", "Bob", "Charlie", "Diana", "Eve", "Frank", "Grace", "Hank", "Ivy", "Jack",
];

const WORDS: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
];

const LOREM: &[&str] = &[
    "Lorem ipsum dolor sit amet.",
    "The quick brown fox jumps over the lazy dog.",
    "A brief description of something interesting.",
    "This is a short placeholder sentence.",
    "Some additional context for the field.",
];

pub fn fake_value_for_field(name: &str, schema: &SchemaObject) -> serde_json::Value {
    let mut rng = rand::rng();

    // Check enum_values first
    if let Some(ref enums) = schema.enum_values
        && !enums.is_empty()
    {
        let idx = rng.random_range(0..enums.len());
        return enums[idx].clone();
    }

    // Match on schema_type
    match schema.schema_type.as_deref() {
        Some("object") => {
            if let Some(ref props) = schema.properties {
                let mut map = serde_json::Map::new();
                for (prop_name, prop_schema) in props {
                    map.insert(
                        prop_name.clone(),
                        fake_value_for_field(prop_name, prop_schema),
                    );
                }
                serde_json::Value::Object(map)
            } else {
                serde_json::Value::Object(serde_json::Map::new())
            }
        }
        Some("array") => {
            if let Some(ref items) = schema.items {
                let count = rng.random_range(1..=3);
                let arr: Vec<serde_json::Value> = (0..count)
                    .map(|_| fake_value_for_field(name, items))
                    .collect();
                serde_json::Value::Array(arr)
            } else {
                serde_json::Value::Array(vec![])
            }
        }
        Some("integer") => {
            let n: i64 = rng.random_range(1..10000);
            serde_json::Value::Number(serde_json::Number::from(n))
        }
        Some("number") => {
            let n: f64 = rng.random_range(0.0..10000.0);
            serde_json::json!(n)
        }
        Some("boolean") => serde_json::Value::Bool(rng.random::<bool>()),
        Some("string") => {
            let lower = name.to_lowercase();
            let s = if lower.contains("email") {
                let n: u32 = rng.random_range(0..10000);
                format!("user{n}@example.com")
            } else if lower.contains("name") {
                let idx = rng.random_range(0..NAMES.len());
                NAMES[idx].to_string()
            } else if lower.contains("phone") {
                let n: u32 = rng.random_range(1000000..9999999);
                format!("555-{n}")
            } else if lower.contains("url") || lower.contains("website") {
                let n: u32 = rng.random_range(0..10000);
                format!("https://example.com/{n}")
            } else if lower.contains("description") || lower.contains("body") {
                let idx = rng.random_range(0..LOREM.len());
                LOREM[idx].to_string()
            } else {
                let idx = rng.random_range(0..WORDS.len());
                WORDS[idx].to_string()
            };
            serde_json::Value::String(s)
        }
        _ => {
            // None or unknown type -> TEXT with random word
            let idx = rng.random_range(0..WORDS.len());
            serde_json::Value::String(WORDS[idx].to_string())
        }
    }
}

fn map_type(schema: &SchemaObject) -> &str {
    match schema.schema_type.as_deref() {
        Some("integer") => "INTEGER",
        Some("number") => "REAL",
        Some("boolean") => "INTEGER",
        Some("string") => "TEXT",
        Some("object") => "TEXT",
        Some("array") => "TEXT",
        _ => "TEXT",
    }
}

/// Get the effective properties for a schema, handling array-typed definitions.
fn effective_props(
    schema: &SchemaObject,
) -> Option<&std::collections::HashMap<String, SchemaObject>> {
    if let Some(ref props) = schema.properties
        && !props.is_empty()
    {
        return Some(props);
    }
    if schema.schema_type.as_deref() == Some("array")
        && let Some(ref items) = schema.items
        && let Some(ref props) = items.properties
        && !props.is_empty()
    {
        return Some(props);
    }
    None
}

pub fn seed_table(
    conn: &Connection,
    table_name: &str,
    schema: &SchemaObject,
    count: usize,
) -> Result<(), rusqlite::Error> {
    let props = match effective_props(schema) {
        Some(p) => p,
        None => return Ok(()),
    };

    // Sort column names alphabetically (same as schema.rs)
    let mut col_names: Vec<&String> = props.keys().collect();
    col_names.sort();

    let columns_str = col_names
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");

    let placeholders: Vec<String> = (1..=col_names.len()).map(|i| format!("?{i}")).collect();
    let placeholders_str = placeholders.join(", ");

    let sql = format!("INSERT INTO \"{table_name}\" ({columns_str}) VALUES ({placeholders_str})");

    for _ in 0..count {
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        for col_name in &col_names {
            let col_schema = &props[col_name.as_str()];
            let value = fake_value_for_field(col_name, col_schema);
            let sqlite_type = map_type(col_schema);

            match sqlite_type {
                "INTEGER" => {
                    let n = match &value {
                        serde_json::Value::Number(num) => num.as_i64().unwrap_or(0),
                        serde_json::Value::Bool(b) => {
                            if *b {
                                1
                            } else {
                                0
                            }
                        }
                        _ => 0,
                    };
                    params.push(Box::new(n));
                }
                "REAL" => {
                    let n = match &value {
                        serde_json::Value::Number(num) => num.as_f64().unwrap_or(0.0),
                        _ => 0.0,
                    };
                    params.push(Box::new(n));
                }
                _ => {
                    // TEXT: for objects/arrays, serialize to JSON string
                    let s = match &value {
                        serde_json::Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    params.push(Box::new(s));
                }
            }
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        conn.execute(&sql, param_refs.as_slice())?;
    }

    Ok(())
}

pub fn seed_tables(
    conn: &Connection,
    spec: &SwaggerSpec,
    rows_per_table: usize,
) -> Result<(), rusqlite::Error> {
    seed_tables_filtered(conn, spec, rows_per_table, None)
}

pub fn seed_tables_filtered(
    conn: &Connection,
    spec: &SwaggerSpec,
    rows_per_table: usize,
    only: Option<&std::collections::HashSet<String>>,
) -> Result<(), rusqlite::Error> {
    if let Some(ref definitions) = spec.definitions {
        for (name, schema) in definitions {
            if let Some(filter) = only
                && !filter.contains(name)
            {
                continue;
            }
            seed_table(conn, name, schema, rows_per_table)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SwaggerSpec;
    use crate::schema::create_tables;
    use rusqlite::Connection;

    fn setup() -> (Connection, SwaggerSpec) {
        let mut spec = SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap();
        spec.resolve_refs();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn, &spec).unwrap();
        (conn, spec)
    }

    #[test]
    fn test_seed_counts() {
        let (conn, spec) = setup();
        seed_tables(&conn, &spec, 10).unwrap();

        let defs = spec.definitions.as_ref().unwrap();
        for table_name in defs.keys() {
            let sql = format!("SELECT COUNT(*) FROM \"{table_name}\"");
            let count: i64 = conn.query_row(&sql, [], |row| row.get(0)).unwrap();
            assert_eq!(count, 10, "Table {table_name} should have 10 rows");
        }
    }

    #[test]
    fn test_json_columns() {
        let (conn, spec) = setup();
        seed_tables(&conn, &spec, 5).unwrap();

        // Pet.category is an object -> stored as JSON TEXT
        let rows: Vec<String> = {
            let mut stmt = conn.prepare("SELECT \"category\" FROM \"Pet\"").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        for row in &rows {
            let val: serde_json::Value = serde_json::from_str(row)
                .unwrap_or_else(|_| panic!("category should be valid JSON, got: {row}"));
            assert!(
                val.is_object(),
                "category should be a JSON object, got: {val}"
            );
        }

        // Pet.tags is an array -> stored as JSON TEXT
        let rows: Vec<String> = {
            let mut stmt = conn.prepare("SELECT \"tags\" FROM \"Pet\"").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        for row in &rows {
            let val: serde_json::Value = serde_json::from_str(row)
                .unwrap_or_else(|_| panic!("tags should be valid JSON, got: {row}"));
            assert!(val.is_array(), "tags should be a JSON array, got: {val}");
        }
    }

    #[test]
    fn test_enum_values() {
        let (conn, spec) = setup();
        seed_tables(&conn, &spec, 20).unwrap();

        let valid = ["available", "pending", "sold"];
        let mut stmt = conn.prepare("SELECT \"status\" FROM \"Pet\"").unwrap();
        let statuses: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(statuses.len(), 20);
        for status in &statuses {
            assert!(
                valid.contains(&status.as_str()),
                "status '{status}' should be one of {valid:?}"
            );
        }
    }
}
