mod parser;
mod schema;
mod seeder;
mod server;

use clap::Parser;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser)]
#[command(name = "mirage", about = "Swagger 2.0 mock API server")]
struct Cli {
    /// Path to the Swagger spec file
    spec: PathBuf,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3000)]
    port: u16,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let mut spec = parser::SwaggerSpec::from_file(cli.spec.to_str().unwrap()).unwrap();
    spec.resolve_refs();

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    schema::create_tables(&conn, &spec).unwrap();
    seeder::seed_tables(&conn, &spec, 10).unwrap();

    let db: server::Db = Arc::new(Mutex::new(conn));

    let spec_info = server::SpecInfo {
        title: spec.info.title.clone(),
        version: spec.info.version.clone(),
    };
    let endpoints: Vec<server::EndpointInfo> = spec
        .path_operations()
        .iter()
        .map(|(path, method, _)| server::EndpointInfo {
            method: method.to_string(),
            path: path.to_string(),
        })
        .collect();

    let state = server::AppState {
        db,
        spec_info,
        endpoints,
    };
    let router = server::build_router(&spec, state);

    println!("Mirage server running on port {}", cli.port);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .unwrap();
    axum::serve(listener, router).await.unwrap();
}
