// Axum API server

use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use rusqlite::types::Value as SqlValue;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};

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
}

impl RouteRegistry {
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            spec_info: None,
            endpoints: Vec::new(),
            spec: None,
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

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub registry: Registry,
}

#[derive(Clone, Serialize)]
pub struct SpecInfo {
    pub title: String,
    pub version: String,
}

#[derive(Clone, Serialize, Deserialize)]
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

    let cols_str = insert_cols.join(", ");
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

    let db = state.db.clone();

    match (m, has_path_param) {
        ("get", true) => get_single(table, db, param_value.unwrap()).await,
        ("get", false) => get_collection(table, db).await,
        ("post", _) => post_create(table, db, body).await,
        ("delete", true) => delete_single(table, db, param_value.unwrap()).await,
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
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

    let spec = {
        let reg = state.registry.read().unwrap();
        match &reg.spec {
            Some(s) => s.clone(),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "No spec imported"})),
                )
                    .into_response();
            }
        }
    };

    // Drop old tables, create new ones, seed
    {
        let conn = state.db.lock().unwrap();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for table in &tables {
            conn.execute(&format!("DROP TABLE IF EXISTS \"{table}\""), [])
                .unwrap();
        }
        schema::create_tables(&conn, &spec).unwrap();
        seeder::seed_tables(&conn, &spec, seed_count).unwrap();
    }

    // Build route entries from selected endpoints
    let selected: HashSet<(String, String)> = config
        .endpoints
        .iter()
        .map(|e| (e.method.to_lowercase(), e.path.clone()))
        .collect();

    let routes: Vec<RouteEntry> = spec
        .path_operations()
        .iter()
        .filter(|(path, method, _)| selected.contains(&(method.to_string(), path.to_string())))
        .map(|(path, method, _)| {
            let table = table_name_from_path(path);
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

pub fn populate_registry(reg: &mut RouteRegistry, spec: &SwaggerSpec) {
    let spec_info = SpecInfo {
        title: spec.info.title.clone(),
        version: spec.info.version.clone(),
    };

    let mut routes: Vec<RouteEntry> = Vec::new();
    let mut registered: HashSet<String> = HashSet::new();

    for (path, method, _op) in spec.path_operations() {
        let table = table_name_from_path(path);
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
    reg.spec = Some(spec.clone());
}

pub fn build_router(state: AppState) -> Router {
    let admin_api = Router::new()
        .route("/spec", get(admin_spec))
        .route("/endpoints", get(admin_endpoints))
        .route("/import", post(admin_import))
        .route("/configure", post(admin_configure))
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
        spec.resolve_refs();
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn, &spec).unwrap();
        seed_tables(&conn, &spec, 5).unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        populate_registry(&mut registry.write().unwrap(), &spec);
        let state = AppState { db, registry };
        build_router(state)
    }

    fn setup_empty() -> Router {
        let conn = Connection::open_in_memory().unwrap();
        let db: Db = Arc::new(Mutex::new(conn));
        let registry = Arc::new(RwLock::new(RouteRegistry::new()));
        let state = AppState { db, registry };
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
        let state = AppState {
            db,
            registry: registry.clone(),
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
}
