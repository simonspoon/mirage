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
seeder.rs       Generate fake rows, INSERT into tables
    |
    v
server.rs       Dynamic route matching, CRUD handlers, admin API
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

Generates fake data for each table. Uses field name heuristics (e.g., "name" gets a person name, "email" gets an email pattern) and respects `enum` constraints from the spec. Nested objects and arrays are serialized to JSON strings.

### `server.rs`

Axum-based HTTP server with two responsibilities:

1. **Admin API** — import specs, configure endpoints, serve the embedded SolidJS UI
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
| POST | `/_api/admin/import` | Parse and store a spec |
| POST | `/_api/admin/configure` | Create tables, seed data, activate routes |
