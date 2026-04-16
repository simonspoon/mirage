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
