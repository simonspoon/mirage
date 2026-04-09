mod parser;
mod schema;
mod seeder;
mod server;

use clap::Parser;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Parser)]
#[command(name = "mirage", about = "Swagger 2.0 mock API server")]
struct Cli {
    /// Path to Swagger 2.0 spec file (optional -- use admin UI to import)
    spec: Option<PathBuf>,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3000)]
    port: u16,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db: server::Db = Arc::new(Mutex::new(conn));

    let registry = Arc::new(RwLock::new(server::RouteRegistry::new()));

    // If spec provided, auto-import and configure
    if let Some(spec_path) = &cli.spec {
        let mut spec = parser::SwaggerSpec::from_file(spec_path.to_str().unwrap()).unwrap();
        spec.resolve_refs();

        // Create tables and seed
        {
            let conn = db.lock().unwrap();
            schema::create_tables(&conn, &spec).unwrap();
            seeder::seed_tables(&conn, &spec, 10).unwrap();
        }

        // Populate registry
        server::populate_registry(&mut registry.write().unwrap(), &spec);
    }

    let state = server::AppState { db, registry };
    let router = server::build_router(state);

    println!("Mirage server running on port {}", cli.port);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .unwrap();
    axum::serve(listener, router).await.unwrap();
}
