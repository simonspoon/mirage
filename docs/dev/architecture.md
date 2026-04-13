# Architecture

Mirage is a Swagger 2.0 mock API server. It parses a spec, generates SQLite tables from definitions, seeds fake data, and serves dynamic CRUD endpoints — all in-memory with no external dependencies.

## Data Flow

```
Swagger 2.0 spec (JSON/YAML)
    |
    v
parser.rs       Parse spec, resolve $ref pointers
    |
    v
schema.rs       Generate CREATE TABLE DDL from definitions
    |
    v
                 +--------- rules.rs ------------+
                 | validate constraint rules     |
                 +-------------------------------+
    |
    v
seeder.rs       Generate fake rows with rule overrides + compare-repair,
                INSERT into SQLite tables
composer.rs     Parallel path: build shared entity pools and compose JSON
                response documents; same rule application as seeder
    |
    v
server.rs       Dynamic route matching, CRUD handlers, admin API,
                recipe persistence
    |
    v
HTTP responses (JSON)
```

## Modules

### `parser.rs`

Deserializes Swagger 2.0 specs (JSON or YAML via `serde_yaml`). Resolves `$ref` pointers by walking all schema objects and replacing references with the target definition's fields. Entry point: `SwaggerSpec::from_file()` followed by `spec.resolve_refs()`.

### `schema.rs`

Converts Swagger definitions into SQLite DDL. Each definition becomes a table; each property becomes a column. Type mapping is intentionally simple — objects and arrays are stored as JSON `TEXT`.

### `seeder.rs`

Generates fake data for each table (the **SQLite row path**). Uses field name heuristics (e.g., "name" gets a person name, "email" gets an email pattern), respects `enum` constraints from the spec, and threads recipe rules through a three-pass `seed_table`: generate → compare-repair → bind. Nested objects and arrays are serialized to JSON strings.

### `composer.rs`

Document-based generator (the **JSON response path**) used when a recipe configures shared entity pools or when responses need full structured documents (not flat table rows). Builds `DocumentStore` maps of shared pool entities, composes documents for each endpoint response via `compose_documents`, and applies the same constraint rules as the seeder (field-level pre-pass + compare-repair post-pass).

### `rules.rs`

The constraint-rules subsystem. Defines the `Rule` enum (five variants: `Range`, `Choice`, `Const`, `Pattern`, `Compare`) and `CompareOp` (`eq`/`neq`/`gt`/`gte`/`lt`/`lte`, numeric AND string). Responsibilities:

- **Parse** `parse_rules()` — deserialize a JSON array of rules from recipe storage
- **Validate** `validate_rules()` — reject duplicate field-level rules, compare cycles (three-color DFS), compare self-loops, cross-definition compares, unknown field references, rule/type mismatches, invalid regexes, empty choice lists, Range with min>max
- **Apply (field-level)** `generate_for_field_rule()` / `generate_for_pattern()` (via `rand_regex` with `max_repeat=100`) — resolve Range/Choice/Const/Pattern rules before falling through to the x-faker / format / heuristic / type layers
- **Apply (cross-field)** `apply_compare_rules()` + `repair_left()` — in-place row repair after initial generation, i64-preserving for ints, all-op repair for strings

The same rule machinery is used by both `seeder::seed_table` (SQLite rows) and `composer::generate_pools` / `compose_documents` (JSON documents).

### `recipe.rs`

SQLite-backed recipe persistence. A recipe bundles everything a user configures for a spec: selected endpoints, seed count, shared pool sizes, quantity configs, faker rules, and constraint rules. Stored as JSON-string columns in a `recipes` table. Idempotent `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`-style migrations so existing databases pick up new columns (including `rules TEXT NOT NULL DEFAULT '[]'`) without a schema reset. CRUD helpers: `create_recipe`, `list_recipes`, `get_recipe`, `update_recipe`, `update_recipe_config`, `delete_recipe`.

### `entity_graph.rs`

Builds a graph of a spec's definitions for the admin UI's visualization. Nodes are definition names; edges are `$ref` relationships; roots are the definitions that appear directly in endpoint responses; shared entities are definitions referenced by more than one parent. Also surfaces scalar property metadata (name, type, format) for faker-rule authoring and array property metadata for shared-pool configuration.

### `server.rs`

Axum-based HTTP server with two responsibilities:

1. **Admin API** — import specs, configure endpoints, manage recipes (CRUD + activate + export/import), browse tables and the entity graph, serve the embedded SolidJS UI
2. **Mock API** — a single catch-all handler that consults `RouteRegistry` to dispatch GET/POST/DELETE requests to the right table

## Key Types

| Type | Module | Purpose |
|------|--------|---------|
| `SwaggerSpec` | parser | Top-level spec: info, paths, definitions |
| `PathItem` | parser | Operations for a single path (get/post/put/delete/patch) |
| `Operation` | parser | Single HTTP operation with parameters and responses |
| `SchemaObject` | parser | Schema definition: type, properties, items, enum, $ref |
| `RouteRegistry` | server | Runtime state: active routes, spec info, endpoints |
| `RouteEntry` | server | Single route: method, pattern, table name, has_path_param |
| `AppState` | server | Shared state: `Db` + `Registry` |
| `EndpointInfo` | server | Serializable endpoint (method + path) for admin API |
| `SpecInfo` | server | Spec title + version for admin UI display |

## Dynamic Routing

Routes are not registered as axum routes. Instead, `build_router` registers a single catch-all:

```rust
.route("/{*path}", any(catch_all_handler))
```

On each request, `catch_all_handler`:

1. Reads `RouteRegistry` (via `Arc<RwLock>`)
2. Calls `match_route()` to find a matching `RouteEntry` by comparing path segments and method
3. Dispatches to the appropriate handler function:

| Match | Handler |
|-------|---------|
| GET, no path param | `get_collection` — `SELECT * FROM "table"` |
| GET, with path param | `get_single` — `SELECT * FROM "table" WHERE rowid = ?` |
| POST | `post_create` — `INSERT INTO "table"`, returns created row |
| DELETE, with path param | `delete_single` — `DELETE FROM "table" WHERE rowid = ?` |

`match_route()` compares request path segments against route pattern segments. Segments wrapped in `{}` match any value and capture it as the path parameter.

## State Management

```rust
pub struct AppState {
    pub db: Db,           // Arc<Mutex<Connection>> — in-memory SQLite
    pub registry: Registry, // Arc<RwLock<RouteRegistry>> — route config
}
```

- **Db**: `Arc<Mutex<rusqlite::Connection>>` — single in-memory SQLite connection, mutex-guarded for concurrent handler access
- **Registry**: `Arc<RwLock<RouteRegistry>>` — read-locked for every request (route matching), write-locked only during import/configure

## Admin UI

The SolidJS frontend is built to `ui/dist/` and embedded into the binary via `rust-embed`:

```rust
#[derive(Embed)]
#[folder = "ui/dist/"]
struct AdminAssets;
```

Served at `/_admin/`. The UI is a three-step wizard:

1. **Idle** — paste a Swagger 2.0 spec (JSON or YAML)
2. **Selecting** — choose which endpoints to activate, set seed count
3. **Running** — shows active endpoints table

Admin API endpoints:

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/_api/admin/spec` | Current spec info (title, version) |
| GET | `/_api/admin/endpoints` | List active endpoints |
| GET | `/_api/admin/definitions` | Definition names from the current spec |
| GET | `/_api/admin/routes` | All routes (active + inactive) from the current spec |
| GET | `/_api/admin/tables` | List seeded SQLite tables |
| GET | `/_api/admin/tables/:name` | Rows for a single seeded table |
| GET | `/_api/admin/log` | Recent request log entries |
| POST | `/_api/admin/import` | Parse and store a spec |
| POST | `/_api/admin/configure` | Create tables, seed data, activate routes |
| GET | `/_api/admin/graph` | Entity graph for the current spec |
| POST | `/_api/admin/graph` | Entity graph for a provided spec |
| GET | `/_api/admin/recipes` | List saved recipes |
| POST | `/_api/admin/recipes` | Create a recipe (validates rules) |
| GET | `/_api/admin/recipes/:id` | Get a single recipe |
| PUT | `/_api/admin/recipes/:id` | Update a recipe (validates rules) |
| DELETE | `/_api/admin/recipes/:id` | Delete a recipe |
| GET | `/_api/admin/recipes/:id/config` | Get parsed pools/quantities/faker rules/constraint rules |
| PUT | `/_api/admin/recipes/:id/config` | Update pools/quantities/faker rules/constraint rules |
| POST | `/_api/admin/recipes/:id/activate` | Apply a recipe and start serving |
| GET | `/_api/admin/recipes/:id/export` | Export a recipe as JSON |
| POST | `/_api/admin/recipes/import` | Import a previously exported recipe |
