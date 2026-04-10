// Axum API server

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use rusqlite::types::Value as SqlValue;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};

use crate::composer::DocumentStore;
use crate::parser::SwaggerSpec;
use crate::schema;
use crate::seeder;

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
}

pub type Registry = Arc<RwLock<RouteRegistry>>;

const MAX_LOG_ENTRIES: usize = 500;

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub status: u16,
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

async fn catch_all_handler(
    method: axum::http::Method,
    uri: axum::http::Uri,
    State(state): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let path = uri.path();
    let m = method_str(&method);

    let (table, has_path_param, param_value) = {
        let registry = state.registry.read().unwrap();
        let mut found = None;
        for route in &registry.routes {
            if route.method != m {
                continue;
            }
            if let Some(param_value) = match_route(&route.pattern, path) {
                found = Some((route.table.clone(), route.has_path_param, param_value));
                break;
            }
        }
        match found {
            Some(f) => f,
            None => return StatusCode::NOT_FOUND.into_response(),
        }
    };

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

fn log_request(log: &RequestLog, method: &str, path: &str, status: u16) {
    let entry = LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        method: method.to_uppercase(),
        path: path.to_string(),
        status,
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
            serde_json::json!({
                "method": r.method,
                "path": r.pattern,
                "definition": r.table,
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
            let table = raw_op_map
                .get(&(*path, *method))
                .and_then(|raw_op| crate::parser::primary_response_def(raw_op))
                .unwrap_or_else(|| table_name_from_path(path));
            RouteEntry {
                method: method.to_string(),
                pattern: path.to_string(),
                table,
                has_path_param: path.contains('{'),
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
        if let Err(e) = seeder::seed_tables_filtered(&conn, &spec, seed_count, Some(&needed_defs)) {
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
}

#[derive(Deserialize)]
struct UpdateRecipeRequest {
    name: String,
    spec_source: String,
    endpoints: Vec<EndpointInfo>,
    seed_count: Option<i64>,
    shared_pools: Option<serde_json::Value>,
    quantity_configs: Option<serde_json::Value>,
}

async fn admin_create_recipe(
    State(state): State<AppState>,
    Json(body): Json<CreateRecipeRequest>,
) -> Response {
    // Validate the spec_source is valid swagger
    let _spec: SwaggerSpec = match serde_yaml::from_str(&body.spec_source) {
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
    let _spec: SwaggerSpec = match serde_yaml::from_str(&body.spec_source) {
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
            let table = raw_op_map
                .get(&(*path, *method))
                .and_then(|raw_op| crate::parser::primary_response_def(raw_op))
                .unwrap_or_else(|| table_name_from_path(path));
            RouteEntry {
                method: method.to_string(),
                pattern: path.to_string(),
                table,
                has_path_param: path.contains('{'),
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
        if let Err(e) = seeder::seed_tables_filtered(&conn, &spec, seed_count, Some(&needed_defs)) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("Failed to seed data: {e}")})),
            )
                .into_response();
        }
    }

    // Generate document store using composer
    let entity_graph = crate::entity_graph::build_entity_graph(&raw_spec, &selected_ops);
    let pools = crate::composer::generate_pools(&spec, &pool_config);

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
            Json(serde_json::json!({
                "shared_pools": shared_pools,
                "quantity_configs": quantity_configs,
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
    let conn = state.recipe_db.lock().unwrap();
    match crate::recipe::update_recipe_config(&conn, id, &shared_pools_str, &quantity_configs_str) {
        Ok(true) => Json(serde_json::json!({
            "shared_pools": body.shared_pools,
            "quantity_configs": body.quantity_configs,
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
        let table = raw_op_map
            .get(&(path, method))
            .and_then(|raw_op| crate::parser::primary_response_def(raw_op))
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

    let response = next.run(req).await;

    if should_log {
        log_request(&state.log, &method, &path, response.status().as_u16());
    }

    response
}

pub fn build_router(state: AppState) -> Router {
    let admin_api = Router::new()
        .route("/spec", get(admin_spec))
        .route("/endpoints", get(admin_endpoints))
        .route("/definitions", get(admin_definitions))
        .route("/routes", get(admin_routes))
        .route("/tables", get(admin_tables))
        .route("/tables/{name}", get(admin_table_data))
        .route("/log", get(admin_log))
        .route("/import", post(admin_import))
        .route("/configure", post(admin_configure))
        .route("/graph", post(admin_graph))
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
}
