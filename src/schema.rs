// SQLite schema generator
#![allow(dead_code)]

use crate::parser::{SchemaObject, SwaggerSpec};

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

/// Get the effective properties for a schema, handling array-typed definitions
/// that wrap their properties under `items`.
fn effective_props(
    schema: &SchemaObject,
) -> Option<&std::collections::HashMap<String, SchemaObject>> {
    if let Some(ref props) = schema.properties
        && !props.is_empty()
    {
        return Some(props);
    }
    // For array-typed definitions, look at items.properties
    if schema.schema_type.as_deref() == Some("array")
        && let Some(ref items) = schema.items
        && let Some(ref props) = items.properties
        && !props.is_empty()
    {
        return Some(props);
    }
    None
}

pub fn generate_table_sql(name: &str, schema: &SchemaObject) -> String {
    let required: Vec<&str> = schema
        .required
        .as_ref()
        .map(|r| r.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();

    match effective_props(schema) {
        Some(props) => {
            let mut columns: Vec<String> = props
                .iter()
                .map(|(col_name, col_schema)| {
                    let sqlite_type = map_type(col_schema);
                    if required.contains(&col_name.as_str()) {
                        format!("\"{col_name}\" {sqlite_type} NOT NULL")
                    } else {
                        format!("\"{col_name}\" {sqlite_type}")
                    }
                })
                .collect();
            columns.sort();
            format!(
                "CREATE TABLE IF NOT EXISTS \"{name}\" ({})",
                columns.join(", ")
            )
        }
        None => format!("CREATE TABLE IF NOT EXISTS \"{name}\" (id INTEGER PRIMARY KEY)"),
    }
}

pub fn create_tables(
    conn: &rusqlite::Connection,
    spec: &SwaggerSpec,
) -> Result<(), rusqlite::Error> {
    create_tables_filtered(conn, spec, None)
}

pub fn create_tables_filtered(
    conn: &rusqlite::Connection,
    spec: &SwaggerSpec,
    only: Option<&std::collections::HashSet<String>>,
) -> Result<(), rusqlite::Error> {
    if let Some(ref definitions) = spec.definitions {
        for (name, schema) in definitions {
            if let Some(filter) = only
                && !filter.contains(name)
            {
                continue;
            }
            let sql = generate_table_sql(name, schema);
            conn.execute(&sql, [])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SwaggerSpec;
    use rusqlite::Connection;

    fn load_spec() -> SwaggerSpec {
        let mut spec = SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap();
        spec.resolve_refs();
        spec
    }

    #[test]
    fn test_category_ddl() {
        let spec = load_spec();
        let defs = spec.definitions.as_ref().unwrap();
        let category = defs.get("Category").unwrap();
        let ddl = generate_table_sql("Category", category);
        assert!(
            ddl.contains("\"id\" INTEGER"),
            "should contain '\"id\" INTEGER'"
        );
        assert!(
            ddl.contains("\"name\" TEXT"),
            "should contain '\"name\" TEXT'"
        );
    }

    #[test]
    fn test_pet_ddl_columns() {
        let spec = load_spec();
        let defs = spec.definitions.as_ref().unwrap();
        let pet = defs.get("Pet").unwrap();
        let ddl = generate_table_sql("Pet", pet);
        assert!(
            ddl.contains("\"name\" TEXT NOT NULL"),
            "name should be NOT NULL"
        );
        assert!(
            ddl.contains("\"status\" TEXT"),
            "should contain '\"status\" TEXT'"
        );
        assert!(
            ddl.contains("\"category\" TEXT"),
            "should contain '\"category\" TEXT'"
        );
        assert!(
            ddl.contains("\"tags\" TEXT"),
            "should contain '\"tags\" TEXT'"
        );
    }

    #[test]
    fn test_ddl_executes() {
        let spec = load_spec();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn, &spec).unwrap();
    }

    #[test]
    fn test_all_definitions_produce_tables() {
        let spec = load_spec();
        let defs = spec.definitions.as_ref().unwrap();
        for (name, schema) in defs {
            let ddl = generate_table_sql(name, schema);
            assert!(
                ddl.starts_with("CREATE TABLE"),
                "DDL for {name} should start with CREATE TABLE"
            );
        }
    }
}
