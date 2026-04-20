mod composer;
mod entity_graph;
mod parser;
mod recipe;
mod rules;
mod schema;
mod seeder;
mod server;

use clap::{Parser, Subcommand};
use std::collections::HashSet;
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
    /// Manage recipes on a running mirage server
    Recipes(RecipesArgs),
}

#[derive(clap::Args)]
struct RecipesArgs {
    #[command(subcommand)]
    verb: RecipesCommand,
}

#[derive(Subcommand)]
enum RecipesCommand {
    /// List recipes as a JSON array of summaries
    List {
        /// Admin server URL (default: http://localhost:3737, env: MIRAGE_URL)
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Show a single recipe (nested config fields parsed)
    Show {
        /// Recipe id
        id: i64,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Delete a recipe by id
    Delete {
        /// Recipe id
        id: i64,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Clone a recipe by id
    Clone {
        /// Recipe id
        id: i64,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Activate a recipe (applies its spec, endpoints, seeds, rules)
    Activate {
        /// Recipe id
        id: i64,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Create a new recipe from a spec file, endpoints file, and optional config
    Create {
        /// Recipe name
        #[arg(long)]
        name: String,
        /// Path to the Swagger spec file (becomes spec_source)
        #[arg(long)]
        spec_file: PathBuf,
        /// Path to a JSON file containing `[{"method":"...","path":"..."}, ...]`
        #[arg(long)]
        endpoints_file: PathBuf,
        /// Seed count (rows generated per table)
        #[arg(long)]
        seed_count: Option<i64>,
        /// Optional JSON file containing any of
        /// quantity_configs / faker_rules / rules / frozen_rows / custom_lists
        #[arg(long)]
        config_file: Option<PathBuf>,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Import a recipe from an exported-recipe JSON file
    Import {
        /// Path to an exported-recipe JSON file (produced by `recipes export`)
        #[arg(long)]
        file: PathBuf,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Export a recipe to a JSON file or stdout
    Export {
        /// Recipe id
        id: i64,
        /// Optional path to write the exported JSON to (default: stdout)
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
    },
    /// Manage a recipe's parsed config (quantity configs, faker rules, rules, frozen rows, custom lists)
    Config(ConfigArgs),
}

#[derive(clap::Args)]
struct ConfigArgs {
    #[command(subcommand)]
    verb: ConfigCommand,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Replace a recipe's config from a JSON file
    ///
    /// The file must match the body accepted by
    /// `PUT /_api/admin/recipes/:id/config` (an object with any of
    /// quantity_configs / faker_rules / rules / frozen_rows / custom_lists).
    /// The whole config is replaced; there are no partial-patch semantics.
    Apply {
        /// Recipe id
        id: i64,
        /// Path to a JSON file matching the admin API config body
        #[arg(long)]
        file: PathBuf,
        #[arg(long, env = "MIRAGE_URL")]
        url: Option<String>,
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

    // Classify extension-only roots BEFORE resolve_refs (allOf structure is lost after resolution)
    let ext_only_roots = parser::extension_only_roots(&spec);

    spec.resolve_refs();

    let definitions = spec.definitions.as_ref();
    let def_count = definitions.map(|d| d.len()).unwrap_or(0);
    let path_count = spec.paths.len();

    println!("Spec: {spec_path}");
    println!("  Definitions: {def_count}");
    println!("  Paths: {path_count}");

    let mut stub_count = 0usize;
    let mut skipped_count = 0usize;

    if let Some(defs) = definitions {
        let mut names: Vec<&String> = defs.keys().collect();
        names.sort();

        println!();
        for name in &names {
            if ext_only_roots.contains(name.as_str()) {
                skipped_count += 1;
                println!("  [SKIPPED — extension-only root] \"{name}\"");
                continue;
            }

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
    println!("  Skipped (extension-only roots): {skipped_count}");
}

const DEFAULT_MIRAGE_URL: &str = "http://localhost:3737";

fn resolve_base_url(flag: &Option<String>) -> String {
    flag.as_deref().unwrap_or(DEFAULT_MIRAGE_URL).to_string()
}

/// Print `{"error": <msg>}` to stderr and exit 1.
fn emit_err_and_exit(msg: impl Into<String>) -> ! {
    let payload = serde_json::json!({ "error": msg.into() });
    eprintln!("{payload}");
    std::process::exit(1);
}

/// If a response is non-2xx, exit 1 after printing a JSON error object. When
/// the body parses as `{"error": "..."}`, forward the server's message. When
/// it does not, synthesise a generic "HTTP <status>: <body>" message.
async fn ensure_success(resp: reqwest::Response) -> reqwest::Response {
    if resp.status().is_success() {
        return resp;
    }
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    let msg = match serde_json::from_str::<serde_json::Value>(&body_text) {
        Ok(v) => match v.get("error").and_then(|e| e.as_str()) {
            Some(s) => s.to_string(),
            None => format!("HTTP {}: {}", status.as_u16(), body_text),
        },
        Err(_) => {
            if body_text.is_empty() {
                format!("HTTP {}", status.as_u16())
            } else {
                format!("HTTP {}: {}", status.as_u16(), body_text)
            }
        }
    };
    emit_err_and_exit(msg);
}

/// Read a file to a String, or emit `{"error":"..."}` to stderr and exit 1.
fn read_file_or_exit(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| emit_err_and_exit(format!("failed to read {}: {e}", path.display())))
}

/// Parse a JSON string into a `serde_json::Value`, or emit `{"error":"..."}`
/// to stderr and exit 1. `ctx` is a short label used in the error message
/// (e.g. the flag name or file purpose).
fn parse_json_or_exit(s: &str, ctx: &str) -> serde_json::Value {
    serde_json::from_str(s)
        .unwrap_or_else(|e| emit_err_and_exit(format!("invalid JSON in {ctx}: {e}")))
}

/// Take a recipe JSON value (as returned by the admin API) and expand the
/// JSON-string config fields (`selected_endpoints`, `quantity_configs`,
/// `faker_rules`, `rules`, `frozen_rows`, `custom_lists`) into nested JSON
/// values so downstream consumers do not see double-encoded strings.
fn parse_nested_config(recipe: &mut serde_json::Value) {
    const STRING_JSON_FIELDS: &[&str] = &[
        "selected_endpoints",
        "quantity_configs",
        "faker_rules",
        "rules",
        "frozen_rows",
        "custom_lists",
    ];
    let Some(obj) = recipe.as_object_mut() else {
        return;
    };
    for field in STRING_JSON_FIELDS {
        let Some(v) = obj.get_mut(*field) else {
            continue;
        };
        let Some(s) = v.as_str() else {
            continue;
        };
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            *v = parsed;
        }
    }
}

async fn run_recipes(args: &RecipesArgs) {
    let client = reqwest::Client::new();
    match &args.verb {
        RecipesCommand::List { url } => {
            let base = resolve_base_url(url);
            let resp = client
                .get(format!("{base}/_api/admin/recipes"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            let recipes: Vec<serde_json::Value> = resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            let summaries: Vec<serde_json::Value> = recipes
                .iter()
                .map(|r| {
                    let endpoint_count = r
                        .get("selected_endpoints")
                        .and_then(|v| v.as_str())
                        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(s).ok())
                        .map(|v| v.len())
                        .unwrap_or(0);
                    serde_json::json!({
                        "id": r.get("id").cloned().unwrap_or(serde_json::Value::Null),
                        "name": r.get("name").cloned().unwrap_or(serde_json::Value::Null),
                        "seed_count": r.get("seed_count").cloned().unwrap_or(serde_json::Value::Null),
                        "endpoint_count": endpoint_count,
                    })
                })
                .collect();
            println!("{}", serde_json::Value::Array(summaries));
        }
        RecipesCommand::Show { id, url } => {
            let base = resolve_base_url(url);
            let resp = client
                .get(format!("{base}/_api/admin/recipes/{id}"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            let mut recipe: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            parse_nested_config(&mut recipe);
            println!("{recipe}");
        }
        RecipesCommand::Delete { id, url } => {
            let base = resolve_base_url(url);
            let resp = client
                .delete(format!("{base}/_api/admin/recipes/{id}"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            // Drain body (server returns 204 NO_CONTENT with no body).
            let _ = resp.bytes().await;
            let payload = serde_json::json!({ "id": id, "deleted": true });
            println!("{payload}");
        }
        RecipesCommand::Clone { id, url } => {
            let base = resolve_base_url(url);
            let resp = client
                .post(format!("{base}/_api/admin/recipes/{id}/clone"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            let mut recipe: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            parse_nested_config(&mut recipe);
            println!("{recipe}");
        }
        RecipesCommand::Activate { id, url } => {
            let base = resolve_base_url(url);
            // Fetch recipe first: validates existence (404 surfaces here) and
            // lets us echo the name in the activation confirmation.
            let get_resp = client
                .get(format!("{base}/_api/admin/recipes/{id}"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let get_resp = ensure_success(get_resp).await;
            let recipe: serde_json::Value = get_resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            let name = recipe
                .get("name")
                .cloned()
                .unwrap_or(serde_json::Value::Null);

            // Activate.
            let act_resp = client
                .post(format!("{base}/_api/admin/recipes/{id}/activate"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let act_resp = ensure_success(act_resp).await;
            let act_body: serde_json::Value = act_resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));

            let status = act_body
                .get("status")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let endpoints = act_body
                .get("endpoints")
                .cloned()
                .unwrap_or(serde_json::Value::Array(Vec::new()));
            let payload = serde_json::json!({
                "id": id,
                "name": name,
                "status": status,
                "endpoints": endpoints,
            });
            println!("{payload}");
        }
        RecipesCommand::Create {
            name,
            spec_file,
            endpoints_file,
            seed_count,
            config_file,
            url,
        } => {
            let base = resolve_base_url(url);

            let spec_source = read_file_or_exit(spec_file);
            let endpoints_raw = read_file_or_exit(endpoints_file);
            let endpoints = parse_json_or_exit(&endpoints_raw, "--endpoints-file");
            if !endpoints.is_array() {
                emit_err_and_exit(
                    "--endpoints-file must contain a JSON array of {method, path} objects",
                );
            }

            let mut body = serde_json::json!({
                "name": name,
                "spec_source": spec_source,
                "endpoints": endpoints,
            });
            let body_obj = body.as_object_mut().expect("body is an object");
            if let Some(n) = seed_count {
                body_obj.insert("seed_count".to_string(), serde_json::json!(n));
            }
            if let Some(cfg_path) = config_file {
                let cfg_raw = read_file_or_exit(cfg_path);
                let cfg = parse_json_or_exit(&cfg_raw, "--config-file");
                let cfg_obj = cfg.as_object().unwrap_or_else(|| {
                    emit_err_and_exit("--config-file must contain a JSON object")
                });
                const CONFIG_KEYS: &[&str] = &[
                    "quantity_configs",
                    "faker_rules",
                    "rules",
                    "frozen_rows",
                    "custom_lists",
                ];
                for k in CONFIG_KEYS {
                    if let Some(v) = cfg_obj.get(*k) {
                        body_obj.insert((*k).to_string(), v.clone());
                    }
                }
            }

            let resp = client
                .post(format!("{base}/_api/admin/recipes"))
                .json(&body)
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            let recipe: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            println!("{recipe}");
        }
        RecipesCommand::Import { file, url } => {
            let base = resolve_base_url(url);
            let raw = read_file_or_exit(file);
            let body = parse_json_or_exit(&raw, "--file");
            let resp = client
                .post(format!("{base}/_api/admin/recipes/import"))
                .json(&body)
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            let recipe: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            println!("{recipe}");
        }
        RecipesCommand::Export { id, file, url } => {
            let base = resolve_base_url(url);
            let resp = client
                .get(format!("{base}/_api/admin/recipes/{id}/export"))
                .send()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
            let resp = ensure_success(resp).await;
            let body = resp
                .text()
                .await
                .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
            match file {
                Some(path) => {
                    std::fs::write(path, &body).unwrap_or_else(|e| {
                        emit_err_and_exit(format!("failed to write {}: {e}", path.display()))
                    });
                }
                None => {
                    println!("{body}");
                }
            }
        }
        RecipesCommand::Config(cfg_args) => match &cfg_args.verb {
            ConfigCommand::Apply { id, file, url } => {
                let base = resolve_base_url(url);
                let raw = read_file_or_exit(file);
                let body = parse_json_or_exit(&raw, "--file");
                let resp = client
                    .put(format!("{base}/_api/admin/recipes/{id}/config"))
                    .json(&body)
                    .send()
                    .await
                    .unwrap_or_else(|e| emit_err_and_exit(format!("request failed: {e}")));
                let resp = ensure_success(resp).await;
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .unwrap_or_else(|e| emit_err_and_exit(format!("invalid response body: {e}")));
                println!("{body}");
            }
        },
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Handle inspect subcommand
    if let Some(Commands::Inspect { spec }) = &cli.command {
        run_inspect(spec);
        return;
    }

    // Handle recipes subcommand
    if let Some(Commands::Recipes(args)) = &cli.command {
        run_recipes(args).await;
        return;
    }

    // Default: serve mode
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let db: server::Db = Arc::new(Mutex::new(conn));

    // File-backed DB for recipe storage (persists across restarts)
    let recipe_conn = rusqlite::Connection::open("mirage.db").unwrap();
    recipe::init_recipe_db(&recipe_conn).unwrap();
    let recipe_db: server::Db = Arc::new(Mutex::new(recipe_conn));

    let registry = Arc::new(RwLock::new(server::RouteRegistry::new()));

    // If spec provided, auto-import and configure
    if let Some(spec_path) = &cli.spec {
        let mut spec = parser::SwaggerSpec::from_file(spec_path.to_str().unwrap()).unwrap();
        let raw_spec = spec.clone();

        // Build def sets from raw_spec BEFORE resolve_refs()
        let all_ops: Vec<(String, String)> = raw_spec
            .path_operations()
            .iter()
            .map(|(path, method, _)| (path.to_string(), method.to_string()))
            .collect();
        let response_defs = parser::definitions_for_paths(&raw_spec, &all_ops, false);

        // Classify extension-only roots BEFORE resolve_refs (allOf structure is lost after resolution)
        let ext_only_roots = parser::extension_only_roots(&raw_spec);
        let response_defs: HashSet<String> =
            response_defs.difference(&ext_only_roots).cloned().collect();

        // Compute topo order BEFORE resolve_refs — topo_sort_defs reads raw
        // $ref paths, which resolve_refs clears.
        let topo_order = parser::topo_sort_defs(&response_defs, raw_spec.definitions.as_ref());

        spec.resolve_refs();

        // Create tables only for response_defs, seed only response_defs.
        // Seed in topological order so referenced def tables land before parents.
        {
            let conn = db.lock().unwrap();
            schema::create_tables_filtered(&conn, &spec, Some(&response_defs), None).unwrap();
            seeder::seed_tables_filtered(
                &conn,
                &spec,
                10,
                Some(&response_defs),
                None,
                None,
                Some(&topo_order),
            )
            .unwrap();
        }

        // Populate registry
        server::populate_registry(&mut registry.write().unwrap(), &spec, &raw_spec);

        // Populate document store using composer with default configs
        let entity_graph = entity_graph::build_entity_graph(&raw_spec, &all_ops);
        let no_faker_rules = composer::FakerRules::new();
        let no_recipe_rules: Vec<rules::Rule> = Vec::new();
        let mut quantities = composer::QuantityConfigs::new();
        for def_name in &response_defs {
            quantities.insert(
                def_name.clone(),
                composer::QuantityConfig { min: 10, max: 10 },
            );
        }
        let all_endpoints: Vec<server::EndpointInfo> = raw_spec
            .path_operations()
            .iter()
            .map(|(path, method, _)| server::EndpointInfo {
                method: method.to_string(),
                path: path.to_string(),
            })
            .collect();
        let composed = {
            let conn = db.lock().unwrap();
            composer::compose_documents(
                &spec,
                &raw_spec,
                &entity_graph,
                &quantities,
                &all_endpoints,
                &no_faker_rules,
                &no_recipe_rules,
                &conn,
            )
        };
        seeder::insert_composed_rows(&db.lock().unwrap(), &composed).unwrap();
    }

    let log: server::RequestLog = Arc::new(Mutex::new(Vec::new()));
    let state = server::AppState {
        db,
        registry,
        log,
        recipe_db,
    };
    let router = server::build_router(state);

    println!("Mirage server running on port {}", cli.port);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", cli.port))
        .await
        .unwrap();
    axum::serve(listener, router).await.unwrap();
}
