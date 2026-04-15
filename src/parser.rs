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
    #[serde(rename = "x-faker")]
    pub x_faker: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseShape {
    Definition(String),
    Primitive(String),
    PrimitiveArray(String),
    FreeformObject,
    Empty,
}

/// Classify the shape of a response schema (works on raw, pre-resolve_refs schemas).
pub fn classify_response_schema(schema: &SchemaObject) -> ResponseShape {
    // 1. Direct $ref → Definition
    if let Some(ref ref_path) = schema.ref_path {
        let name = ref_path.rsplit('/').next().unwrap_or(ref_path);
        return ResponseShape::Definition(name.to_string());
    }

    // 2-3. Array type
    if schema.schema_type.as_deref() == Some("array") {
        if let Some(ref items) = schema.items {
            // 2. Array with $ref items → Definition (inner def name)
            if let Some(ref ref_path) = items.ref_path {
                let name = ref_path.rsplit('/').next().unwrap_or(ref_path);
                return ResponseShape::Definition(name.to_string());
            }
            // 3. Array with non-ref items → PrimitiveArray
            let item_type = items.schema_type.as_deref().unwrap_or("string").to_string();
            return ResponseShape::PrimitiveArray(item_type);
        }
        // Array with no items specified → PrimitiveArray with default
        return ResponseShape::PrimitiveArray("string".to_string());
    }

    // 4. Known scalar types → Primitive
    match schema.schema_type.as_deref() {
        Some("string") | Some("integer") | Some("boolean") | Some("number") => {
            return ResponseShape::Primitive(schema.schema_type.clone().unwrap());
        }
        _ => {}
    }

    // 5. Object type or has properties/additionalProperties → FreeformObject
    if schema.schema_type.as_deref() == Some("object")
        || schema.properties.is_some()
        || schema.additional_properties.is_some()
    {
        return ResponseShape::FreeformObject;
    }

    // 6. Otherwise → Empty
    ResponseShape::Empty
}

/// Get the primary response shape for a given operation.
/// Prefers 200, then 201, then any other 2xx response.
pub fn primary_response_shape(op: &Operation) -> ResponseShape {
    // Check 200 first, then 201
    for code in &["200", "201"] {
        if let Some(resp) = op.responses.get(*code) {
            if let Some(schema) = &resp.schema {
                return classify_response_schema(schema);
            }
            // no schema on this response: continue to next priority code
        }
    }
    // Then any other 2xx (sorted)
    let mut keys: Vec<&String> = op.responses.keys().collect();
    keys.sort();
    for key in keys {
        if key.starts_with('2') && key != "200" && key != "201" {
            if let Some(schema) = &op.responses[key].schema {
                return classify_response_schema(schema);
            }
        }
    }
    ResponseShape::Empty
}

/// Get the primary response definition name for a given operation.
/// Prefers 200, then 201, then any other 2xx response.
pub fn primary_response_def(op: &Operation) -> Option<String> {
    match primary_response_shape(op) {
        ResponseShape::Definition(name) => Some(name),
        _ => None,
    }
}

pub(crate) fn collect_schema_refs(
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
    if let Some(ap) = &schema.additional_properties {
        collect_schema_refs(ap, defs, spec_definitions);
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
            schema.x_faker = resolved.x_faker.clone();
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
                    member.x_faker = resolved.x_faker.clone();
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

    // --- Test helpers ---

    fn schema_default() -> SchemaObject {
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

    fn schema_ref(def_name: &str) -> SchemaObject {
        SchemaObject {
            ref_path: Some(format!("#/definitions/{}", def_name)),
            ..schema_default()
        }
    }

    fn schema_typed(t: &str) -> SchemaObject {
        SchemaObject {
            schema_type: Some(t.to_string()),
            ..schema_default()
        }
    }

    fn schema_array(items: SchemaObject) -> SchemaObject {
        SchemaObject {
            schema_type: Some("array".to_string()),
            items: Some(Box::new(items)),
            ..schema_default()
        }
    }

    fn op_with_response(status_code: &str, schema: Option<SchemaObject>) -> Operation {
        let mut responses = HashMap::new();
        responses.insert(
            status_code.to_string(),
            Response {
                description: Some("test".to_string()),
                schema,
            },
        );
        Operation {
            operation_id: None,
            parameters: None,
            responses,
            summary: None,
            description: None,
        }
    }

    fn op_with_responses(entries: Vec<(&str, Option<SchemaObject>)>) -> Operation {
        let mut responses = HashMap::new();
        for (code, schema) in entries {
            responses.insert(
                code.to_string(),
                Response {
                    description: Some("test".to_string()),
                    schema,
                },
            );
        }
        Operation {
            operation_id: None,
            parameters: None,
            responses,
            summary: None,
            description: None,
        }
    }

    // --- Group 1: classify_response_schema per variant ---

    #[test]
    fn test_classify_definition_direct_ref() {
        let s = schema_ref("Pet");
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::Definition("Pet".to_string())
        );
    }

    #[test]
    fn test_classify_definition_array_of_refs() {
        let s = schema_array(schema_ref("Pet"));
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::Definition("Pet".to_string())
        );
    }

    #[test]
    fn test_classify_primitive_string() {
        let s = schema_typed("string");
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::Primitive("string".to_string())
        );
    }

    #[test]
    fn test_classify_primitive_integer() {
        let s = schema_typed("integer");
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::Primitive("integer".to_string())
        );
    }

    #[test]
    fn test_classify_primitive_boolean() {
        let s = schema_typed("boolean");
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::Primitive("boolean".to_string())
        );
    }

    #[test]
    fn test_classify_primitive_array() {
        let s = schema_array(schema_typed("string"));
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::PrimitiveArray("string".to_string())
        );
    }

    #[test]
    fn test_classify_primitive_array_integer_items() {
        let s = schema_array(schema_typed("integer"));
        assert_eq!(
            classify_response_schema(&s),
            ResponseShape::PrimitiveArray("integer".to_string())
        );
    }

    #[test]
    fn test_classify_freeform_object_with_additional_properties() {
        let s = SchemaObject {
            schema_type: Some("object".to_string()),
            additional_properties: Some(Box::new(schema_typed("string"))),
            ..schema_default()
        };
        assert_eq!(classify_response_schema(&s), ResponseShape::FreeformObject);
    }

    #[test]
    fn test_classify_freeform_object_bare_object() {
        let s = schema_typed("object");
        assert_eq!(classify_response_schema(&s), ResponseShape::FreeformObject);
    }

    #[test]
    fn test_classify_empty_schema() {
        let s = schema_default();
        assert_eq!(classify_response_schema(&s), ResponseShape::Empty);
    }

    // --- Group 2: primary_response_shape ---

    #[test]
    fn test_primary_shape_200_definition() {
        let op = op_with_response("200", Some(schema_ref("Pet")));
        assert_eq!(
            primary_response_shape(&op),
            ResponseShape::Definition("Pet".to_string())
        );
    }

    #[test]
    fn test_primary_shape_200_primitive() {
        let op = op_with_response("200", Some(schema_typed("string")));
        assert_eq!(
            primary_response_shape(&op),
            ResponseShape::Primitive("string".to_string())
        );
    }

    #[test]
    fn test_primary_shape_200_primitive_array() {
        let op = op_with_response("200", Some(schema_array(schema_typed("integer"))));
        assert_eq!(
            primary_response_shape(&op),
            ResponseShape::PrimitiveArray("integer".to_string())
        );
    }

    #[test]
    fn test_primary_shape_200_freeform() {
        let op = op_with_response("200", Some(schema_typed("object")));
        assert_eq!(primary_response_shape(&op), ResponseShape::FreeformObject);
    }

    #[test]
    fn test_primary_shape_no_success_response() {
        let op = op_with_response("404", None);
        assert_eq!(primary_response_shape(&op), ResponseShape::Empty);
    }

    #[test]
    fn test_primary_shape_prefers_200_over_201() {
        let op = op_with_responses(vec![
            ("200", Some(schema_ref("Cat"))),
            ("201", Some(schema_ref("Dog"))),
        ]);
        assert_eq!(
            primary_response_shape(&op),
            ResponseShape::Definition("Cat".to_string())
        );
    }

    #[test]
    fn test_primary_shape_201_fallback() {
        let op = op_with_responses(vec![("404", None), ("201", Some(schema_ref("Dog")))]);
        assert_eq!(
            primary_response_shape(&op),
            ResponseShape::Definition("Dog".to_string())
        );
    }

    #[test]
    fn test_primary_shape_no_schema_on_200() {
        let op = op_with_response("200", None);
        assert_eq!(primary_response_shape(&op), ResponseShape::Empty);
    }

    #[test]
    fn test_primary_shape_200_no_schema_falls_through_to_201() {
        let op = op_with_responses(vec![("200", None), ("201", Some(schema_ref("Dog")))]);
        assert_eq!(
            primary_response_shape(&op),
            ResponseShape::Definition("Dog".to_string())
        );
        assert_eq!(primary_response_def(&op), Some("Dog".to_string()));
    }

    // --- Group 3: primary_response_def regression ---

    #[test]
    fn test_primary_response_def_returns_some_for_ref() {
        let op = op_with_response("200", Some(schema_ref("Pet")));
        assert_eq!(primary_response_def(&op), Some("Pet".to_string()));
    }

    #[test]
    fn test_primary_response_def_returns_some_for_array_ref() {
        let op = op_with_response("200", Some(schema_array(schema_ref("Pet"))));
        assert_eq!(primary_response_def(&op), Some("Pet".to_string()));
    }

    #[test]
    fn test_primary_response_def_returns_none_for_primitive() {
        let op = op_with_response("200", Some(schema_typed("string")));
        assert_eq!(primary_response_def(&op), None);
    }

    #[test]
    fn test_primary_response_def_returns_none_when_no_schema() {
        let op = op_with_response("200", None);
        assert_eq!(primary_response_def(&op), None);
    }

    // --- Group 4: petstore fixture ---

    #[test]
    fn test_primary_shape_petstore_get_pet() {
        let spec = load_petstore();
        let path_item = spec.paths.get("/pet/{petId}").unwrap();
        let get_op = path_item.get.as_ref().unwrap();
        assert_eq!(
            primary_response_shape(get_op),
            ResponseShape::Definition("Pet".to_string())
        );
    }

    #[test]
    fn test_primary_shape_petstore_delete_pet() {
        let spec = load_petstore();
        let path_item = spec.paths.get("/pet/{petId}").unwrap();
        let delete_op = path_item.delete.as_ref().unwrap();
        assert_eq!(primary_response_shape(delete_op), ResponseShape::Empty);
    }
}
