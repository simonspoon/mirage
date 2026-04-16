use std::net::TcpListener;
use std::process::{Child, Command};

struct MirageServer {
    child: Child,
    port: u16,
}

impl MirageServer {
    fn start(spec_path: &str, probe_path: &str) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let child = Command::new(env!("CARGO_BIN_EXE_mirage"))
            .args([spec_path, "--port", &port.to_string()])
            .spawn()
            .expect("failed to start mirage");

        let server = Self { child, port };
        let client = reqwest::blocking::Client::new();
        let base = format!("http://127.0.0.1:{}", port);
        for _ in 0..50 {
            if client.get(format!("{}{}", base, probe_path)).send().is_ok() {
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        drop(server);
        panic!("mirage server did not start within 5 seconds");
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}

impl Drop for MirageServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn test_e2e_get_collection() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    let resp = client.get(server.url("/pet")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(body.is_array());
    assert!(!body.as_array().unwrap().is_empty());
}

#[test]
fn test_e2e_get_single() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    // Use rowid 1 directly — the server resolves single items by SQLite rowid,
    // not by the seeded "id" column value.
    let resp = client.get(server.url("/pet/1")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let pet: serde_json::Value = resp.json().unwrap();
    assert!(pet.get("name").is_some());
}

#[test]
fn test_e2e_get_not_found() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    let resp = client.get(server.url("/pet/999999")).send().unwrap();
    assert_eq!(resp.status(), 404);
}

#[test]
fn test_e2e_post_create() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    let new_pet = serde_json::json!({
        "name": "TestDog",
        "status": "available"
    });
    let resp = client
        .post(server.url("/pet"))
        .json(&new_pet)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().unwrap();
    assert!(created.get("id").is_some());
}

#[test]
fn test_e2e_delete() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    // Use rowid 1 directly — same reason as test_e2e_get_single.
    let resp = client.delete(server.url("/pet/1")).send().unwrap();
    assert_eq!(resp.status(), 204);

    // Verify it's gone
    let resp = client.get(server.url("/pet/1")).send().unwrap();
    assert_eq!(resp.status(), 404);
}

#[test]
fn test_e2e_admin_page() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    let resp = client.get(server.url("/_admin/")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"));
}

#[test]
fn test_e2e_admin_api_endpoints() {
    let server = MirageServer::start("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(server.url("/_api/admin/endpoints"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(body.is_array());
    assert!(!body.as_array().unwrap().is_empty());
}

#[test]
fn test_primitives_fixture_all_types() {
    let server = MirageServer::start("tests/fixtures/mega.yaml", "/primitives");
    let client = reqwest::blocking::Client::new();

    // Build a recipe that reuses the same mega.yaml spec and attaches a Const
    // rule on Primitives.const_field, proving end-to-end rule propagation.
    let spec_source = std::fs::read_to_string("tests/fixtures/mega.yaml").unwrap();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let recipe_name = format!("mega-primitives-{nanos}");
    let body = serde_json::json!({
        "name": recipe_name,
        "spec_source": spec_source,
        "endpoints": [{"method": "get", "path": "/primitives"}],
        "seed_count": 5,
        "rules": [
            {"kind": "const", "field": "Primitives.const_field", "value": "FIXED_VALUE"}
        ]
    });
    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201, "create recipe should return 201");
    let created: serde_json::Value = resp.json().unwrap();
    let id = created["id"]
        .as_i64()
        .expect("recipe response should contain numeric id");

    let resp = client
        .post(server.url(&format!("/_api/admin/recipes/{id}/activate")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200, "activate recipe should return 200");

    let resp = client.get(server.url("/primitives")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let arr = body
        .as_array()
        .expect("/primitives response should be a JSON array");
    assert!(!arr.is_empty(), "array should have at least one row");

    let date_re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    let email_re = regex::Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").unwrap();
    let allowed_enum = ["alpha", "beta", "gamma"];

    for (idx, row) in arr.iter().enumerate() {
        let ctx = || format!("row {idx}: {row}");

        assert!(
            row["str_plain"].is_string(),
            "str_plain must be string — {}",
            ctx()
        );

        let i32v = row["int32_field"]
            .as_i64()
            .unwrap_or_else(|| panic!("int32_field must be integer — {}", ctx()));
        assert!(
            (i32::MIN as i64..=i32::MAX as i64).contains(&i32v),
            "int32_field {i32v} out of i32 range — {}",
            ctx()
        );
        assert!(
            row["int64_field"].is_i64(),
            "int64_field must be integer — {}",
            ctx()
        );

        let ff = row["float_field"]
            .as_f64()
            .unwrap_or_else(|| panic!("float_field must be number — {}", ctx()));
        assert!(
            ff.is_finite(),
            "float_field {ff} must be finite — {}",
            ctx()
        );
        let df = row["double_field"]
            .as_f64()
            .unwrap_or_else(|| panic!("double_field must be number — {}", ctx()));
        assert!(
            df.is_finite(),
            "double_field {df} must be finite — {}",
            ctx()
        );

        assert!(
            row["bool_field"].is_boolean(),
            "bool_field must be JSON boolean — {}",
            ctx()
        );
        assert!(
            !row["bool_field"].is_number(),
            "bool_field must not be JSON number — {}",
            ctx()
        );

        let date_str = row["date_field"]
            .as_str()
            .unwrap_or_else(|| panic!("date_field must be string — {}", ctx()));
        assert!(
            date_re.is_match(date_str),
            "date_field '{date_str}' must match YYYY-MM-DD — {}",
            ctx()
        );
        chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .unwrap_or_else(|e| panic!("date_field '{date_str}' parse failed: {e} — {}", ctx()));

        let dt_str = row["datetime_field"]
            .as_str()
            .unwrap_or_else(|| panic!("datetime_field must be string — {}", ctx()));
        chrono::DateTime::parse_from_rfc3339(dt_str)
            .unwrap_or_else(|e| panic!("datetime_field '{dt_str}' not RFC3339: {e} — {}", ctx()));

        let uuid_str = row["uuid_field"]
            .as_str()
            .unwrap_or_else(|| panic!("uuid_field must be string — {}", ctx()));
        uuid::Uuid::parse_str(uuid_str)
            .unwrap_or_else(|e| panic!("uuid_field '{uuid_str}' parse failed: {e} — {}", ctx()));

        let email_str = row["email_field"]
            .as_str()
            .unwrap_or_else(|| panic!("email_field must be string — {}", ctx()));
        assert!(
            email_str.contains('@'),
            "email_field '{email_str}' must contain '@' — {}",
            ctx()
        );
        assert!(
            email_re.is_match(email_str),
            "email_field '{email_str}' must match basic email regex — {}",
            ctx()
        );

        let enum_str = row["enum_field"]
            .as_str()
            .unwrap_or_else(|| panic!("enum_field must be string — {}", ctx()));
        assert!(
            allowed_enum.contains(&enum_str),
            "enum_field '{enum_str}' must be one of {allowed_enum:?} — {}",
            ctx()
        );

        let const_str = row["const_field"]
            .as_str()
            .unwrap_or_else(|| panic!("const_field must be string — {}", ctx()));
        assert_eq!(
            const_str,
            "FIXED_VALUE",
            "const_field should be FIXED_VALUE per recipe rule — {}",
            ctx()
        );
    }
}

/// Unwrap possibly TEXT-backed JSON: row_to_json only reparses top-level
/// TEXT, so nested JSON objects remain as strings inside the outer object.
fn unwrap_json(v: &serde_json::Value) -> serde_json::Value {
    if let Some(s) = v.as_str() {
        serde_json::from_str(s)
            .unwrap_or_else(|e| panic!("expected nested JSON string, parse failed: {e} — raw={s}"))
    } else if v.is_object() {
        v.clone()
    } else {
        panic!("expected string or object, got: {v}");
    }
}

#[test]
fn test_composition_merged_fields() {
    let server = MirageServer::start("tests/fixtures/mega.yaml", "/composed");
    let client = reqwest::blocking::Client::new();

    let resp = client.get(server.url("/composed")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let arr = body
        .as_array()
        .expect("/composed response should be a JSON array");
    assert!(!arr.is_empty(), "array should have at least one row");
    let first = &arr[0];

    let resp = client.get(server.url("/composed/1")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let single: serde_json::Value = resp.json().unwrap();

    for (label, row) in [("collection[0]", first), ("single", &single)] {
        let ctx = || format!("{label}: {row}");

        // Set A — base+extension merge proof
        assert!(
            row.as_object()
                .map(|o| o.contains_key("created_at"))
                .unwrap_or(false),
            "row must contain created_at — {}",
            ctx()
        );
        let created_at = &row["created_at"];
        assert!(
            created_at.is_string(),
            "created_at must be string — {}",
            ctx()
        );
        assert!(
            !created_at.as_str().unwrap().is_empty(),
            "created_at must be non-empty — {}",
            ctx()
        );
        assert!(
            row["updated_by"].is_string(),
            "updated_by must be string — {}",
            ctx()
        );
        assert!(row["id"].is_i64(), "id must be integer — {}", ctx());
        assert!(row["title"].is_string(), "title must be string — {}", ctx());
        assert!(
            row["priority"].is_i64(),
            "priority must be integer — {}",
            ctx()
        );
        assert!(
            row.as_object()
                .map(|o| o.contains_key("owner"))
                .unwrap_or(false),
            "row must contain owner — {}",
            ctx()
        );

        // Set B — ≥2-hop $ref chain resolved
        let owner_raw = &row["owner"];
        assert!(
            owner_raw.is_string() || owner_raw.is_object(),
            "owner must be string or object — {}",
            ctx()
        );
        let owner_val = unwrap_json(owner_raw);
        assert!(
            owner_val["name"].is_string(),
            "owner.name must be string — {}",
            ctx()
        );
        assert!(
            owner_val
                .as_object()
                .map(|o| o.contains_key("address"))
                .unwrap_or(false),
            "owner must contain address — {}",
            ctx()
        );
        let address_raw = &owner_val["address"];
        assert!(
            address_raw.is_string() || address_raw.is_object(),
            "address must be string or object — {}",
            ctx()
        );
        let address_val = unwrap_json(address_raw);
        assert!(
            address_val["city"].is_string(),
            "address.city must be string — {}",
            ctx()
        );
        assert!(
            address_val["country"].is_string(),
            "address.country must be string — {}",
            ctx()
        );
    }
}

#[test]
fn test_shared_type_pool() {
    // Owner appears via two paths — directly at /owners (array) and indirectly
    // as ComposedEntity.owner (embedded $ref) on /composed. After recipe
    // activation, the composed collection URL is /{table.to_lowercase()}, i.e.
    // /composedentity per src/server.rs:1768.
    //
    // This test guards endpoint reachability + Owner shape round-trip through
    // DB for both paths. Cross-endpoint pool-consumption identity is NOT
    // asserted — compose_documents doesn't consume the pool today (tracked
    // separately as task ldum).
    let server = MirageServer::start("tests/fixtures/mega.yaml", "/owners");
    let client = reqwest::blocking::Client::new();

    let spec_source = std::fs::read_to_string("tests/fixtures/mega.yaml").unwrap();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let recipe_name = format!("mega-shared-owner-{nanos}");
    let body = serde_json::json!({
        "name": recipe_name,
        "spec_source": spec_source,
        "endpoints": [
            {"method": "get", "path": "/owners"},
            {"method": "get", "path": "/composed/{id}"}
        ],
        "seed_count": 5,
        "shared_pools": {
            "Owner": {"is_shared": true, "pool_size": 3}
        }
    });
    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201, "create recipe should return 201");
    let created: serde_json::Value = resp.json().unwrap();
    let id = created["id"]
        .as_i64()
        .expect("recipe response should contain numeric id");

    let resp = client
        .post(server.url(&format!("/_api/admin/recipes/{id}/activate")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200, "activate recipe should return 200");

    // /owners — direct Owner array endpoint.
    let resp = client.get(server.url("/owners")).send().unwrap();
    assert_eq!(resp.status(), 200, "/owners should return 200");
    let body: serde_json::Value = resp.json().unwrap();
    let owners = body
        .as_array()
        .expect("/owners response should be a JSON array");
    assert!(!owners.is_empty(), "/owners array should be non-empty");

    for (idx, row) in owners.iter().enumerate() {
        let ctx = || format!("/owners row {idx}: {row}");
        assert!(
            row["name"].is_string(),
            "owner.name must be string — {}",
            ctx()
        );
        let address_raw = &row["address"];
        assert!(
            address_raw.is_string() || address_raw.is_object(),
            "owner.address must be string or object — {}",
            ctx()
        );
        let address = unwrap_json(address_raw);
        assert!(
            address["city"].is_string(),
            "owner.address.city must be string — {}",
            ctx()
        );
        assert!(
            address["country"].is_string(),
            "owner.address.country must be string — {}",
            ctx()
        );
    }

    // /composedentity — ComposedEntity collection URL post-activation is
    // /{table.to_lowercase()} (src/server.rs:1768), NOT /composed.
    let resp = client.get(server.url("/composedentity")).send().unwrap();
    assert_eq!(resp.status(), 200, "/composedentity should return 200");
    let body: serde_json::Value = resp.json().unwrap();
    let composed = body
        .as_array()
        .expect("/composedentity response should be a JSON array");
    assert!(
        !composed.is_empty(),
        "/composedentity array should be non-empty"
    );

    for (idx, row) in composed.iter().enumerate() {
        let ctx = || format!("/composedentity row {idx}: {row}");
        let owner_raw = &row["owner"];
        assert!(
            owner_raw.is_string() || owner_raw.is_object(),
            "composed.owner must be string or object — {}",
            ctx()
        );
        let owner = unwrap_json(owner_raw);
        assert!(
            owner["name"].is_string(),
            "composed.owner.name must be string — {}",
            ctx()
        );
        let address_raw = &owner["address"];
        assert!(
            address_raw.is_string() || address_raw.is_object(),
            "composed.owner.address must be string or object — {}",
            ctx()
        );
        let address = unwrap_json(address_raw);
        assert!(
            address["city"].is_string(),
            "composed.owner.address.city must be string — {}",
            ctx()
        );
        assert!(
            address["country"].is_string(),
            "composed.owner.address.country must be string — {}",
            ctx()
        );
    }
}

#[test]
fn test_endpoint_method_coverage() {
    // Exercises every supported HTTP method + parameter style on a flat-primitive
    // Widget resource declared in mega.yaml. Single fn per acceptance criteria.
    let server = MirageServer::start("tests/fixtures/mega.yaml", "/widgets");
    let client = reqwest::blocking::Client::new();

    // (a) GET collection — auto-seeded baseline.
    let resp = client.get(server.url("/widgets")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let arr = body
        .as_array()
        .expect("/widgets response should be a JSON array");
    assert!(!arr.is_empty(), "/widgets array should have >=1 seeded row");
    assert!(
        arr[0]["name"].is_string(),
        "widget[0].name must be string — row: {}",
        arr[0]
    );
    assert!(
        arr[0]["id"].is_i64(),
        "widget[0].id must be integer — row: {}",
        arr[0]
    );
    let baseline_len = arr.len();

    // (b) POST create — id echoed from last_insert_rowid (src/server.rs:355).
    let payload = serde_json::json!({
        "name": "coverage-widget",
        "price": 9.99,
        "status": "active"
    });
    let resp = client
        .post(server.url("/widgets"))
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().unwrap();
    let new_id = created["id"]
        .as_i64()
        .expect("POST response must include integer id");
    assert_eq!(
        created["name"].as_str(),
        Some("coverage-widget"),
        "POST response name mismatch — body: {created}"
    );

    // (c) GET single — lookup by rowid returned from POST. Note: stored `id`
    //     column is NULL because POST payload omitted id (echoed id in POST
    //     response is last_insert_rowid only, not persisted into the column).
    //     Thus we assert name round-trip + reachability only, not id equality.
    //     Deviation logged on task mlsz.
    let resp = client
        .get(server.url(&format!("/widgets/{new_id}")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let fetched: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        fetched["name"].as_str(),
        Some("coverage-widget"),
        "GET single name mismatch — body: {fetched}"
    );

    // (d) DELETE — 204 + empty body.
    let resp = client
        .delete(server.url(&format!("/widgets/{new_id}")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 204);
    let body_text = resp.text().unwrap();
    assert!(
        body_text.is_empty(),
        "DELETE body must be empty, got: {body_text}"
    );

    // (e) GET single non-existent — 404.
    let resp = client.get(server.url("/widgets/999999")).send().unwrap();
    assert_eq!(resp.status(), 404);

    // (f) PUT — pins current 405 fall-through at src/server.rs:505. Uses
    //     auto-seeded rowid 1 (not new_id, which was deleted in step d).
    //     Literal 405 assertion so any silent 200/500 future regression fails
    //     this pin.
    let update = serde_json::json!({
        "name": "updated-widget",
        "price": 19.99,
        "status": "inactive"
    });
    let resp = client
        .put(server.url("/widgets/1"))
        .json(&update)
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        405,
        "PUT pin: expect 405 from dispatch fall-through at src/server.rs:505"
    );

    // (g) Query-param — pins accept-but-ignore behavior. Filter must NOT reduce
    //     result length vs. baseline_len (is_array alone would be redundant with
    //     step a).
    let resp = client
        .get(server.url("/widgets?filter=anything"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let filtered = body
        .as_array()
        .expect("/widgets?filter= response should be a JSON array");
    assert_eq!(
        filtered.len(),
        baseline_len,
        "query-param pin: filtered length ({}) must equal unfiltered baseline ({}) — filter currently accept-but-ignore",
        filtered.len(),
        baseline_len
    );
}
