use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};

struct MirageServer {
    child: Child,
    port: u16,
    workdir: Option<PathBuf>,
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

        let server = Self {
            child,
            port,
            workdir: None,
        };
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

    /// Start mirage in an isolated working directory so its `mirage.db`
    /// recipe store cannot collide with parallel tests. `spec_path` is
    /// resolved relative to CARGO_MANIFEST_DIR (so it stays valid after
    /// chdir into the tempdir).
    fn start_isolated(spec_rel_path: &str, probe_path: &str) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let spec_abs = manifest_dir.join(spec_rel_path);
        let workdir = std::env::temp_dir().join(format!("mirage-test-{port}"));
        std::fs::create_dir_all(&workdir).expect("create workdir");

        let child = Command::new(env!("CARGO_BIN_EXE_mirage"))
            .current_dir(&workdir)
            .args([spec_abs.to_str().unwrap(), "--port", &port.to_string()])
            .spawn()
            .expect("failed to start mirage");

        let server = Self {
            child,
            port,
            workdir: Some(workdir),
        };
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

    /// Stop the server but preserve its workdir (and therefore `mirage.db`)
    /// so a subsequent `start_in_existing_dir` call exercises restart-survival.
    /// Consumes `self`; the returned workdir is owned by the caller and will
    /// NOT be auto-cleaned by Drop.
    #[allow(dead_code)]
    fn stop_preserve_dir(mut self) -> PathBuf {
        let dir = self.workdir.take().expect("workdir required to preserve");
        let _ = self.child.kill();
        let _ = self.child.wait();
        dir
    }

    /// Relaunch mirage against an existing workdir (carrying forward its
    /// `mirage.db`). Picks a fresh port so tests do not race on the old one.
    #[allow(dead_code)]
    fn start_in_existing_dir(workdir: PathBuf, spec_rel_path: &str, probe_path: &str) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let spec_abs = manifest_dir.join(spec_rel_path);

        let child = Command::new(env!("CARGO_BIN_EXE_mirage"))
            .current_dir(&workdir)
            .args([spec_abs.to_str().unwrap(), "--port", &port.to_string()])
            .spawn()
            .expect("failed to restart mirage");

        let server = Self {
            child,
            port,
            workdir: Some(workdir),
        };
        let client = reqwest::blocking::Client::new();
        let base = format!("http://127.0.0.1:{}", port);
        for _ in 0..50 {
            if client.get(format!("{}{}", base, probe_path)).send().is_ok() {
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        drop(server);
        panic!("mirage restarted server did not come up within 5 seconds");
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for MirageServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(dir) = &self.workdir {
            let _ = std::fs::remove_dir_all(dir);
        }
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

// `test_shared_type_pool` deleted — the shared_pools user surface is gone.
// Cross-endpoint nested-$ref reuse is now driven by the SQLite backing table
// (task yhgg); coverage for that behavior lives in that task's tests.

/// Implicit pool identity (one-hop): every nested $ref value in an endpoint
/// response must resolve to an actual row in the target def's backing SQLite
/// table. Asserts the invariant parent task baqf requires — composer samples
/// nested $refs from the backing table (sibling kxlm) after topological
/// seeding (sibling vjeu), so embedded rows are joinable back to their source
/// table.
///
/// One-hop: /owners → owner.address must exist in the Address table. Address
/// has no endpoint of its own so its seeded rows are never overwritten by a
/// later compose pass, which is exactly why this hop holds.
///
/// The two-hop chain (/composed → composed.owner ⊆ Owner table) lives in
/// `test_implicit_pool_two_hop_composed_owner` below, currently `#[ignore]`
/// pending the streaming-insert follow-up (limbo task thoh).
#[test]
fn test_implicit_pool_nested_ref() {
    use std::collections::HashSet;

    let server = MirageServer::start("tests/fixtures/mega.yaml", "/owners");
    let client = reqwest::blocking::Client::new();

    let resp = client.get(server.url("/owners")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let owners_body: serde_json::Value = resp.json().unwrap();
    let owners = owners_body
        .as_array()
        .expect("/owners response should be a JSON array");
    assert!(
        !owners.is_empty(),
        "/owners array should have >=1 seeded row"
    );

    let owner_addr_tuples: HashSet<(String, String)> = owners
        .iter()
        .map(|row| {
            let addr = unwrap_json(&row["address"]);
            let city = addr["city"]
                .as_str()
                .unwrap_or_else(|| panic!("owner.address.city missing — row: {row}"))
                .to_string();
            let country = addr["country"]
                .as_str()
                .unwrap_or_else(|| panic!("owner.address.country missing — row: {row}"))
                .to_string();
            (city, country)
        })
        .collect();
    assert!(
        !owner_addr_tuples.is_empty(),
        "expected at least one owner.address tuple"
    );

    let resp = client
        .get(server.url("/_api/admin/tables/Address"))
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Address admin table endpoint should exist"
    );
    let addr_body: serde_json::Value = resp.json().unwrap();
    let addr_rows = addr_body["rows"]
        .as_array()
        .expect("/_api/admin/tables/Address should return {rows: [...]}");
    assert!(
        !addr_rows.is_empty(),
        "Address table should have seeded rows"
    );

    let addr_table_tuples: HashSet<(String, String)> = addr_rows
        .iter()
        .map(|row| {
            let city = row["city"]
                .as_str()
                .unwrap_or_else(|| panic!("Address.city missing — row: {row}"))
                .to_string();
            let country = row["country"]
                .as_str()
                .unwrap_or_else(|| panic!("Address.country missing — row: {row}"))
                .to_string();
            (city, country)
        })
        .collect();

    let missing: Vec<_> = owner_addr_tuples
        .difference(&addr_table_tuples)
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "every owner.address (city,country) must exist in Address table \
         — {} missing tuples: {:?}; owners={:?}; table={:?}",
        missing.len(),
        missing,
        owner_addr_tuples,
        addr_table_tuples
    );
}

/// Implicit pool identity (two-hop chain): every nested $ref value two levels
/// deep must still resolve to a row in the target def's backing table. For
/// mega.yaml this is /composed → composed.owner ⊆ Owner table (and by
/// extension, composed.owner.address ⊆ Address table, already covered by the
/// one-hop test).
///
/// Guaranteed by streaming per-def compose+insert in the activation path:
/// `composer::compose_documents` fires an `on_def_composed` callback after
/// each def composes, and both call sites (`src/main.rs` default-activation
/// and `src/server.rs::admin_activate_recipe`) pass a closure that calls
/// `seeder::insert_composed_rows` for that single def. Result: when
/// `ComposedEntity` composes, the `Owner` table already holds the composed
/// Owner rows, so nested `$ref` sampling draws from them (task thoh under
/// parent baqf).
#[test]
fn test_implicit_pool_two_hop_composed_owner() {
    use std::collections::HashSet;

    let server = MirageServer::start("tests/fixtures/mega.yaml", "/composed");
    let client = reqwest::blocking::Client::new();

    let resp = client.get(server.url("/composed")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let composed_body: serde_json::Value = resp.json().unwrap();
    let composed = composed_body
        .as_array()
        .expect("/composed response should be a JSON array");
    assert!(
        !composed.is_empty(),
        "/composed array should have >=1 seeded row"
    );

    // Owner has no id in mega.yaml — natural key is (name, address.city,
    // address.country). Use that tuple to join composed.owner to Owner rows.
    let composed_owner_keys: HashSet<(String, String, String)> = composed
        .iter()
        .map(|row| {
            let owner = unwrap_json(&row["owner"]);
            let name = owner["name"]
                .as_str()
                .unwrap_or_else(|| panic!("composed.owner.name missing — row: {row}"))
                .to_string();
            let addr = unwrap_json(&owner["address"]);
            let city = addr["city"]
                .as_str()
                .unwrap_or_else(|| panic!("composed.owner.address.city missing — row: {row}"))
                .to_string();
            let country = addr["country"]
                .as_str()
                .unwrap_or_else(|| panic!("composed.owner.address.country missing — row: {row}"))
                .to_string();
            (name, city, country)
        })
        .collect();
    assert!(
        !composed_owner_keys.is_empty(),
        "expected at least one composed.owner key"
    );

    let resp = client
        .get(server.url("/_api/admin/tables/Owner"))
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "Owner admin table endpoint should exist"
    );
    let owner_body: serde_json::Value = resp.json().unwrap();
    let owner_rows = owner_body["rows"]
        .as_array()
        .expect("/_api/admin/tables/Owner should return {rows: [...]}");
    assert!(
        !owner_rows.is_empty(),
        "Owner table should have seeded rows"
    );

    let owner_table_keys: HashSet<(String, String, String)> = owner_rows
        .iter()
        .map(|row| {
            let name = row["name"]
                .as_str()
                .unwrap_or_else(|| panic!("Owner.name missing — row: {row}"))
                .to_string();
            let addr = unwrap_json(&row["address"]);
            let city = addr["city"]
                .as_str()
                .unwrap_or_else(|| panic!("Owner.address.city missing — row: {row}"))
                .to_string();
            let country = addr["country"]
                .as_str()
                .unwrap_or_else(|| panic!("Owner.address.country missing — row: {row}"))
                .to_string();
            (name, city, country)
        })
        .collect();

    let owner_missing: Vec<_> = composed_owner_keys
        .difference(&owner_table_keys)
        .cloned()
        .collect();
    assert!(
        owner_missing.is_empty(),
        "every composed.owner (name, city, country) must exist in Owner table \
         — {} missing keys: {:?}; composed_owners={:?}; table={:?}",
        owner_missing.len(),
        owner_missing,
        composed_owner_keys,
        owner_table_keys
    );
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

#[test]
fn test_response_shape_coverage() {
    // Exercises the response shapes the mirage parser handles correctly today
    // against disjoint paths declared in mega.yaml:
    //   (a) /gadgets/{id} — plain 200 single-object $ref   → Definition(Gadget)
    //   (b) /labels       — plain 200 array-of-primitive   → PrimitiveArray
    //   (c) /ping         — 204 only, no schema            → Empty
    //   (e) /things/{id}  — flat $ref with 404 miss-branch
    // Shape B (wrapped-array $ref) is covered separately by the ignored
    // regression test test_response_shape_b_wrapped_array_regression.
    // No recipe activation.
    //
    // DEVIATION (Shape A path): the PM plan probed /gadgets for single-object
    // but mirage's catch_all dispatches any non-path-param GET through
    // get_collection, which always returns an array (src/server.rs:502). The
    // only way to observe a Definition-shape single-object response is via a
    // path-param route (get_single). Added /gadgets/{id} to the fixture and
    // probe there. /gadgets collection still exists as the auto-registered
    // array counterpart and is asserted in the admin-API endpoints section.
    let server = MirageServer::start("tests/fixtures/mega.yaml", "/gadgets");
    let client = reqwest::blocking::Client::new();

    // (a) Plain single-object — status==200 literal; body must be an object
    //     (explicitly NOT array). Probe a flat scalar prop to prove the row
    //     was seeded from the Gadget definition, not a stub fall-back.
    let resp = client.get(server.url("/gadgets/1")).send().unwrap();
    assert_eq!(resp.status(), 200, "/gadgets/1 must return literal 200");
    let body: serde_json::Value = resp.json().unwrap();
    assert!(
        body.is_object(),
        "/gadgets/1 body must be a JSON object — got: {body}"
    );
    assert!(
        !body.is_array(),
        "/gadgets/1 body must NOT be an array — got: {body}"
    );
    assert!(
        body["id"].is_i64() || body["name"].is_string(),
        "/gadgets/1 object must carry at least one Gadget scalar (id or name) — body: {body}"
    );

    // (b) Primitive-array virtual root — status==200, body is array, every
    //     element is a string. Loop instead of asserting arr[0] so a mixed
    //     or partially-wrong seed would still be caught.
    let resp = client.get(server.url("/labels")).send().unwrap();
    assert_eq!(resp.status(), 200, "/labels must return literal 200");
    let body: serde_json::Value = resp.json().unwrap();
    let labels = body.as_array().expect("/labels body must be a JSON array");
    assert!(!labels.is_empty(), "/labels array must be non-empty");
    for (idx, elem) in labels.iter().enumerate() {
        assert!(
            elem.is_string(),
            "/labels[{idx}] must be a string — got: {elem}"
        );
    }

    // (c) Empty response — status==204 literal, body text empty. GET only
    //     because the mirage parser does not list HEAD in path_operations.
    let resp = client.get(server.url("/ping")).send().unwrap();
    assert_eq!(resp.status(), 204, "/ping must return literal 204");
    let text = resp.text().unwrap();
    assert!(text.is_empty(), "/ping body must be empty — got: {text}");

    // (d) Shape B wrapped-array — Shape B body + graph.roots regression
    //     assertions live in test_response_shape_b_wrapped_array_regression
    //     (ignored). This fn intentionally does NOT probe /catalog so the
    //     four passing shapes stay green on the main verify gate.

    // (e) Path-param + 404 miss-branch. First prove /things/1 is reachable
    //     (rules out route-missing false-positive on the 404 probe). 404 id
    //     MUST be a valid integer — string triggers 400 at src/server.rs:224.
    let resp = client.get(server.url("/things/1")).send().unwrap();
    assert_eq!(
        resp.status(),
        200,
        "/things/1 must return 200 (auto-seeded rowid 1)"
    );
    let resp = client.get(server.url("/things/999999")).send().unwrap();
    assert_eq!(
        resp.status(),
        404,
        "/things/999999 must return 404 for known-missing integer id"
    );

    // Admin-API coverage — each new op present in /_api/admin/endpoints.
    let resp = client
        .get(server.url("/_api/admin/endpoints"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let endpoints: serde_json::Value = resp.json().unwrap();
    let eps = endpoints
        .as_array()
        .expect("/_api/admin/endpoints body must be a JSON array");
    let has_endpoint = |method: &str, path: &str| {
        eps.iter().any(|e| {
            e["method"].as_str().map(|m| m.to_lowercase()) == Some(method.to_string())
                && e["path"].as_str() == Some(path)
        })
    };
    for (method, path) in [
        ("get", "/gadgets"),
        ("get", "/gadgets/{id}"),
        ("get", "/labels"),
        ("get", "/ping"),
        ("get", "/catalog"),
        ("get", "/things"),
        ("get", "/things/{id}"),
    ] {
        assert!(
            has_endpoint(method, path),
            "admin endpoints must list {} {} — got: {eps:?}",
            method.to_uppercase(),
            path
        );
    }

    // Admin graph — assert shape-specific placement per entity_graph.rs.
    let resp = client.get(server.url("/_api/admin/graph")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let graph: serde_json::Value = resp.json().unwrap();
    let roots = graph["roots"]
        .as_object()
        .expect("graph.roots must be a JSON object");
    let virtual_roots = graph["virtual_roots"]
        .as_array()
        .expect("graph.virtual_roots must be a JSON array");

    let root_contains = |def_name: &str, path: &str| -> bool {
        roots
            .get(def_name)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|e| e["path"].as_str() == Some(path)))
            .unwrap_or(false)
    };

    assert!(
        root_contains("Gadget", "/gadgets"),
        "graph.roots[Gadget] must include /gadgets — got roots: {:?}",
        roots.keys().collect::<Vec<_>>()
    );
    assert!(
        root_contains("Thing", "/things/{id}"),
        "graph.roots[Thing] must include /things/{{id}} — got roots: {:?}",
        roots.keys().collect::<Vec<_>>()
    );

    let virtual_paths: Vec<&str> = virtual_roots
        .iter()
        .filter_map(|v| v["endpoint"]["path"].as_str())
        .collect();
    assert!(
        virtual_paths.contains(&"/labels"),
        "graph.virtual_roots must include /labels — got: {virtual_paths:?}"
    );
    assert!(
        !virtual_paths.contains(&"/ping"),
        "graph.virtual_roots must NOT include /ping (Empty branch is skipped) — got: {virtual_paths:?}"
    );
    let ping_in_roots = roots
        .values()
        .filter_map(|v| v.as_array())
        .any(|arr| arr.iter().any(|e| e["path"].as_str() == Some("/ping")));
    assert!(
        !ping_in_roots,
        "graph.roots must NOT contain /ping (Empty branch is skipped) — got roots: {:?}",
        roots.keys().collect::<Vec<_>>()
    );

    // Admin definitions — all four new defs must be keys in the payload.
    // NO count assertions (wrapper stub + future defs would break them).
    let resp = client
        .get(server.url("/_api/admin/definitions"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let defs: serde_json::Value = resp.json().unwrap();
    let defs_obj = defs
        .as_object()
        .expect("/_api/admin/definitions body must be a JSON object");
    for def_name in ["Gadget", "CatalogPage", "CatalogItem", "Thing"] {
        assert!(
            defs_obj.contains_key(def_name),
            "/_api/admin/definitions must contain key {def_name} — got keys: {:?}",
            defs_obj.keys().collect::<Vec<_>>()
        );
    }
}

/// Shape B (wrapped-array) regression detector. Currently #[ignore]-d
/// because mirage's primary_response_def/root_def_name chain returns
/// the WRAPPER def (CatalogPage) instead of the element def (CatalogItem)
/// for responses of form: response.schema = $ref → def of type:array +
/// items:$ref. Downstream the seeder skips the wrapper's stub table so
/// GET /catalog returns []. Un-ignore this test after the Shape B bug is
/// fixed (see follow-up limbo task). Fixture entries for /catalog live
/// in tests/fixtures/mega.yaml.
#[test]
fn test_response_shape_b_wrapped_array_regression() {
    let server = MirageServer::start("tests/fixtures/mega.yaml", "/gadgets");
    let client = reqwest::blocking::Client::new();

    // Body contract — GET /catalog must return 200 + non-empty array + every
    // element must carry a CatalogItem scalar (sku or title). Today mirage
    // picks CatalogPage (the wrapper) as the table name, the wrapper stub
    // table seeds zero rows, so /catalog returns []. Post-fix expectation:
    // element-def rows present.
    let resp = client
        .get(server.url("/catalog"))
        .send()
        .expect("GET /catalog must be reachable");
    assert_eq!(
        resp.status(),
        200,
        "Shape B regression: /catalog must return literal 200 post-fix"
    );
    let catalog_body: serde_json::Value = resp
        .json()
        .expect("Shape B regression: /catalog body must be valid JSON");
    let catalog_arr = catalog_body
        .as_array()
        .expect("Shape B regression: /catalog body must be a JSON array (wrapped-array contract)");
    assert!(
        !catalog_arr.is_empty(),
        "Shape B regression: /catalog array must be non-empty post-fix — empty array means \
         the seeder skipped the element (CatalogItem) table because the wrapper (CatalogPage) \
         was picked as root_def_name. Got: {catalog_body}"
    );
    for (idx, elem) in catalog_arr.iter().enumerate() {
        assert!(
            elem["sku"].is_string() || elem["title"].is_string(),
            "Shape B regression: /catalog[{idx}] must expose a CatalogItem scalar (sku or title) \
             — got: {elem}. Wrapper-vs-element bug still live."
        );
    }

    // Admin-graph contract — /catalog must appear under roots["CatalogItem"]
    // (the element def), NOT under roots["CatalogPage"] (the wrapper) and
    // NOT in virtual_roots. primary_response_def/root_def_name must unwrap
    // the wrapper to the element def.
    let resp = client
        .get(server.url("/_api/admin/graph"))
        .send()
        .expect("GET /_api/admin/graph must be reachable");
    assert_eq!(resp.status(), 200);
    let graph: serde_json::Value = resp
        .json()
        .expect("Shape B regression: graph body must be valid JSON");
    let roots = graph["roots"]
        .as_object()
        .expect("Shape B regression: graph.roots must be a JSON object");
    let virtual_roots = graph["virtual_roots"]
        .as_array()
        .expect("Shape B regression: graph.virtual_roots must be a JSON array");

    let root_has_path = |def_name: &str, path: &str| -> bool {
        roots
            .get(def_name)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|e| e["path"].as_str() == Some(path)))
            .unwrap_or(false)
    };

    assert!(
        root_has_path("CatalogItem", "/catalog"),
        "Shape B regression: /catalog must resolve to roots[CatalogItem] (the element def), \
         not roots[CatalogPage] (the wrapper). root_def_name must unwrap wrapped-array defs. \
         Got roots keys: {:?}",
        roots.keys().collect::<Vec<_>>()
    );
    assert!(
        !root_has_path("CatalogPage", "/catalog"),
        "Shape B regression: /catalog must NOT appear under roots[CatalogPage] (wrapper). \
         Got roots keys: {:?}",
        roots.keys().collect::<Vec<_>>()
    );
    let virtual_paths: Vec<&str> = virtual_roots
        .iter()
        .filter_map(|v| v["endpoint"]["path"].as_str())
        .collect();
    assert!(
        !virtual_paths.contains(&"/catalog"),
        "Shape B regression: /catalog must NOT appear in graph.virtual_roots — \
         wrapped-array must resolve to a named element def. Got virtual_paths: {virtual_paths:?}"
    );
}

// ---------------------------------------------------------------------------
// `mirage recipes` CLI (HTTP client against the admin API)
// ---------------------------------------------------------------------------

/// Minimal valid recipe body for admin POST /_api/admin/recipes.
fn recipe_seed_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "spec_source": "swagger: \"2.0\"\ninfo:\n  title: t\n  version: \"1\"\npaths: {}\n",
        "endpoints": [
            { "method": "GET", "path": "/foo" },
            { "method": "POST", "path": "/bar" }
        ],
        "seed_count": 5,
        "shared_pools": {},
        "quantity_configs": {},
        "faker_rules": {},
        "rules": [],
        "frozen_rows": {}
    })
}

fn post_recipe(server: &MirageServer, name: &str) -> serde_json::Value {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&recipe_seed_body(name))
        .send()
        .expect("POST /_api/admin/recipes failed");
    assert_eq!(resp.status(), 201, "create recipe should return 201");
    resp.json().expect("create recipe response should be JSON")
}

fn mirage_cli(server: &MirageServer, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mirage"))
        .arg("recipes")
        .args(args)
        .arg("--url")
        .arg(server.base())
        .output()
        .expect("failed to invoke mirage binary")
}

#[test]
fn test_recipes_cli_list() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let a = post_recipe(&server, "Recipe A");
    let b = post_recipe(&server, "Recipe B");

    let out = mirage_cli(&server, &["list"]);
    assert!(
        out.status.success(),
        "recipes list should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let summaries: Vec<serde_json::Value> =
        serde_json::from_str(&stdout).expect("stdout should be JSON array");
    assert!(summaries.len() >= 2);

    let find = |id: i64| {
        summaries
            .iter()
            .find(|s| s["id"].as_i64() == Some(id))
            .cloned()
    };
    let sa = find(a["id"].as_i64().unwrap()).expect("recipe A summary");
    assert_eq!(sa["name"], "Recipe A");
    assert_eq!(sa["seed_count"], 5);
    assert_eq!(sa["endpoint_count"], 2);
    // Summary must NOT include the full config fields.
    assert!(sa.get("spec_source").is_none());
    assert!(sa.get("quantity_configs").is_none());

    let sb = find(b["id"].as_i64().unwrap()).expect("recipe B summary");
    assert_eq!(sb["name"], "Recipe B");
}

#[test]
fn test_recipes_cli_show() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Show Me");
    let id = created["id"].as_i64().unwrap();

    let out = mirage_cli(&server, &["show", &id.to_string()]);
    assert!(out.status.success(), "recipes show should exit 0");
    let recipe: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(recipe["id"].as_i64(), Some(id));
    assert_eq!(recipe["name"], "Show Me");
    // Config fields expanded, not JSON strings.
    assert!(
        recipe["selected_endpoints"].is_array(),
        "selected_endpoints should be parsed into an array"
    );
    assert_eq!(recipe["selected_endpoints"].as_array().unwrap().len(), 2);
    assert!(
        recipe.get("shared_pools").is_none(),
        "shared_pools surface removed; show response must omit the key"
    );
    assert!(recipe["quantity_configs"].is_object());
    assert!(recipe["faker_rules"].is_object());
    assert!(recipe["rules"].is_array());
    assert!(recipe["frozen_rows"].is_object());
}

#[test]
fn test_recipes_cli_show_404() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let out = mirage_cli(&server, &["show", "999999"]);
    assert!(
        !out.status.success(),
        "recipes show on missing id should exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(
        err.get("error").is_some(),
        "stderr should be {{\"error\": ...}} but was: {stderr}"
    );
}

#[test]
fn test_recipes_cli_delete() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Delete Me");
    let id = created["id"].as_i64().unwrap();

    let out = mirage_cli(&server, &["delete", &id.to_string()]);
    assert!(out.status.success(), "recipes delete should exit 0");
    let confirm: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON confirmation");
    assert_eq!(confirm["id"].as_i64(), Some(id));
    assert_eq!(confirm["deleted"], true);

    // Server no longer has it.
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{id}")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 404);

    // Second delete should now fail.
    let out2 = mirage_cli(&server, &["delete", &id.to_string()]);
    assert!(
        !out2.status.success(),
        "second delete should exit non-zero (404)"
    );
}

#[test]
fn test_recipes_cli_clone() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Clone Source");
    let src_id = created["id"].as_i64().unwrap();

    let out = mirage_cli(&server, &["clone", &src_id.to_string()]);
    assert!(out.status.success(), "recipes clone should exit 0");
    let cloned: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_ne!(
        cloned["id"].as_i64(),
        Some(src_id),
        "clone must get fresh id"
    );
    assert_eq!(cloned["name"], "Clone Source (copy)");
    // Config fields expanded.
    assert!(cloned["selected_endpoints"].is_array());
    assert_eq!(cloned["selected_endpoints"].as_array().unwrap().len(), 2);
    assert!(
        cloned.get("shared_pools").is_none(),
        "shared_pools surface removed; clone response must omit the key"
    );
}

#[test]
fn test_recipes_cli_activate() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Activate Me");
    let id = created["id"].as_i64().unwrap();

    let out = mirage_cli(&server, &["activate", &id.to_string()]);
    assert!(
        out.status.success(),
        "recipes activate should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let confirm: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON confirmation");
    assert_eq!(confirm["id"].as_i64(), Some(id));
    assert_eq!(confirm["name"], "Activate Me");
    assert_eq!(confirm["status"], "activated");
    assert!(
        confirm["endpoints"].is_array(),
        "endpoints should be a JSON array, got: {}",
        confirm
    );
}

#[test]
fn test_recipes_cli_activate_404() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let out = mirage_cli(&server, &["activate", "999999"]);
    assert!(
        !out.status.success(),
        "activate on missing id should exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(
        err.get("error").is_some(),
        "stderr should be {{\"error\": ...}} but was: {stderr}"
    );
}

#[test]
fn test_recipes_cli_honours_mirage_url_env() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    post_recipe(&server, "EnvRecipe");

    let out = Command::new(env!("CARGO_BIN_EXE_mirage"))
        .env("MIRAGE_URL", server.base())
        .args(["recipes", "list"])
        .output()
        .expect("failed to invoke mirage binary");
    assert!(
        out.status.success(),
        "MIRAGE_URL env var must be honoured. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let summaries: Vec<serde_json::Value> = serde_json::from_str(&stdout).unwrap();
    assert!(summaries.iter().any(|s| s["name"] == "EnvRecipe"));
}

#[test]
fn test_recipes_cli_http_failure_bad_url() {
    // Find a guaranteed-closed port by binding and dropping the listener.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = listener.local_addr().unwrap().port();
        drop(listener);
        p
    };
    let bogus = format!("http://127.0.0.1:{port}");
    let out = Command::new(env!("CARGO_BIN_EXE_mirage"))
        .args(["recipes", "list", "--url", &bogus])
        .output()
        .expect("failed to invoke mirage binary");
    assert!(!out.status.success(), "unreachable host must exit non-zero");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(err.get("error").is_some());
}

/// Allocate a per-test scratch directory (distinct from MirageServer.workdir
/// so test inputs are not cleaned up when the server drops).
fn scratch_dir(tag: &str) -> PathBuf {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = std::env::temp_dir().join(format!("mirage-cli-{tag}-{port}"));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

const MINI_SPEC: &str = "swagger: \"2.0\"\ninfo:\n  title: t\n  version: \"1\"\npaths: {}\n";

#[test]
fn test_recipes_cli_create() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let scratch = scratch_dir("create");

    let spec_path = scratch.join("spec.yaml");
    std::fs::write(&spec_path, MINI_SPEC).unwrap();

    let endpoints_path = scratch.join("endpoints.json");
    std::fs::write(
        &endpoints_path,
        r#"[{"method":"GET","path":"/foo"},{"method":"POST","path":"/bar"},{"method":"DELETE","path":"/baz"}]"#,
    )
    .unwrap();

    let config_path = scratch.join("config.json");
    // An extra `shared_pools` key exercises back-compat parse-ignore on the
    // server; the `custom_lists` payload is what we actually round-trip.
    std::fs::write(
        &config_path,
        r#"{
            "shared_pools": {"legacy": {"is_shared": true, "pool_size": 3}},
            "quantity_configs": {},
            "faker_rules": {},
            "custom_lists": {"color": ["red","blue"]},
            "rules": [],
            "frozen_rows": {}
        }"#,
    )
    .unwrap();

    let out = mirage_cli(
        &server,
        &[
            "create",
            "--name",
            "Created From CLI",
            "--spec-file",
            spec_path.to_str().unwrap(),
            "--endpoints-file",
            endpoints_path.to_str().unwrap(),
            "--seed-count",
            "7",
            "--config-file",
            config_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "recipes create should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let recipe: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert!(recipe["id"].as_i64().is_some(), "response has id");
    assert_eq!(recipe["name"], "Created From CLI");
    assert_eq!(recipe["seed_count"], 7);
    assert_eq!(recipe["spec_source"], MINI_SPEC);
    // selected_endpoints is stored as a JSON string by the server; the raw
    // create response mirrors that shape (siblings show/clone expand; create
    // does not per design).
    let endpoints_str = recipe["selected_endpoints"]
        .as_str()
        .expect("selected_endpoints is JSON string in raw create response");
    let endpoints_parsed: Vec<serde_json::Value> =
        serde_json::from_str(endpoints_str).expect("selected_endpoints parses");
    assert_eq!(endpoints_parsed.len(), 3);
    assert_eq!(endpoints_parsed[0]["method"], "GET");
    assert_eq!(endpoints_parsed[0]["path"], "/foo");
    // shared_pools has been removed from the Recipe struct — the create
    // response must omit the key entirely, even though the CLI body may have
    // included it (back-compat parse-ignore).
    assert!(
        recipe.get("shared_pools").is_none(),
        "shared_pools surface removed; raw create response must omit the key"
    );
    // custom_lists round-tripped as a JSON string on the raw create response.
    let lists_str = recipe["custom_lists"]
        .as_str()
        .expect("custom_lists string");
    assert!(lists_str.contains("color"));

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_create_without_config_or_seed() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let scratch = scratch_dir("create-min");

    let spec_path = scratch.join("spec.yaml");
    std::fs::write(&spec_path, MINI_SPEC).unwrap();
    let endpoints_path = scratch.join("endpoints.json");
    std::fs::write(&endpoints_path, r#"[{"method":"GET","path":"/x"}]"#).unwrap();

    let out = mirage_cli(
        &server,
        &[
            "create",
            "--name",
            "Minimal",
            "--spec-file",
            spec_path.to_str().unwrap(),
            "--endpoints-file",
            endpoints_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "recipes create (minimal) should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let recipe: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert_eq!(recipe["name"], "Minimal");
    // Server default seed_count = 10.
    assert_eq!(recipe["seed_count"], 10);

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_create_bad_endpoints_file() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let scratch = scratch_dir("create-bad");

    let spec_path = scratch.join("spec.yaml");
    std::fs::write(&spec_path, MINI_SPEC).unwrap();
    let endpoints_path = scratch.join("endpoints.json");
    // Not an array — a plain object.
    std::fs::write(&endpoints_path, r#"{"method":"GET","path":"/foo"}"#).unwrap();

    let out = mirage_cli(
        &server,
        &[
            "create",
            "--name",
            "Bad",
            "--spec-file",
            spec_path.to_str().unwrap(),
            "--endpoints-file",
            endpoints_path.to_str().unwrap(),
        ],
    );
    assert!(
        !out.status.success(),
        "bad endpoints file must cause non-zero exit"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(
        err["error"].as_str().unwrap().contains("array"),
        "error message should mention array shape: {stderr}"
    );

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_create_missing_spec_file() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let scratch = scratch_dir("create-missing");

    let endpoints_path = scratch.join("endpoints.json");
    std::fs::write(&endpoints_path, r#"[{"method":"GET","path":"/x"}]"#).unwrap();
    let missing = scratch.join("nope.yaml");

    let out = mirage_cli(
        &server,
        &[
            "create",
            "--name",
            "Missing",
            "--spec-file",
            missing.to_str().unwrap(),
            "--endpoints-file",
            endpoints_path.to_str().unwrap(),
        ],
    );
    assert!(
        !out.status.success(),
        "missing spec file must exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(
        err["error"].as_str().unwrap().contains("failed to read"),
        "error should mention read failure: {stderr}"
    );

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_export_stdout() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Export Me");
    let id = created["id"].as_i64().unwrap();

    let out = mirage_cli(&server, &["export", &id.to_string()]);
    assert!(
        out.status.success(),
        "recipes export should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let exported: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should be JSON");
    assert_eq!(exported["mirage_recipe"], 2);
    assert_eq!(exported["name"], "Export Me");
    assert!(
        exported["spec_source"].is_string(),
        "spec_source should be a string"
    );
    assert!(
        exported["selected_endpoints"].is_array(),
        "selected_endpoints should be expanded (not JSON string)"
    );
    assert!(
        exported.get("shared_pools").is_none(),
        "shared_pools surface removed; export must omit the key"
    );
    assert!(exported["rules"].is_array());
}

#[test]
fn test_recipes_cli_export_to_file() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Export To File");
    let id = created["id"].as_i64().unwrap();
    let scratch = scratch_dir("export-file");
    let out_path = scratch.join("exported.json");

    let out = mirage_cli(
        &server,
        &[
            "export",
            &id.to_string(),
            "--file",
            out_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "recipes export --file should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // stdout should be empty when --file is given.
    assert!(
        out.stdout.is_empty(),
        "stdout should be empty when --file is used; got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let contents = std::fs::read_to_string(&out_path).expect("exported file exists");
    let exported: serde_json::Value =
        serde_json::from_str(&contents).expect("file contents should be JSON");
    assert_eq!(exported["mirage_recipe"], 2);
    assert_eq!(exported["name"], "Export To File");
    assert!(exported["spec_source"].is_string());

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_export_404() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let out = mirage_cli(&server, &["export", "999999"]);
    assert!(
        !out.status.success(),
        "export on missing id should exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(err.get("error").is_some());
}

#[test]
fn test_recipes_cli_import_roundtrip() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Round Trip");
    let src_id = created["id"].as_i64().unwrap();
    let scratch = scratch_dir("import");

    // Export via CLI to file.
    let exported_path = scratch.join("recipe.json");
    let out = mirage_cli(
        &server,
        &[
            "export",
            &src_id.to_string(),
            "--file",
            exported_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "export step of round trip must succeed"
    );

    // Import the exported file back.
    let out = mirage_cli(
        &server,
        &["import", "--file", exported_path.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "recipes import should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let imported: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert!(
        imported["id"].as_i64().is_some(),
        "imported recipe has an id"
    );
    assert_ne!(
        imported["id"].as_i64(),
        Some(src_id),
        "imported recipe gets a fresh id"
    );
    assert_eq!(imported["name"], "Round Trip");
    assert_eq!(imported["seed_count"], 5);

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_import_bad_file() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let scratch = scratch_dir("import-bad");

    // File that is not valid JSON at all.
    let bad = scratch.join("bad.json");
    std::fs::write(&bad, "this is not json").unwrap();

    let out = mirage_cli(&server, &["import", "--file", bad.to_str().unwrap()]);
    assert!(!out.status.success(), "bad JSON must exit non-zero");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(
        err["error"].as_str().unwrap().contains("invalid JSON"),
        "error should mention invalid JSON: {stderr}"
    );

    // File that is JSON but missing the mirage_recipe marker — server rejects.
    let wrong = scratch.join("wrong.json");
    std::fs::write(&wrong, r#"{"name":"no marker"}"#).unwrap();
    let out = mirage_cli(&server, &["import", "--file", wrong.to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "missing mirage_recipe marker must exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(err.get("error").is_some());

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_config_apply() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Apply Me");
    let id = created["id"].as_i64().unwrap();
    let scratch = scratch_dir("config-apply");

    let cfg_path = scratch.join("cfg.json");
    // An extra `shared_pools` key exercises back-compat parse-ignore on the
    // server; the rest of the payload is what we actually round-trip.
    std::fs::write(
        &cfg_path,
        r#"{
            "shared_pools": {"Pet": {"is_shared": true, "pool_size": 7}},
            "quantity_configs": {"Pet.tags": {"min": 1, "max": 3}},
            "faker_rules": {"Pet.name": {"kind": "FirstName"}},
            "rules": [],
            "frozen_rows": {}
        }"#,
    )
    .unwrap();

    let out = mirage_cli(
        &server,
        &[
            "config",
            "apply",
            &id.to_string(),
            "--file",
            cfg_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "recipes config apply should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let echoed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    assert!(
        echoed.get("shared_pools").is_none(),
        "shared_pools surface removed; config apply echo must omit the key"
    );
    assert_eq!(echoed["quantity_configs"]["Pet.tags"]["max"], 3);
    assert_eq!(echoed["faker_rules"]["Pet.name"]["kind"], "FirstName");
    assert!(echoed["rules"].is_array());
    assert!(echoed["frozen_rows"].is_object());

    // Re-fetch via show to confirm server persisted the replacement.
    let out = mirage_cli(&server, &["show", &id.to_string()]);
    assert!(out.status.success());
    let recipe: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("show stdout should be JSON");
    assert!(
        recipe.get("shared_pools").is_none(),
        "shared_pools surface removed; show response must omit the key"
    );
    assert_eq!(recipe["quantity_configs"]["Pet.tags"]["min"], 1);

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_config_apply_404() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let scratch = scratch_dir("config-apply-404");

    let cfg_path = scratch.join("cfg.json");
    std::fs::write(
        &cfg_path,
        r#"{"shared_pools":{},"quantity_configs":{},"faker_rules":{}}"#,
    )
    .unwrap();

    let out = mirage_cli(
        &server,
        &[
            "config",
            "apply",
            "999999",
            "--file",
            cfg_path.to_str().unwrap(),
        ],
    );
    assert!(
        !out.status.success(),
        "config apply on missing id should exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(err.get("error").is_some());

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_config_apply_bad_file() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Bad File Target");
    let id = created["id"].as_i64().unwrap();
    let scratch = scratch_dir("config-apply-bad");

    // File that is not valid JSON at all.
    let bad = scratch.join("bad.json");
    std::fs::write(&bad, "not json").unwrap();

    let out = mirage_cli(
        &server,
        &[
            "config",
            "apply",
            &id.to_string(),
            "--file",
            bad.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success(), "bad JSON must exit non-zero");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(
        err["error"].as_str().unwrap().contains("invalid JSON"),
        "error should mention invalid JSON: {stderr}"
    );

    // JSON but missing required keys (server rejects deserialize).
    let missing = scratch.join("missing.json");
    std::fs::write(&missing, r#"{"shared_pools": {}}"#).unwrap();

    let out = mirage_cli(
        &server,
        &[
            "config",
            "apply",
            &id.to_string(),
            "--file",
            missing.to_str().unwrap(),
        ],
    );
    assert!(
        !out.status.success(),
        "missing required config keys must exit non-zero"
    );
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    let err: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("stderr should be JSON error");
    assert!(err.get("error").is_some());

    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn test_recipes_cli_config_apply_honours_mirage_url_env() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let created = post_recipe(&server, "Env URL");
    let id = created["id"].as_i64().unwrap();
    let scratch = scratch_dir("config-apply-env");

    let cfg_path = scratch.join("cfg.json");
    std::fs::write(
        &cfg_path,
        r#"{"shared_pools":{},"quantity_configs":{},"faker_rules":{}}"#,
    )
    .unwrap();

    // No --url flag; MIRAGE_URL env points at the server.
    let out = Command::new(env!("CARGO_BIN_EXE_mirage"))
        .arg("recipes")
        .arg("config")
        .arg("apply")
        .arg(id.to_string())
        .arg("--file")
        .arg(cfg_path.to_str().unwrap())
        .env("MIRAGE_URL", server.base())
        .output()
        .expect("failed to invoke mirage binary");
    assert!(
        out.status.success(),
        "config apply should honour MIRAGE_URL env. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    std::fs::remove_dir_all(&scratch).ok();
}

// ---------------------------------------------------------------------------
// Recipe custom_lists persistence (task afje).
//
// Four tests, one per acceptance criterion on the parent story "custom lists
// persist with the recipe across restarts":
//   1. restart-survival: GET config yields identical payload after DB round-trip
//   2. export / clone / import round-trip: CLI flows preserve custom_lists
//   3. fresh-server activation seeding: every seeded value comes from list
//   4. delete cleanup: DELETE A returns 404 on subsequent GET; B is untouched
//
// Plumbing owned by siblings omhh (custom_lists column + CRUD + config API +
// export/import/clone) and wwql (seed-time custom-list resolution via
// FakerStrategy::Custom). afje adds only integration-test coverage.
// ---------------------------------------------------------------------------

/// Build a POST /_api/admin/recipes body that exercises custom_lists. Uses
/// the petstore fixture so activation can seed the /pet collection and
/// Pet.name (string) is a valid target for the custom-list faker rule.
fn recipe_body_with_custom_lists(
    name: &str,
    custom_lists: serde_json::Value,
    faker_rules: serde_json::Value,
    seed_count: i64,
) -> serde_json::Value {
    let spec_source = std::fs::read_to_string("tests/fixtures/petstore.yaml").unwrap();
    serde_json::json!({
        "name": name,
        "spec_source": spec_source,
        "endpoints": [
            { "method": "get", "path": "/pet/{petId}" },
        ],
        "seed_count": seed_count,
        "shared_pools": {},
        "quantity_configs": {},
        "faker_rules": faker_rules,
        "rules": [],
        "frozen_rows": {},
        "custom_lists": custom_lists,
    })
}

/// AC1: recipe round-trips through an actual server restart (DB reopen).
/// custom_lists and faker_rules come back identical post-restart.
#[test]
fn test_recipe_custom_lists_persist_restart() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lists = serde_json::json!({ "Greetings": ["hi", "hey", "howdy"] });
    let faker_rules = serde_json::json!({ "Pet.name": "Greetings" });
    let body = recipe_body_with_custom_lists(
        &format!("persist-restart-{nanos}"),
        lists.clone(),
        faker_rules.clone(),
        5,
    );

    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201, "create recipe should return 201");
    let created: serde_json::Value = resp.json().unwrap();
    let id = created["id"].as_i64().expect("numeric id");

    // Baseline read of the config endpoint, pre-restart.
    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{id}/config")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let pre: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        pre["custom_lists"], lists,
        "pre-restart custom_lists should match what was written"
    );
    assert_eq!(
        pre["faker_rules"], faker_rules,
        "pre-restart faker_rules should match what was written"
    );

    // Actual restart: kill the server but keep the workdir so mirage.db survives.
    let workdir = server.stop_preserve_dir();
    let server =
        MirageServer::start_in_existing_dir(workdir, "tests/fixtures/petstore.yaml", "/pet");

    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{id}/config")))
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "GET config after restart must still find the recipe"
    );
    let post: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        post["custom_lists"], lists,
        "custom_lists must survive server restart"
    );
    assert_eq!(
        post["faker_rules"], faker_rules,
        "faker_rules must survive server restart"
    );
}

/// AC2: export file includes custom_lists, clone copies them, and import into
/// a fresh server reproduces the original list map.
#[test]
fn test_recipe_custom_lists_persist_export_clone() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lists = serde_json::json!({ "Greetings": ["hi", "hey", "howdy"] });
    let faker_rules = serde_json::json!({ "Pet.name": "Greetings" });
    let body = recipe_body_with_custom_lists(
        &format!("persist-export-{nanos}"),
        lists.clone(),
        faker_rules.clone(),
        5,
    );

    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().unwrap();
    let id = created["id"].as_i64().unwrap();

    // --- Export to file and assert the JSON carries custom_lists verbatim.
    let scratch = scratch_dir("custom-lists-export");
    let exported_path = scratch.join("recipe.json");

    let out = mirage_cli(
        &server,
        &[
            "export",
            &id.to_string(),
            "--file",
            exported_path.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "recipes export should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let contents = std::fs::read_to_string(&exported_path).expect("exported file exists");
    let exported: serde_json::Value =
        serde_json::from_str(&contents).expect("exported file should be JSON");
    assert_eq!(
        exported["custom_lists"], lists,
        "exported JSON must include custom_lists verbatim"
    );

    // --- Clone: new recipe gets the same custom_lists.
    let out = mirage_cli(&server, &["clone", &id.to_string()]);
    assert!(
        out.status.success(),
        "recipes clone should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cloned: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("clone stdout should be JSON");
    let cloned_id = cloned["id"].as_i64().expect("clone has numeric id");
    assert_ne!(cloned_id, id, "clone must get a fresh id");

    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{cloned_id}/config")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cloned_cfg: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        cloned_cfg["custom_lists"], lists,
        "clone must preserve custom_lists"
    );
    assert_eq!(
        cloned_cfg["faker_rules"], faker_rules,
        "clone must preserve faker_rules"
    );

    // --- Import into a SECOND, fresh server. New DB, new custom_lists row.
    let server2 = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let out = mirage_cli(
        &server2,
        &["import", "--file", exported_path.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "recipes import on fresh server should exit 0. stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let imported: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("import stdout should be JSON");
    let imported_id = imported["id"].as_i64().expect("imported recipe has id");

    let resp = client
        .get(server2.url(&format!("/_api/admin/recipes/{imported_id}/config")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let imported_cfg: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        imported_cfg["custom_lists"], lists,
        "imported recipe on fresh server must carry custom_lists"
    );
    assert_eq!(
        imported_cfg["faker_rules"], faker_rules,
        "imported recipe on fresh server must carry faker_rules"
    );

    std::fs::remove_dir_all(&scratch).ok();
}

/// AC3: activation on a fresh server seeds the collection using the custom
/// list. Every seeded pet.name value must appear in the list's values.
#[test]
fn test_recipe_custom_lists_persist_activate_seed() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let allowed = ["hi", "hey", "howdy"];
    let lists = serde_json::json!({ "Greetings": allowed });
    let faker_rules = serde_json::json!({ "Pet.name": "Greetings" });
    // seed_count=20 gives sampling headroom but keeps the test fast.
    let body = recipe_body_with_custom_lists(
        &format!("persist-activate-{nanos}"),
        lists.clone(),
        faker_rules.clone(),
        20,
    );

    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created: serde_json::Value = resp.json().unwrap();
    let id = created["id"].as_i64().unwrap();

    let resp = client
        .post(server.url(&format!("/_api/admin/recipes/{id}/activate")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200, "activate should return 200");

    let resp = client.get(server.url("/pet")).send().unwrap();
    assert_eq!(
        resp.status(),
        200,
        "GET /pet should return 200 post-activate"
    );
    let body: serde_json::Value = resp.json().unwrap();
    let arr = body
        .as_array()
        .expect("/pet response should be a JSON array");
    assert!(!arr.is_empty(), "/pet should have at least one row");

    let allowed_set: std::collections::HashSet<&str> = allowed.iter().copied().collect();
    for (idx, pet) in arr.iter().enumerate() {
        let name = pet["name"]
            .as_str()
            .unwrap_or_else(|| panic!("row {idx}: pet.name must be a string, got {pet}"));
        assert!(
            allowed_set.contains(name),
            "row {idx}: pet.name {name:?} must be drawn from custom list Greetings={allowed:?}, got {pet}"
        );
    }
}

/// AC4: deleting recipe A returns 404 on subsequent GET, and recipe B's
/// custom_lists remain intact (no cross-recipe leak).
#[test]
fn test_recipe_custom_lists_persist_delete_no_leak() {
    let server = MirageServer::start_isolated("tests/fixtures/petstore.yaml", "/pet");
    let client = reqwest::blocking::Client::new();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let lists_a = serde_json::json!({ "ListA": ["a1", "a2"] });
    let body_a = recipe_body_with_custom_lists(
        &format!("persist-delete-a-{nanos}"),
        lists_a.clone(),
        serde_json::json!({}),
        5,
    );
    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body_a)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created_a: serde_json::Value = resp.json().unwrap();
    let id_a = created_a["id"].as_i64().unwrap();

    let lists_b = serde_json::json!({ "ListB": ["b1", "b2", "b3"] });
    let body_b = recipe_body_with_custom_lists(
        &format!("persist-delete-b-{nanos}"),
        lists_b.clone(),
        serde_json::json!({}),
        5,
    );
    let resp = client
        .post(server.url("/_api/admin/recipes"))
        .json(&body_b)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let created_b: serde_json::Value = resp.json().unwrap();
    let id_b = created_b["id"].as_i64().unwrap();
    assert_ne!(id_a, id_b, "two recipes must have distinct ids");

    // Delete A.
    let resp = client
        .delete(server.url(&format!("/_api/admin/recipes/{id_a}")))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "DELETE recipe A must return 2xx, got {}",
        resp.status()
    );

    // Subsequent GET for A must be 404.
    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{id_a}")))
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "GET on deleted recipe A must be 404, got {}",
        resp.status()
    );

    // Config endpoint for A also 404.
    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{id_a}/config")))
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "GET config on deleted recipe A must be 404"
    );

    // Recipe B untouched — custom_lists still surface ListB exactly.
    let resp = client
        .get(server.url(&format!("/_api/admin/recipes/{id_b}/config")))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cfg_b: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        cfg_b["custom_lists"], lists_b,
        "recipe B custom_lists must be unchanged after A is deleted"
    );
}
