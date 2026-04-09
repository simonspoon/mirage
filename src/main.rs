mod parser;
mod schema;
mod seeder;
mod server;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Parser)]
#[command(name = "mirage", about = "Swagger 2.0 mock API server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to Swagger 2.0 spec file (optional -- use admin UI to import)
    spec: Option<PathBuf>,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3737)]
    port: u16,
}

#[derive(Subcommand)]
enum Commands {
    /// Inspect a Swagger spec file and show diagnostic info
    Inspect {
        /// Path to the Swagger spec file
        spec: String,
    },
}

/// SQL reserved words that may cause issues as column names.
const SQL_RESERVED: &[&str] = &[
    "abort",
    "action",
    "add",
    "after",
    "all",
    "alter",
    "always",
    "analyze",
    "and",
    "as",
    "asc",
    "attach",
    "autoincrement",
    "before",
    "begin",
    "between",
    "by",
    "cascade",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "commit",
    "conflict",
    "constraint",
    "create",
    "cross",
    "current",
    "current_date",
    "current_time",
    "current_timestamp",
    "database",
    "default",
    "deferrable",
    "deferred",
    "delete",
    "desc",
    "detach",
    "distinct",
    "do",
    "drop",
    "each",
    "else",
    "end",
    "escape",
    "except",
    "exclude",
    "exclusive",
    "exists",
    "explain",
    "fail",
    "filter",
    "first",
    "following",
    "for",
    "foreign",
    "from",
    "full",
    "generated",
    "glob",
    "group",
    "groups",
    "having",
    "if",
    "ignore",
    "immediate",
    "in",
    "index",
    "indexed",
    "initially",
    "inner",
    "insert",
    "instead",
    "intersect",
    "into",
    "is",
    "isnull",
    "join",
    "key",
    "last",
    "left",
    "like",
    "limit",
    "match",
    "materialized",
    "natural",
    "no",
    "not",
    "nothing",
    "notnull",
    "null",
    "nulls",
    "of",
    "offset",
    "on",
    "or",
    "order",
    "others",
    "outer",
    "over",
    "partition",
    "plan",
    "pragma",
    "preceding",
    "primary",
    "query",
    "raise",
    "range",
    "recursive",
    "references",
    "regexp",
    "reindex",
    "release",
    "rename",
    "replace",
    "restrict",
    "returning",
    "right",
    "rollback",
    "row",
    "rows",
    "savepoint",
    "select",
    "set",
    "table",
    "temp",
    "temporary",
    "then",
    "ties",
    "to",
    "transaction",
    "trigger",
    "unbounded",
    "union",
    "unique",
    "update",
    "using",
    "vacuum",
    "value",
    "values",
    "view",
    "virtual",
    "when",
    "where",
    "window",
    "with",
    "without",
];

fn run_inspect(spec_path: &str) {
    let mut spec = match parser::SwaggerSpec::from_file(spec_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error parsing spec: {e}");
            std::process::exit(1);
        }
    };
    spec.resolve_refs();

    let definitions = spec.definitions.as_ref();
    let def_count = definitions.map(|d| d.len()).unwrap_or(0);
    let path_count = spec.paths.len();

    println!("Spec: {spec_path}");
    println!("  Definitions: {def_count}");
    println!("  Paths: {path_count}");

    let mut stub_count = 0usize;

    if let Some(defs) = definitions {
        let mut names: Vec<&String> = defs.keys().collect();
        names.sort();

        println!();
        for name in &names {
            let schema = &defs[*name];
            let props = schema.properties.as_ref();
            let col_count = props.map(|p| p.len()).unwrap_or(0);

            if col_count == 0
                || (col_count == 1 && props.map(|p| p.contains_key("id")).unwrap_or(false))
            {
                stub_count += 1;
                println!("  [STUB] \"{name}\" -- 1 column (likely allOf or empty)");
            } else {
                println!("  [TABLE] \"{name}\" -- {col_count} columns");
            }

            // Flag reserved-word columns
            if let Some(p) = props {
                let mut reserved_cols: Vec<&String> = p
                    .keys()
                    .filter(|k| SQL_RESERVED.contains(&k.to_lowercase().as_str()))
                    .collect();
                reserved_cols.sort();
                for col in &reserved_cols {
                    println!("    WARNING: \"{col}\" is a SQL reserved word");
                }
            }
        }
    }

    println!();
    println!("  Stubs (allOf/empty): {stub_count}");
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Handle inspect subcommand
    if let Some(Commands::Inspect { spec }) = &cli.command {
        run_inspect(spec);
        return;
    }

    // Default: serve mode
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

    let log: server::RequestLog = Arc::new(Mutex::new(Vec::new()));
    let state = server::AppState { db, registry, log };
    let router = server::build_router(state);

    println!("Mirage server running on port {}", cli.port);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .unwrap();
    axum::serve(listener, router).await.unwrap();
}
