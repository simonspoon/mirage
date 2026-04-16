// SQLite schema generator
#![allow(dead_code)]

use crate::parser::{SchemaObject, SwaggerSpec};

fn map_type(schema: &SchemaObject) -> &str {
    match schema.schema_type.as_deref() {
        Some("integer") => "INTEGER",
        Some("number") => "REAL",
        Some("boolean") => "BOOLEAN",
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
    create_tables_filtered(conn, spec, None, None)
}

pub fn create_tables_filtered(
    conn: &rusqlite::Connection,
    spec: &SwaggerSpec,
    only: Option<&std::collections::HashSet<String>>,
    skip: Option<&std::collections::HashSet<String>>,
) -> Result<(), rusqlite::Error> {
    if let Some(ref definitions) = spec.definitions {
        for (name, schema) in definitions {
            if let Some(filter) = only
                && !filter.contains(name)
            {
                continue;
            }
            if let Some(skip_set) = skip
                && skip_set.contains(name)
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

    #[test]
    fn test_schema_map_type_boolean_returns_boolean() {
        use crate::parser::SchemaObject;
        use std::collections::HashMap;

        let bool_schema = SchemaObject {
            schema_type: Some("boolean".to_string()),
            format: None,
            properties: None,
            items: None,
            required: None,
            ref_path: None,
            enum_values: None,
            description: None,
            additional_properties: None,
            all_of: None,
            x_faker: None,
        };
        assert_eq!(map_type(&bool_schema), "BOOLEAN");

        // End-to-end: DDL for a definition with a boolean column must use BOOLEAN.
        let mut props = HashMap::new();
        props.insert("flag".to_string(), bool_schema);
        let parent = SchemaObject {
            schema_type: Some("object".to_string()),
            format: None,
            properties: Some(props),
            items: None,
            required: None,
            ref_path: None,
            enum_values: None,
            description: None,
            additional_properties: None,
            all_of: None,
            x_faker: None,
        };
        let ddl = generate_table_sql("Thing", &parent);
        assert!(
            ddl.contains("\"flag\" BOOLEAN"),
            "DDL should declare flag BOOLEAN, got: {ddl}"
        );
    }

    #[test]
    fn test_skip_extension_only_roots() {
        use crate::parser::{self, Info, SchemaObject};
        use std::collections::HashMap;

        fn so() -> SchemaObject {
            SchemaObject {
                schema_type: None,
                format: None,
                properties: None,
                items: None,
                required: None,
                ref_path: None,
                enum_values: None,
                description: None,
                additional_properties: None,
                all_of: None,
                x_faker: None,
            }
        }

        // BaseType: extension-only root (no own properties beyond id, only used via allOf)
        let mut definitions = HashMap::new();
        definitions.insert(
            "BaseType".to_string(),
            SchemaObject {
                schema_type: Some("object".to_string()),
                properties: Some(HashMap::from([(
                    "id".to_string(),
                    SchemaObject {
                        schema_type: Some("string".to_string()),
                        ..so()
                    },
                )])),
                ..so()
            },
        );
        // ChildType: extends BaseType via allOf, adds own properties
        definitions.insert(
            "ChildType".to_string(),
            SchemaObject {
                all_of: Some(vec![
                    SchemaObject {
                        ref_path: Some("#/definitions/BaseType".to_string()),
                        ..so()
                    },
                    SchemaObject {
                        schema_type: Some("object".to_string()),
                        properties: Some(HashMap::from([(
                            "child_field".to_string(),
                            SchemaObject {
                                schema_type: Some("string".to_string()),
                                ..so()
                            },
                        )])),
                        ..so()
                    },
                ]),
                ..so()
            },
        );

        let mut spec = SwaggerSpec {
            swagger: "2.0".to_string(),
            info: Info {
                title: "test".to_string(),
                version: "1.0".to_string(),
            },
            paths: HashMap::new(),
            definitions: Some(definitions),
        };

        // Compute skip set from raw spec BEFORE resolve_refs
        let skip_set = parser::extension_only_roots(&spec);
        assert!(
            skip_set.contains("BaseType"),
            "BaseType should be extension-only root, got: {:?}",
            skip_set
        );

        // Resolve refs (merges allOf)
        spec.resolve_refs();

        // Create tables, skipping extension-only roots
        let conn = Connection::open_in_memory().unwrap();
        create_tables_filtered(&conn, &spec, None, Some(&skip_set)).unwrap();

        // BaseType table should NOT exist
        let base_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='BaseType'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!base_exists, "BaseType table should not be created");

        // ChildType table should exist
        let child_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ChildType'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(child_exists, "ChildType table should exist");

        // ChildType should have both inherited (id) and own (child_field) columns
        let mut stmt = conn.prepare("PRAGMA table_info('ChildType')").unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            columns.contains(&"id".to_string()),
            "ChildType should have inherited 'id' column, got: {:?}",
            columns
        );
        assert!(
            columns.contains(&"child_field".to_string()),
            "ChildType should have own 'child_field' column, got: {:?}",
            columns
        );
    }
}
