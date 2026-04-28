# Architecture

Mirage is a Swagger 2.0 mock API server. It parses a spec, generates SQLite tables from definitions, seeds fake data, and serves dynamic CRUD endpoints — all in-memory with no external dependencies.

## Data Flow

```
Swagger 2.0 spec (JSON/YAML)
    |
    v
parser.rs          Parse spec, resolve $ref pointers
    |
    v
schema.rs          Generate CREATE TABLE DDL from definitions, run
                   CREATE TABLE against in-memory SQLite
    |
    v
                    +--------- rules.rs ------------+
                    | validate constraint rules     |
                    +-------------------------------+
    |
    v
seeder.rs          Seed rows (field-level rule pre-pass + compare-repair),
                   INSERT into SQLite tables. Primes tables before the
                   composer pass overwrites them.
    |
    v
composer.rs        Compose response documents for each endpoint;
                   same rule machinery as the seeder. Nested $ref samples
                   are drawn from each def's SQLite backing table.
                   Produces an in-memory DocumentStore of composed rows.
    |
    v
seeder::insert_composed_rows
                   Truncate each composed table and INSERT the composed
                   rows. After this pass, SQLite is the single source of
                   truth for every active endpoint.
    |
    v
frozen-rows re-apply pass (admin_activate_recipe only)
                   Re-INSERT recipe.frozen_rows over the composed rows
                   so user-pinned values survive recomposition.
    |
    v
server.rs          Dynamic route matching, CRUD handlers, admin API,
                   recipe persistence. All reads go to SQLite.
    |
    v
HTTP responses (JSON)
```

On boot (`main.rs`) the same pipeline runs once with default configs
(no recipe rules, no frozen rows; scalar seed count of 10 per def).
Recipe activation (`admin_activate_recipe` in `server.rs`) runs it
again with the recipe's faker rules, constraint rules, custom lists,
quantity configs, per-definition `seed_counts` (with the scalar
`seed_count` as fallback), and frozen rows. The
`admin_reset_active_recipe` handler (`POST /_api/admin/recipes/reset`)
re-runs the same activation pipeline for the recipe id currently
tracked on `RouteRegistry::active_recipe_id` — process-local state, not
persisted across restarts.

## Modules

### `parser.rs`

Deserializes Swagger 2.0 specs (JSON or YAML via `serde_yaml`). Resolves `$ref` pointers by walking all schema objects and replacing references with the target definition's fields. Entry point: `SwaggerSpec::from_file()` followed by `spec.resolve_refs()`.

### `schema.rs`

Converts Swagger definitions into SQLite DDL. Each definition becomes a table; each property becomes a column. Type mapping is intentionally simple — objects and arrays are stored as JSON `TEXT`.

### `seeder.rs`

Generates fake data for each table (the **SQLite row path**). Uses field name heuristics (e.g., "name" gets a person name, "email" gets an email pattern), respects `enum` constraints from the spec, and threads recipe rules through a three-pass `seed_table`: generate → compare-repair → bind. Nested objects and arrays are serialized to JSON strings.

### `composer.rs`

Document-based generator that produces the canonical row set for every
active endpoint. `compose_documents` assembles a full structured
response document for each endpoint and returns a `DocumentStore`
(the in-memory intermediate type —
`HashMap<String, Vec<serde_json::Value>>` keyed by table name). The
same constraint-rule machinery as the seeder is applied (field-level
pre-pass + compare-repair post-pass). Nested-$ref samples are sourced
implicitly from each def's SQLite backing table at compose time — the
old user-facing `shared_pools` opt-in surface is gone.

The composer does not serve responses directly. The caller
(`server.rs:admin_activate_recipe` and `main.rs` boot) hands the
`DocumentStore` to `seeder::insert_composed_rows`, which truncates the
target tables and INSERTs the composed rows. **SQLite is the single
source of truth at request-time** — every read path, admin or mock,
queries SQLite.

### `learn.rs`

Pure deterministic rule synthesizer for the `mirage recipes learn` CLI
subcommand. Reads JSON sample rows (JSONL or JSON-array), inspects
each property of a target definition, and emits a `LearnPlan` with one
of four decisions per field: apply faker strategy, apply custom list,
apply field-level rule, or skip with a reason. No LLM, no network, no
filesystem access (apart from the dedicated `read_samples` reader
helper). Detection is by regex + simple distinct-count statistics, in
priority order: low-sample skip → const → format-detected faker
(uuid/email/ipv4/date) → choice → custom list (string-only) → numeric
range → too-distinct skip. The `apply_plan` function merges the plan
into an existing recipe config under one of three policies: `merge`
(only fill empty slots), `overwrite` (replace), and `fail` (error on
first conflict). The CLI driver in `main.rs::run_learn` reads samples,
fetches the recipe via the admin API, calls `plan_learn` and
`apply_plan`, and writes the merged config back via
`PUT /_api/admin/recipes/:id/config`.

### `rules.rs`

The constraint-rules subsystem. Defines the `Rule` enum (five variants: `Range`, `Choice`, `Const`, `Pattern`, `Compare`) and `CompareOp` (`eq`/`neq`/`gt`/`gte`/`lt`/`lte`, numeric AND string). Responsibilities:

- **Parse** `parse_rules()` — deserialize a JSON array of rules from recipe storage
- **Validate** `validate_rules()` — reject duplicate field-level rules, compare cycles (three-color DFS), compare self-loops, cross-definition compares, unknown field references, rule/type mismatches, invalid regexes, empty choice lists, Range with min>max
- **Apply (field-level)** `generate_for_field_rule()` / `generate_for_pattern()` (via `rand_regex` with `max_repeat=100`) — resolve Range/Choice/Const/Pattern rules before falling through to the x-faker / format / heuristic / type layers
- **Apply (cross-field)** `apply_compare_rules()` + `repair_left()` — in-place row repair after initial generation, i64-preserving for ints, all-op repair for strings

The same rule machinery is used by both `seeder::seed_table` (SQLite rows) and `composer::compose_documents` (JSON documents).

### `recipe.rs`

SQLite-backed recipe persistence. A recipe bundles everything a user
configures for a spec: selected endpoints, scalar `seed_count`
(fallback default), per-definition `seed_counts` map, quantity configs,
faker rules, custom lists, constraint rules, and **frozen rows**
(user-pinned rows replayed after recomposition). Stored as JSON-string
columns in a `recipes` table. Idempotent `ALTER TABLE ... ADD COLUMN`-style
migrations so existing databases pick up new columns
(`quantity_configs`, `faker_rules`, `rules TEXT NOT NULL DEFAULT '[]'`,
`frozen_rows TEXT NOT NULL DEFAULT '{}'`, `custom_lists TEXT NOT NULL
DEFAULT '{}'`, `seed_counts TEXT NOT NULL DEFAULT '{}'`) without a
schema reset. The legacy `shared_pools` column is retained for schema
back-compat — old rows still carry it but the value is no longer read;
nested-$ref samples are sourced implicitly from each def's SQLite
backing table at compose time. `FrozenRows` is
`HashMap<String, Vec<serde_json::Value>>` keyed by table name.
`seed_counts` parses to `HashMap<String, usize>` keyed by definition
name; defs missing from the map fall back to the scalar `seed_count`
during activation. CRUD helpers: `create_recipe`, `list_recipes`,
`get_recipe`, `update_recipe`, `update_recipe_config`, `delete_recipe`,
plus `find_unique_clone_name` used by the clone endpoint. Migration
note: when the `seed_counts` column is first added by a running server,
existing recipe rows are back-filled by fanning the scalar `seed_count`
onto every definition that appears in the recipe's spec, so per-table
overrides start from a known value rather than empty.

### `entity_graph.rs`

Builds a graph of a spec's definitions for the admin UI's visualization.
Nodes are definition names; edges are `$ref` relationships; roots are
the definitions that appear directly in endpoint responses; shared
entities are definitions referenced by more than one parent. Also
surfaces scalar property metadata (name, type, format) for faker-rule
authoring and array property metadata used by the recipe configure
view.

**Virtual roots** — endpoints whose primary response shape is *not* a
named definition (primitive values, primitive arrays, freeform
objects, empty responses) get no definition node but are still tracked
on `EntityGraph.virtual_roots` as `VirtualRoot { endpoint, shape }` so
the admin UI can surface them. Virtual roots are distinct from
extension-only roots (definitions that only appear as `allOf` bases
and are never used as a root directly). Construction lives at
`src/entity_graph.rs:37` (field), `:203` (init), `:275` (emit).

**Endpoint edges** — `EntityGraph.endpoint_edges` is a deterministic
list of `EndpointEdge { endpoint, target_def, direction }` records
emitted alongside the regular `$ref` edges. Each edge ties a real
endpoint pseudo-node to the definition referenced by its body
parameter (`direction = "input"`) or its primary 2xx response
(`direction = "output"`). Endpoints whose primary response is a virtual
root contribute no edge. Edges to extension-only roots and edges to
definitions absent from `nodes` are filtered out before emit. The
admin UI hides endpoint pseudo-nodes by default and only renders them
when the **Endpoints** layer toggle is on; with the toggle off, the
graph is definition-only.

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

The SolidJS frontend is built to `ui/dist/` and embedded into the
binary via `rust-embed`:

```rust
#[derive(Embed)]
#[folder = "ui/dist/"]
struct AdminAssets;
```

Served at `/_admin/`. The UI is a multi-page single-page app with a
left sidebar for navigation:

- **Dashboard** — import a Swagger 2.0 spec (JSON or YAML), view spec
  info, browse seeded tables, inspect the request log.
- **Schemas** — entity-graph visualization of the current spec.
  Definition nodes, `$ref` edges, response/body roots, shared entities,
  scalar and array property metadata, and virtual roots (endpoints
  whose responses are not a named definition). Layout is rendered by
  `computeDagrePositions` in `ui/src/dagreLayout.ts` and called
  unconditionally from `ui/src/index.tsx`; **Dagre is the sole layout
  engine**. The legacy custom layout in `ui/src/dagLayout.ts` is
  retained only for unit-test coverage of helpers and shared types —
  it is never invoked from the render path. The page exposes an
  **Endpoints** layer toggle (default OFF, signal
  `endpointLayerOn` in `ui/src/index.tsx`) that appends endpoint
  pseudo-nodes plus directed input/output edges from
  `EntityGraph.endpoint_edges` (assembled by `appendEndpointPseudoNodes`).
  When OFF the graph is definition-only.
- **Recipes** — list, create, rename, clone, delete, import, export,
  and activate recipes.
- **Recipe configure wizard** — per-recipe editor. A unified Properties
  table per endpoint/table block exposes inline controls for each
  property row (faker rule, array quantity, constraint-rule chips).
  Shared-pool controls live inline in the table-block header.

Admin API endpoints:

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/_api/admin/spec` | Current spec info (title, version) |
| GET | `/_api/admin/endpoints` | List active endpoints |
| GET | `/_api/admin/definitions` | Definition names from the current spec |
| GET | `/_api/admin/routes` | All routes (active + inactive) from the current spec |
| GET | `/_api/admin/tables` | List seeded SQLite tables |
| GET | `/_api/admin/tables/:name` | Rows for a single seeded table |
| PUT | `/_api/admin/tables/:name/:rowid` | Update a single row in a seeded table |
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
| POST | `/_api/admin/recipes/reset` | Re-run activation for the recipe currently active on this process. 409 when none. |
| GET | `/_api/admin/recipes/:id/export` | Export a recipe as JSON |
| POST | `/_api/admin/recipes/:id/clone` | Clone a recipe |
| POST | `/_api/admin/recipes/import` | Import a previously exported recipe |
