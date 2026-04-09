use std::net::TcpListener;
use std::process::{Child, Command};

struct MirageServer {
    child: Child,
    port: u16,
}

impl MirageServer {
    fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let child = Command::new(env!("CARGO_BIN_EXE_mirage"))
            .args(["tests/fixtures/petstore.yaml", "--port", &port.to_string()])
            .spawn()
            .expect("failed to start mirage");

        let server = Self { child, port };
        let client = reqwest::blocking::Client::new();
        let base = format!("http://127.0.0.1:{}", port);
        for _ in 0..50 {
            if client.get(format!("{}/pet", base)).send().is_ok() {
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
    let server = MirageServer::start();
    let client = reqwest::blocking::Client::new();
    let resp = client.get(server.url("/pet")).send().unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(body.is_array());
    assert!(!body.as_array().unwrap().is_empty());
}

#[test]
fn test_e2e_get_single() {
    let server = MirageServer::start();
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
    let server = MirageServer::start();
    let client = reqwest::blocking::Client::new();
    let resp = client.get(server.url("/pet/999999")).send().unwrap();
    assert_eq!(resp.status(), 404);
}

#[test]
fn test_e2e_post_create() {
    let server = MirageServer::start();
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
    let server = MirageServer::start();
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
    let server = MirageServer::start();
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
    let server = MirageServer::start();
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
