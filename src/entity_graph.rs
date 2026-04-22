use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::parser::{self, ResponseShape, SchemaObject, SwaggerSpec, collect_schema_refs};
use crate::server::EndpointInfo;

#[derive(Debug, Clone, Serialize)]
pub struct ArrayPropInfo {
    pub def_name: String,
    pub prop_name: String,
    pub target_def: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScalarPropInfo {
    pub def_name: String,
    pub prop_name: String,
    pub prop_type: String,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualRoot {
    pub endpoint: EndpointInfo,
    pub shape: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointEdge {
    pub endpoint: EndpointInfo,
    pub target_def: String,
    pub direction: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityGraph {
    pub nodes: Vec<String>,
    pub roots: HashMap<String, Vec<EndpointInfo>>,
    pub edges: HashMap<String, Vec<String>>,
    pub shared_entities: Vec<String>,
    pub array_properties: Vec<ArrayPropInfo>,
    pub scalar_properties: Vec<ScalarPropInfo>,
    pub virtual_roots: Vec<VirtualRoot>,
    pub endpoint_edges: Vec<EndpointEdge>,
}

/// Extract the root definition name from a schema's $ref (or items.$ref for arrays).
///
/// When `spec_defs` is `Some`, performs a strict one-hop unwrap for Shape B
/// (`$ref → named def of type:array + items:$ref`) so the element def name
/// is returned instead of the wrapper. Mirrors
/// `parser::classify_response_schema` Shape B semantics.
fn root_def_name(
    schema: &SchemaObject,
    spec_defs: Option<&HashMap<String, SchemaObject>>,
) -> Option<String> {
    if let Some(ref ref_path) = schema.ref_path {
        let name = ref_path.strip_prefix("#/definitions/")?.to_string();
        // Shape B one-hop unwrap when spec_defs is available.
        if let Some(defs) = spec_defs
            && let Some(target) = defs.get(&name)
            && target.schema_type.as_deref() == Some("array")
            && let Some(ref items) = target.items
            && let Some(ref items_ref) = items.ref_path
            && let Some(element) = items_ref.strip_prefix("#/definitions/")
        {
            return Some(element.to_string());
        }
        return Some(name);
    }
    if schema.schema_type.as_deref() == Some("array")
        && let Some(ref items) = schema.items
        && let Some(ref ref_path) = items.ref_path
    {
        return ref_path
            .strip_prefix("#/definitions/")
            .map(|s| s.to_string());
    }
    None
}

/// Extract direct child definition references from a single schema object (not transitive).
fn direct_child_refs(schema: &SchemaObject) -> Vec<String> {
    let mut children = Vec::new();
    if let Some(props) = &schema.properties {
        for prop in props.values() {
            if let Some(ref ref_path) = prop.ref_path
                && let Some(name) = ref_path.strip_prefix("#/definitions/")
            {
                children.push(name.to_string());
            }
            if let Some(ref items) = prop.items
                && let Some(ref ref_path) = items.ref_path
                && let Some(name) = ref_path.strip_prefix("#/definitions/")
            {
                children.push(name.to_string());
            }
        }
    }
    if let Some(ref items) = schema.items
        && let Some(ref ref_path) = items.ref_path
        && let Some(name) = ref_path.strip_prefix("#/definitions/")
    {
        children.push(name.to_string());
    }
    if let Some(all_of) = &schema.all_of {
        for member in all_of {
            if let Some(ref ref_path) = member.ref_path
                && let Some(name) = ref_path.strip_prefix("#/definitions/")
            {
                children.push(name.to_string());
            }
        }
    }
    if let Some(ap) = &schema.additional_properties
        && let Some(ref ref_path) = ap.ref_path
        && let Some(name) = ref_path.strip_prefix("#/definitions/")
    {
        children.push(name.to_string());
    }
    children.sort();
    children.dedup();
    children
}

fn find_array_properties(
    nodes: &HashSet<String>,
    spec_defs: Option<&HashMap<String, SchemaObject>>,
) -> Vec<ArrayPropInfo> {
    let mut result = Vec::new();
    let defs = match spec_defs {
        Some(d) => d,
        None => return result,
    };
    for node in nodes {
        if let Some(def) = defs.get(node)
            && let Some(props) = &def.properties
        {
            for (prop_name, prop_schema) in props {
                if prop_schema.schema_type.as_deref() == Some("array")
                    && let Some(ref items) = prop_schema.items
                    && let Some(ref ref_path) = items.ref_path
                    && let Some(target) = ref_path.strip_prefix("#/definitions/")
                    && nodes.contains(target)
                {
                    result.push(ArrayPropInfo {
                        def_name: node.clone(),
                        prop_name: prop_name.clone(),
                        target_def: target.to_string(),
                    });
                }
            }
        }
    }
    result.sort_by(|a, b| (&a.def_name, &a.prop_name).cmp(&(&b.def_name, &b.prop_name)));
    result
}

fn find_scalar_properties(
    nodes: &HashSet<String>,
    spec_defs: Option<&HashMap<String, SchemaObject>>,
) -> Vec<ScalarPropInfo> {
    let mut result = Vec::new();
    let defs = match spec_defs {
        Some(d) => d,
        None => return result,
    };
    for node in nodes {
        if let Some(def) = defs.get(node)
            && let Some(props) = &def.properties
        {
            for (prop_name, prop_schema) in props {
                // Skip if it has a $ref
                if prop_schema.ref_path.is_some() {
                    continue;
                }
                let schema_type = match prop_schema.schema_type.as_deref() {
                    Some("string") | Some("integer") | Some("number") | Some("boolean") => {
                        prop_schema.schema_type.as_deref().unwrap()
                    }
                    _ => continue,
                };
                result.push(ScalarPropInfo {
                    def_name: node.clone(),
                    prop_name: prop_name.clone(),
                    prop_type: schema_type.to_string(),
                    format: prop_schema.format.clone(),
                });
            }
        }
    }
    result.sort_by(|a, b| (&a.def_name, &a.prop_name).cmp(&(&b.def_name, &b.prop_name)));
    result
}

fn shape_to_label(shape: &ResponseShape) -> String {
    match shape {
        ResponseShape::Primitive(t) => t.clone(),
        ResponseShape::PrimitiveArray(t) => format!("array<{t}>"),
        ResponseShape::FreeformObject => "object<freeform>".to_string(),
        ResponseShape::Empty => "empty".to_string(),
        ResponseShape::Definition(n) => n.clone(),
    }
}

pub fn build_entity_graph(spec: &SwaggerSpec, selected: &[(String, String)]) -> EntityGraph {
    let spec_defs = spec.definitions.as_ref();
    let mut all_nodes: HashSet<String> = HashSet::new();
    let mut roots: HashMap<String, Vec<EndpointInfo>> = HashMap::new();
    let mut virtual_roots: Vec<VirtualRoot> = Vec::new();
    let mut endpoint_edges: Vec<EndpointEdge> = Vec::new();

    for (path, method) in selected {
        let path_item = match spec.paths.get(path.as_str()) {
            Some(pi) => pi,
            None => continue,
        };
        let op = match method.as_str() {
            "get" => path_item.get.as_ref(),
            "post" => path_item.post.as_ref(),
            "put" => path_item.put.as_ref(),
            "delete" => path_item.delete.as_ref(),
            "patch" => path_item.patch.as_ref(),
            _ => None,
        };
        let op = match op {
            Some(o) => o,
            None => continue,
        };

        let mut endpoint_roots: Vec<String> = Vec::new();

        // Check response schemas for root definition (200, 201, then first 2xx)
        let mut response_root: Option<String> = None;
        for code in &["200", "201"] {
            if let Some(resp) = op.responses.get(*code)
                && let Some(schema) = &resp.schema
                && let Some(name) = root_def_name(schema, spec_defs)
            {
                response_root = Some(name);
                break;
            }
        }
        if response_root.is_none() {
            let mut keys: Vec<&String> = op.responses.keys().collect();
            keys.sort();
            for key in keys {
                if key.starts_with('2')
                    && key != "200"
                    && key != "201"
                    && let Some(schema) = &op.responses[key].schema
                    && let Some(name) = root_def_name(schema, spec_defs)
                {
                    response_root = Some(name);
                    break;
                }
            }
        }
        if let Some(name) = response_root {
            if let Some(defs) = spec_defs
                && defs.contains_key(&name)
            {
                endpoint_edges.push(EndpointEdge {
                    endpoint: EndpointInfo {
                        method: method.clone(),
                        path: path.clone(),
                    },
                    target_def: name.clone(),
                    direction: "output".to_string(),
                });
            }
            endpoint_roots.push(name);
        }

        // Check body parameters for $ref (body params are request-side; no
        // Shape B unwrap — pass None to preserve wrapper-as-root semantics).
        if let Some(params) = &op.parameters {
            for param in params {
                if param.r#in == "body"
                    && let Some(schema) = &param.schema
                    && let Some(name) = root_def_name(schema, None)
                {
                    // Input edge emission is independent of the
                    // endpoint_roots dedup — POST /pet with Pet body AND
                    // Pet response must yield both input and output edges.
                    if let Some(defs) = spec_defs
                        && defs.contains_key(&name)
                    {
                        endpoint_edges.push(EndpointEdge {
                            endpoint: EndpointInfo {
                                method: method.clone(),
                                path: path.clone(),
                            },
                            target_def: name.clone(),
                            direction: "input".to_string(),
                        });
                    }
                    if !endpoint_roots.contains(&name) {
                        endpoint_roots.push(name);
                    }
                }
            }
        }

        // If no definition-based roots found, check for virtual root
        if endpoint_roots.is_empty() {
            let shape = parser::primary_response_shape(op, spec_defs);
            match shape {
                ResponseShape::Definition(_) | ResponseShape::Empty => {}
                _ => {
                    virtual_roots.push(VirtualRoot {
                        endpoint: EndpointInfo {
                            method: method.clone(),
                            path: path.clone(),
                        },
                        shape: shape_to_label(&shape),
                    });
                }
            }
        }

        // For each root, collect all transitive refs
        for root_name in &endpoint_roots {
            // Track this endpoint as a root source
            let ep = EndpointInfo {
                method: method.clone(),
                path: path.clone(),
            };
            roots.entry(root_name.clone()).or_default().push(ep);

            // Collect all transitive refs from this root definition
            if let Some(defs) = spec_defs {
                if let Some(def_schema) = defs.get(root_name) {
                    let mut visited = HashSet::new();
                    visited.insert(root_name.clone());
                    collect_schema_refs(def_schema, &mut visited, spec_defs);
                    all_nodes.extend(visited);
                } else {
                    all_nodes.insert(root_name.clone());
                }
            } else {
                all_nodes.insert(root_name.clone());
            }
        }
    }

    // Filter out extension-only roots from nodes and roots map
    let ext_only = parser::extension_only_roots(spec);
    all_nodes.retain(|n| !ext_only.contains(n));
    roots.retain(|k, _| !ext_only.contains(k));
    endpoint_edges.retain(|e| !ext_only.contains(&e.target_def));
    endpoint_edges.retain(|e| all_nodes.contains(&e.target_def));

    // Build edges for each node
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(defs) = spec_defs {
        for node in &all_nodes {
            if let Some(def_schema) = defs.get(node) {
                let children: Vec<String> = direct_child_refs(def_schema)
                    .into_iter()
                    .filter(|c| all_nodes.contains(c) && c != node)
                    .collect();
                edges.insert(node.clone(), children);
            }
        }
    }

    // shared_entities: defs that appear as roots from 2+ different endpoints
    let mut root_endpoint_count: HashMap<String, HashSet<(String, String)>> = HashMap::new();
    for (def_name, eps) in &roots {
        for ep in eps {
            root_endpoint_count
                .entry(def_name.clone())
                .or_default()
                .insert((ep.method.clone(), ep.path.clone()));
        }
    }
    let mut shared_entities: Vec<String> = root_endpoint_count
        .into_iter()
        .filter(|(_, endpoints)| endpoints.len() >= 2)
        .map(|(name, _)| name)
        .collect();
    shared_entities.sort();

    // Compute array properties before consuming all_nodes
    let array_properties = find_array_properties(&all_nodes, spec_defs);

    // Compute scalar properties
    let scalar_properties = find_scalar_properties(&all_nodes, spec_defs);

    // Sort nodes
    let mut nodes: Vec<String> = all_nodes.into_iter().collect();
    nodes.sort();

    // Sort edges values
    for children in edges.values_mut() {
        children.sort();
    }

    // Sort virtual_roots by (path, method)
    virtual_roots.sort_by(|a, b| {
        (&a.endpoint.path, &a.endpoint.method).cmp(&(&b.endpoint.path, &b.endpoint.method))
    });

    // Sort endpoint_edges by (path, method, direction, target_def) for determinism
    endpoint_edges.sort_by(|a, b| {
        (
            &a.endpoint.path,
            &a.endpoint.method,
            &a.direction,
            &a.target_def,
        )
            .cmp(&(
                &b.endpoint.path,
                &b.endpoint.method,
                &b.direction,
                &b.target_def,
            ))
    });

    EntityGraph {
        nodes,
        roots,
        edges,
        shared_entities,
        array_properties,
        scalar_properties,
        virtual_roots,
        endpoint_edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SwaggerSpec;

    fn load_petstore() -> SwaggerSpec {
        let yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let spec: SwaggerSpec = serde_yaml::from_str(&yaml).unwrap();
        spec
    }

    #[test]
    fn test_graph_pet_endpoint_nodes() {
        let spec = load_petstore();
        let selected = vec![("/pet/{petId}".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.nodes.len(), 3);
        assert!(graph.nodes.contains(&"Pet".to_string()));
        assert!(graph.nodes.contains(&"Category".to_string()));
        assert!(graph.nodes.contains(&"Tag".to_string()));
        assert!(graph.roots.contains_key("Pet"));
        assert!(
            graph
                .edges
                .get("Pet")
                .unwrap()
                .contains(&"Category".to_string())
        );
        assert!(graph.edges.get("Pet").unwrap().contains(&"Tag".to_string()));

        // Pet.tags is an array referencing Tag -> should appear
        assert_eq!(graph.array_properties.len(), 1);
        assert_eq!(graph.array_properties[0].def_name, "Pet");
        assert_eq!(graph.array_properties[0].prop_name, "tags");
        assert_eq!(graph.array_properties[0].target_def, "Tag");
        // photoUrls is a primitive array (string) -> should NOT appear
        assert!(graph.virtual_roots.is_empty());
    }

    #[test]
    fn test_graph_edges_pet() {
        let spec = load_petstore();
        let selected = vec![("/pet/{petId}".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        // Category and Tag have no outbound edges to other definitions in the graph
        let cat_edges = graph.edges.get("Category").cloned().unwrap_or_default();
        let tag_edges = graph.edges.get("Tag").cloned().unwrap_or_default();
        assert!(
            cat_edges.is_empty(),
            "Category should have no outbound edges"
        );
        assert!(tag_edges.is_empty(), "Tag should have no outbound edges");
        assert!(graph.virtual_roots.is_empty());
    }

    #[test]
    fn test_graph_empty_selection() {
        let spec = load_petstore();
        let selected: Vec<(String, String)> = vec![];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.nodes.is_empty());
        assert!(graph.roots.is_empty());
        assert!(graph.edges.is_empty());
        assert!(graph.shared_entities.is_empty());
        assert!(graph.array_properties.is_empty());
        assert!(graph.virtual_roots.is_empty());
        assert!(graph.endpoint_edges.is_empty());
    }

    #[test]
    fn test_graph_post_endpoint() {
        let spec = load_petstore();
        let selected = vec![("/pet".to_string(), "post".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.nodes.len(), 3);
        assert!(graph.nodes.contains(&"Pet".to_string()));
        assert!(graph.nodes.contains(&"Category".to_string()));
        assert!(graph.nodes.contains(&"Tag".to_string()));
        assert!(graph.virtual_roots.is_empty());
    }

    #[test]
    fn test_graph_multiple_endpoints_merged() {
        let spec = load_petstore();
        let selected = vec![
            ("/pet/{petId}".to_string(), "get".to_string()),
            ("/pet".to_string(), "post".to_string()),
        ];
        let graph = build_entity_graph(&spec, &selected);
        // Pet is root from both endpoints
        let pet_roots = graph.roots.get("Pet").unwrap();
        assert_eq!(pet_roots.len(), 2);
        // Still only 3 unique nodes
        assert_eq!(graph.nodes.len(), 3);
        assert!(graph.virtual_roots.is_empty());
    }

    #[test]
    fn test_graph_unknown_endpoint() {
        let spec = load_petstore();
        let selected = vec![("/does/not/exist".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.nodes.is_empty());
        assert!(graph.roots.is_empty());
        assert!(graph.virtual_roots.is_empty());
    }

    #[test]
    fn test_graph_no_definitions() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Minimal
  version: "1.0"
paths:
  /test:
    get:
      responses:
        "200":
          description: ok
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/test".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.nodes.is_empty());
        assert!(graph.roots.is_empty());
    }

    #[test]
    fn test_graph_cycle_safe() {
        let yaml = r##"
swagger: "2.0"
info:
  title: Cycle Test
  version: "1.0"
paths:
  /test:
    get:
      responses:
        "200":
          description: ok
          schema:
            $ref: "#/definitions/A"
definitions:
  A:
    type: object
    properties:
      b:
        $ref: "#/definitions/B"
  B:
    type: object
    properties:
      a:
        $ref: "#/definitions/A"
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/test".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.nodes.contains(&"A".to_string()));
        assert!(graph.nodes.contains(&"B".to_string()));
        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn test_graph_primitive_array_virtual_root() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Array Test
  version: "1.0"
paths:
  /numbers:
    get:
      responses:
        "200":
          description: ok
          schema:
            type: array
            items:
              type: integer
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/numbers".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.virtual_roots.len(), 1);
        assert_eq!(graph.virtual_roots[0].shape, "array<integer>");
        assert!(graph.nodes.is_empty());
    }

    #[test]
    fn test_graph_scalar_virtual_root() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Scalar Test
  version: "1.0"
paths:
  /name:
    get:
      responses:
        "200":
          description: ok
          schema:
            type: string
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/name".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.virtual_roots.len(), 1);
        assert_eq!(graph.virtual_roots[0].shape, "string");
    }

    #[test]
    fn test_graph_freeform_object_virtual_root() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Freeform Test
  version: "1.0"
paths:
  /data:
    get:
      responses:
        "200":
          description: ok
          schema:
            type: object
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/data".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.virtual_roots.len(), 1);
        assert_eq!(graph.virtual_roots[0].shape, "object<freeform>");
    }

    #[test]
    fn test_graph_empty_response() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Empty Test
  version: "1.0"
paths:
  /ping:
    get:
      responses:
        "200":
          description: ok
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/ping".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.virtual_roots.is_empty());
    }

    // --- root_def_name: Shape B one-hop unwrap ---

    fn ref_schema(def: &str) -> SchemaObject {
        SchemaObject {
            schema_type: None,
            format: None,
            properties: None,
            items: None,
            required: None,
            ref_path: Some(format!("#/definitions/{def}")),
            enum_values: None,
            description: None,
            additional_properties: None,
            all_of: None,
            x_faker: None,
        }
    }

    fn array_ref_def_schema(element: &str) -> SchemaObject {
        SchemaObject {
            schema_type: Some("array".to_string()),
            format: None,
            properties: None,
            items: Some(Box::new(ref_schema(element))),
            required: None,
            ref_path: None,
            enum_values: None,
            description: None,
            additional_properties: None,
            all_of: None,
            x_faker: None,
        }
    }

    #[test]
    fn test_root_def_name_shape_b_unwraps() {
        // Shape B: $ref → CatalogPage (type:array + items.$ref CatalogItem).
        // With spec_defs provided, must return CatalogItem (element), not
        // CatalogPage (wrapper).
        let mut defs = HashMap::new();
        defs.insert(
            "CatalogPage".to_string(),
            array_ref_def_schema("CatalogItem"),
        );
        defs.insert(
            "CatalogItem".to_string(),
            SchemaObject {
                schema_type: Some("object".to_string()),
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
        let schema = ref_schema("CatalogPage");
        assert_eq!(
            root_def_name(&schema, Some(&defs)),
            Some("CatalogItem".to_string())
        );
    }

    #[test]
    fn test_root_def_name_direct_ref_unchanged() {
        // Direct $ref to a non-array def. Unwrap must be a no-op.
        let mut defs = HashMap::new();
        defs.insert(
            "Pet".to_string(),
            SchemaObject {
                schema_type: Some("object".to_string()),
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
        let schema = ref_schema("Pet");
        assert_eq!(root_def_name(&schema, Some(&defs)), Some("Pet".to_string()));
    }

    #[test]
    fn test_root_def_name_array_of_ref_unchanged() {
        // Inline array with items.$ref — not Shape B. Must return element
        // name directly (existing behavior).
        let schema = SchemaObject {
            schema_type: Some("array".to_string()),
            format: None,
            properties: None,
            items: Some(Box::new(ref_schema("Pet"))),
            required: None,
            ref_path: None,
            enum_values: None,
            description: None,
            additional_properties: None,
            all_of: None,
            x_faker: None,
        };
        assert_eq!(root_def_name(&schema, None), Some("Pet".to_string()));
        // Same result with spec_defs provided.
        let defs = HashMap::new();
        assert_eq!(root_def_name(&schema, Some(&defs)), Some("Pet".to_string()));
    }

    #[test]
    fn test_root_def_name_shape_b_missing_def() {
        // Wrapper def absent from spec_defs → falls back to Some(wrapper).
        // No panic, no drop to None.
        let defs: HashMap<String, SchemaObject> = HashMap::new();
        let schema = ref_schema("CatalogPage");
        assert_eq!(
            root_def_name(&schema, Some(&defs)),
            Some("CatalogPage".to_string())
        );
    }

    // --- endpoint_edges: directed endpoint→schema edges ---

    #[test]
    fn test_endpoint_edges_output_only_get_pet() {
        let spec = load_petstore();
        let selected = vec![("/pet/{petId}".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 1);
        let e = &graph.endpoint_edges[0];
        assert_eq!(e.target_def, "Pet");
        assert_eq!(e.direction, "output");
        assert_eq!(e.endpoint.method, "get");
        assert_eq!(e.endpoint.path, "/pet/{petId}");
    }

    #[test]
    fn test_endpoint_edges_input_and_output_post_pet() {
        let spec = load_petstore();
        let selected = vec![("/pet".to_string(), "post".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 2);
        assert!(graph.endpoint_edges.iter().any(|e| e.target_def == "Pet"
            && e.direction == "input"
            && e.endpoint.method == "post"
            && e.endpoint.path == "/pet"));
        assert!(graph.endpoint_edges.iter().any(|e| e.target_def == "Pet"
            && e.direction == "output"
            && e.endpoint.method == "post"
            && e.endpoint.path == "/pet"));
    }

    #[test]
    fn test_endpoint_edges_no_ref_endpoint_absent() {
        let spec = load_petstore();
        let selected = vec![("/pet/{petId}".to_string(), "delete".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.endpoint_edges.is_empty());
        assert!(graph.virtual_roots.is_empty());
    }

    #[test]
    fn test_endpoint_edges_merged_get_and_post() {
        let spec = load_petstore();
        let selected = vec![
            ("/pet/{petId}".to_string(), "get".to_string()),
            ("/pet".to_string(), "post".to_string()),
        ];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 3);
        let out_count = graph
            .endpoint_edges
            .iter()
            .filter(|e| e.direction == "output")
            .count();
        let in_count = graph
            .endpoint_edges
            .iter()
            .filter(|e| e.direction == "input")
            .count();
        assert_eq!(out_count, 2);
        assert_eq!(in_count, 1);
        // regression guard: Pet remains root from both endpoints
        let pet_roots = graph.roots.get("Pet").unwrap();
        assert_eq!(pet_roots.len(), 2);
    }

    #[test]
    fn test_endpoint_edges_virtual_root_endpoint_absent() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Array Test
  version: "1.0"
paths:
  /numbers:
    get:
      responses:
        "200":
          description: ok
          schema:
            type: array
            items:
              type: integer
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/numbers".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.endpoint_edges.is_empty());
        assert_eq!(graph.virtual_roots.len(), 1);
    }

    #[test]
    fn test_endpoint_edges_primary_2xx_selection_200_wins_over_201() {
        let yaml = r##"
swagger: "2.0"
info:
  title: Multi2xx
  version: "1.0"
paths:
  /thing:
    get:
      responses:
        "200":
          description: ok
          schema:
            $ref: "#/definitions/AlphaResp"
        "201":
          description: created
          schema:
            $ref: "#/definitions/BetaResp"
definitions:
  AlphaResp:
    type: object
    properties:
      id:
        type: integer
  BetaResp:
    type: object
    properties:
      id:
        type: integer
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/thing".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 1);
        assert_eq!(graph.endpoint_edges[0].target_def, "AlphaResp");
        assert_eq!(graph.endpoint_edges[0].direction, "output");
    }

    #[test]
    fn test_endpoint_edges_primary_2xx_falls_back_to_201() {
        let yaml = r##"
swagger: "2.0"
info:
  title: Only201
  version: "1.0"
paths:
  /thing:
    post:
      responses:
        "201":
          description: created
          schema:
            $ref: "#/definitions/BetaResp"
definitions:
  BetaResp:
    type: object
    properties:
      id:
        type: integer
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/thing".to_string(), "post".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 1);
        assert_eq!(graph.endpoint_edges[0].target_def, "BetaResp");
    }

    #[test]
    fn test_endpoint_edges_primary_2xx_falls_back_to_first_2xx() {
        let yaml = r##"
swagger: "2.0"
info:
  title: Only2xx
  version: "1.0"
paths:
  /thing:
    post:
      responses:
        "202":
          description: accepted
          schema:
            $ref: "#/definitions/GammaResp"
        "204":
          description: no content
definitions:
  GammaResp:
    type: object
    properties:
      id:
        type: integer
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/thing".to_string(), "post".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 1);
        assert_eq!(graph.endpoint_edges[0].target_def, "GammaResp");
    }

    #[test]
    fn test_endpoint_edges_non_definition_response_absent() {
        let yaml = r#"
swagger: "2.0"
info:
  title: Primitive
  version: "1.0"
paths:
  /name:
    get:
      responses:
        "200":
          description: ok
          schema:
            type: string
"#;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/name".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.endpoint_edges.is_empty());
        assert_eq!(graph.virtual_roots.len(), 1);
    }

    #[test]
    fn test_endpoint_edges_extension_only_target_filtered() {
        // Base is used only as allOf ref → extension-only root.
        // Endpoint response $ref points at Base, so the edge target falls
        // into ext_only and must be filtered.
        let yaml = r##"
swagger: "2.0"
info:
  title: ExtOnly
  version: "1.0"
paths:
  /base:
    get:
      responses:
        "200":
          description: ok
          schema:
            $ref: "#/definitions/Base"
definitions:
  Base:
    type: object
    properties:
      id:
        type: integer
  Child:
    allOf:
      - $ref: "#/definitions/Base"
      - type: object
        properties:
          extra:
            type: string
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let ext_only = parser::extension_only_roots(&spec);
        let selected = vec![("/base".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert!(
            graph
                .endpoint_edges
                .iter()
                .all(|e| !ext_only.contains(&e.target_def))
        );
    }

    #[test]
    fn test_endpoint_edges_shape_b_unwraps_to_element() {
        let yaml = r##"
swagger: "2.0"
info:
  title: ShapeB
  version: "1.0"
paths:
  /catalog:
    get:
      responses:
        "200":
          description: ok
          schema:
            $ref: "#/definitions/CatalogPage"
definitions:
  CatalogPage:
    type: array
    items:
      $ref: "#/definitions/CatalogItem"
  CatalogItem:
    type: object
    properties:
      id:
        type: integer
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/catalog".to_string(), "get".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 1);
        assert_eq!(graph.endpoint_edges[0].target_def, "CatalogItem");
        assert_eq!(graph.endpoint_edges[0].direction, "output");
    }

    #[test]
    fn test_endpoint_edges_body_param_no_shape_b_unwrap() {
        let yaml = r##"
swagger: "2.0"
info:
  title: BodyWrapper
  version: "1.0"
paths:
  /items:
    post:
      parameters:
        - in: body
          name: body
          required: true
          schema:
            $ref: "#/definitions/CreateReq"
      responses:
        "200":
          description: ok
definitions:
  CreateReq:
    type: array
    items:
      $ref: "#/definitions/Item"
  Item:
    type: object
    properties:
      id:
        type: integer
"##;
        let spec: SwaggerSpec = serde_yaml::from_str(yaml).unwrap();
        let selected = vec![("/items".to_string(), "post".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        assert_eq!(graph.endpoint_edges.len(), 1);
        assert_eq!(graph.endpoint_edges[0].target_def, "CreateReq");
        assert_eq!(graph.endpoint_edges[0].direction, "input");
    }

    #[test]
    fn test_endpoint_edges_empty_selection() {
        let spec = load_petstore();
        let selected: Vec<(String, String)> = vec![];
        let graph = build_entity_graph(&spec, &selected);
        assert!(graph.endpoint_edges.is_empty());
    }

    #[test]
    fn test_entity_graph_endpoint_edges_serde_roundtrip() {
        let spec = load_petstore();
        let selected = vec![("/pet".to_string(), "post".to_string())];
        let graph = build_entity_graph(&spec, &selected);
        let v = serde_json::to_value(&graph).unwrap();
        assert!(v["endpoint_edges"].is_array());
        let arr = v["endpoint_edges"].as_array().unwrap();
        assert!(!arr.is_empty());
        for el in arr {
            assert!(el.get("endpoint").is_some());
            assert!(el.get("target_def").is_some());
            assert!(el.get("direction").is_some());
        }
    }
}
