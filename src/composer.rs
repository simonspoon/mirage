// Document-based seeder

use std::collections::{HashMap, HashSet};

use rand::RngExt;

use crate::entity_graph::EntityGraph;
use crate::parser::{SchemaObject, SwaggerSpec};
use crate::rules::{self, FieldRuleMap, Rule, build_compare_rules_by_def, build_field_rule_map};
use crate::seeder::{FakerStrategy, fake_value_for_field, fake_value_for_field_layered};
use crate::server::EndpointInfo;

pub type FakerRules = HashMap<String, HashMap<String, FakerStrategy>>;

pub struct QuantityConfig {
    pub min: usize,
    pub max: usize,
}

pub type QuantityConfigs = HashMap<String, QuantityConfig>;

pub type DocumentStore = HashMap<String, Vec<serde_json::Value>>;

/// Compose documents for each selected endpoint's response definition.
/// `spec` (resolved) is used for schema generation (inlined properties).
/// `raw_spec` (unresolved) is used for definition name lookup (retains $ref paths).
pub fn compose_documents(
    spec: &SwaggerSpec,
    raw_spec: &SwaggerSpec,
    _graph: &EntityGraph,
    quantities: &QuantityConfigs,
    selected_endpoints: &[EndpointInfo],
    faker_rules: &FakerRules,
    recipe_rules: &[Rule],
) -> DocumentStore {
    let defs = match &spec.definitions {
        Some(d) => d,
        None => return DocumentStore::new(),
    };

    let raw_defs = raw_spec.definitions.as_ref();

    // Build a lookup of (path, method) -> definition name from the raw spec
    let raw_ops = raw_spec.path_operations();
    let raw_op_map: HashMap<(&str, &str), &crate::parser::Operation> = raw_ops
        .iter()
        .map(|(path, method, op)| ((*path, *method), *op))
        .collect();

    let field_rule_map = build_field_rule_map(recipe_rules);
    let compare_rules_by_def = build_compare_rules_by_def(recipe_rules);

    let mut store = DocumentStore::new();

    for endpoint in selected_endpoints {
        let def_name = raw_op_map
            .get(&(endpoint.path.as_str(), endpoint.method.as_str()))
            .and_then(|raw_op| crate::parser::primary_response_def(raw_op, raw_defs));
        let def_name = match def_name {
            Some(n) => n,
            None => continue,
        };

        // Skip if we already composed documents for this definition
        if store.contains_key(&def_name) {
            continue;
        }

        let schema = match defs.get(&def_name) {
            Some(s) => s,
            None => continue,
        };

        let (min, max) = match quantities.get(&def_name) {
            Some(qc) => (qc.min, qc.max),
            None => (10, 10),
        };

        let count = if min == max {
            min
        } else {
            let mut rng = rand::rng();
            rng.random_range(min..=max)
        };

        let def_compare_rules = compare_rules_by_def.get(&def_name).map(Vec::as_slice);
        let raw_schema = raw_defs.and_then(|d| d.get(&def_name));

        let mut docs = Vec::with_capacity(count);
        for i in 0..count {
            let mut doc = generate_document_from_schema(
                &def_name,
                schema,
                raw_schema,
                raw_defs,
                faker_rules,
                &field_rule_map,
                def_compare_rules,
            );
            // Assign incremental id
            if let serde_json::Value::Object(ref mut map) = doc {
                map.insert(
                    "id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(i + 1)),
                );
            }
            docs.push(doc);
        }
        store.insert(def_name, docs);
    }

    store
}

/// Generate a single document from a schema by faking each property.
fn generate_document_from_schema(
    def_name: &str,
    schema: &SchemaObject,
    raw_schema: Option<&SchemaObject>,
    raw_defs: Option<&HashMap<String, SchemaObject>>,
    faker_rules: &FakerRules,
    field_rule_map: &FieldRuleMap,
    compare_rules: Option<&[Rule]>,
) -> serde_json::Value {
    let props = match &schema.properties {
        Some(p) => p,
        None => return fake_value_for_field(def_name, schema),
    };

    // When raw_schema has no direct properties but uses allOf at the definition
    // root, flatten its allOf members (following $ref chains via raw_defs).
    // Binding this owner before `raw_props` keeps the borrow alive across the
    // loop below.
    let flattened_raw_props =
        raw_schema.and_then(|rs| flatten_allof_raw_props(rs, raw_defs, &mut HashSet::new()));

    let raw_props = raw_schema
        .and_then(|s| s.properties.as_ref())
        .or(flattened_raw_props.as_ref());

    let mut map = serde_json::Map::new();

    for (prop_name, prop_schema) in props {
        let raw_prop = raw_props.and_then(|rp| rp.get(prop_name));
        let value = generate_property_value(
            prop_name,
            prop_schema,
            raw_prop,
            def_name,
            faker_rules,
            field_rule_map,
        );
        map.insert(prop_name.clone(), value);
    }

    if let Some(rules) = compare_rules {
        rules::apply_compare_rules(&mut map, rules);
    }

    serde_json::Value::Object(map)
}

/// Generate a value for a single property via faker rules + schema heuristics.
fn generate_property_value(
    prop_name: &str,
    prop_schema: &SchemaObject,
    _raw_prop: Option<&SchemaObject>,
    def_name: &str,
    faker_rules: &FakerRules,
    field_rule_map: &FieldRuleMap,
) -> serde_json::Value {
    let recipe_rule = field_rule_map.get(&(def_name.to_string(), prop_name.to_string()));
    let faker_rule = faker_rules.get(def_name).and_then(|m| m.get(prop_name));
    fake_value_for_field_layered(prop_name, prop_schema, recipe_rule, faker_rule)
}

/// Flatten a raw schema's allOf members into a single property map, preserving
/// `ref_path` values on property schemas so downstream consumers (e.g. implicit
/// nested-ref sampling from SQLite backing tables, task yhgg) can still see
/// the ref target. Mirrors the allOf merge in `parser::resolve_schema`
/// (vec-order `.extend`, later wins).
///
/// Returns `Some(map)` when `schema.all_of` is set (map may merge in existing
/// `schema.properties` clones first), `None` otherwise. `$ref`-only members
/// are resolved via `raw_defs` (the same raw definitions map `schema` came
/// from). The `visited` set guards against cyclic `allOf` chains
/// (mirrors `parser::resolve_schema`'s cycle guard).
fn flatten_allof_raw_props(
    schema: &SchemaObject,
    raw_defs: Option<&HashMap<String, SchemaObject>>,
    visited: &mut HashSet<String>,
) -> Option<HashMap<String, SchemaObject>> {
    let all_of = schema.all_of.as_ref()?;

    let mut merged: HashMap<String, SchemaObject> = schema.properties.clone().unwrap_or_default();

    for member in all_of {
        if let Some(ref ref_path) = member.ref_path {
            let def_name = ref_path.strip_prefix("#/definitions/").unwrap_or(ref_path);
            if visited.contains(def_name) {
                continue;
            }
            let resolved = match raw_defs.and_then(|d| d.get(def_name)) {
                Some(r) => r,
                None => continue,
            };
            visited.insert(def_name.to_string());
            if let Some(props) = resolved.properties.as_ref() {
                merged.extend(props.clone());
            }
            if let Some(nested) = flatten_allof_raw_props(resolved, raw_defs, visited) {
                merged.extend(nested);
            }
            visited.remove(def_name);
        } else {
            if let Some(props) = member.properties.as_ref() {
                merged.extend(props.clone());
            }
            if let Some(nested) = flatten_allof_raw_props(member, raw_defs, visited) {
                merged.extend(nested);
            }
        }
    }

    Some(merged)
}

/// Parse QuantityConfigs from a JSON string. Returns empty map on parse failure.
pub fn parse_quantity_configs(json_str: &str) -> QuantityConfigs {
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return QuantityConfigs::new(),
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return QuantityConfigs::new(),
    };

    let mut configs = QuantityConfigs::new();
    for (def_name, val) in obj {
        if let Some(inner) = val.as_object() {
            let min = inner.get("min").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let max = inner.get("max").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            configs.insert(def_name.clone(), QuantityConfig { min, max });
        }
    }
    configs
}

/// Parse custom lists from the recipe-level JSON blob.
/// Input format: `{"ListName": ["val", "val", ...], ...}`.
/// Non-string array elements are dropped. Empty or malformed lists are skipped.
pub fn parse_custom_lists(json_str: &str) -> HashMap<String, Vec<String>> {
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return HashMap::new(),
    };

    let mut lists = HashMap::new();
    for (name, val) in obj {
        let arr = match val.as_array() {
            Some(a) => a,
            None => continue,
        };
        let values: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if values.is_empty() {
            continue;
        }
        lists.insert(name.clone(), values);
    }
    lists
}

/// Parse FakerRules from a JSON string.
/// Input format: {"DefName.propName": "strategy", ...}
/// Splits on first dot to get (def_name, prop_name). Strategy strings map to FakerStrategy via serde.
/// Unknown strategies or "auto" are skipped (not inserted).
///
/// `custom_lists` shadows built-in strategies: if `strategy_str` matches a key
/// in `custom_lists`, the resolved FakerStrategy becomes
/// `Custom(values.clone())` — checked BEFORE serde lookup so collisions (e.g.
/// a custom list named "email") win over the built-in variant.
pub fn parse_faker_rules(
    json_str: &str,
    custom_lists: &HashMap<String, Vec<String>>,
) -> FakerRules {
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return FakerRules::new(),
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return FakerRules::new(),
    };

    let mut rules = FakerRules::new();
    for (key, val) in obj {
        let strategy_str = match val.as_str() {
            Some(s) => s,
            None => continue,
        };
        // Skip "auto" -- it means use the default heuristic
        if strategy_str == "auto" {
            continue;
        }
        // Custom list shadow: check user-defined lists FIRST so a name
        // collision with a built-in (e.g. "email") resolves to the list.
        let strategy: FakerStrategy = if let Some(values) = custom_lists.get(strategy_str) {
            FakerStrategy::Custom(values.clone())
        } else {
            match serde_json::from_value(serde_json::json!(strategy_str)) {
                Ok(s) => s,
                Err(_) => continue,
            }
        };
        // Split on first dot
        let dot = match key.find('.') {
            Some(d) => d,
            None => continue,
        };
        let def_name = &key[..dot];
        let prop_name = &key[dot + 1..];
        rules
            .entry(def_name.to_string())
            .or_default()
            .insert(prop_name.to_string(), strategy);
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_graph::build_entity_graph;
    use crate::parser::SwaggerSpec;

    fn load_petstore_resolved() -> SwaggerSpec {
        let mut spec = SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap();
        spec.resolve_refs();
        spec
    }

    fn load_petstore_raw() -> SwaggerSpec {
        SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap()
    }

    #[test]
    fn test_compose_documents_quantity_range() {
        let spec = load_petstore_resolved();
        let raw_spec = load_petstore_raw();
        let selected_ops = vec![("/pet/{petId}".to_string(), "get".to_string())];
        let graph = build_entity_graph(&raw_spec, &selected_ops);

        let mut quantities = QuantityConfigs::new();
        quantities.insert("Pet".to_string(), QuantityConfig { min: 7, max: 7 });

        let endpoints = vec![EndpointInfo {
            method: "get".to_string(),
            path: "/pet/{petId}".to_string(),
        }];

        let docs = compose_documents(
            &spec,
            &raw_spec,
            &graph,
            &quantities,
            &endpoints,
            &FakerRules::new(),
            &[],
        );

        assert!(docs.contains_key("Pet"), "should have Pet documents");
        assert_eq!(
            docs.get("Pet").unwrap().len(),
            7,
            "should have exactly 7 Pet documents"
        );

        // Each document should be an object with expected fields
        for doc in docs.get("Pet").unwrap() {
            assert!(doc.is_object());
            assert!(doc.get("id").is_some());
            assert!(doc.get("name").is_some());
            assert!(doc.get("status").is_some());
        }
    }

    #[test]
    fn test_quantity_config_deserialization() {
        let json = r#"{"Pet": {"min": 5, "max": 15}}"#;
        let configs = parse_quantity_configs(json);
        assert_eq!(configs.len(), 1);
        let pet_config = configs.get("Pet").unwrap();
        assert_eq!(pet_config.min, 5);
        assert_eq!(pet_config.max, 15);

        // Invalid JSON
        let configs = parse_quantity_configs("not json");
        assert!(configs.is_empty());

        // Empty object
        let configs = parse_quantity_configs("{}");
        assert!(configs.is_empty());
    }

    /// Helper: create a SchemaObject with string properties (no refs).
    fn string_schema(fields: &[&str]) -> SchemaObject {
        let mut props = HashMap::new();
        for &f in fields {
            props.insert(
                f.to_string(),
                SchemaObject {
                    schema_type: Some("string".to_string()),
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
                },
            );
        }
        SchemaObject {
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
        }
    }

    /// Helper: create a SchemaObject with string fields plus a $ref field.
    fn schema_with_ref(fields: &[&str], ref_field: &str, ref_target: &str) -> SchemaObject {
        let mut schema = string_schema(fields);
        let props = schema.properties.as_mut().unwrap();
        props.insert(
            ref_field.to_string(),
            SchemaObject {
                schema_type: None,
                format: None,
                properties: None,
                items: None,
                required: None,
                ref_path: Some(format!("#/definitions/{}", ref_target)),
                enum_values: None,
                description: None,
                additional_properties: None,
                all_of: None,
                x_faker: None,
            },
        );
        schema
    }

    /// Helper: create a SchemaObject whose root is allOf of the given members.
    fn schema_allof(members: Vec<SchemaObject>) -> SchemaObject {
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
            all_of: Some(members),
            x_faker: None,
        }
    }

    /// Helper: create a SchemaObject that is a pure $ref.
    fn schema_ref_only(target: &str) -> SchemaObject {
        SchemaObject {
            schema_type: None,
            format: None,
            properties: None,
            items: None,
            required: None,
            ref_path: Some(format!("#/definitions/{}", target)),
            enum_values: None,
            description: None,
            additional_properties: None,
            all_of: None,
            x_faker: None,
        }
    }

    #[test]
    fn test_flatten_allof_raw_props_inline_only() {
        // allOf of two inline members; no raw_defs needed.
        let member_a = string_schema(&["a1", "a2"]);
        let member_b = string_schema(&["b1"]);
        let schema = schema_allof(vec![member_a, member_b]);

        let merged = flatten_allof_raw_props(&schema, None, &mut HashSet::new())
            .expect("allOf present -> Some");
        let keys: HashSet<&str> = merged.keys().map(String::as_str).collect();
        assert_eq!(keys, HashSet::from(["a1", "a2", "b1"]));
    }

    #[test]
    fn test_flatten_allof_raw_props_ref_resolves_via_raw_defs() {
        // Base has {x,y}. Composed is allOf [$ref Base, inline {z}].
        let mut raw_defs: HashMap<String, SchemaObject> = HashMap::new();
        raw_defs.insert("Base".to_string(), string_schema(&["x", "y"]));

        let composed = schema_allof(vec![schema_ref_only("Base"), string_schema(&["z"])]);

        let merged = flatten_allof_raw_props(&composed, Some(&raw_defs), &mut HashSet::new())
            .expect("allOf present -> Some");
        let keys: HashSet<&str> = merged.keys().map(String::as_str).collect();
        assert_eq!(keys, HashSet::from(["x", "y", "z"]));
    }

    #[test]
    fn test_flatten_allof_raw_props_mixed_members_preserve_ref_path() {
        // An inline member contains a property that is a $ref. Its ref_path
        // must survive the merge so pool lookup can still fire downstream.
        let inline = schema_with_ref(&["title"], "owner", "Owner");
        let schema = schema_allof(vec![inline]);

        let merged = flatten_allof_raw_props(&schema, None, &mut HashSet::new())
            .expect("allOf present -> Some");
        let owner_prop = merged.get("owner").expect("owner prop present");
        assert_eq!(
            owner_prop.ref_path.as_deref(),
            Some("#/definitions/Owner"),
            "ref_path on property must be preserved for pool lookup"
        );
    }

    #[test]
    fn test_flatten_allof_raw_props_nested_allof_recurses() {
        // Outer allOf -> $ref Middle; Middle itself is allOf-rooted with
        // $ref Leaf + inline props.
        let leaf = string_schema(&["leaf_prop"]);
        let middle = schema_allof(vec![schema_ref_only("Leaf"), string_schema(&["mid_prop"])]);
        let outer = schema_allof(vec![
            schema_ref_only("Middle"),
            string_schema(&["outer_prop"]),
        ]);

        let mut raw_defs: HashMap<String, SchemaObject> = HashMap::new();
        raw_defs.insert("Leaf".to_string(), leaf);
        raw_defs.insert("Middle".to_string(), middle);

        let merged = flatten_allof_raw_props(&outer, Some(&raw_defs), &mut HashSet::new())
            .expect("allOf present -> Some");
        let keys: HashSet<&str> = merged.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            HashSet::from(["leaf_prop", "mid_prop", "outer_prop"]),
            "all levels of allOf chain should contribute properties"
        );
    }

    #[test]
    fn test_flatten_allof_raw_props_missing_ref_skipped() {
        // allOf references a def that doesn't exist in raw_defs -> skip
        // silently, return remaining keys.
        let schema = schema_allof(vec![
            schema_ref_only("DoesNotExist"),
            string_schema(&["present"]),
        ]);
        let raw_defs: HashMap<String, SchemaObject> = HashMap::new();

        let merged = flatten_allof_raw_props(&schema, Some(&raw_defs), &mut HashSet::new())
            .expect("allOf present -> Some (even with only resolvable keys)");
        let keys: HashSet<&str> = merged.keys().map(String::as_str).collect();
        assert_eq!(keys, HashSet::from(["present"]));
    }

    #[test]
    fn test_flatten_allof_raw_props_cycle_safe() {
        // A allOf [$ref B]; B allOf [$ref A]. Must not stack-overflow.
        let a = schema_allof(vec![schema_ref_only("B")]);
        let b = schema_allof(vec![schema_ref_only("A")]);
        let mut raw_defs: HashMap<String, SchemaObject> = HashMap::new();
        raw_defs.insert("A".to_string(), a.clone());
        raw_defs.insert("B".to_string(), b);

        // Walk starting from A -- cycle guard prevents infinite recursion.
        let merged = flatten_allof_raw_props(&a, Some(&raw_defs), &mut HashSet::new());
        assert!(
            merged.is_some(),
            "cyclic allOf should return Some (possibly empty), not panic"
        );
    }

    #[test]
    fn test_flatten_allof_raw_props_properties_plus_allof_merges() {
        // Schema has BOTH properties=Some and all_of=Some. Merge must include
        // both (R2 gate widening: gate on all_of.is_some(), not
        // properties.is_none()).
        let mut schema = string_schema(&["own_prop"]);
        schema.all_of = Some(vec![string_schema(&["allof_prop"])]);

        let merged = flatten_allof_raw_props(&schema, None, &mut HashSet::new())
            .expect("allOf present -> Some");
        let keys: HashSet<&str> = merged.keys().map(String::as_str).collect();
        assert_eq!(keys, HashSet::from(["own_prop", "allof_prop"]));
    }
}
