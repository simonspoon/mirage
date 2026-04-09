// Axum API server

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rusqlite::types::Value as SqlValue;
use rust_embed::Embed;
use serde::Serialize;

use crate::parser::SwaggerSpec;

pub type Db = Arc<Mutex<rusqlite::Connection>>;

#[derive(Embed)]
#[folder = "ui/dist/"]
struct AdminAssets;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub spec_info: SpecInfo,
    pub endpoints: Vec<EndpointInfo>,
}

#[derive(Clone, Serialize)]
pub struct SpecInfo {
    pub title: String,
    pub version: String,
}

#[derive(Clone, Serialize)]
pub struct EndpointInfo {
    pub method: String,
    pub path: String,
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

async fn get_collection(table: String, db: Db) -> impl IntoResponse {
    let conn = db.lock().unwrap();
    let sql = format!("SELECT * FROM {table}");
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({})),
            );
        }
    };
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let rows: Vec<serde_json::Value> = stmt
        .query_map([], |row| row_to_json(&col_names, row))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    (StatusCode::OK, Json(serde_json::Value::Array(rows)))
}

async fn get_single(table: String, db: Db, id: i64) -> impl IntoResponse {
    let conn = db.lock().unwrap();
    let sql = format!("SELECT * FROM {table} WHERE rowid = ?");
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({})),
            );
        }
    };
    let col_names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    match stmt.query_row([id], |row| row_to_json(&col_names, row)) {
        Ok(val) => (StatusCode::OK, Json(val)),
        Err(rusqlite::Error::QueryReturnedNoRows) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({})),
        ),
    }
}

async fn post_create(table: String, db: Db, body: serde_json::Value) -> impl IntoResponse {
    let conn = db.lock().unwrap();

    // Get column names from the table
    let col_names: Vec<String> = {
        let sql = format!("PRAGMA table_info({table})");
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
            );
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
        );
    }

    let cols_str = insert_cols.join(", ");
    let placeholders: Vec<String> = (1..=insert_cols.len()).map(|i| format!("?{i}")).collect();
    let placeholders_str = placeholders.join(", ");
    let sql = format!("INSERT INTO {table} ({cols_str}) VALUES ({placeholders_str})");

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        insert_vals.iter().map(|p| p.as_ref()).collect();
    if conn.execute(&sql, param_refs.as_slice()).is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "insert failed"})),
        );
    }

    let new_id = conn.last_insert_rowid();
    let mut result = obj.clone();
    result.insert("id".to_string(), serde_json::json!(new_id));

    (StatusCode::CREATED, Json(serde_json::Value::Object(result)))
}

async fn delete_single(table: String, db: Db, id: i64) -> impl IntoResponse {
    let conn = db.lock().unwrap();
    let sql = format!("DELETE FROM {table} WHERE rowid = ?");
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

fn swagger_to_axum_path(path: &str) -> String {
    // Axum 0.8 uses {param} syntax, same as Swagger
    path.to_string()
}

async fn admin_spec(State(state): State<AppState>) -> Json<SpecInfo> {
    Json(state.spec_info)
}

async fn admin_endpoints(State(state): State<AppState>) -> Json<Vec<EndpointInfo>> {
    Json(state.endpoints)
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

pub fn build_router(spec: &SwaggerSpec, state: AppState) -> Router {
    let db = state.db.clone();
    let mut router = Router::new();
    let mut registered: HashSet<String> = HashSet::new();

    for (path, method, _op) in spec.path_operations() {
        let table = table_name_from_path(path);
        let axum_path = swagger_to_axum_path(path);
        let has_path_param = path.contains('{');

        let key = format!("{method}:{axum_path}");
        if registered.contains(&key) {
            continue;
        }
        registered.insert(key);

        match (method, has_path_param) {
            ("get", true) => {
                let t = table.clone();
                let d = db.clone();
                router = router.route(
                    &axum_path,
                    get(move |Path(id): Path<i64>| get_single(t, d, id)),
                );
            }
            ("get", false) => {
                let t = table.clone();
                let d = db.clone();
                router = router.route(&axum_path, get(move || get_collection(t, d)));
            }
            ("post", _) => {
                let t = table.clone();
                let d = db.clone();
                router = router.route(
                    &axum_path,
                    post(move |Json(body): Json<serde_json::Value>| post_create(t, d, body)),
                );
            }
            ("delete", true) => {
                let t = table.clone();
                let d = db.clone();
                router = router.route(
                    &axum_path,
                    delete(move |Path(id): Path<i64>| delete_single(t, d, id)),
                );
            }
            _ => {}
        }

        // Auto-register collection GET for any table that has routes
        if !has_path_param {
            let coll_key = format!("get:{axum_path}");
            if !registered.contains(&coll_key) {
                let t = table.clone();
                let d = db.clone();
                router = router.route(&axum_path, get(move || get_collection(t, d)));
                registered.insert(coll_key);
            }
        } else {
            // For paths like /pet/:petId, also register /pet as collection
            let base = format!(
                "/{}",
                path.trim_start_matches('/').split('/').next().unwrap_or("")
            );
            let coll_key = format!("get:{base}");
            if !registered.contains(&coll_key) {
                let t = table.clone();
                let d = db.clone();
                router = router.route(&base, get(move || get_collection(t, d)));
                registered.insert(coll_key);
            }
        }
    }

    let admin_api = Router::new()
        .route("/spec", get(admin_spec))
        .route("/endpoints", get(admin_endpoints))
        .with_state(state);

    router
        .nest("/_api/admin", admin_api)
        .route(
            "/_admin",
            get(|| async { axum::response::Redirect::permanent("/_admin/") }),
        )
        .route("/_admin/", get(serve_admin))
        .route("/_admin/{*path}", get(serve_admin))
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
        let spec_info = SpecInfo {
            title: spec.info.title.clone(),
            version: spec.info.version.clone(),
        };
        let endpoints: Vec<EndpointInfo> = spec
            .path_operations()
            .iter()
            .map(|(path, method, _)| EndpointInfo {
                method: method.to_string(),
                path: path.to_string(),
            })
            .collect();
        let state = AppState {
            db,
            spec_info,
            endpoints,
        };
        build_router(&spec, state)
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
}
