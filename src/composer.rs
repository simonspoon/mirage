// Document-based seeder with shared entity pools

use std::collections::{HashMap, HashSet, VecDeque};

use rand::RngExt;

use crate::entity_graph::EntityGraph;
use crate::parser::{SchemaObject, SwaggerSpec};
use crate::rules::{self, FieldRuleMap, Rule, build_compare_rules_by_def, build_field_rule_map};
use crate::seeder::{FakerStrategy, fake_value_for_field, fake_value_for_field_layered};
use crate::server::EndpointInfo;

pub type FakerRules = HashMap<String, HashMap<String, FakerStrategy>>;

pub type SharedPoolConfig = HashMap<String, usize>;

pub struct QuantityConfig {
    pub min: usize,
    pub max: usize,
}

pub type QuantityConfigs = HashMap<String, QuantityConfig>;

pub type DocumentStore = HashMap<String, Vec<serde_json::Value>>;

/// Generate pools of shared entities according to pool_config.
/// Each pool entry maps a definition name to N generated instances.
/// `raw_spec` retains `$ref` paths (cleared by resolve_refs) and is used
/// to discover inter-pool dependencies so pools are generated in
/// topological order. `spec` (resolved) is used for actual generation.
pub fn generate_pools(
    spec: &SwaggerSpec,
    raw_spec: &SwaggerSpec,
    pool_config: &SharedPoolConfig,
    faker_rules: &FakerRules,
    recipe_rules: &[Rule],
) -> DocumentStore {
    let defs = match &spec.definitions {
        Some(d) => d,
        None => return DocumentStore::new(),
    };

    let raw_defs = raw_spec.definitions.as_ref();

    let field_rule_map = build_field_rule_map(recipe_rules);
    let compare_rules_by_def = build_compare_rules_by_def(recipe_rules);

    // Topologically sort pool definitions so dependencies generate first.
    let sorted_names = topo_sort_pool_defs(raw_spec, pool_config);

    let mut store = DocumentStore::new();

    for def_name in &sorted_names {
        let pool_size = match pool_config.get(def_name) {
            Some(&s) => s,
            None => continue,
        };
        let schema = match defs.get(def_name) {
            Some(s) => s,
            None => continue,
        };
        let raw_schema = raw_defs.and_then(|d| d.get(def_name));

        let mut instances = Vec::with_capacity(pool_size);
        for i in 0..pool_size {
            let mut doc = generate_document_from_schema(
                def_name,
                schema,
                raw_schema,
                raw_defs,
                &store,
                faker_rules,
                &field_rule_map,
                compare_rules_by_def.get(def_name).map(Vec::as_slice),
            );
            // Assign a stable id to each pool entity
            if let serde_json::Value::Object(ref mut map) = doc {
                map.insert(
                    "id".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(i + 1)),
                );
            }
            instances.push(doc);
        }
        store.insert(def_name.clone(), instances);
    }

    store
}

/// Build a dependency graph from the raw (unresolved) spec and topologically
/// sort pool definition names using Kahn's algorithm.
/// Nodes involved in cycles are appended after sorted nodes (graceful degradation).
fn topo_sort_pool_defs(raw_spec: &SwaggerSpec, pool_config: &SharedPoolConfig) -> Vec<String> {
    let raw_defs = match &raw_spec.definitions {
        Some(d) => d,
        None => {
            let mut names: Vec<String> = pool_config.keys().cloned().collect();
            names.sort();
            return names;
        }
    };

    let pool_names: HashSet<String> = pool_config.keys().cloned().collect();

    // edges[A] = {B, ...} means "A depends on B" (B must generate before A).
    let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
    for name in &pool_names {
        edges.entry(name.clone()).or_default();
    }

    for name in &pool_names {
        if let Some(schema) = raw_defs.get(name) {
            let deps = collect_ref_deps(schema);
            for dep in deps {
                if pool_names.contains(&dep) && dep != *name {
                    edges.entry(name.clone()).or_default().insert(dep);
                }
            }
        }
    }

    // in_degree[X] = number of dependencies X has (edges from X to others).
    let mut in_deg: HashMap<String, usize> = HashMap::new();
    for name in &pool_names {
        in_deg.insert(name.clone(), 0);
    }
    for (name, deps) in &edges {
        // name depends on each dep, so in topo order name comes after its deps.
        // In Kahn's with "before" edges (dep -> name), in_degree of name = deps.len().
        *in_deg.entry(name.clone()).or_insert(0) = deps.len();
    }

    // Reverse edges: rev[dep] = set of names that depend on dep
    let mut rev_edges: HashMap<String, Vec<String>> = HashMap::new();
    for (name, deps) in &edges {
        for dep in deps {
            rev_edges.entry(dep.clone()).or_default().push(name.clone());
        }
    }

    // Start with nodes that have no dependencies (in_degree == 0)
    let mut initial: Vec<String> = in_deg
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    initial.sort();

    let mut queue: VecDeque<String> = initial.into_iter().collect();
    let mut sorted: Vec<String> = Vec::new();

    while let Some(name) = queue.pop_front() {
        sorted.push(name.clone());
        if let Some(dependents) = rev_edges.get(&name) {
            let mut next: Vec<String> = Vec::new();
            for dependent in dependents {
                let deg = in_deg.get_mut(dependent).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    next.push(dependent.clone());
                }
            }
            next.sort();
            for n in next {
                queue.push_back(n);
            }
        }
    }

    // Append any nodes stuck in cycles (graceful degradation)
    if sorted.len() < pool_names.len() {
        let sorted_set: HashSet<&str> = sorted.iter().map(|s| s.as_str()).collect();
        let mut remaining: Vec<String> = pool_names
            .iter()
            .filter(|n| !sorted_set.contains(n.as_str()))
            .cloned()
            .collect();
        remaining.sort();
        sorted.extend(remaining);
    }

    sorted
}

/// Collect definition names referenced via `$ref` in a schema's properties and
/// array items. Scans one level deep (direct properties). For allOf-rooted
/// schemas, also scans each allOf member's properties and the member's own
/// `$ref` (so topo sort picks up pool deps that live inside allOf members).
fn collect_ref_deps(schema: &SchemaObject) -> Vec<String> {
    let mut deps = Vec::new();
    collect_ref_deps_from_props(&schema.properties, &mut deps);
    if let Some(ref all_of) = schema.all_of {
        for member in all_of {
            if let Some(ref ref_path) = member.ref_path
                && let Some(name) = ref_path.strip_prefix("#/definitions/")
            {
                deps.push(name.to_string());
            }
            collect_ref_deps_from_props(&member.properties, &mut deps);
        }
    }
    deps
}

fn collect_ref_deps_from_props(
    props: &Option<HashMap<String, SchemaObject>>,
    deps: &mut Vec<String>,
) {
    if let Some(props) = props {
        for prop_schema in props.values() {
            if let Some(ref ref_path) = prop_schema.ref_path
                && let Some(name) = ref_path.strip_prefix("#/definitions/")
            {
                deps.push(name.to_string());
            }
            // Check array items
            if let Some(ref items) = prop_schema.items
                && let Some(ref ref_path) = items.ref_path
                && let Some(name) = ref_path.strip_prefix("#/definitions/")
            {
                deps.push(name.to_string());
            }
        }
    }
}

/// Compose documents for each selected endpoint's response definition.
/// Uses shared pools where configured, generates fresh fakes otherwise.
/// `spec` (resolved) is used for schema generation (inlined properties).
/// `raw_spec` (unresolved) is used for definition name lookup (retains $ref paths).
#[allow(clippy::too_many_arguments)]
pub fn compose_documents(
    spec: &SwaggerSpec,
    raw_spec: &SwaggerSpec,
    _graph: &EntityGraph,
    pools: &DocumentStore,
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
                pools,
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

/// Generate a single document from a schema, sampling from pools for $ref properties.
/// When `raw_schema` is provided, its property `ref_path` values are used for pool
/// lookups (the resolved schema has `ref_path` cleared by `resolve_refs`).
#[allow(clippy::too_many_arguments)]
fn generate_document_from_schema(
    def_name: &str,
    schema: &SchemaObject,
    raw_schema: Option<&SchemaObject>,
    raw_defs: Option<&HashMap<String, SchemaObject>>,
    pools: &DocumentStore,
    faker_rules: &FakerRules,
    field_rule_map: &FieldRuleMap,
    compare_rules: Option<&[Rule]>,
) -> serde_json::Value {
    let props = match &schema.properties {
        Some(p) => p,
        None => return fake_value_for_field(def_name, schema),
    };

    // When raw_schema has no direct properties but uses allOf at the definition
    // root, flatten its allOf members (following $ref chains via raw_defs) so
    // property-level $ref lookups for pool sampling still work. Binding this
    // owner before `raw_props` keeps the borrow alive across the loop below.
    let flattened_raw_props =
        raw_schema.and_then(|rs| flatten_allof_raw_props(rs, raw_defs, &mut HashSet::new()));

    let raw_props = raw_schema
        .and_then(|s| s.properties.as_ref())
        .or(flattened_raw_props.as_ref());

    let mut map = serde_json::Map::new();
    let mut rng = rand::rng();

    for (prop_name, prop_schema) in props {
        let raw_prop = raw_props.and_then(|rp| rp.get(prop_name));
        let value = generate_property_value(
            prop_name,
            prop_schema,
            raw_prop,
            pools,
            &mut rng,
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

/// Generate a value for a single property, consulting pools for $ref targets.
/// `raw_prop` (from the unresolved spec) is checked first for `ref_path` since
/// the resolved schema has those cleared.
#[allow(clippy::too_many_arguments)]
fn generate_property_value(
    prop_name: &str,
    prop_schema: &SchemaObject,
    raw_prop: Option<&SchemaObject>,
    pools: &DocumentStore,
    rng: &mut impl rand::Rng,
    def_name: &str,
    faker_rules: &FakerRules,
    field_rule_map: &FieldRuleMap,
) -> serde_json::Value {
    // Check if this is an array with $ref items pointing to a pool
    if prop_schema.schema_type.as_deref() == Some("array")
        || raw_prop.and_then(|r| r.schema_type.as_deref()) == Some("array")
    {
        // Try raw_prop items ref first, then resolved items ref
        let raw_items_ref = raw_prop
            .and_then(|r| r.items.as_ref())
            .and_then(|i| ref_target_name(i));
        let resolved_items_ref = prop_schema.items.as_ref().and_then(|i| ref_target_name(i));
        let target = raw_items_ref.or(resolved_items_ref);

        if let Some(target_def) = target
            && let Some(pool) = pools.get(&target_def)
            && !pool.is_empty()
        {
            let count = rng.random_range(1..=3usize);
            let arr: Vec<serde_json::Value> = (0..count)
                .map(|_| {
                    let idx = rng.random_range(0..pool.len());
                    pool[idx].clone()
                })
                .collect();
            return serde_json::Value::Array(arr);
        }
        // Otherwise generate fresh array items
        return fake_value_for_field(prop_name, prop_schema);
    }

    // Check if this property references a pooled definition (object $ref)
    // Try raw_prop ref first, then resolved schema ref
    let raw_ref = raw_prop.and_then(ref_target_name);
    let resolved_ref = ref_target_name(prop_schema);
    let target = raw_ref.or(resolved_ref);

    if let Some(target_def) = target
        && let Some(pool) = pools.get(&target_def)
        && !pool.is_empty()
    {
        let idx = rng.random_range(0..pool.len());
        return pool[idx].clone();
    }

    let recipe_rule = field_rule_map.get(&(def_name.to_string(), prop_name.to_string()));
    let faker_rule = faker_rules.get(def_name).and_then(|m| m.get(prop_name));
    fake_value_for_field_layered(prop_name, prop_schema, recipe_rule, faker_rule)
}

/// Extract the definition name from a schema if it has a $ref.
/// Works on resolved schemas that still have ref_path, and also on schemas
/// that have been resolved (properties inlined) by checking if the schema
/// looks like a known definition.
fn ref_target_name(schema: &SchemaObject) -> Option<String> {
    if let Some(ref ref_path) = schema.ref_path {
        return ref_path
            .strip_prefix("#/definitions/")
            .map(|s| s.to_string());
    }
    None
}

/// Flatten a raw schema's allOf members into a single property map, preserving
/// `ref_path` values on property schemas so pool lookups in
/// `generate_property_value` still fire. Mirrors the allOf merge in
/// `parser::resolve_schema` (vec-order `.extend`, later wins).
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

/// Parse a SharedPoolConfig from a JSON string. Returns empty map on parse failure.
pub fn parse_shared_pools(json_str: &str) -> SharedPoolConfig {
    // The recipe stores shared_pools as {"DefName": {"is_shared": true, "pool_size": N}, ...}
    // We extract definition names where is_shared is true and map to pool_size.
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return SharedPoolConfig::new(),
    };

    let obj = match parsed.as_object() {
        Some(o) => o,
        None => return SharedPoolConfig::new(),
    };

    let mut config = SharedPoolConfig::new();
    for (def_name, val) in obj {
        if let Some(inner) = val.as_object() {
            let is_shared = inner
                .get("is_shared")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if is_shared {
                let pool_size =
                    inner.get("pool_size").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                config.insert(def_name.clone(), pool_size);
            }
        } else if let Some(size) = val.as_u64() {
            // Simple format: {"DefName": 5}
            config.insert(def_name.clone(), size as usize);
        }
    }
    config
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

/// Parse FakerRules from a JSON string.
/// Input format: {"DefName.propName": "strategy", ...}
/// Splits on first dot to get (def_name, prop_name). Strategy strings map to FakerStrategy via serde.
/// Unknown strategies or "auto" are skipped (not inserted).
pub fn parse_faker_rules(json_str: &str) -> FakerRules {
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
        let strategy: FakerStrategy = match serde_json::from_value(serde_json::json!(strategy_str))
        {
            Ok(s) => s,
            Err(_) => continue,
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
    fn test_generate_pools_exact_count() {
        let spec = load_petstore_resolved();
        let raw_spec = load_petstore_raw();
        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("Category".to_string(), 5);
        pool_config.insert("Tag".to_string(), 3);

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        assert_eq!(pools.get("Category").unwrap().len(), 5);
        assert_eq!(pools.get("Tag").unwrap().len(), 3);

        // Each pool entity should be an object with an id
        for doc in pools.get("Category").unwrap() {
            assert!(doc.is_object(), "pool entity should be an object");
            assert!(doc.get("id").is_some(), "pool entity should have id");
            assert!(doc.get("name").is_some(), "Category should have name");
        }
    }

    #[test]
    fn test_generate_pools_empty() {
        let spec = load_petstore_resolved();
        let raw_spec = load_petstore_raw();
        let pool_config = SharedPoolConfig::new();

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        assert!(pools.is_empty(), "empty config should produce empty pools");
    }

    #[test]
    fn test_compose_documents_quantity_range() {
        let spec = load_petstore_resolved();
        let raw_spec = load_petstore_raw();
        let selected_ops = vec![("/pet/{petId}".to_string(), "get".to_string())];
        let graph = build_entity_graph(&raw_spec, &selected_ops);

        let pools = DocumentStore::new();
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
            &pools,
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
    fn test_pool_config_deserialization() {
        // Recipe-style format: {"Pet": {"is_shared": true, "pool_size": 5}}
        let json = r#"{"Category": {"is_shared": true, "pool_size": 5}, "Tag": {"is_shared": false, "pool_size": 3}}"#;
        let config = parse_shared_pools(json);
        assert_eq!(config.len(), 1, "only Category is_shared=true");
        assert_eq!(*config.get("Category").unwrap(), 5);

        // Simple format: {"Category": 10}
        let json = r#"{"Category": 10}"#;
        let config = parse_shared_pools(json);
        assert_eq!(*config.get("Category").unwrap(), 10);

        // Invalid JSON
        let config = parse_shared_pools("not json");
        assert!(config.is_empty());

        // Empty object
        let config = parse_shared_pools("{}");
        assert!(config.is_empty());
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

    #[test]
    fn test_compose_with_pools() {
        let spec = load_petstore_resolved();
        let raw_spec = load_petstore_raw();
        let selected_ops = vec![("/pet/{petId}".to_string(), "get".to_string())];
        let graph = build_entity_graph(&raw_spec, &selected_ops);

        // Create a Category pool
        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("Category".to_string(), 3);
        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        // Capture the Category pool's (id, name) tuples for subset assertion below.
        let category_pool_tuples: HashSet<(i64, String)> = pools
            .get("Category")
            .expect("Category pool generated")
            .iter()
            .map(|c| {
                (
                    c["id"].as_i64().expect("pool Category.id must be integer"),
                    c["name"]
                        .as_str()
                        .expect("pool Category.name must be string")
                        .to_string(),
                )
            })
            .collect();

        let mut quantities = QuantityConfigs::new();
        quantities.insert("Pet".to_string(), QuantityConfig { min: 5, max: 5 });

        let endpoints = vec![EndpointInfo {
            method: "get".to_string(),
            path: "/pet/{petId}".to_string(),
        }];

        let docs = compose_documents(
            &spec,
            &raw_spec,
            &graph,
            &pools,
            &quantities,
            &endpoints,
            &FakerRules::new(),
            &[],
        );

        let pets = docs.get("Pet").unwrap();
        assert_eq!(pets.len(), 5);

        for pet in pets {
            assert!(pet.is_object());
            let category = pet.get("category").expect("pet.category present");
            assert!(category.is_object(), "pet.category must be object");
            let cat_id = category["id"]
                .as_i64()
                .expect("pet.category.id must be integer from pool");
            let cat_name = category["name"]
                .as_str()
                .expect("pet.category.name must be string from pool")
                .to_string();
            assert!(
                (1..=3).contains(&cat_id),
                "pet.category.id must be in 1..=3, got {cat_id}"
            );
            assert!(
                category_pool_tuples.contains(&(cat_id, cat_name.clone())),
                "pet.category (id={cat_id}, name={cat_name}) must match a pool entry"
            );
        }
    }

    /// Build a minimal SwaggerSpec with the given definitions.
    fn build_spec(defs: HashMap<String, SchemaObject>) -> SwaggerSpec {
        SwaggerSpec {
            swagger: "2.0".to_string(),
            info: crate::parser::Info {
                title: "test".to_string(),
                version: "1.0".to_string(),
            },
            paths: HashMap::new(),
            definitions: Some(defs),
        }
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

    /// Helper: create a SchemaObject with string fields plus an array-of-$ref field.
    fn schema_with_array_ref(fields: &[&str], ref_field: &str, ref_target: &str) -> SchemaObject {
        let mut schema = string_schema(fields);
        let props = schema.properties.as_mut().unwrap();
        props.insert(
            ref_field.to_string(),
            SchemaObject {
                schema_type: Some("array".to_string()),
                format: None,
                properties: None,
                items: Some(Box::new(SchemaObject {
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
                })),
                required: None,
                ref_path: None,
                enum_values: None,
                description: None,
                additional_properties: None,
                all_of: None,
                x_faker: None,
            },
        );
        schema
    }

    /// Helper: build a resolved version of a raw spec by inlining refs.
    fn resolve(raw: &SwaggerSpec) -> SwaggerSpec {
        let mut spec = raw.clone();
        spec.resolve_refs();
        spec
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
    fn generate_pools_ref_field_samples_from_accumulated_store() {
        // PatientDto has {name: string}
        // HospitalListDto has {patient: $ref PatientDto, address: string}
        let mut defs = HashMap::new();
        defs.insert("PatientDto".to_string(), string_schema(&["name"]));
        defs.insert(
            "HospitalListDto".to_string(),
            schema_with_ref(&["address"], "patient", "PatientDto"),
        );

        let raw_spec = build_spec(defs);
        let spec = resolve(&raw_spec);

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("PatientDto".to_string(), 3);
        pool_config.insert("HospitalListDto".to_string(), 2);

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        assert_eq!(pools.get("PatientDto").unwrap().len(), 3);
        assert_eq!(pools.get("HospitalListDto").unwrap().len(), 2);

        // Each HospitalListDto.patient should be one of the PatientDto pool entries
        let patient_pool = pools.get("PatientDto").unwrap();
        for hospital in pools.get("HospitalListDto").unwrap() {
            let patient = hospital.get("patient").expect("should have patient field");
            assert!(
                patient.is_object(),
                "patient should be an object sampled from pool"
            );
            // The patient value should match one of the pool entries (by id)
            let patient_id = patient.get("id");
            assert!(
                patient_id.is_some(),
                "sampled patient should have id from pool"
            );
            let matches = patient_pool.iter().any(|p| p.get("id") == patient_id);
            assert!(matches, "patient should match a pool entry by id");
        }
    }

    #[test]
    fn generate_pools_dependency_order_is_deterministic() {
        let mut defs = HashMap::new();
        defs.insert("PatientDto".to_string(), string_schema(&["name"]));
        defs.insert(
            "HospitalListDto".to_string(),
            schema_with_ref(&["address"], "patient", "PatientDto"),
        );

        let raw_spec = build_spec(defs);
        let spec = resolve(&raw_spec);

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("PatientDto".to_string(), 3);
        pool_config.insert("HospitalListDto".to_string(), 2);

        // Run 20 times and verify structure is always correct
        for _ in 0..20 {
            let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);
            assert_eq!(pools.get("PatientDto").unwrap().len(), 3);
            assert_eq!(pools.get("HospitalListDto").unwrap().len(), 2);

            let patient_pool = pools.get("PatientDto").unwrap();
            for hospital in pools.get("HospitalListDto").unwrap() {
                let patient = hospital.get("patient").unwrap();
                let patient_id = patient.get("id");
                assert!(patient_id.is_some());
                assert!(patient_pool.iter().any(|p| p.get("id") == patient_id));
            }
        }
    }

    #[test]
    fn generate_pools_three_level_chain() {
        // C has {name: string}
        // B has {label: string, c: $ref C}
        // A has {title: string, b: $ref B}
        let mut defs = HashMap::new();
        defs.insert("C".to_string(), string_schema(&["name"]));
        defs.insert("B".to_string(), schema_with_ref(&["label"], "c", "C"));
        defs.insert("A".to_string(), schema_with_ref(&["title"], "b", "B"));

        let raw_spec = build_spec(defs);
        let spec = resolve(&raw_spec);

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("C".to_string(), 4);
        pool_config.insert("B".to_string(), 3);
        pool_config.insert("A".to_string(), 2);

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        assert_eq!(pools.get("C").unwrap().len(), 4);
        assert_eq!(pools.get("B").unwrap().len(), 3);
        assert_eq!(pools.get("A").unwrap().len(), 2);

        let c_pool = pools.get("C").unwrap();
        let b_pool = pools.get("B").unwrap();

        // Each B.c should match a C pool entry
        for b in b_pool {
            let c_val = b.get("c").expect("B should have c field");
            assert!(c_val.is_object());
            assert!(c_pool.iter().any(|c| c.get("id") == c_val.get("id")));
        }

        // Each A.b should match a B pool entry
        for a in pools.get("A").unwrap() {
            let b_val = a.get("b").expect("A should have b field");
            assert!(b_val.is_object());
            assert!(b_pool.iter().any(|b| b.get("id") == b_val.get("id")));
        }
    }

    #[test]
    fn generate_pools_array_ref_samples_from_accumulated_store() {
        // TagDto has {label: string}
        // PostDto has {title: string, tags: array of $ref TagDto}
        let mut defs = HashMap::new();
        defs.insert("TagDto".to_string(), string_schema(&["label"]));
        defs.insert(
            "PostDto".to_string(),
            schema_with_array_ref(&["title"], "tags", "TagDto"),
        );

        let raw_spec = build_spec(defs);
        let spec = resolve(&raw_spec);

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("TagDto".to_string(), 5);
        pool_config.insert("PostDto".to_string(), 3);

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        assert_eq!(pools.get("TagDto").unwrap().len(), 5);
        assert_eq!(pools.get("PostDto").unwrap().len(), 3);

        let tag_pool = pools.get("TagDto").unwrap();
        for post in pools.get("PostDto").unwrap() {
            let tags = post.get("tags").expect("PostDto should have tags field");
            assert!(tags.is_array(), "tags should be an array");
            let tags_arr = tags.as_array().unwrap();
            assert!(!tags_arr.is_empty(), "tags array should not be empty");
            for tag in tags_arr {
                assert!(tag.is_object(), "each tag should be an object from pool");
                let tag_id = tag.get("id");
                assert!(tag_id.is_some());
                assert!(tag_pool.iter().any(|t| t.get("id") == tag_id));
            }
        }
    }

    #[test]
    fn test_generate_pools_mega_owner_shape() {
        // Parse the mega fixture both raw and resolved, then generate a shared
        // pool of 3 Owners. Verify ids == {1,2,3} and every entry has the
        // expected cross-definition $ref shape (name + address.city/country).
        let raw_spec = SwaggerSpec::from_file("tests/fixtures/mega.yaml").unwrap();
        let mut spec = SwaggerSpec::from_file("tests/fixtures/mega.yaml").unwrap();
        spec.resolve_refs();

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("Owner".to_string(), 3);

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        let owner_pool = pools.get("Owner").expect("Owner pool generated");
        assert_eq!(owner_pool.len(), 3, "Owner pool should have 3 entries");

        let ids: HashSet<i64> = owner_pool
            .iter()
            .map(|e| {
                e["id"]
                    .as_i64()
                    .expect("Owner entry should have integer id")
            })
            .collect();
        assert_eq!(
            ids,
            HashSet::from([1, 2, 3]),
            "Owner pool ids should be exactly {{1,2,3}}"
        );

        for entry in owner_pool {
            assert!(entry.is_object(), "Owner entry should be an object");
            assert!(
                entry["name"].is_string(),
                "Owner.name must be string — entry: {entry}"
            );
            let address = entry
                .get("address")
                .expect("Owner should have address field");
            assert!(
                address.is_object(),
                "Owner.address must be an object (cross-def $ref resolved) — entry: {entry}"
            );
            assert!(
                address["city"].is_string(),
                "Owner.address.city must be string — entry: {entry}"
            );
            assert!(
                address["country"].is_string(),
                "Owner.address.country must be string — entry: {entry}"
            );
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

    #[test]
    fn test_compose_with_pools_allof_root() {
        // Mirrors mega.yaml ComposedEntity shape:
        //   Address { city, country }
        //   Owner   { name, address: $ref Address }
        //   BaseAudit { created_at, updated_at }
        //   ComposedEntity allOf [$ref BaseAudit, inline { title, priority, owner: $ref Owner }]
        //
        // Pool config: Owner=3, ComposedEntity=4. Assertions:
        //   * owner_ids observed on ComposedEntity.owner all drawn from {1,2,3}
        //   * owner.address present as object (proves real pool object, not faked)
        let mut defs: HashMap<String, SchemaObject> = HashMap::new();
        defs.insert("Address".to_string(), string_schema(&["city", "country"]));
        defs.insert(
            "Owner".to_string(),
            schema_with_ref(&["name"], "address", "Address"),
        );
        defs.insert(
            "BaseAudit".to_string(),
            string_schema(&["created_at", "updated_at"]),
        );

        // Inline member: { title, priority, owner: $ref Owner }
        let inline_member = schema_with_ref(&["title", "priority"], "owner", "Owner");
        defs.insert(
            "ComposedEntity".to_string(),
            schema_allof(vec![schema_ref_only("BaseAudit"), inline_member]),
        );

        let raw_spec = build_spec(defs);
        let spec = resolve(&raw_spec);

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("Owner".to_string(), 3);
        pool_config.insert("ComposedEntity".to_string(), 4);

        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        let owner_pool = pools.get("Owner").expect("Owner pool generated");
        assert_eq!(owner_pool.len(), 3);
        let owner_id_set: HashSet<i64> = owner_pool
            .iter()
            .map(|o| o["id"].as_i64().expect("Owner.id must be integer"))
            .collect();
        assert_eq!(owner_id_set, HashSet::from([1, 2, 3]));

        let composed = pools
            .get("ComposedEntity")
            .expect("ComposedEntity pool generated");
        assert_eq!(composed.len(), 4);

        for entry in composed {
            let owner = entry
                .get("owner")
                .expect("ComposedEntity.owner present (allOf-rooted)");
            assert!(owner.is_object(), "owner must be sampled pool object");
            let oid = owner["id"]
                .as_i64()
                .expect("sampled owner.id must be integer");
            assert!(
                owner_id_set.contains(&oid),
                "owner.id {oid} must be drawn from Owner pool ids {owner_id_set:?}"
            );
            // Proves the owner came from the real pool (which itself had its
            // $ref address resolved) and not from synthetic fake generation.
            assert!(
                owner.get("address").map(|a| a.is_object()).unwrap_or(false),
                "owner.address must be an object (real pool entry) -- entry: {entry}"
            );
        }
    }

    #[test]
    fn generate_pools_cycle_does_not_panic() {
        // X has {y: $ref Y}
        // Y has {x: $ref X}
        let mut defs = HashMap::new();
        defs.insert("X".to_string(), schema_with_ref(&["name"], "y", "Y"));
        defs.insert("Y".to_string(), schema_with_ref(&["label"], "x", "X"));

        let raw_spec = build_spec(defs);
        let spec = resolve(&raw_spec);

        let mut pool_config = SharedPoolConfig::new();
        pool_config.insert("X".to_string(), 2);
        pool_config.insert("Y".to_string(), 2);

        // Should not panic -- cycles degrade gracefully
        let pools = generate_pools(&spec, &raw_spec, &pool_config, &FakerRules::new(), &[]);

        assert_eq!(pools.get("X").unwrap().len(), 2);
        assert_eq!(pools.get("Y").unwrap().len(), 2);
    }
}
