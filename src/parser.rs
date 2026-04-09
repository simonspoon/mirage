// Swagger 2.0 parser
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SwaggerSpec {
    pub swagger: String,
    pub info: Info,
    pub paths: HashMap<String, PathItem>,
    pub definitions: Option<HashMap<String, SchemaObject>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Info {
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PathItem {
    pub get: Option<Operation>,
    pub post: Option<Operation>,
    pub put: Option<Operation>,
    pub delete: Option<Operation>,
    pub patch: Option<Operation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub operation_id: Option<String>,
    pub parameters: Option<Vec<Parameter>>,
    pub responses: HashMap<String, Response>,
    pub summary: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "in")]
    pub r#in: String,
    pub required: Option<bool>,
    pub schema: Option<SchemaObject>,
    #[serde(rename = "type")]
    pub param_type: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Response {
    pub description: Option<String>,
    pub schema: Option<SchemaObject>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchemaObject {
    #[serde(rename = "type")]
    pub schema_type: Option<String>,
    pub format: Option<String>,
    pub properties: Option<HashMap<String, SchemaObject>>,
    pub items: Option<Box<SchemaObject>>,
    pub required: Option<Vec<String>>,
    #[serde(rename = "$ref")]
    pub ref_path: Option<String>,
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<serde_json::Value>>,
    pub description: Option<String>,
    #[serde(rename = "additionalProperties")]
    pub additional_properties: Option<Box<SchemaObject>>,
    #[serde(rename = "allOf")]
    pub all_of: Option<Vec<SchemaObject>>,
}

impl SwaggerSpec {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let spec: SwaggerSpec = serde_yaml::from_str(&contents)?;
        Ok(spec)
    }

    pub fn resolve_refs(&mut self) {
        let definitions = match &self.definitions {
            Some(defs) => defs.clone(),
            None => return,
        };

        // Resolve refs in definitions themselves
        if let Some(ref mut defs) = self.definitions {
            let keys: Vec<String> = defs.keys().cloned().collect();
            for key in keys {
                let mut visited = HashSet::new();
                if let Some(schema) = defs.get_mut(&key) {
                    resolve_schema(schema, &definitions, &mut visited);
                }
            }
        }

        // Resolve refs in paths
        for path_item in self.paths.values_mut() {
            let operations = [
                path_item.get.as_mut(),
                path_item.post.as_mut(),
                path_item.put.as_mut(),
                path_item.delete.as_mut(),
                path_item.patch.as_mut(),
            ];
            for op in operations.into_iter().flatten() {
                // Resolve parameter schemas
                if let Some(ref mut params) = op.parameters {
                    for param in params.iter_mut() {
                        if let Some(ref mut schema) = param.schema {
                            let mut visited = HashSet::new();
                            resolve_schema(schema, &definitions, &mut visited);
                        }
                    }
                }
                // Resolve response schemas
                for response in op.responses.values_mut() {
                    if let Some(ref mut schema) = response.schema {
                        let mut visited = HashSet::new();
                        resolve_schema(schema, &definitions, &mut visited);
                    }
                }
            }
        }
    }

    pub fn definition_names(&self) -> Vec<&str> {
        match &self.definitions {
            Some(defs) => defs.keys().map(|k| k.as_str()).collect(),
            None => Vec::new(),
        }
    }

    pub fn path_operations(&self) -> Vec<(&str, &str, &Operation)> {
        let mut result = Vec::new();
        for (path, item) in &self.paths {
            if let Some(ref op) = item.get {
                result.push((path.as_str(), "get", op));
            }
            if let Some(ref op) = item.post {
                result.push((path.as_str(), "post", op));
            }
            if let Some(ref op) = item.put {
                result.push((path.as_str(), "put", op));
            }
            if let Some(ref op) = item.delete {
                result.push((path.as_str(), "delete", op));
            }
            if let Some(ref op) = item.patch {
                result.push((path.as_str(), "patch", op));
            }
        }
        result
    }
}

/// Extract definition names referenced by a set of operations.
/// Must be called BEFORE resolve_refs() since it reads $ref paths.
pub fn definitions_for_paths(spec: &SwaggerSpec, paths: &[(String, String)]) -> HashSet<String> {
    let mut defs = HashSet::new();
    let spec_defs = spec.definitions.as_ref();
    for (path, method) in paths {
        if let Some(path_item) = spec.paths.get(path.as_str()) {
            let op = match method.as_str() {
                "get" => path_item.get.as_ref(),
                "post" => path_item.post.as_ref(),
                "put" => path_item.put.as_ref(),
                "delete" => path_item.delete.as_ref(),
                "patch" => path_item.patch.as_ref(),
                _ => None,
            };
            if let Some(op) = op {
                for response in op.responses.values() {
                    if let Some(schema) = &response.schema {
                        collect_schema_refs(schema, &mut defs, spec_defs);
                    }
                }
                if let Some(params) = &op.parameters {
                    for param in params {
                        if param.r#in == "body"
                            && let Some(schema) = &param.schema
                        {
                            collect_schema_refs(schema, &mut defs, spec_defs);
                        }
                    }
                }
            }
        }
    }
    defs
}

/// Extract the primary definition name from the success response of an operation.
/// Checks "200", "201", then other 2xx codes in order.
fn response_def_name(schema: &SchemaObject) -> Option<String> {
    // Direct $ref
    if let Some(ref ref_path) = schema.ref_path {
        return ref_path
            .strip_prefix("#/definitions/")
            .map(|s| s.to_string());
    }
    // Array wrapping a $ref in items
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

/// Get the primary response definition name for a given operation.
/// Prefers 200, then 201, then any other 2xx response.
pub fn primary_response_def(op: &Operation) -> Option<String> {
    // Check 200 first, then 201
    for code in &["200", "201"] {
        if let Some(resp) = op.responses.get(*code)
            && let Some(schema) = &resp.schema
            && let Some(name) = response_def_name(schema)
        {
            return Some(name);
        }
    }
    // Then any other 2xx
    let mut keys: Vec<&String> = op.responses.keys().collect();
    keys.sort();
    for key in keys {
        if key.starts_with('2')
            && key != "200"
            && key != "201"
            && let Some(schema) = &op.responses[key].schema
            && let Some(name) = response_def_name(schema)
        {
            return Some(name);
        }
    }
    None
}

fn collect_schema_refs(
    schema: &SchemaObject,
    defs: &mut HashSet<String>,
    spec_definitions: Option<&HashMap<String, SchemaObject>>,
) {
    if let Some(ref_path) = &schema.ref_path
        && let Some(name) = ref_path.strip_prefix("#/definitions/")
        && defs.insert(name.to_string())
    {
        // Recurse into the referenced definition if available
        if let Some(all_defs) = spec_definitions
            && let Some(def_schema) = all_defs.get(name)
        {
            collect_schema_refs(def_schema, defs, spec_definitions);
        }
    }
    if let Some(items) = &schema.items {
        collect_schema_refs(items, defs, spec_definitions);
    }
    if let Some(props) = &schema.properties {
        for prop in props.values() {
            collect_schema_refs(prop, defs, spec_definitions);
        }
    }
    if let Some(all_of) = &schema.all_of {
        for member in all_of {
            collect_schema_refs(member, defs, spec_definitions);
        }
    }
}

fn resolve_schema(
    schema: &mut SchemaObject,
    definitions: &HashMap<String, SchemaObject>,
    visited: &mut HashSet<String>,
) {
    // If this schema is a $ref, resolve it
    if let Some(ref ref_path) = schema.ref_path.clone() {
        let def_name = ref_path.strip_prefix("#/definitions/").unwrap_or(ref_path);

        // Circular reference detection
        if visited.contains(def_name) {
            return;
        }

        if let Some(resolved) = definitions.get(def_name) {
            visited.insert(def_name.to_string());
            // Inline the resolved definition's fields
            schema.schema_type = resolved.schema_type.clone();
            schema.format = resolved.format.clone();
            schema.properties = resolved.properties.clone();
            schema.items = resolved.items.clone();
            schema.required = resolved.required.clone();
            schema.enum_values = resolved.enum_values.clone();
            schema.description = resolved.description.clone();
            schema.additional_properties = resolved.additional_properties.clone();
            schema.all_of = resolved.all_of.clone();
            // Clear the ref now that it's resolved
            schema.ref_path = None;
        }
    }

    // Handle allOf: merge all member schemas into this schema
    if let Some(all_of) = schema.all_of.take() {
        let mut merged_props: HashMap<String, SchemaObject> =
            schema.properties.take().unwrap_or_default();
        let mut merged_required: Vec<String> = schema.required.take().unwrap_or_default();

        for mut member in all_of {
            // Resolve $ref members first
            if let Some(ref ref_path) = member.ref_path.clone() {
                let def_name = ref_path.strip_prefix("#/definitions/").unwrap_or(ref_path);
                if !visited.contains(def_name)
                    && let Some(resolved) = definitions.get(def_name)
                {
                    // Snapshot visited set so sibling allOf members can resolve
                    // the same definitions independently (prevents contamination)
                    let mut member_visited = visited.clone();
                    member_visited.insert(def_name.to_string());
                    member.schema_type = resolved.schema_type.clone();
                    member.format = resolved.format.clone();
                    member.properties = resolved.properties.clone();
                    member.items = resolved.items.clone();
                    member.required = resolved.required.clone();
                    member.enum_values = resolved.enum_values.clone();
                    member.description = resolved.description.clone();
                    member.additional_properties = resolved.additional_properties.clone();
                    member.all_of = resolved.all_of.clone();
                    member.ref_path = None;

                    // Recursively resolve allOf within the resolved member
                    resolve_schema(&mut member, definitions, &mut member_visited);
                    // Propagate cycle protection back to parent
                    visited.extend(member_visited);
                }
            } else {
                // Inline schema member -- resolve any nested refs
                resolve_schema(&mut member, definitions, visited);
            }

            // Merge properties
            if let Some(props) = member.properties {
                merged_props.extend(props);
            }
            // Merge required
            if let Some(req) = member.required {
                for r in req {
                    if !merged_required.contains(&r) {
                        merged_required.push(r);
                    }
                }
            }
        }

        if !merged_props.is_empty() {
            schema.properties = Some(merged_props);
        }
        if !merged_required.is_empty() {
            schema.required = Some(merged_required);
        }
        if schema.schema_type.is_none() {
            schema.schema_type = Some("object".to_string());
        }
    }

    // Recursively resolve nested schemas
    if let Some(ref mut properties) = schema.properties {
        for prop_schema in properties.values_mut() {
            // Handle property-level allOf (e.g. "entryUser": {"allOf": [{"$ref": "..."}]})
            if let Some(ref all_of) = prop_schema.all_of.clone()
                && all_of.len() == 1
                && all_of[0].ref_path.is_some()
            {
                let mut resolved = all_of[0].clone();
                resolve_schema(&mut resolved, definitions, visited);
                *prop_schema = resolved;
                continue;
            }
            resolve_schema(prop_schema, definitions, visited);
        }
    }

    if let Some(ref mut items) = schema.items {
        resolve_schema(items, definitions, visited);
    }

    if let Some(ref mut additional) = schema.additional_properties {
        resolve_schema(additional, definitions, visited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_petstore() -> SwaggerSpec {
        SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap()
    }

    #[test]
    fn test_parse_petstore() {
        let spec = load_petstore();
        assert_eq!(spec.swagger, "2.0");
        assert_eq!(spec.info.title, "Petstore");
        assert!(!spec.paths.is_empty());
        assert!(spec.definitions.is_some());
    }

    #[test]
    fn test_pet_definition() {
        let spec = load_petstore();
        let defs = spec.definitions.as_ref().unwrap();
        let pet = defs.get("Pet").expect("Pet definition should exist");
        let props = pet.properties.as_ref().expect("Pet should have properties");
        assert!(props.contains_key("id"));
        assert!(props.contains_key("name"));
        assert!(props.contains_key("status"));
    }

    #[test]
    fn test_get_pet_path() {
        let spec = load_petstore();
        let path_item = spec
            .paths
            .get("/pet/{petId}")
            .expect("/pet/{petId} should exist");
        let get_op = path_item.get.as_ref().expect("GET operation should exist");
        assert_eq!(get_op.operation_id.as_deref(), Some("getPetById"));
    }

    #[test]
    fn test_resolve_ref() {
        let mut spec = load_petstore();
        spec.resolve_refs();
        let defs = spec.definitions.as_ref().unwrap();
        let pet = defs.get("Pet").unwrap();
        let props = pet.properties.as_ref().unwrap();
        let category = props.get("category").unwrap();
        // After resolution, category should have its own properties inlined
        assert!(
            category.ref_path.is_none(),
            "ref_path should be cleared after resolution"
        );
        let cat_props = category
            .properties
            .as_ref()
            .expect("Category should have inlined properties");
        assert!(cat_props.contains_key("id"));
        assert!(cat_props.contains_key("name"));
    }

    #[test]
    fn test_array_items_ref() {
        let mut spec = load_petstore();
        spec.resolve_refs();
        let defs = spec.definitions.as_ref().unwrap();
        let pet = defs.get("Pet").unwrap();
        let props = pet.properties.as_ref().unwrap();
        let tags = props.get("tags").unwrap();
        assert_eq!(tags.schema_type.as_deref(), Some("array"));
        let items = tags.items.as_ref().expect("tags should have items");
        assert!(items.ref_path.is_none(), "Tag ref should be resolved");
        let tag_props = items
            .properties
            .as_ref()
            .expect("Tag items should have inlined properties");
        assert!(tag_props.contains_key("id"));
        assert!(tag_props.contains_key("name"));
    }

    #[test]
    fn test_response_schema() {
        let spec = load_petstore();
        let path_item = spec.paths.get("/pet/{petId}").unwrap();
        let get_op = path_item.get.as_ref().unwrap();
        let response_200 = get_op
            .responses
            .get("200")
            .expect("200 response should exist");
        let schema = response_200
            .schema
            .as_ref()
            .expect("200 response should have a schema");
        assert_eq!(schema.ref_path.as_deref(), Some("#/definitions/Pet"));
    }
}
