// Axum API server

use std::collections::{HashMap, HashSet};
use std::error::Error as _;
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use http_body_util::LengthLimitError;
use rusqlite::types::Value as SqlValue;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};

use crate::composer::DocumentStore;
use crate::parser::{ResponseShape, SwaggerSpec};
use crate::schema;
use crate::seeder;

use fake::Fake;
use rand::RngExt;

pub type Db = Arc<Mutex<rusqlite::Connection>>;

#[derive(Embed)]
#[folder = "ui/dist/"]
struct AdminAssets;

pub struct RouteRegistry {
    pub routes: Vec<RouteEntry>,
    pub spec_info: Option<SpecInfo>,
    pub endpoints: Vec<EndpointInfo>,
    pub spec: Option<SwaggerSpec>,
    pub raw_spec: Option<SwaggerSpec>, // before resolve_refs, keeps $ref paths
}

impl RouteRegistry {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            spec_info: None,
            endpoints: Vec::new(),
            spec: None,
            raw_spec: None,
        }
    }
}

pub struct RouteEntry {
    pub method: String,
    pub pattern: String,
    pub table: String,
    pub has_path_param: bool,
    pub shape: ResponseShape,
}

pub type Registry = Arc<RwLock<RouteRegistry>>;

const MAX_LOG_ENTRIES: usize = 500;

/// Maximum size (in bytes) of a request or response body the logging
/// middleware will buffer in memory. Bodies larger than this are not
/// silently dropped: the request path returns 413, the response path
/// substitutes a sentinel in the captured log.
const LOG_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Maximum size (in bytes) of a body string actually stored in a
/// `LogEntry`. With `MAX_LOG_ENTRIES` capped by count rather than size
/// and `LOG_BODY_LIMIT_BYTES` allowing 16 MB bodies through, an
/// uncapped store could grow to multi-GB. Anything beyond this is
/// truncated with a marker suffix.
const LOG_BODY_STORE_MAX: usize = 64 * 1024;

/// Truncate a body string for storage in `LogEntry`, leaving a marker
/// indicating how many bytes the original payload contained.
fn truncate_for_log(s: &str) -> String {
    let total = s.len();
    if total <= LOG_BODY_STORE_MAX {
        return s.to_string();
    }
    // Find a UTF-8 boundary at or below LOG_BODY_STORE_MAX so we never
    // slice through a multi-byte character.
    let mut end = LOG_BODY_STORE_MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 48);
    out.push_str(&s[..end]);
    out.push_str(&format!("[truncated: {total} bytes total]"));
    out
}

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
}

pub type RequestLog = Arc<Mutex<Vec<LogEntry>>>;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub registry: Registry,
    pub log: RequestLog,
    pub recipe_db: Db,
    pub documents: Arc<RwLock<DocumentStore>>,
}

#[derive(Clone, Serialize)]
pub struct SpecInfo {
    pub title: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointInfo {
    pub method: String,
    pub path: String,
}

#[derive(Deserialize)]
pub struct ConfigureRequest {
    pub endpoints: Vec<EndpointInfo>,
    pub seed_count: Option<usize>,
}

fn row_to_json(
    col_names: &[String],
    row: &rusqlite::Row,
) -> Result<serde_json::Value, rusqlite::Error> {
    let mut map = serde_json::Map::new();
    for (idx, name) in col_names.iter().enumerate() {
        let val: SqlValue = row.get(idx)?;
        let json_val = match val {
            SqlValue::Null => serde_json::Value::Null,
            SqlValue::Integer(n) => serde_json::Value::Number(serde_json::Number::from(n)),
            SqlValue::Real(f) => serde_json::Number::from_f64(f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            SqlValue::Text(s) => {
                // Try to parse as JSON for nested objects/arrays
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&s) {
                    if parsed.is_object() || parsed.is_array() {
                        parsed
                    } else {
                        serde_json::Value::String(s)
                    }
                } else {
                    serde_json::Value::String(s)
                }
            }
            SqlValue::Blob(b) => serde_json::Value::String(String::from_utf8_lossy(&b).into()),
        };
        map.insert(name.clone(), json_val);
    }
    Ok(serde_json::Value::Object(map))
}

async fn get_collection(table: String, db: Db) -> Response {
    let conn = db.lock().unwrap();
    let sql = format!("SELECT * FROM \"{table}\"");
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({})),
            )
                .into_response();
        }
    };
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| row_to_json(&col_names, row))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    (StatusCode::OK, Json(serde_json::Value::Array(rows))).into_response()
}

async fn get_single(table: String, db: Db, id: String) -> Response {
    let id: i64 = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid id"})),
            )
                .into_response();
        }
    };
    let conn = db.lock().unwrap();
    let sql = format!("SELECT * FROM \"{table}\" WHERE rowid = ?");
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({})),
            )
                .into_response();
        }
    };
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    match stmt.query_row([id], |row| row_to_json(&col_names, row)) {
        Ok(val) => (StatusCode::OK, Json(val)).into_response(),
        Err(rusqlite::Error::QueryReturnedNoRows) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({})),
        )
            .into_response(),
    }
}

async fn post_create(table: String, db: Db, body: Option<Json<serde_json::Value>>) -> Response {
    let body = match body {
        Some(Json(v)) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "expected JSON body"})),
            )
                .into_response();
        }
    };
    let conn = db.lock().unwrap();

    // Get column names from the table
    let col_names: Vec<String> = {
        let sql = format!("PRAGMA table_info(\"{table}\")");
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };

    let obj = match body.as_object() {
        Some(o) => o,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "expected JSON object"})),
            )
                .into_response();
        }
    };

    // Build columns and values from the body, only including columns that exist in the table
    let mut insert_cols = Vec::new();
    let mut insert_vals: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for col in &col_names {
        if let Some(val) = obj.get(col) {
            insert_cols.push(col.clone());
            match val {
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        insert_vals.push(Box::new(i));
                    } else if let Some(f) = n.as_f64() {
                        insert_vals.push(Box::new(f));
                    } else {
                        insert_vals.push(Box::new(n.to_string()));
                    }
                }
                serde_json::Value::String(s) => {
                    insert_vals.push(Box::new(s.clone()));
                }
                serde_json::Value::Bool(b) => {
                    insert_vals.push(Box::new(if *b { 1i64 } else { 0i64 }));
                }
                serde_json::Value::Null => {
                    insert_vals.push(Box::new(rusqlite::types::Null));
                }
                other => {
                    // Objects and arrays -> JSON string
                    insert_vals.push(Box::new(serde_json::to_string(other).unwrap_or_default()));
                }
            }
        }
    }

    if insert_cols.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "no valid columns in body"})),
        )
            .into_response();
    }

    let cols_str = insert_cols
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders: Vec<String> = (1..=insert_cols.len()).map(|i| format!("?{i}")).collect();
    let placeholders_str = placeholders.join(", ");
    let sql = format!("INSERT INTO \"{table}\" ({cols_str}) VALUES ({placeholders_str})");

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        insert_vals.iter().map(|p| p.as_ref()).collect();
    if conn.execute(&sql, param_refs.as_slice()).is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "insert failed"})),
        )
            .into_response();
    }

    let new_id = conn.last_insert_rowid();
    let mut result = obj.clone();
    result.insert("id".to_string(), serde_json::json!(new_id));

    (StatusCode::CREATED, Json(serde_json::Value::Object(result))).into_response()
}

async fn delete_single(table: String, db: Db, id: String) -> Response {
    let id: i64 = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid id"})),
            )
                .into_response();
        }
    };
    let conn = db.lock().unwrap();
    let sql = format!("DELETE FROM \"{table}\" WHERE rowid = ?");
    match conn.execute(&sql, [id]) {
        Ok(changes) if changes > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn table_name_from_path(path: &str) -> String {
    let segment = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("unknown");
    let mut chars = segment.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => "Unknown".to_string(),
    }
}

fn method_str(method: &axum::http::Method) -> &str {
    match *method {
        axum::http::Method::GET => "get",
        axum::http::Method::POST => "post",
        axum::http::Method::PUT => "put",
        axum::http::Method::DELETE => "delete",
        axum::http::Method::PATCH => "patch",
        _ => "unknown",
    }
}

/// Match a route pattern against a request path.
/// Returns `Some(None)` for a collection match (no path param captured),
/// `Some(Some(id))` for a param match, `None` for no match.
fn match_route(pattern: &str, path: &str) -> Option<Option<String>> {
    let pattern_segments: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let path_segments: Vec<&str> = path.trim_matches('/').split('/').collect();

    if pattern_segments.len() != path_segments.len() {
        return None;
    }

    let mut captured: Option<String> = None;
    for (pat, seg) in pattern_segments.iter().zip(path_segments.iter()) {
        if pat.starts_with('{') && pat.ends_with('}') {
            captured = Some(seg.to_string());
        } else if pat != seg {
            return None;
        }
    }

    Some(captured)
}

fn doc_get_collection(table: String, documents: Arc<RwLock<DocumentStore>>) -> Response {
    let docs = documents.read().unwrap();
    match docs.get(&table) {
        Some(items) => (
            StatusCode::OK,
            Json(serde_json::Value::Array(items.clone())),
        )
            .into_response(),
        None => (StatusCode::OK, Json(serde_json::Value::Array(vec![]))).into_response(),
    }
}

fn doc_get_single(table: String, documents: Arc<RwLock<DocumentStore>>, id: String) -> Response {
    let id_num: i64 = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid id"})),
            )
                .into_response();
        }
    };
    let docs = documents.read().unwrap();
    if let Some(items) = docs.get(&table) {
        for item in items {
            if let Some(doc_id) = item.get("id").and_then(|v| v.as_i64())
                && doc_id == id_num
            {
                return (StatusCode::OK, Json(item.clone())).into_response();
            }
        }
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "not found"})),
    )
        .into_response()
}

fn doc_post_create(
    table: String,
    documents: Arc<RwLock<DocumentStore>>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let body = match body {
        Some(Json(v)) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "expected JSON body"})),
            )
                .into_response();
        }
    };
    let mut docs = documents.write().unwrap();
    let items = docs.entry(table).or_default();
    let new_id = items.len() + 1;
    let mut doc = body;
    if let serde_json::Value::Object(ref mut map) = doc {
        map.insert(
            "id".to_string(),
            serde_json::Value::Number(serde_json::Number::from(new_id)),
        );
    }
    items.push(doc.clone());
    (StatusCode::CREATED, Json(doc)).into_response()
}

fn doc_delete_single(table: String, documents: Arc<RwLock<DocumentStore>>, id: String) -> Response {
    let id_num: i64 = match id.parse() {
        Ok(n) => n,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid id"})),
            )
                .into_response();
        }
    };
    let mut docs = documents.write().unwrap();
    if let Some(items) = docs.get_mut(&table)
        && let Some(pos) = items
            .iter()
            .position(|item| item.get("id").and_then(|v| v.as_i64()) == Some(id_num))
    {
        items.remove(pos);
        return StatusCode::NO_CONTENT.into_response();
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "not found"})),
    )
        .into_response()
}

const PRIMITIVE_ARRAY_LEN: usize = 3;

fn generate_primitive_value(t: &str) -> serde_json::Value {
    match t {
        "integer" => serde_json::Value::Number(rand::rng().random_range(1..1000).into()),
        "number" => serde_json::json!(rand::rng().random_range(0.0f64..100.0)),
        "string" => serde_json::Value::String(fake::faker::lorem::en::Word().fake()),
        "boolean" => serde_json::Value::Bool(rand::rng().random::<bool>()),
        _ => serde_json::Value::Null,
    }
}

async fn catch_all_handler(
    method: axum::http::Method,
    uri: axum::http::Uri,
    State(state): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let path = uri.path();
    let m = method_str(&method);

    let (table, has_path_param, param_value, shape) = {
        let registry = state.registry.read().unwrap();
        let mut found = None;
        for route in &registry.routes {
            if route.method != m {
                continue;
            }
            if let Some(param_value) = match_route(&route.pattern, path) {
                found = Some((
                    route.table.clone(),
                    route.has_path_param,
                    param_value,
                    route.shape.clone(),
                ));
                break;
            }
        }
        match found {
            Some(f) => f,
            None => return StatusCode::NOT_FOUND.into_response(),
        }
    };

    match shape {
        ResponseShape::Definition(_) => { /* fall through to docs/db path below */ }
        ResponseShape::Primitive(ref t) => {
            return Json(generate_primitive_value(t)).into_response();
        }
        ResponseShape::PrimitiveArray(ref t) => {
            return Json(
                (0..PRIMITIVE_ARRAY_LEN)
                    .map(|_| generate_primitive_value(t))
                    .collect::<Vec<_>>(),
            )
            .into_response();
        }
        ResponseShape::FreeformObject => {
            return Json(serde_json::Map::new()).into_response();
        }
        ResponseShape::Empty if table.is_empty() => {
            return StatusCode::NO_CONTENT.into_response();
        }
        ResponseShape::Empty => { /* fall through — DELETE/POST with a backing table */ }
    }

    // Check if document store has data for this definition
    let has_documents = {
        let docs = state.documents.read().unwrap();
        docs.contains_key(&table)
    };

    if has_documents {
        match (m, has_path_param) {
            ("get", true) => doc_get_single(table, state.documents.clone(), param_value.unwrap()),
            ("get", false) => doc_get_collection(table, state.documents.clone()),
            ("post", _) => doc_post_create(table, state.documents.clone(), body),
            ("delete", true) => {
                doc_delete_single(table, state.documents.clone(), param_value.unwrap())
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    } else {
        let db = state.db.clone();
        match (m, has_path_param) {
            ("get", true) => get_single(table, db, param_value.unwrap()).await,
            ("get", false) => get_collection(table, db).await,
            ("post", _) => post_create(table, db, body).await,
            ("delete", true) => delete_single(table, db, param_value.unwrap()).await,
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }
}

fn log_request(
    log: &RequestLog,
    method: &str,
    path: &str,
    status: u16,
    request_body: Option<String>,
    response_body: Option<String>,
) {
    let entry = LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        method: method.to_uppercase(),
        path: path.to_string(),
        status,
        request_body: request_body.map(|s| truncate_for_log(&s)),
        response_body: response_body.map(|s| truncate_for_log(&s)),
    };
    let mut log = log.lock().unwrap();
    log.push(entry);
    let len = log.len();
    if len > MAX_LOG_ENTRIES {
        log.drain(..len - MAX_LOG_ENTRIES);
    }
}

async fn admin_log(State(state): State<AppState>) -> Response {
    let log = state.log.lock().unwrap();
    Json(log.clone()).into_response()
}

async fn admin_spec(State(state): State<AppState>) -> Response {
    let reg = state.registry.read().unwrap();
    match &reg.spec_info {
        Some(info) => Json(info.clone()).into_response(),
        None => Json(serde_json::json!({"title": "Mirage", "version": "No spec loaded"}))
            .into_response(),
    }
}

async fn admin_endpoints(State(state): State<AppState>) -> Response {
    let reg = state.registry.read().unwrap();
    Json(reg.endpoints.clone()).into_response()
}

async fn admin_definitions(State(state): State<AppState>) -> Response {
    let reg = state.registry.read().unwrap();
    let raw_spec = match &reg.raw_spec {
        Some(s) => s,
        None => return Json(serde_json::json!({})).into_response(),
    };
    let definitions = match &raw_spec.definitions {
        Some(d) => d,
        None => return Json(serde_json::json!({})).into_response(),
    };

    let mut result = serde_json::Map::new();

    for (def_name, schema) in definitions {
        let mut def_obj = serde_json::Map::new();

        // Handle description at definition level
        if let Some(desc) = &schema.description {
            def_obj.insert("description".to_string(), serde_json::json!(desc));
        }

        // Collect properties and required fields, handling allOf
        let mut all_props: HashMap<String, &crate::parser::SchemaObject> = HashMap::new();
        let mut all_required: Vec<String> = schema.required.clone().unwrap_or_default();
        let mut extends: Option<String> = None;

        if let Some(all_of) = &schema.all_of {
            for member in all_of {
                if let Some(ref ref_path) = member.ref_path {
                    // A $ref-only member is a "base type"
                    let base = ref_path.strip_prefix("#/definitions/").unwrap_or(ref_path);
                    extends = Some(base.to_string());
                }
                if let Some(props) = &member.properties {
                    for (k, v) in props {
                        all_props.insert(k.clone(), v);
                    }
                }
                if let Some(req) = &member.required {
                    for r in req {
                        if !all_required.contains(r) {
                            all_required.push(r.clone());
                        }
                    }
                }
            }
        }

        if let Some(props) = &schema.properties {
            for (k, v) in props {
                all_props.insert(k.clone(), v);
            }
        }

        if let Some(ext) = &extends {
            def_obj.insert("extends".to_string(), serde_json::json!(ext));
        }

        let mut props_obj = serde_json::Map::new();
        for (prop_name, prop_schema) in &all_props {
            let prop_type = if prop_schema.ref_path.is_some() {
                "object".to_string()
            } else {
                prop_schema.schema_type.clone().unwrap_or_default()
            };

            let ref_name = prop_schema
                .ref_path
                .as_ref()
                .map(|r| r.strip_prefix("#/definitions/").unwrap_or(r).to_string());

            let is_array = prop_schema.schema_type.as_deref() == Some("array");

            let items_ref = if is_array {
                prop_schema.items.as_ref().and_then(|items| {
                    items
                        .ref_path
                        .as_ref()
                        .map(|r| r.strip_prefix("#/definitions/").unwrap_or(r).to_string())
                })
            } else {
                None
            };

            let required = all_required.contains(prop_name);

            props_obj.insert(
                prop_name.clone(),
                serde_json::json!({
                    "type": prop_type,
                    "format": prop_schema.format,
                    "required": required,
                    "ref_name": ref_name,
                    "is_array": is_array,
                    "items_ref": items_ref,
                    "enum_values": prop_schema.enum_values,
                    "description": prop_schema.description,
                }),
            );
        }

        def_obj.insert(
            "properties".to_string(),
            serde_json::Value::Object(props_obj),
        );
        result.insert(def_name.clone(), serde_json::Value::Object(def_obj));
    }

    Json(serde_json::Value::Object(result)).into_response()
}

async fn admin_routes(State(state): State<AppState>) -> Response {
    let reg = state.registry.read().unwrap();
    let routes: Vec<serde_json::Value> = reg
        .routes
        .iter()
        .map(|r| {
            let (kind, detail) = match &r.shape {
                ResponseShape::Definition(name) => ("definition", Some(name.as_str())),
                ResponseShape::Primitive(t) => ("primitive", Some(t.as_str())),
                ResponseShape::PrimitiveArray(t) => ("primitive_array", Some(t.as_str())),
                ResponseShape::FreeformObject => ("freeform_object", None),
                ResponseShape::Empty => ("empty", None),
            };
            serde_json::json!({
                "method": r.method,
                "path": r.pattern,
                "definition": if matches!(r.shape, ResponseShape::Definition(_)) {
                    serde_json::Value::String(r.table.clone())
                } else {
                    serde_json::Value::Null
                },
                "shape": {
                    "kind": kind,
                    "detail": detail,
                },
            })
        })
        .collect();
    Json(serde_json::json!(routes)).into_response()
}

async fn admin_tables(State(state): State<AppState>) -> Response {
    let conn = state.db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let tables: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            Ok(name)
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .map(|name| {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", name), [], |row| {
                    row.get(0)
                })
                .unwrap_or(0);
            serde_json::json!({"name": name, "row_count": count})
        })
        .collect();
    Json(tables).into_response()
}

async fn admin_table_data(
    State(state): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let conn = state.db.lock().unwrap();

    // Check table exists
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?",
            [&name],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "table not found"})),
        )
            .into_response();
    }

    // Get column info
    let columns: Vec<serde_json::Value> = {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info(\"{}\")", name))
            .unwrap();
        stmt.query_map([], |row| {
            let col_name: String = row.get(1)?;
            let col_type: String = row.get(2)?;
            Ok(serde_json::json!({"name": col_name, "type": col_type}))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    };

    // Get rows
    let sql = format!("SELECT rowid, * FROM \"{}\"", name);
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "query failed"})),
            )
                .into_response();
        }
    };
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| row_to_json(&col_names, row))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    Json(serde_json::json!({"columns": columns, "rows": rows})).into_response()
}

async fn admin_update_table_row(
    State(state): State<AppState>,
    axum::extract::Path((name, rowid)): axum::extract::Path<(String, i64)>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let updates = match body.as_object() {
        Some(map) if !map.is_empty() => map.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "body must be a non-empty JSON object"})),
            )
                .into_response();
        }
    };

    let conn = state.db.lock().unwrap();

    // Check table exists
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?",
            [&name],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "table not found"})),
        )
            .into_response();
    }

    // Get valid column names from PRAGMA table_info (SQL injection prevention)
    let valid_columns: HashSet<String> = {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info(\"{}\")", name))
            .unwrap();
        stmt.query_map([], |row| {
            let col_name: String = row.get(1)?;
            Ok(col_name)
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    };

    // Validate all column names in the request body
    let mut invalid_cols: Vec<String> = Vec::new();
    for key in updates.keys() {
        if !valid_columns.contains(key) {
            invalid_cols.push(key.clone());
        }
    }
    if !invalid_cols.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("unknown columns: {}", invalid_cols.join(", "))
            })),
        )
            .into_response();
    }

    // Build parameterized UPDATE statement with validated column names
    let set_clauses: Vec<String> = updates
        .keys()
        .enumerate()
        .map(|(i, col)| format!("\"{}\" = ?{}", col, i + 1))
        .collect();
    let sql = format!(
        "UPDATE \"{}\" SET {} WHERE rowid = ?{}",
        name,
        set_clauses.join(", "),
        updates.len() + 1
    );

    // Build parameter values
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for val in updates.values() {
        match val {
            serde_json::Value::Null => params.push(Box::new(rusqlite::types::Null)),
            serde_json::Value::Bool(b) => params.push(Box::new(*b)),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    params.push(Box::new(i));
                } else if let Some(f) = n.as_f64() {
                    params.push(Box::new(f));
                } else {
                    params.push(Box::new(n.to_string()));
                }
            }
            serde_json::Value::String(s) => params.push(Box::new(s.clone())),
            // Objects and arrays stored as JSON text
            other => params.push(Box::new(other.to_string())),
        }
    }
    params.push(Box::new(rowid));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let affected = match conn.execute(&sql, param_refs.as_slice()) {
        Ok(n) => n,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    if affected == 0 {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "row not found"})),
        )
            .into_response();
    }

    // Re-query the updated row
    let select_sql = format!("SELECT rowid, * FROM \"{}\" WHERE rowid = ?", name);
    let mut stmt = conn.prepare(&select_sql).unwrap();
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let row = stmt
        .query_row([rowid], |row| row_to_json(&col_names, row))
        .unwrap();

    Json(row).into_response()
}

async fn admin_import(State(state): State<AppState>, body: String) -> Response {
    let spec: SwaggerSpec = match serde_yaml::from_str(&body) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };
    let mut spec = spec;
    let raw_spec = spec.clone(); // keep unresolved copy for $ref extraction
    spec.resolve_refs();

    let endpoints: Vec<EndpointInfo> = spec
        .path_operations()
        .iter()
        .map(|(path, method, _)| EndpointInfo {
            method: method.to_string(),
            path: path.to_string(),
        })
        .collect();
    let spec_info = SpecInfo {
        title: spec.info.title.clone(),
        version: spec.info.version.clone(),
    };

    let mut reg = state.registry.write().unwrap();
    reg.spec = Some(spec);
    reg.raw_spec = Some(raw_spec);
    reg.spec_info = Some(spec_info.clone());

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "spec_info": spec_info,
            "endpoints": endpoints,
        })),
    )
        .into_response()
}

async fn admin_configure(
    State(state): State<AppState>,
    Json(config): Json<ConfigureRequest>,
) -> Response {
    let seed_count = config.seed_count.unwrap_or(10);

    let (spec, raw_spec) = {
        let reg = state.registry.read().unwrap();
        match (&reg.spec, &reg.raw_spec) {
            (Some(s), Some(r)) => (s.clone(), r.clone()),
            (Some(s), None) => (s.clone(), s.clone()),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "No spec imported"})),
                )
                    .into_response();
            }
        }
    };

    // Build selected endpoint list for definition lookup
    let selected: HashSet<(String, String)> = config
        .endpoints
        .iter()
        .map(|e| (e.method.to_lowercase(), e.path.clone()))
        .collect();

    let selected_ops: Vec<(String, String)> = config
        .endpoints
        .iter()
        .map(|e| (e.path.clone(), e.method.to_lowercase()))
        .collect();

    // Extract definition names from the unresolved spec's $ref paths
    let needed_defs = crate::parser::definitions_for_paths(&raw_spec, &selected_ops);

    // Build raw op map for $ref-based table name lookups
    let raw_ops = raw_spec.path_operations();
    let raw_op_map: std::collections::HashMap<(&str, &str), &crate::parser::Operation> = raw_ops
        .iter()
        .map(|(path, method, op)| ((*path, *method), *op))
        .collect();

    let routes: Vec<RouteEntry> = spec
        .path_operations()
        .iter()
        .filter(|(path, method, _)| selected.contains(&(method.to_string(), path.to_string())))
        .map(|(path, method, _)| {
            let raw_op = raw_op_map.get(&(*path, *method));
            let shape = raw_op
                .map(|op| crate::parser::primary_response_shape(op))
                .unwrap_or(ResponseShape::Empty);
            let table = raw_op
                .and_then(|op| crate::parser::primary_response_def(op))
                .unwrap_or_else(|| table_name_from_path(path));
            RouteEntry {
                method: method.to_string(),
                pattern: path.to_string(),
                table,
                has_path_param: path.contains('{'),
                shape,
            }
        })
        .collect();

    // Also add GET collection routes for tables that have any route
    let mut collection_routes: Vec<RouteEntry> = Vec::new();
    let mut seen_tables: HashSet<String> = HashSet::new();
    for route in &routes {
        if !route.has_path_param && route.method == "get" {
            continue;
        }
        let base = format!("/{}", route.table.to_lowercase());
        if seen_tables.insert(route.table.clone())
            && !routes
                .iter()
                .any(|r| r.method == "get" && r.pattern == base && !r.has_path_param)
        {
            collection_routes.push(RouteEntry {
                method: "get".to_string(),
                pattern: base,
                table: route.table.clone(),
                has_path_param: false,
                shape: route.shape.clone(),
            });
        }
    }

    let mut all_routes = routes;
    all_routes.extend(collection_routes);

    // Use definitions referenced by selected operations as the table filter
    // Drop old tables, create only needed ones, seed
    {
        let conn = state.db.lock().unwrap();
        let existing: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for table in &existing {
            conn.execute(&format!("DROP TABLE IF EXISTS \"{table}\""), [])
                .unwrap();
        }
        if let Err(e) = schema::create_tables_filtered(&conn, &spec, Some(&needed_defs)) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to create tables: {e}")})),
            )
                .into_response();
        }
        if let Err(e) =
            seeder::seed_tables_filtered(&conn, &spec, seed_count, Some(&needed_defs), None, None)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to seed data: {e}")})),
            )
                .into_response();
        }
    }

    let endpoints: Vec<EndpointInfo> = all_routes
        .iter()
        .map(|r| EndpointInfo {
            method: r.method.clone(),
            path: r.pattern.clone(),
        })
        .collect();

    {
        let mut reg = state.registry.write().unwrap();
        reg.routes = all_routes;
        reg.endpoints = endpoints;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "configured"})),
    )
        .into_response()
}

#[derive(Deserialize)]
struct CreateRecipeRequest {
    name: String,
    spec_source: String,
    endpoints: Vec<EndpointInfo>,
    seed_count: Option<i64>,
    shared_pools: Option<serde_json::Value>,
    quantity_configs: Option<serde_json::Value>,
    faker_rules: Option<serde_json::Value>,
    rules: Option<serde_json::Value>,
    frozen_rows: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct UpdateRecipeRequest {
    name: String,
    spec_source: String,
    endpoints: Vec<EndpointInfo>,
    seed_count: Option<i64>,
    shared_pools: Option<serde_json::Value>,
    quantity_configs: Option<serde_json::Value>,
    faker_rules: Option<serde_json::Value>,
    rules: Option<serde_json::Value>,
    frozen_rows: Option<serde_json::Value>,
}

async fn admin_create_recipe(
    State(state): State<AppState>,
    Json(body): Json<CreateRecipeRequest>,
) -> Response {
    // Validate the spec_source is valid swagger
    let parsed_spec: SwaggerSpec = match serde_yaml::from_str(&body.spec_source) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid spec: {e}")})),
            )
                .into_response();
        }
    };

    let endpoints_json = match serde_json::to_string(&body.endpoints) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Failed to serialize endpoints: {e}")})),
            )
                .into_response();
        }
    };

    let seed_count = body.seed_count.unwrap_or(10);
    let shared_pools_str = body
        .shared_pools
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));
    let quantity_configs_str = body
        .quantity_configs
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));
    let faker_rules_str = body
        .faker_rules
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));
    let rules_str = body
        .rules
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
    let frozen_rows_str = body
        .frozen_rows
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));

    // Validate rules against the spec (resolve refs first so field lookups work)
    if let Some(ref rs) = rules_str
        && let Err(e) = validate_recipe_rules(rs, &parsed_spec)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid rules: {e}")})),
        )
            .into_response();
    }

    let recipe = {
        let conn = state.recipe_db.lock().unwrap();
        match crate::recipe::create_recipe(
            &conn,
            &body.name,
            &body.spec_source,
            &endpoints_json,
            seed_count,
            shared_pools_str.as_deref(),
            quantity_configs_str.as_deref(),
            faker_rules_str.as_deref(),
            rules_str.as_deref(),
            frozen_rows_str.as_deref(),
        ) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Failed to create recipe: {e}")})),
                )
                    .into_response();
            }
        }
    };

    (StatusCode::CREATED, Json(serde_json::json!(recipe))).into_response()
}

/// Validate a JSON rules string against a swagger spec. Returns Err with a
/// human-readable message if the rules are invalid. Resolves refs on a clone
/// of the spec so the original is untouched.
fn validate_recipe_rules(rules_json: &str, spec: &SwaggerSpec) -> Result<(), String> {
    let rules = crate::rules::parse_rules(rules_json)?;
    let mut resolved = spec.clone();
    resolved.resolve_refs();
    crate::rules::validate_rules(&rules, Some(&resolved))
}

async fn admin_list_recipes(State(state): State<AppState>) -> Response {
    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::list_recipes(&conn) {
        Ok(recipes) => Json(serde_json::json!(recipes)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to list recipes: {e}")})),
        )
            .into_response(),
    }
}

async fn admin_get_recipe(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::get_recipe(&conn, id) {
        Ok(Some(recipe)) => Json(serde_json::json!(recipe)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "recipe not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to get recipe: {e}")})),
        )
            .into_response(),
    }
}

async fn admin_delete_recipe(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::delete_recipe(&conn, id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "recipe not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to delete recipe: {e}")})),
        )
            .into_response(),
    }
}

async fn admin_update_recipe(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<UpdateRecipeRequest>,
) -> Response {
    // Validate the spec_source is valid swagger
    let parsed_spec: SwaggerSpec = match serde_yaml::from_str(&body.spec_source) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid spec: {e}")})),
            )
                .into_response();
        }
    };

    let endpoints_json = match serde_json::to_string(&body.endpoints) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Failed to serialize endpoints: {e}")})),
            )
                .into_response();
        }
    };

    let seed_count = body.seed_count.unwrap_or(10);
    let shared_pools_str = body
        .shared_pools
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());
    let quantity_configs_str = body
        .quantity_configs
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());
    let faker_rules_str = body
        .faker_rules
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());
    let rules_str = body
        .rules
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
        .unwrap_or_else(|| "[]".to_string());
    let frozen_rows_str = body
        .frozen_rows
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_else(|| "{}".to_string());

    // Validate rules with the same checks as create_recipe.
    if let Err(e) = validate_recipe_rules(&rules_str, &parsed_spec) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid rules: {e}")})),
        )
            .into_response();
    }

    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::update_recipe(
        &conn,
        id,
        &body.name,
        &body.spec_source,
        &endpoints_json,
        seed_count,
        &shared_pools_str,
        &quantity_configs_str,
        &faker_rules_str,
        &rules_str,
        &frozen_rows_str,
    ) {
        Ok(true) => match crate::recipe::get_recipe(&conn, id) {
            Ok(Some(recipe)) => Json(serde_json::json!(recipe)).into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "recipe not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to update recipe: {e}")})),
        )
            .into_response(),
    }
}

async fn admin_export_recipe(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::get_recipe(&conn, id) {
        Ok(Some(recipe)) => {
            let endpoints: serde_json::Value =
                serde_json::from_str(&recipe.selected_endpoints).unwrap_or(serde_json::json!([]));
            let shared_pools: serde_json::Value =
                serde_json::from_str(&recipe.shared_pools).unwrap_or(serde_json::json!({}));
            let quantity_configs: serde_json::Value =
                serde_json::from_str(&recipe.quantity_configs).unwrap_or(serde_json::json!({}));
            let faker_rules: serde_json::Value =
                serde_json::from_str(&recipe.faker_rules).unwrap_or(serde_json::json!({}));
            let rules: serde_json::Value =
                serde_json::from_str(&recipe.rules).unwrap_or(serde_json::json!([]));
            let frozen_rows: serde_json::Value =
                serde_json::from_str(&recipe.frozen_rows).unwrap_or(serde_json::json!({}));

            let export = serde_json::json!({
                "mirage_recipe": 2,
                "name": recipe.name,
                "spec_source": recipe.spec_source,
                "selected_endpoints": endpoints,
                "seed_count": recipe.seed_count,
                "shared_pools": shared_pools,
                "quantity_configs": quantity_configs,
                "faker_rules": faker_rules,
                "rules": rules,
                "frozen_rows": frozen_rows,
            });

            let filename = format!(
                "{}.mirage.json",
                recipe
                    .name
                    .to_lowercase()
                    .replace(|c: char| !c.is_alphanumeric(), "-")
            );

            (
                StatusCode::OK,
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        "application/json".to_string(),
                    ),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                serde_json::to_string_pretty(&export).unwrap(),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "recipe not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to export recipe: {e}")})),
        )
            .into_response(),
    }
}

async fn admin_import_recipe(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    // Validate format marker
    let _import_version = match body.get("mirage_recipe").and_then(|v| v.as_i64()) {
        Some(v @ (1 | 2)) => v,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Not a valid Mirage recipe file (missing or unsupported mirage_recipe version)"})),
            )
                .into_response();
        }
    };

    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing recipe name"})),
            )
                .into_response();
        }
    };

    let spec_source = match body.get("spec_source").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Missing spec_source"})),
            )
                .into_response();
        }
    };

    // Validate the spec is parseable
    let parsed_spec: SwaggerSpec = match serde_yaml::from_str(&spec_source) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid spec in recipe: {e}")})),
            )
                .into_response();
        }
    };

    let endpoints = match body.get("selected_endpoints") {
        Some(v) => match serde_json::to_string(v) {
            Ok(j) => j,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": format!("Invalid selected_endpoints: {e}")})),
                )
                    .into_response();
            }
        },
        None => "[]".to_string(),
    };

    let seed_count = body
        .get("seed_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(10);
    let shared_pools_str = body
        .get("shared_pools")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));
    let quantity_configs_str = body
        .get("quantity_configs")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));
    let faker_rules_str = body
        .get("faker_rules")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));
    let rules_str = body
        .get("rules")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()));
    let frozen_rows_str = body
        .get("frozen_rows")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()));

    // Validate rules from the imported recipe.
    if let Some(ref rs) = rules_str
        && let Err(e) = validate_recipe_rules(rs, &parsed_spec)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid rules in imported recipe: {e}")})),
        )
            .into_response();
    }

    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::create_recipe(
        &conn,
        &name,
        &spec_source,
        &endpoints,
        seed_count,
        shared_pools_str.as_deref(),
        quantity_configs_str.as_deref(),
        faker_rules_str.as_deref(),
        rules_str.as_deref(),
        frozen_rows_str.as_deref(),
    ) {
        Ok(recipe) => (StatusCode::CREATED, Json(serde_json::json!(recipe))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to import recipe: {e}")})),
        )
            .into_response(),
    }
}

async fn admin_activate_recipe(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    // Load recipe
    let recipe = {
        let conn = state.recipe_db.lock().unwrap();
        match crate::recipe::get_recipe(&conn, id) {
            Ok(Some(r)) => r,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "recipe not found"})),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Failed to load recipe: {e}")})),
                )
                    .into_response();
            }
        }
    };

    // Parse spec from recipe
    let mut spec: SwaggerSpec = match serde_yaml::from_str(&recipe.spec_source) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid spec in recipe: {e}")})),
            )
                .into_response();
        }
    };
    let raw_spec = spec.clone();
    spec.resolve_refs();

    // Parse selected endpoints
    let endpoints: Vec<EndpointInfo> = match serde_json::from_str(&recipe.selected_endpoints) {
        Ok(e) => e,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Invalid endpoints in recipe: {e}")})),
            )
                .into_response();
        }
    };

    let seed_count = recipe.seed_count as usize;

    // Parse shared_pools and quantity_configs from recipe
    let pool_config = crate::composer::parse_shared_pools(&recipe.shared_pools);
    let quantity_configs = crate::composer::parse_quantity_configs(&recipe.quantity_configs);
    let faker_rules = crate::composer::parse_faker_rules(&recipe.faker_rules);
    // Parse recipe rules. If parsing fails (corrupt store), fall back to no
    // rules but log the failure rather than aborting activation.
    let recipe_rules: Vec<crate::rules::Rule> = match crate::rules::parse_rules(&recipe.rules) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "Warning: failed to parse rules for recipe {}: {e}",
                recipe.id
            );
            Vec::new()
        }
    };

    // Store spec in registry (same as admin_import)
    {
        let spec_info = SpecInfo {
            title: spec.info.title.clone(),
            version: spec.info.version.clone(),
        };
        let mut reg = state.registry.write().unwrap();
        reg.spec = Some(spec.clone());
        reg.raw_spec = Some(raw_spec.clone());
        reg.spec_info = Some(spec_info);
    }

    // Run configure logic (same as admin_configure)
    let config = ConfigureRequest {
        endpoints: endpoints.clone(),
        seed_count: Some(seed_count),
    };

    let selected: HashSet<(String, String)> = config
        .endpoints
        .iter()
        .map(|e| (e.method.to_lowercase(), e.path.clone()))
        .collect();

    let selected_ops: Vec<(String, String)> = config
        .endpoints
        .iter()
        .map(|e| (e.path.clone(), e.method.to_lowercase()))
        .collect();

    let needed_defs = crate::parser::definitions_for_paths(&raw_spec, &selected_ops);

    let raw_ops = raw_spec.path_operations();
    let raw_op_map: std::collections::HashMap<(&str, &str), &crate::parser::Operation> = raw_ops
        .iter()
        .map(|(path, method, op)| ((*path, *method), *op))
        .collect();

    let routes: Vec<RouteEntry> = spec
        .path_operations()
        .iter()
        .filter(|(path, method, _)| selected.contains(&(method.to_string(), path.to_string())))
        .map(|(path, method, _)| {
            let raw_op = raw_op_map.get(&(*path, *method));
            let shape = raw_op
                .map(|op| crate::parser::primary_response_shape(op))
                .unwrap_or(ResponseShape::Empty);
            let table = raw_op
                .and_then(|op| crate::parser::primary_response_def(op))
                .unwrap_or_else(|| table_name_from_path(path));
            RouteEntry {
                method: method.to_string(),
                pattern: path.to_string(),
                table,
                has_path_param: path.contains('{'),
                shape,
            }
        })
        .collect();

    let mut collection_routes: Vec<RouteEntry> = Vec::new();
    let mut seen_tables: HashSet<String> = HashSet::new();
    for route in &routes {
        if !route.has_path_param && route.method == "get" {
            continue;
        }
        let base = format!("/{}", route.table.to_lowercase());
        if seen_tables.insert(route.table.clone())
            && !routes
                .iter()
                .any(|r| r.method == "get" && r.pattern == base && !r.has_path_param)
        {
            collection_routes.push(RouteEntry {
                method: "get".to_string(),
                pattern: base,
                table: route.table.clone(),
                has_path_param: false,
                shape: route.shape.clone(),
            });
        }
    }

    let mut all_routes = routes;
    all_routes.extend(collection_routes);

    // Drop old tables, create only needed ones, seed
    {
        let conn = state.db.lock().unwrap();
        let existing: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for table in &existing {
            conn.execute(&format!("DROP TABLE IF EXISTS \"{table}\""), [])
                .unwrap();
        }
        if let Err(e) = schema::create_tables_filtered(&conn, &spec, Some(&needed_defs)) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to create tables: {e}")})),
            )
                .into_response();
        }

        // Insert frozen rows before seeding
        let frozen: crate::recipe::FrozenRows = match serde_json::from_str(&recipe.frozen_rows) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "Warning: failed to parse frozen_rows for recipe {}: {e}",
                    recipe.id
                );
                std::collections::HashMap::new()
            }
        };
        if !frozen.is_empty() {
            let spec_defs = spec.definitions.as_ref();
            for (table_name, rows) in &frozen {
                if !needed_defs.contains(table_name) {
                    eprintln!(
                        "Warning: frozen_rows references unknown table \"{table_name}\", skipping"
                    );
                    continue;
                }
                let valid_columns: HashSet<String> = spec_defs
                    .and_then(|defs| defs.get(table_name))
                    .and_then(|schema| schema.properties.as_ref())
                    .map(|props| props.keys().cloned().collect())
                    .unwrap_or_default();
                let safe_table = table_name.replace('"', "\"\"");
                for row in rows {
                    let obj = match row.as_object() {
                        Some(o) => o,
                        None => {
                            eprintln!(
                                "Warning: frozen_rows entry for \"{table_name}\" is not an object, skipping"
                            );
                            continue;
                        }
                    };
                    let mut col_names: Vec<String> = Vec::new();
                    let mut col_values: Vec<String> = Vec::new();
                    for (col, val) in obj {
                        if !valid_columns.contains(col) {
                            eprintln!(
                                "Warning: frozen_rows column \"{col}\" not in spec for \"{table_name}\", skipping column"
                            );
                            continue;
                        }
                        let safe_col = col.replace('"', "\"\"");
                        col_names.push(format!("\"{}\"", safe_col));
                        let sql_val = match val {
                            serde_json::Value::Null => "NULL".to_string(),
                            serde_json::Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::String(s) => {
                                format!("'{}'", s.replace('\'', "''"))
                            }
                            other => {
                                format!(
                                    "'{}'",
                                    serde_json::to_string(other)
                                        .unwrap_or_default()
                                        .replace('\'', "''")
                                )
                            }
                        };
                        col_values.push(sql_val);
                    }
                    if !col_names.is_empty() {
                        let sql = format!(
                            "INSERT INTO \"{}\" ({}) VALUES ({})",
                            safe_table,
                            col_names.join(", "),
                            col_values.join(", ")
                        );
                        if let Err(e) = conn.execute(&sql, []) {
                            eprintln!(
                                "Warning: failed to insert frozen row into \"{table_name}\": {e}"
                            );
                        }
                    } else if !obj.is_empty() {
                        eprintln!(
                            "[frozen] Skipping row for table '{}': all columns were invalid",
                            table_name
                        );
                    }
                }
            }
        }

        if let Err(e) = seeder::seed_tables_filtered(
            &conn,
            &spec,
            seed_count,
            Some(&needed_defs),
            Some(&faker_rules),
            Some(&recipe_rules),
        ) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to seed data: {e}")})),
            )
                .into_response();
        }
    }

    // Generate document store using composer
    let entity_graph = crate::entity_graph::build_entity_graph(&raw_spec, &selected_ops);
    let pools = crate::composer::generate_pools(&spec, &pool_config, &faker_rules, &recipe_rules);

    // Build quantity configs: use recipe quantity_configs, with seed_count as default
    let mut effective_quantities = quantity_configs;
    // For definitions that don't have explicit quantity configs, use seed_count
    for def_name in &needed_defs {
        effective_quantities
            .entry(def_name.clone())
            .or_insert(crate::composer::QuantityConfig {
                min: seed_count,
                max: seed_count,
            });
    }

    let composed = crate::composer::compose_documents(
        &spec,
        &raw_spec,
        &entity_graph,
        &pools,
        &effective_quantities,
        &endpoints,
        &faker_rules,
        &recipe_rules,
    );

    // Store composed documents
    {
        let mut docs = state.documents.write().unwrap();
        *docs = composed;
    }

    let activated_endpoints: Vec<EndpointInfo> = all_routes
        .iter()
        .map(|r| EndpointInfo {
            method: r.method.clone(),
            path: r.pattern.clone(),
        })
        .collect();

    {
        let mut reg = state.registry.write().unwrap();
        reg.routes = all_routes;
        reg.endpoints = activated_endpoints.clone();
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "activated",
            "endpoints": activated_endpoints,
        })),
    )
        .into_response()
}

async fn admin_get_recipe_config(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::get_recipe(&conn, id) {
        Ok(Some(recipe)) => {
            let shared_pools: serde_json::Value =
                serde_json::from_str(&recipe.shared_pools).unwrap_or(serde_json::json!({}));
            let quantity_configs: serde_json::Value =
                serde_json::from_str(&recipe.quantity_configs).unwrap_or(serde_json::json!({}));
            let faker_rules: serde_json::Value =
                serde_json::from_str(&recipe.faker_rules).unwrap_or(serde_json::json!({}));
            let rules: serde_json::Value =
                serde_json::from_str(&recipe.rules).unwrap_or(serde_json::json!([]));
            let frozen_rows: serde_json::Value =
                serde_json::from_str(&recipe.frozen_rows).unwrap_or(serde_json::json!({}));
            Json(serde_json::json!({
                "shared_pools": shared_pools,
                "quantity_configs": quantity_configs,
                "faker_rules": faker_rules,
                "rules": rules,
                "frozen_rows": frozen_rows,
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "recipe not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to get recipe: {e}")})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct UpdateRecipeConfigRequest {
    shared_pools: serde_json::Value,
    quantity_configs: serde_json::Value,
    faker_rules: serde_json::Value,
    #[serde(default)]
    rules: serde_json::Value,
    #[serde(default = "default_frozen_rows")]
    frozen_rows: serde_json::Value,
}

fn default_frozen_rows() -> serde_json::Value {
    serde_json::json!({})
}

async fn admin_put_recipe_config(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<UpdateRecipeConfigRequest>,
) -> Response {
    let shared_pools_str =
        serde_json::to_string(&body.shared_pools).unwrap_or_else(|_| "{}".to_string());
    let quantity_configs_str =
        serde_json::to_string(&body.quantity_configs).unwrap_or_else(|_| "{}".to_string());
    let faker_rules_str =
        serde_json::to_string(&body.faker_rules).unwrap_or_else(|_| "{}".to_string());
    let rules_str = if body.rules.is_null() {
        "[]".to_string()
    } else {
        serde_json::to_string(&body.rules).unwrap_or_else(|_| "[]".to_string())
    };
    let frozen_rows_str = if body.frozen_rows.is_null() {
        "{}".to_string()
    } else {
        serde_json::to_string(&body.frozen_rows).unwrap_or_else(|_| "{}".to_string())
    };

    // Validate rules against the recipe's spec.
    {
        let conn = state.recipe_db.lock().unwrap();
        let existing = match crate::recipe::get_recipe(&conn, id) {
            Ok(Some(r)) => r,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "recipe not found"})),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Failed to load recipe: {e}")})),
                )
                    .into_response();
            }
        };
        let parsed_spec: SwaggerSpec = match serde_yaml::from_str(&existing.spec_source) {
            Ok(s) => s,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Stored spec invalid: {e}")})),
                )
                    .into_response();
            }
        };
        if let Err(e) = validate_recipe_rules(&rules_str, &parsed_spec) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("Invalid rules: {e}")})),
            )
                .into_response();
        }
    }

    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::update_recipe_config(
        &conn,
        id,
        &shared_pools_str,
        &quantity_configs_str,
        &faker_rules_str,
        &rules_str,
        &frozen_rows_str,
    ) {
        Ok(true) => Json(serde_json::json!({
            "shared_pools": body.shared_pools,
            "quantity_configs": body.quantity_configs,
            "faker_rules": body.faker_rules,
            "rules": serde_json::from_str::<serde_json::Value>(&rules_str).unwrap_or(serde_json::json!([])),
            "frozen_rows": serde_json::from_str::<serde_json::Value>(&frozen_rows_str).unwrap_or(serde_json::json!({})),
        }))
        .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "recipe not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to update config: {e}")})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct GraphRequest {
    spec_source: String,
    endpoints: Vec<EndpointInfo>,
}

async fn admin_graph_current(State(state): State<AppState>) -> impl IntoResponse {
    let reg = state.registry.read().unwrap();
    let raw_spec = match &reg.raw_spec {
        Some(s) => s,
        None => {
            return Json(serde_json::json!({"nodes":[],"edges":{},"shared_entities":[],"roots":{},"array_properties":[],"virtual_roots":[]})).into_response();
        }
    };
    let selected: Vec<(String, String)> = if reg.endpoints.is_empty() {
        raw_spec
            .path_operations()
            .iter()
            .map(|(path, method, _)| (path.to_string(), method.to_string()))
            .collect()
    } else {
        reg.endpoints
            .iter()
            .map(|e| (e.path.clone(), e.method.to_lowercase()))
            .collect()
    };
    let graph = crate::entity_graph::build_entity_graph(raw_spec, &selected);
    Json(serde_json::json!(graph)).into_response()
}

async fn admin_graph(Json(req): Json<GraphRequest>) -> impl IntoResponse {
    let spec: SwaggerSpec = match serde_yaml::from_str(&req.spec_source) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };
    let selected: Vec<(String, String)> = req
        .endpoints
        .iter()
        .map(|e| (e.path.clone(), e.method.to_lowercase()))
        .collect();
    let graph = crate::entity_graph::build_entity_graph(&spec, &selected);
    (StatusCode::OK, Json(serde_json::json!(graph))).into_response()
}

async fn serve_admin(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().strip_prefix("/_admin/").unwrap_or("");
    let path = if path.is_empty() { "index.html" } else { path };

    match AdminAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => match AdminAssets::get("index.html") {
            Some(file) => (
                [(axum::http::header::CONTENT_TYPE, "text/html")],
                file.data.into_owned(),
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

pub fn populate_registry(reg: &mut RouteRegistry, spec: &SwaggerSpec, raw_spec: &SwaggerSpec) {
    let spec_info = SpecInfo {
        title: spec.info.title.clone(),
        version: spec.info.version.clone(),
    };

    let mut routes: Vec<RouteEntry> = Vec::new();
    let mut registered: HashSet<String> = HashSet::new();

    // Use the raw (unresolved) spec for $ref-based table name lookups
    let raw_ops = raw_spec.path_operations();
    let raw_op_map: std::collections::HashMap<(&str, &str), &crate::parser::Operation> = raw_ops
        .iter()
        .map(|(path, method, op)| ((*path, *method), *op))
        .collect();

    for (path, method, _op) in spec.path_operations() {
        let raw_op = raw_op_map.get(&(path, method));
        let shape = raw_op
            .map(|op| crate::parser::primary_response_shape(op))
            .unwrap_or(ResponseShape::Empty);
        let table = raw_op
            .and_then(|op| crate::parser::primary_response_def(op))
            .unwrap_or_else(|| table_name_from_path(path));
        let has_path_param = path.contains('{');

        let key = format!("{method}:{path}");
        if registered.contains(&key) {
            continue;
        }
        registered.insert(key);

        routes.push(RouteEntry {
            method: method.to_string(),
            pattern: path.to_string(),
            table: table.clone(),
            has_path_param,
            shape: shape.clone(),
        });

        // Auto-register collection GET for any table that has routes
        if has_path_param {
            let base = format!(
                "/{}",
                path.trim_start_matches('/').split('/').next().unwrap_or("")
            );
            let coll_key = format!("get:{base}");
            if !registered.contains(&coll_key) {
                routes.push(RouteEntry {
                    method: "get".to_string(),
                    pattern: base,
                    table: table.clone(),
                    has_path_param: false,
                    shape: shape.clone(),
                });
                registered.insert(coll_key);
            }
        }
    }

    let endpoints: Vec<EndpointInfo> = routes
        .iter()
        .map(|r| EndpointInfo {
            method: r.method.clone(),
            path: r.pattern.clone(),
        })
        .collect();

    reg.routes = routes;
    reg.spec_info = Some(spec_info);
    reg.endpoints = endpoints;
    reg.raw_spec = Some(raw_spec.clone());
    reg.spec = Some(spec.clone());
}

async fn logging_middleware(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    // Skip static asset requests and log endpoint itself
    let should_log = !path.starts_with("/_admin")
        && path != "/_api/admin/log"
        && path != "/_api/admin/spec"
        && path != "/_api/admin/endpoints"
        && path != "/_api/admin/tables";

    if !should_log {
        return next.run(req).await;
    }

    // Buffer request body. We must read the whole body in order to log it
    // and then reconstruct the request for downstream handlers. If the body
    // is larger than `LOG_BODY_LIMIT_BYTES` the original stream is partially
    // consumed and unrecoverable, so we cannot forward the request: return
    // 413 Payload Too Large and record the event in the log.
    let (parts, body) = req.into_parts();
    let req_bytes = match axum::body::to_bytes(body, LOG_BODY_LIMIT_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) => {
            let is_limit = err
                .source()
                .and_then(|s| s.downcast_ref::<LengthLimitError>())
                .is_some();
            if is_limit {
                eprintln!(
                    "Warning: request body for {method} {path} exceeded {LOG_BODY_LIMIT_BYTES} bytes ({err}); returning 413"
                );
                let sentinel =
                    format!("<body too large to log: exceeded {LOG_BODY_LIMIT_BYTES} bytes>");
                log_request(
                    &state.log,
                    &method,
                    &path,
                    StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
                    Some(sentinel),
                    None,
                );
                return (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(serde_json::json!({
                        "error": format!(
                            "request body exceeds limit of {LOG_BODY_LIMIT_BYTES} bytes"
                        )
                    })),
                )
                    .into_response();
            } else {
                eprintln!("logging_middleware request body read error (non-limit): {err}");
                let sentinel = format!("<body read error: {err}>");
                log_request(
                    &state.log,
                    &method,
                    &path,
                    StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    Some(sentinel),
                    None,
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "failed to read request body"
                    })),
                )
                    .into_response();
            }
        }
    };
    let request_body = if req_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&req_bytes).into_owned())
    };
    let req = axum::http::Request::from_parts(parts, axum::body::Body::from(req_bytes));

    let response = next.run(req).await;
    let status = response.status().as_u16();

    // Buffer response body. If it exceeds the limit we log a sentinel and
    // forward the response with an empty body (matching the prior failure
    // mode). The 16 MB ceiling makes this branch a safeguard, not a hot
    // path; very-large responses remain a known theoretical hole.
    let (mut parts, body) = response.into_parts();
    let (response_body, forward_bytes) = match axum::body::to_bytes(body, LOG_BODY_LIMIT_BYTES)
        .await
    {
        Ok(bytes) => {
            let logged = if bytes.is_empty() {
                None
            } else {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            };
            (logged, bytes)
        }
        Err(err) => {
            let is_limit = err
                .source()
                .and_then(|s| s.downcast_ref::<LengthLimitError>())
                .is_some();
            if is_limit {
                eprintln!(
                    "Warning: response body for {method} {path} exceeded {LOG_BODY_LIMIT_BYTES} bytes ({err}); logging sentinel"
                );
            } else {
                eprintln!("logging_middleware response body read error (non-limit): {err}");
            }
            let sentinel =
                format!("<body too large to log: exceeded {LOG_BODY_LIMIT_BYTES} bytes>");
            parts.headers.remove(axum::http::header::CONTENT_LENGTH);
            parts.headers.remove(axum::http::header::TRANSFER_ENCODING);
            (Some(sentinel), axum::body::Bytes::new())
        }
    };

    log_request(
        &state.log,
        &method,
        &path,
        status,
        request_body,
        response_body,
    );

    Response::from_parts(parts, axum::body::Body::from(forward_bytes))
}

pub fn build_router(state: AppState) -> Router {
    let admin_api = Router::new()
        .route("/spec", get(admin_spec))
        .route("/endpoints", get(admin_endpoints))
        .route("/definitions", get(admin_definitions))
        .route("/routes", get(admin_routes))
        .route("/tables", get(admin_tables))
        .route("/tables/{name}", get(admin_table_data))
        .route("/tables/{name}/{rowid}", put(admin_update_table_row))
        .route("/log", get(admin_log))
        .route("/import", post(admin_import))
        .route("/configure", post(admin_configure))
        .route("/graph", get(admin_graph_current).post(admin_graph))
        .route(
            "/recipes",
            post(admin_create_recipe).get(admin_list_recipes),
        )
        .route(
            "/recipes/{id}",
            get(admin_get_recipe)
                .delete(admin_delete_recipe)
                .put(admin_update_recipe),
        )
        .route("/recipes/{id}/export", get(admin_export_recipe))
        .route("/recipes/import", post(admin_import_recipe))
        .route("/recipes/{id}/activate", post(admin_activate_recipe))
        .route(
            "/recipes/{id}/config",
            get(admin_get_recipe_config).put(admin_put_recipe_config),
        )
        .with_state(state.clone());

    Router::new()
        .nest("/_api/admin", admin_api)
        .route(
            "/_admin",
            get(|| async { axum::response::Redirect::permanent("/_admin/") }),
        )
        .route("/_admin/", get(serve_admin))
        .route("/_admin/{*path}", get(serve_admin))
        .route("/{*path}", any(catch_all_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            logging_middleware,
        ))
        // Raise axum's 2 MB default body limit so the `Json` extractor
        // accepts payloads up to `LOG_BODY_LIMIT_BYTES`.  Note:
        // `DefaultBodyLimit` only sets a request extension that extractors
        // check — it does NOT eagerly cap the body stream.  Our logging
        // middleware calls `to_bytes` first (with the same limit), so it is
        // the actual first line of enforcement.
        .layer(DefaultBodyLimit::max(LOG_BODY_LIMIT_BYTES))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SwaggerSpec;
    use crate::schema::create_tables;
    use crate::seeder::seed_tables;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use rusqlite::Connection;
    use tower::ServiceExt;

    fn setup() -> Router {
        let mut spec = SwaggerSpec::from_file("tests/fixtures/petstore.yaml").unwrap();
        let raw_spec = spec.clone();
        spec.resolve_refs();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn, &spec).unwrap();
        seed_tables(&conn, &spec, 5).unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        populate_registry(&mut registry.write().unwrap(), &spec, &raw_spec);
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let recipe_conn = Connection::open_in_memory().unwrap();
        crate::recipe::init_recipe_db(&recipe_conn).unwrap();
        let recipe_db: Db = Arc::new(Mutex::new(recipe_conn));
        let state = AppState {
            db,
            registry,
            log,
            recipe_db,
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        };
        build_router(state)
    }

    fn setup_empty() -> Router {
        let conn = Connection::open_in_memory().unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let recipe_conn = Connection::open_in_memory().unwrap();
        crate::recipe::init_recipe_db(&recipe_conn).unwrap();
        let recipe_db: Db = Arc::new(Mutex::new(recipe_conn));
        let state = AppState {
            db,
            registry,
            log,
            recipe_db,
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        };
        build_router(state)
    }

    #[tokio::test]
    async fn test_get_collection() {
        let router = setup();
        let req = Request::builder().uri("/pet").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("response should be an array");
        assert_eq!(arr.len(), 5);
    }

    #[tokio::test]
    async fn test_get_single() {
        let router = setup();
        let req = Request::builder()
            .uri("/pet/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_object(), "response should be a JSON object");
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let router = setup();
        let req = Request::builder()
            .uri("/pet/99999")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_post_create() {
        let router = setup();
        let body = serde_json::json!({
            "name": "Fluffy",
            "status": "available"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/pet")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("id").is_some(), "response should contain an id");
    }

    #[tokio::test]
    async fn test_delete() {
        let router = setup();

        // DELETE /pet/1
        let req = Request::builder()
            .method("DELETE")
            .uri("/pet/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // GET /pet/1 should now 404
        let req = Request::builder()
            .uri("/pet/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_admin_page() {
        let router = setup();
        let req = Request::builder()
            .uri("/_admin/")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/html"),
            "expected text/html, got {content_type}"
        );
    }

    #[tokio::test]
    async fn test_admin_api_spec() {
        let router = setup();
        let req = Request::builder()
            .uri("/_api/admin/spec")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("title").is_some(), "spec should have title");
        assert!(json.get("version").is_some(), "spec should have version");
    }

    #[tokio::test]
    async fn test_admin_api_endpoints() {
        let router = setup();
        let req = Request::builder()
            .uri("/_api/admin/endpoints")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("endpoints should be an array");
        assert!(!arr.is_empty(), "endpoints should not be empty");
        let first = &arr[0];
        assert!(first.get("method").is_some(), "endpoint should have method");
        assert!(first.get("path").is_some(), "endpoint should have path");
    }

    #[tokio::test]
    async fn test_import_spec() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/import")
            .header("content-type", "text/plain")
            .body(Body::from(spec_yaml))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json.get("spec_info").is_some(),
            "import response should have spec_info"
        );
        assert!(
            json.get("endpoints").is_some(),
            "import response should have endpoints"
        );
        let endpoints = json["endpoints"].as_array().unwrap();
        assert!(!endpoints.is_empty(), "should have discovered endpoints");
    }

    #[tokio::test]
    async fn test_configure() {
        let conn = Connection::open_in_memory().unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let recipe_conn = Connection::open_in_memory().unwrap();
        crate::recipe::init_recipe_db(&recipe_conn).unwrap();
        let recipe_db: Db = Arc::new(Mutex::new(recipe_conn));
        let state = AppState {
            db,
            registry: registry.clone(),
            log,
            recipe_db,
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        };
        let router = build_router(state);

        // Import spec first
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/import")
            .header("content-type", "text/plain")
            .body(Body::from(spec_yaml))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Configure with selected endpoints
        let config = serde_json::json!({
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"},
                {"method": "delete", "path": "/pet/{petId}"}
            ],
            "seed_count": 3
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/configure")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&config).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Now GET /pet should work (auto-registered collection)
        let req = Request::builder().uri("/pet").body(Body::empty()).unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 3, "should have 3 seeded rows");
    }

    #[tokio::test]
    async fn test_no_spec_startup() {
        let router = setup_empty();

        // Admin spec endpoint should still work
        let req = Request::builder()
            .uri("/_api/admin/spec")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // But /pet should 404 since no routes are configured
        let req = Request::builder().uri("/pet").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    fn recipe_test_body() -> serde_json::Value {
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        serde_json::json!({
            "name": "My Petstore",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"},
                {"method": "delete", "path": "/pet/{petId}"}
            ],
            "seed_count": 5
        })
    }

    #[tokio::test]
    async fn test_create_recipe() {
        let router = setup_empty();
        let body = recipe_test_body();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("id").is_some(), "response should have id");
        assert!(
            json.get("created_at").is_some(),
            "response should have created_at"
        );
        assert_eq!(json["name"], "My Petstore");
        assert_eq!(json["seed_count"], 5);
    }

    #[tokio::test]
    async fn test_list_recipes() {
        let router = setup_empty();
        let body = recipe_test_body();

        // Create first recipe
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Create second recipe
        let mut body2 = recipe_test_body();
        body2["name"] = serde_json::json!("Second Recipe");
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body2).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // List recipes
        let req = Request::builder()
            .uri("/_api/admin/recipes")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("response should be an array");
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn test_get_recipe() {
        let router = setup_empty();
        let body = recipe_test_body();

        // Create recipe
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Get by id
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "My Petstore");

        // Get non-existent
        let req = Request::builder()
            .uri("/_api/admin/recipes/99999")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_recipe() {
        let router = setup_empty();
        let body = recipe_test_body();

        // Create recipe
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Delete
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/_api/admin/recipes/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Get should 404
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_activate_recipe() {
        let router = setup_empty();
        let body = recipe_test_body();

        // Create recipe
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Activate
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "activated");
        assert!(
            json.get("endpoints").is_some(),
            "response should have endpoints"
        );

        // Mock endpoint should now work
        let req = Request::builder().uri("/pet").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 5, "should have 5 seeded rows from recipe");
    }

    #[tokio::test]
    async fn test_recipe_survives_configure() {
        let conn = Connection::open_in_memory().unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let recipe_conn = Connection::open_in_memory().unwrap();
        crate::recipe::init_recipe_db(&recipe_conn).unwrap();
        let recipe_db: Db = Arc::new(Mutex::new(recipe_conn));
        let state = AppState {
            db,
            registry,
            log,
            recipe_db,
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        };
        let router = build_router(state);

        // Create a recipe
        let body = recipe_test_body();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Import spec and configure mock DB (this recreates mock tables)
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/import")
            .header("content-type", "text/plain")
            .body(Body::from(spec_yaml))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let config = serde_json::json!({
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 3
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/configure")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&config).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Recipe should still be there
        let req = Request::builder()
            .uri("/_api/admin/recipes")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("response should be an array");
        assert_eq!(arr.len(), 1, "recipe should survive mock DB configure");
    }

    #[tokio::test]
    async fn test_admin_graph_petstore() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "spec_source": spec_yaml,
            "endpoints": [{"method": "get", "path": "/pet/{petId}"}]
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/graph")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let nodes = json["nodes"].as_array().unwrap();
        let node_strs: Vec<&str> = nodes.iter().map(|n| n.as_str().unwrap()).collect();
        assert!(node_strs.contains(&"Pet"));
        assert!(node_strs.contains(&"Category"));
        assert!(node_strs.contains(&"Tag"));
    }

    #[tokio::test]
    async fn test_admin_graph_empty_endpoints() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "spec_source": spec_yaml,
            "endpoints": []
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/graph")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let nodes = json["nodes"].as_array().unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn test_admin_graph_invalid_spec() {
        let router = setup_empty();
        let body = serde_json::json!({
            "spec_source": "not valid yaml {{{{",
            "endpoints": [{"method": "get", "path": "/test"}]
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/graph")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("error").is_some(), "should have error field");
    }

    #[tokio::test]
    async fn test_get_recipe_config() {
        let router = setup_empty();
        let body = recipe_test_body();

        // Create recipe
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // GET config -> 200 with default empty pools/configs
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["shared_pools"], serde_json::json!({}));
        assert_eq!(json["quantity_configs"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_put_recipe_config() {
        let router = setup_empty();
        let body = recipe_test_body();

        // Create recipe
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // PUT config with values
        let config = serde_json::json!({
            "shared_pools": {"Pet": {"is_shared": true, "pool_size": 5}},
            "quantity_configs": {"Pet.tags": {"min": 1, "max": 3}},
            "faker_rules": {},
        });
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&config).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET back -> values match
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["shared_pools"]["Pet"]["pool_size"],
            serde_json::json!(5)
        );
        assert_eq!(
            json["quantity_configs"]["Pet.tags"]["max"],
            serde_json::json!(3)
        );
    }

    #[tokio::test]
    async fn test_put_recipe_config_404() {
        let router = setup_empty();

        // PUT config for non-existent id -> 404
        let config = serde_json::json!({
            "shared_pools": {},
            "quantity_configs": {},
            "faker_rules": {},
        });
        let req = Request::builder()
            .method("PUT")
            .uri("/_api/admin/recipes/99999/config")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&config).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_create_recipe_with_config() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Configured Petstore",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
            ],
            "seed_count": 5,
            "shared_pools": {"Pet": {"is_shared": true, "pool_size": 10}},
            "quantity_configs": {"Pet.tags": {"min": 2, "max": 5}},
        });

        // Create recipe with config
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();
        assert_eq!(created["name"], "Configured Petstore");

        // GET recipe shows config fields
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // shared_pools and quantity_configs are stored as JSON strings; the server
        // serializes Recipe with serde so they come back as strings
        let shared_pools: serde_json::Value =
            serde_json::from_str(json["shared_pools"].as_str().unwrap()).unwrap();
        assert_eq!(shared_pools["Pet"]["pool_size"], serde_json::json!(10));
        let quantity_configs: serde_json::Value =
            serde_json::from_str(json["quantity_configs"].as_str().unwrap()).unwrap();
        assert_eq!(quantity_configs["Pet.tags"]["max"], serde_json::json!(5));
    }

    // -------------------------------------------------------------------
    // Recipe rules HTTP tests
    // -------------------------------------------------------------------

    fn recipe_body_with_rules(rules: serde_json::Value) -> serde_json::Value {
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        serde_json::json!({
            "name": "Ruled Petstore",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
            ],
            "seed_count": 5,
            "rules": rules,
        })
    }

    #[tokio::test]
    async fn test_create_recipe_with_range_rule() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "range", "field": "Pet.id", "min": 1, "max": 100}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_recipe_with_choice_rule() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "choice", "field": "Pet.status", "options": ["available", "pending"]}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_recipe_with_const_rule() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "const", "field": "Pet.name", "value": "Rex"}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_recipe_with_pattern_rule() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "pattern", "field": "Pet.name", "regex": "[A-Z][a-z]{2,5}"}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_recipe_with_compare_rule_literal() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "compare", "left": "Pet.id", "op": "gt", "right": 100}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_recipe_rejects_conflicting_rules() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "range", "field": "Pet.id", "min": 1, "max": 100},
            {"kind": "const", "field": "Pet.id", "value": 42}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let err = json["error"].as_str().unwrap();
        assert!(
            err.contains("Pet.id"),
            "error should mention Pet.id, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_create_recipe_rejects_unknown_field() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "const", "field": "Pet.bogus_field", "value": 1}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_recipe_rejects_bad_regex() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "pattern", "field": "Pet.name", "regex": "[unterminated"}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_recipe_rejects_compare_cycle() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "compare", "left": "Category.id", "op": "gt", "right": "Tag.id"},
            {"kind": "compare", "left": "Tag.id", "op": "gt", "right": "Category.id"}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        // This is rejected at the cross-def check (compare cross-definition not supported).
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_update_recipe_validates_rules() {
        let router = setup_empty();

        // Create a recipe without rules.
        let body = recipe_test_body();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // PUT with conflicting rules.
        let mut update = recipe_test_body();
        update["rules"] = serde_json::json!([
            {"kind": "range", "field": "Pet.id", "min": 1, "max": 100},
            {"kind": "const", "field": "Pet.id", "value": 42}
        ]);
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/_api/admin/recipes/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&update).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "PUT should reject conflicting rules just like POST"
        );

        // PUT with valid rules succeeds.
        let mut update = recipe_test_body();
        update["rules"] = serde_json::json!([
            {"kind": "range", "field": "Pet.id", "min": 1, "max": 100}
        ]);
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/_api/admin/recipes/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&update).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_recipe_config_includes_rules() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "const", "field": "Pet.name", "value": "Bingo"}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // GET /_api/admin/recipes/:id/config should include rules.
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rules = json["rules"].as_array().expect("rules should be an array");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["kind"], "const");
        assert_eq!(rules[0]["field"], "Pet.name");
        assert_eq!(rules[0]["value"], "Bingo");
    }

    #[tokio::test]
    async fn test_put_recipe_config_with_rules() {
        let router = setup_empty();

        // Create a recipe.
        let body = recipe_test_body();
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // PUT config with rules.
        let body = serde_json::json!({
            "shared_pools": {},
            "quantity_configs": {},
            "faker_rules": {},
            "rules": [
                {"kind": "const", "field": "Pet.name", "value": "Whiskers"}
            ]
        });
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // GET should reflect the new rules.
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rules = json["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["value"], "Whiskers");
    }

    #[tokio::test]
    async fn test_activate_recipe_applies_rules() {
        let router = setup_empty();
        let body = recipe_body_with_rules(serde_json::json!([
            {"kind": "const", "field": "Pet.name", "value": "Cosmo"}
        ]));
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Activate.
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Hit /pet -- composer documents drive the response. Every name should
        // be "Cosmo".
        let req = Request::builder().uri("/pet").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("response should be an array");
        assert!(!arr.is_empty(), "should have at least one pet");
        for pet in arr {
            assert_eq!(
                pet["name"].as_str().unwrap_or(""),
                "Cosmo",
                "all pets should be named Cosmo, got {pet}"
            );
        }
    }

    /// Build a recipe-create body whose `spec_source` is the petstore YAML
    /// padded to roughly `target_size` bytes. Padding is added as a top-level
    /// `x-padding:` key whose scalar value is a long string of `x`s, which
    /// keeps the document valid YAML and a valid swagger 2.0 spec (unknown
    /// `x-` extensions are ignored by serde via `#[serde(deny_unknown_fields)]`
    /// being absent on `SwaggerSpec`).
    fn padded_recipe_body(name: &str, target_size: usize) -> serde_json::Value {
        let base = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let mut spec = base.clone();
        if !spec.ends_with('\n') {
            spec.push('\n');
        }
        let prefix = "x-padding: \"";
        let suffix = "\"\n";
        let overhead = spec.len() + prefix.len() + suffix.len();
        if target_size > overhead {
            let pad_len = target_size - overhead;
            spec.push_str(prefix);
            spec.extend(std::iter::repeat_n('x', pad_len));
            spec.push_str(suffix);
        }
        serde_json::json!({
            "name": name,
            "spec_source": spec,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 1
        })
    }

    #[tokio::test]
    async fn test_recipe_spec_source_just_under_1mb() {
        let router = setup_empty();
        let body = padded_recipe_body("Padded ~999KB", 999 * 1024);
        let payload = serde_json::to_string(&body).unwrap();
        // Sanity: the wire payload is at least the spec size.
        assert!(payload.len() >= 999 * 1024);
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(payload))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_recipe_spec_source_just_over_1mb() {
        // Regression test for the bug where `to_bytes(.., 1024 * 1024)` in
        // logging_middleware silently dropped bodies > 1 MB and the
        // downstream Json extractor returned 400 EOF.
        let router = setup_empty();
        let body = padded_recipe_body("Padded ~1.1MB", 1_100_000);
        let payload = serde_json::to_string(&body).unwrap();
        assert!(payload.len() > 1024 * 1024);
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(payload))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_recipe_spec_source_at_16mb() {
        // Pad the spec close to but just under the 16 MB middleware ceiling.
        // The JSON envelope adds a small amount of overhead so we leave a
        // 256 KB margin to stay below the limit.
        let router = setup_empty();
        let target = 16 * 1024 * 1024 - 256 * 1024;
        let body = padded_recipe_body("Padded ~16MB", target);
        let payload = serde_json::to_string(&body).unwrap();
        assert!(payload.len() > 15 * 1024 * 1024);
        assert!(payload.len() < 16 * 1024 * 1024);
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(payload))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_recipe_spec_source_zero_bytes() {
        // Confirm we did not suppress legitimate validation: an empty
        // spec_source must still be rejected by the spec parser.
        let router = setup_empty();
        let body = serde_json::json!({
            "name": "Empty Spec",
            "spec_source": "",
            "endpoints": [],
            "seed_count": 1
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_truncate_for_log_short_string_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_for_log(s), s);
    }

    #[test]
    fn test_truncate_for_log_long_string_truncated() {
        let s = "a".repeat(LOG_BODY_STORE_MAX + 1024);
        let out = truncate_for_log(&s);
        assert!(out.starts_with(&"a".repeat(LOG_BODY_STORE_MAX)));
        assert!(out.contains("[truncated:"));
        assert!(out.len() < s.len());
    }

    #[tokio::test]
    async fn test_admin_update_row_happy_path() {
        let router = setup();
        let body = serde_json::json!({"name": "UpdatedPetName"});
        let req = Request::builder()
            .method("PUT")
            .uri("/_api/admin/tables/Pet/1")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("rowid").is_some(), "response should contain rowid");
        assert_eq!(json["name"], "UpdatedPetName");
    }

    #[tokio::test]
    async fn test_admin_update_row_unknown_table() {
        let router = setup();
        let body = serde_json::json!({"name": "x"});
        let req = Request::builder()
            .method("PUT")
            .uri("/_api/admin/tables/nonexistent/1")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "table not found");
    }

    #[tokio::test]
    async fn test_admin_update_row_unknown_rowid() {
        let router = setup();
        let body = serde_json::json!({"name": "x"});
        let req = Request::builder()
            .method("PUT")
            .uri("/_api/admin/tables/Pet/99999")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "row not found");
    }

    #[tokio::test]
    async fn test_admin_update_row_unknown_column() {
        let router = setup();
        let body = serde_json::json!({"bogus_col": "x"});
        let req = Request::builder()
            .method("PUT")
            .uri("/_api/admin/tables/Pet/1")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let error = json["error"].as_str().unwrap();
        assert!(
            error.contains("bogus_col"),
            "error should mention the invalid column name"
        );
    }

    #[tokio::test]
    async fn test_create_recipe_with_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Frozen Petstore",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 2,
            "frozen_rows": {
                "Pet": [
                    {"name": "Frosty", "status": "available"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("id").is_some());
        let frozen: serde_json::Value =
            serde_json::from_str(json["frozen_rows"].as_str().unwrap()).unwrap();
        let pets = frozen["Pet"].as_array().unwrap();
        assert_eq!(pets.len(), 1);
        assert_eq!(pets[0]["name"], "Frosty");
    }

    #[tokio::test]
    async fn test_frozen_rows_crud_roundtrip() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Roundtrip",
            "spec_source": spec_yaml,
            "endpoints": [{"method": "get", "path": "/pet/{petId}"}],
            "seed_count": 1,
            "frozen_rows": {
                "Pet": [
                    {"name": "Alpha", "status": "pending"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // GET recipe should include frozen_rows
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let got: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let frozen: serde_json::Value =
            serde_json::from_str(got["frozen_rows"].as_str().unwrap()).unwrap();
        assert_eq!(frozen["Pet"][0]["name"], "Alpha");

        // GET config should include frozen_rows
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let config: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(config["frozen_rows"]["Pet"][0]["name"], "Alpha");

        // PUT config with updated frozen_rows
        let new_config = serde_json::json!({
            "shared_pools": {},
            "quantity_configs": {},
            "faker_rules": {},
            "rules": [],
            "frozen_rows": {
                "Pet": [
                    {"name": "Beta", "status": "sold"}
                ]
            }
        });
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&new_config).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let updated: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(updated["frozen_rows"]["Pet"][0]["name"], "Beta");

        // GET config again to confirm persistence
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/config"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let final_config: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(final_config["frozen_rows"]["Pet"][0]["name"], "Beta");
    }

    #[tokio::test]
    async fn test_activate_recipe_with_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Frozen Activate",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 2,
            "frozen_rows": {
                "Pet": [
                    {"name": "Glacier", "status": "available"},
                    {"name": "Iceberg", "status": "sold"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Activate
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify frozen rows are in the DB via admin tables endpoint
        let req = Request::builder()
            .uri("/_api/admin/tables/Pet")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let table_data: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let rows = table_data["rows"].as_array().unwrap();
        // Should have frozen rows + seeded rows (2 frozen + 2 seeded = 4)
        assert!(
            rows.len() >= 4,
            "expected at least 4 rows (2 frozen + 2 seeded), got {}",
            rows.len()
        );
        // Check that the frozen rows are present (they were inserted first)
        let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
        assert!(
            names.contains(&"Glacier"),
            "should contain frozen row 'Glacier'"
        );
        assert!(
            names.contains(&"Iceberg"),
            "should contain frozen row 'Iceberg'"
        );
    }

    #[tokio::test]
    async fn test_activate_recipe_frozen_rows_skips_invalid_table() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Bad Table",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 1,
            "frozen_rows": {
                "NonExistentTable": [
                    {"col": "val"}
                ],
                "Pet": [
                    {"name": "Survivor", "status": "available"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Activate should succeed (bad table is skipped with warning)
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // The valid frozen row should be in the DB
        let req = Request::builder()
            .uri("/_api/admin/tables/Pet")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let table_data: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let rows = table_data["rows"].as_array().unwrap();
        let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
        assert!(
            names.contains(&"Survivor"),
            "should contain frozen row 'Survivor'"
        );
    }

    #[tokio::test]
    async fn test_activate_recipe_frozen_rows_skips_invalid_column() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Bad Column",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 1,
            "frozen_rows": {
                "Pet": [
                    {"name": "ValidPet", "status": "available", "bogus_column": "should_be_skipped"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Activate should succeed (invalid column is skipped)
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Valid columns should have been inserted into the DB
        let req = Request::builder()
            .uri("/_api/admin/tables/Pet")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let table_data: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let rows = table_data["rows"].as_array().unwrap();
        let names: Vec<&str> = rows.iter().filter_map(|r| r["name"].as_str()).collect();
        assert!(
            names.contains(&"ValidPet"),
            "should contain frozen row 'ValidPet'"
        );
    }

    #[tokio::test]
    async fn test_activate_recipe_default_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Default Frozen",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 2
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Activation with default empty frozen_rows should succeed
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_activate_recipe_malformed_frozen_rows() {
        // Build state manually so we can access recipe_db directly
        let conn = Connection::open_in_memory().unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let recipe_conn = Connection::open_in_memory().unwrap();
        crate::recipe::init_recipe_db(&recipe_conn).unwrap();
        let recipe_db: Db = Arc::new(Mutex::new(recipe_conn));
        let state = AppState {
            db,
            registry,
            log,
            recipe_db: recipe_db.clone(),
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        };
        let router = build_router(state);

        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Malformed Frozen",
            "spec_source": spec_yaml,
            "endpoints": [
                {"method": "get", "path": "/pet/{petId}"},
                {"method": "post", "path": "/pet"}
            ],
            "seed_count": 2
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Write corrupt JSON directly into the frozen_rows column
        {
            let rdb = recipe_db.lock().unwrap();
            rdb.execute(
                "UPDATE \"recipes\" SET \"frozen_rows\" = ?1 WHERE \"id\" = ?2",
                rusqlite::params!["not valid json {{{", id],
            )
            .unwrap();
        }

        // Activation should still succeed — malformed frozen_rows falls back to empty
        let req = Request::builder()
            .method("POST")
            .uri(format!("/_api/admin/recipes/{id}/activate"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_export_recipe_includes_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let body = serde_json::json!({
            "name": "Export Frozen",
            "spec_source": spec_yaml,
            "endpoints": [{"method": "get", "path": "/pet/{petId}"}],
            "seed_count": 1,
            "frozen_rows": {
                "Pet": [
                    {"name": "Snowball", "status": "available"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/export"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let export: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(export["mirage_recipe"], 2);
        let frozen = &export["frozen_rows"];
        assert!(frozen.is_object(), "frozen_rows should be an object");
        let pets = frozen["Pet"].as_array().unwrap();
        assert_eq!(pets.len(), 1);
        assert_eq!(pets[0]["name"], "Snowball");
    }

    #[tokio::test]
    async fn test_import_recipe_v2_with_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let import_body = serde_json::json!({
            "mirage_recipe": 2,
            "name": "Imported V2",
            "spec_source": spec_yaml,
            "selected_endpoints": [{"method": "get", "path": "/pet/{petId}"}],
            "seed_count": 1,
            "shared_pools": {},
            "quantity_configs": {},
            "faker_rules": {},
            "rules": [],
            "frozen_rows": {
                "Pet": [
                    {"name": "Icicle", "status": "pending"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes/import")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&import_body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let frozen: serde_json::Value =
            serde_json::from_str(created["frozen_rows"].as_str().unwrap()).unwrap();
        let pets = frozen["Pet"].as_array().unwrap();
        assert_eq!(pets.len(), 1);
        assert_eq!(pets[0]["name"], "Icicle");
        assert_eq!(pets[0]["status"], "pending");
    }

    #[tokio::test]
    async fn test_import_recipe_v1_no_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
        let import_body = serde_json::json!({
            "mirage_recipe": 1,
            "name": "Imported V1",
            "spec_source": spec_yaml,
            "selected_endpoints": [{"method": "get", "path": "/pet/{petId}"}],
            "seed_count": 1,
            "shared_pools": {},
            "quantity_configs": {},
            "faker_rules": {},
            "rules": []
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes/import")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&import_body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let frozen: serde_json::Value =
            serde_json::from_str(created["frozen_rows"].as_str().unwrap()).unwrap();
        assert!(frozen.is_object(), "frozen_rows should default to object");
        assert_eq!(
            frozen,
            serde_json::json!({}),
            "v1 import should default frozen_rows to empty object"
        );
    }

    #[tokio::test]
    async fn test_export_import_roundtrip_frozen_rows() {
        let router = setup_empty();
        let spec_yaml = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();

        // Create a recipe with frozen_rows
        let body = serde_json::json!({
            "name": "Roundtrip Export",
            "spec_source": spec_yaml,
            "endpoints": [{"method": "get", "path": "/pet/{petId}"}],
            "seed_count": 2,
            "frozen_rows": {
                "Pet": [
                    {"name": "Glacier", "status": "sold"},
                    {"name": "Tundra", "status": "available"}
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let id = created["id"].as_i64().unwrap();

        // Export the recipe
        let req = Request::builder()
            .uri(format!("/_api/admin/recipes/{id}/export"))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let export: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let original_frozen = &export["frozen_rows"];

        // Import the exported recipe (change name to avoid collision concerns)
        let mut import_body = export.clone();
        import_body["name"] = serde_json::json!("Roundtrip Imported");
        let req = Request::builder()
            .method("POST")
            .uri("/_api/admin/recipes/import")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&import_body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let imported: serde_json::Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let imported_frozen: serde_json::Value =
            serde_json::from_str(imported["frozen_rows"].as_str().unwrap()).unwrap();

        assert_eq!(
            *original_frozen, imported_frozen,
            "frozen_rows should survive export/import roundtrip"
        );
        let pets = imported_frozen["Pet"].as_array().unwrap();
        assert_eq!(pets.len(), 2);
        assert_eq!(pets[0]["name"], "Glacier");
        assert_eq!(pets[1]["name"], "Tundra");
    }

    /// Build a router with a single manually-registered route for a given shape.
    fn setup_with_shape(method: &str, pattern: &str, shape: ResponseShape) -> Router {
        let conn = Connection::open_in_memory().unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        {
            let mut reg = registry.write().unwrap();
            reg.routes.push(RouteEntry {
                method: method.to_string(),
                pattern: pattern.to_string(),
                table: String::new(),
                has_path_param: false,
                shape,
            });
        }
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let recipe_conn = Connection::open_in_memory().unwrap();
        crate::recipe::init_recipe_db(&recipe_conn).unwrap();
        let recipe_db: Db = Arc::new(Mutex::new(recipe_conn));
        let state = AppState {
            db,
            registry,
            log,
            recipe_db,
            documents: Arc::new(RwLock::new(std::collections::HashMap::new())),
        };
        build_router(state)
    }

    #[tokio::test]
    async fn test_catch_all_primitive_integer() {
        let router = setup_with_shape(
            "get",
            "/health/count",
            ResponseShape::Primitive("integer".into()),
        );
        let req = Request::builder()
            .uri("/health/count")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_number(), "expected a JSON number, got {json}");
    }

    #[tokio::test]
    async fn test_catch_all_primitive_string() {
        let router = setup_with_shape(
            "get",
            "/health/name",
            ResponseShape::Primitive("string".into()),
        );
        let req = Request::builder()
            .uri("/health/name")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_string(), "expected a JSON string, got {json}");
    }

    #[tokio::test]
    async fn test_catch_all_primitive_array_integer() {
        let router = setup_with_shape(
            "get",
            "/health/counts",
            ResponseShape::PrimitiveArray("integer".into()),
        );
        let req = Request::builder()
            .uri("/health/counts")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().expect("expected a JSON array");
        assert_eq!(arr.len(), 3, "expected 3 elements in array");
        for item in arr {
            assert!(
                item.is_number(),
                "expected a JSON number in array, got {item}"
            );
        }
    }

    #[tokio::test]
    async fn test_catch_all_freeform_object() {
        let router = setup_with_shape("get", "/health/meta", ResponseShape::FreeformObject);
        let req = Request::builder()
            .uri("/health/meta")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_object(), "expected a JSON object, got {json}");
    }

    #[tokio::test]
    async fn test_catch_all_empty() {
        let router = setup_with_shape("get", "/health/ping", ResponseShape::Empty);
        let req = Request::builder()
            .uri("/health/ping")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(body.is_empty(), "expected empty body for 204");
    }
}
