# Mirage Command Reference

Mirage is a Swagger 2.0 mock API server. It reads a spec, generates an in-memory SQLite database seeded with fake data, and serves REST endpoints that respond to GET, POST, and DELETE requests. All state is ephemeral — the database resets on restart.

---

## CLI Reference

```
mirage [OPTIONS] [SPEC]
mirage inspect <SPEC>
```

### Arguments

| Argument | Type | Required | Description |
|---|---|---|---|
| `SPEC` | file path | No | Path to a Swagger 2.0 spec file (YAML or JSON). If omitted, start the server without routes and use the admin UI to import a spec at runtime. |

### Options

| Flag | Short | Default | Description |
|---|---|---|---|
| `--port` | `-p` | `3737` | TCP port to listen on. |
| `--help` | | | Print help and exit. |

### Examples

Start on the default port with no spec loaded:

```bash
mirage
```

Start with a spec file, auto-imported and seeded with 10 rows per table:

```bash
mirage petstore.yaml
```

Listen on a custom port:

```bash
mirage --port 8080 petstore.yaml
mirage -p 8080 petstore.yaml
```

When a spec file is provided at startup, mirage:

1. Parses the spec and resolves `$ref` references.
2. Creates SQLite tables from the spec's definitions.
3. Seeds each table with 10 fake rows.
4. Registers all routes from the spec immediately — no wizard step required.

### `mirage inspect <SPEC>`

Parses a Swagger spec file and prints a diagnostic summary without starting the server. Useful for auditing a spec before loading it.

| Argument | Type | Required | Description |
|---|---|---|---|
| `SPEC` | file path | Yes | Path to the Swagger 2.0 spec file. |

The output lists each definition as one of:

- `[TABLE] "Name" -- N columns` — a concrete definition that would be materialised as a SQLite table.
- `[STUB] "Name" -- 1 column (likely allOf or empty)` — a definition with no useful columns (empty body or pure `allOf` wrapper).
- `[SKIPPED — extension-only root] "Name"` — a base definition whose only usage is via `allOf`, never referenced directly from a response.

Column names that collide with SQL reserved words are flagged with `WARNING:` lines. A trailing summary reports the stub and skipped counts.

```bash
mirage inspect petstore.yaml
```

---

## Admin UI

```
http://localhost:3737/_admin/
```

The admin UI is a SolidJS single-page application embedded in the binary. It provides a three-step wizard for importing and configuring the mock server at runtime. Navigating to `/_admin` (without trailing slash) redirects permanently to `/_admin/`.

### Step 1: Idle — Import Spec

The initial screen shows a textarea. Paste the full contents of a Swagger 2.0 spec (YAML or JSON) and click **Import Spec**.

The UI posts the raw spec text to `/_api/admin/import`. On success it advances to Step 2.

On load, the UI checks `/_api/admin/spec` and `/_api/admin/endpoints`. If a spec is already active (loaded via CLI or a previous session), it skips directly to Step 3.

### Step 2: Selecting — Choose Endpoints and Seed Count

After a successful import, the UI displays every operation found in the spec as a checkbox list. All endpoints are checked by default. Uncheck any you do not want the mock server to serve.

A **Seed count** field (default: 10, range 1-100) controls how many fake rows to generate per table.

Click **Start Mock Server** to post the selection to `/_api/admin/configure`. The server drops and recreates all tables, seeds them, and activates the selected routes.

### Step 3: Running — Active Endpoints

Once configured, the UI shows a table of all active endpoints (method and path). This includes any collection GET routes that were auto-registered by the server (see [Auto-registered collection routes](#auto-registered-collection-routes)).

Click **Import New Spec** to return to Step 1. This clears the local UI state only — the server continues serving the previously configured routes until a new `/_api/admin/configure` call is made.

### Recipe configure view

The recipe configure view (opened via **Edit configuration** on any saved recipe) is the authoring surface for the non-endpoint recipe fields: shared pools, per-property quantity, faker strategies, and constraint rules. It is organised as one **table block** per endpoint group, each containing a unified **Properties** list.

**Table block header.** Each block header names the underlying definition (or virtual root) and carries shared-pool controls inline — a toggle to enable pool sharing for this table and a numeric input for pool size. Filter chips on the view let you scope the list to a specific endpoint or table.

**Properties list.** Every property of every response/body definition used by the selected endpoints appears as a single row, regardless of whether it is an array, scalar, or nested reference. Each row carries its controls inline:

- **Faker control** — dropdown selecting the faker strategy for this property (or `default` to fall back to the `x-faker` / format / heuristic pipeline).
- **Array quantity** — `min`/`max` numeric inputs, shown on every row; only take effect for array-typed properties, but are always visible to keep the row shape uniform.
- **Constraint rule chips** — compact chips representing the `range`, `choice`, `const`, `pattern`, and `compare` rules attached to this property. Click a chip to edit; click the `+` chip to add a new rule. Rule validation runs on save (see [Constraint Rules](#constraint-rules)).

Changes are persisted via `PUT /_api/admin/recipes/:id/config`. Clicking **Activate recipe** commits the saved config to the running server via `POST /_api/admin/recipes/:id/activate`, which drops and re-seeds the mock tables, re-applies any `frozen_rows`, and swaps the active route set.

---

## Admin API Reference

All admin API endpoints are under `/_api/admin/`. They are always available regardless of whether a spec has been loaded.

### GET /_api/admin/spec

Returns metadata about the currently loaded spec.

**Request:** No body.

**Response:**

| Field | Type | Description |
|---|---|---|
| `title` | string | The `info.title` value from the spec. |
| `version` | string | The `info.version` value from the spec. |

If no spec is loaded, returns `{"title": "Mirage", "version": "No spec loaded"}` with status 200.

**Example:**

```bash
curl http://localhost:3737/_api/admin/spec
```

```json
{
  "title": "Petstore",
  "version": "1.0.0"
}
```

---

### GET /_api/admin/endpoints

Returns the list of routes currently registered and serving mock traffic.

**Request:** No body.

**Response:** A JSON array of endpoint objects. Empty array if no spec has been configured.

Each element:

| Field | Type | Description |
|---|---|---|
| `method` | string | HTTP method in lowercase (`get`, `post`, `delete`, etc.). |
| `path` | string | Path pattern, e.g. `/pet` or `/pet/{petId}`. |

**Example:**

```bash
curl http://localhost:3737/_api/admin/endpoints
```

```json
[
  {"method": "get",    "path": "/pet"},
  {"method": "get",    "path": "/pet/{petId}"},
  {"method": "post",   "path": "/pet"},
  {"method": "delete", "path": "/pet/{petId}"}
]
```

---

### POST /_api/admin/import

Parses a Swagger 2.0 spec and returns the list of operations it contains. Does not activate any routes or create any database tables — that happens in the configure step.

**Request:**

- Content-Type: `text/plain`
- Body: Raw Swagger 2.0 spec, YAML or JSON.

**Response on success (200):**

| Field | Type | Description |
|---|---|---|
| `spec_info` | object | `{title, version}` extracted from `info`. |
| `endpoints` | array | All operations found in the spec as `{method, path}` pairs. |

**Response on error (400):**

| Field | Type | Description |
|---|---|---|
| `error` | string | YAML/JSON parse error message. |

**Example:**

```bash
curl -X POST http://localhost:3737/_api/admin/import \
  -H "Content-Type: text/plain" \
  --data-binary @petstore.yaml
```

```json
{
  "spec_info": {
    "title": "Petstore",
    "version": "1.0.0"
  },
  "endpoints": [
    {"method": "get",    "path": "/pet/{petId}"},
    {"method": "post",   "path": "/pet"},
    {"method": "delete", "path": "/pet/{petId}"}
  ]
}
```

The imported spec is held in memory and used by the subsequent configure call. Calling import again replaces it.

---

### POST /_api/admin/configure

Activates the mock server. Drops all existing tables, recreates and seeds them from the previously imported spec, and registers the selected routes.

Must be called after a successful `/_api/admin/import`. Returns 400 if no spec has been imported.

**Request:**

- Content-Type: `application/json`
- Body: `ConfigureRequest`

`ConfigureRequest` fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `endpoints` | array | Yes | The subset of endpoints to activate. Each element is `{method, path}`. |
| `seed_count` | integer | No | Number of fake rows to insert per table. Defaults to 10. |

**Response on success (200):**

```json
{"status": "configured"}
```

**Response on error (400):**

```json
{"error": "No spec imported"}
```

**Example — configure with three endpoints and 5 seed rows:**

```bash
curl -X POST http://localhost:3737/_api/admin/configure \
  -H "Content-Type: application/json" \
  -d '{
    "endpoints": [
      {"method": "get",    "path": "/pet/{petId}"},
      {"method": "post",   "path": "/pet"},
      {"method": "delete", "path": "/pet/{petId}"}
    ],
    "seed_count": 5
  }'
```

```json
{"status": "configured"}
```

After this call, `GET /pet`, `GET /pet/{petId}`, `POST /pet`, and `DELETE /pet/{petId}` are all active.

#### Auto-registered collection routes

When you select a path-parameter route such as `GET /pet/{petId}`, mirage automatically registers `GET /pet` (the collection endpoint) if it is not already in your selection. This ensures the collection is always browsable when individual items are. The auto-registered route appears in the response of `/_api/admin/endpoints` after configure.

### Additional admin endpoints

The admin API exposes a number of introspection and utility routes alongside the core `spec`/`endpoints`/`import`/`configure` flow. All are under `/_api/admin/` and are always reachable once the server is running.

| Method | Path | Purpose |
|---|---|---|
| GET | `/_api/admin/definitions` | Returns the raw Swagger `definitions` map from the currently loaded spec (`$ref` references preserved). Used by the admin UI to render schema details. |
| GET | `/_api/admin/routes` | Returns every route currently registered in the router, including catch-alls and auto-registered collection routes. Supersets `/_api/admin/endpoints`, which only reports the spec-derived routes. |
| GET | `/_api/admin/tables` | Returns the list of SQLite tables created from the current spec. |
| GET | `/_api/admin/tables/{name}` | Returns all rows in a single table. 404 if the table does not exist. |
| PUT | `/_api/admin/tables/{name}/{rowid}` | Replaces the contents of a single row, keyed by SQLite `rowid`. The request body is a JSON object of column values; unknown columns are ignored. |
| GET | `/_api/admin/log` | Returns the in-memory request log — every admin API call and mock-traffic request with method, path, status, request body, and response body. Request/response bodies larger than 16 MB are replaced with a `<body too large ...>` sentinel. |
| GET | `/_api/admin/graph` | Returns the entity-relationship graph derived from the currently active spec and route selection. See [Schemas graph](#schemas-graph) below. |
| POST | `/_api/admin/graph` | Same as the GET form, but accepts a `{endpoints: [{method, path}, ...]}` body so the graph can be previewed for a proposed selection before it is activated. |
| GET | `/_api/admin/recipes/{id}/export` | Returns a recipe as a standalone JSON document suitable for checking into version control or sharing between instances. |
| POST | `/_api/admin/recipes/import` | Creates a new recipe from an exported-recipe JSON document. |
| POST | `/_api/admin/recipes/{id}/clone` | Creates a copy of an existing recipe under a new id (and a new, unique name). |

---

## Recipes

A **recipe** is a saved configuration for a spec. It bundles:

- The spec source (raw YAML/JSON text)
- The subset of endpoints to activate
- The per-table seed count
- **Shared entity pools** — sizes of cross-definition shared pools
- **Quantity configs** — min/max collection sizes for array properties
- **Faker rules** — per-field faker strategy overrides
- **Constraint rules** — bounded ranges, choices, constants, patterns, and cross-field compares
- **Frozen rows** — exact table rows that must be re-inserted on every activate

Recipes are persisted to a `mirage.db` SQLite file in the working directory and survive restarts. All recipe fields (except name, spec source, and seed count) are stored as JSON strings.

### Recipe CRUD endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/_api/admin/recipes` | List all saved recipes |
| POST | `/_api/admin/recipes` | Create a new recipe |
| GET | `/_api/admin/recipes/:id` | Get a single recipe |
| PUT | `/_api/admin/recipes/:id` | Update all recipe fields |
| DELETE | `/_api/admin/recipes/:id` | Delete a recipe |
| GET | `/_api/admin/recipes/:id/config` | Get parsed config (pools/quantities/faker/rules/frozen_rows) |
| PUT | `/_api/admin/recipes/:id/config` | Update the parsed config (accepts `frozen_rows` alongside the other config fields) |
| POST | `/_api/admin/recipes/:id/clone` | Clone a recipe, returning the new record |
| POST | `/_api/admin/recipes/:id/activate` | Apply this recipe and start serving traffic |
| GET | `/_api/admin/recipes/:id/export` | Export a recipe as JSON for backup/transfer |
| POST | `/_api/admin/recipes/import` | Import a previously exported recipe |

### POST /_api/admin/recipes

Creates a new recipe. Validates constraint rules at create time (see [Constraint Rules](#constraint-rules) below); returns 400 with `{"error": "Invalid rules: ..."}` on any validation failure.

**Request body:**

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Display name |
| `spec_source` | string | Yes | Raw Swagger spec text (YAML or JSON) |
| `selected_endpoints` | string (JSON array) | Yes | Serialized `[{method, path}, ...]` |
| `seed_count` | integer | No | Rows per table (default 10) |
| `shared_pools` | string (JSON object) | No | `{"DefName": poolSize, ...}` (default `{}`) |
| `quantity_configs` | string (JSON object) | No | `{"DefName.prop": {min, max}, ...}` (default `{}`) |
| `faker_rules` | string (JSON object) | No | `{"DefName": {"prop": "strategy"}}` (default `{}`) |
| `rules` | string (JSON array) | No | Constraint rules (default `[]`) |
| `frozen_rows` | string (JSON object) | No | `{"TableName": [{...row}, ...]}` — pinned rows re-inserted on every activate (default `{}`) |

### Constraint Rules

Rules shape seeded data beyond what faker strategies alone can express. They run BEFORE and AFTER the standard field-generation layers:

- **Field-level rules** (`range`, `choice`, `const`, `pattern`) override per-field generation *before* the `x-faker` → format → heuristic → type fallback layers.
- **Cross-field rules** (`compare`) run in a per-row repair pass *after* initial generation.

Rules are stored as a JSON array of tagged-union objects (serde `snake_case`). The `kind` field selects the variant:

| Kind | Shape | Effect |
|---|---|---|
| `range` | `{"kind":"range","field":"Pet.age","min":0,"max":20}` | Numeric field clamped to `[min, max]` inclusive |
| `choice` | `{"kind":"choice","field":"Pet.status","options":["available","pending","sold"]}` | Field is one of the listed JSON values |
| `const` | `{"kind":"const","field":"Pet.name","value":"Cosmo"}` | Field is always this exact JSON value |
| `pattern` | `{"kind":"pattern","field":"Tag.code","regex":"[A-Z]{3}-[0-9]{4}"}` | Field is a string matching the regex (generated via `rand_regex`) |
| `compare` | `{"kind":"compare","left":"Pet.id","op":"gt","right":50}` | Cross-field predicate; `right` can be a field path or literal |

`CompareOp` variants (all apply to numeric AND string fields):

`eq`, `neq`, `gt`, `gte`, `lt`, `lte`

Field paths use the `DefName.propName` convention (same as faker rules).

**Validation errors** (rejected on create AND update, returned as `400 {"error": "Invalid rules: ..."}`):

- Duplicate field-level rules on the same field
- Compare rules with a cycle in the dependency graph
- Compare rules with a self-loop (`left == right` field path)
- Compare rules spanning different definitions
- Field references that don't exist in the spec
- Rule/type mismatches (e.g. `range` on a string field, `compare gt` on a boolean)
- Pattern regexes that fail to parse
- `choice` rules with an empty options list
- `range` rules where `min > max`

**Example — create a recipe with all five rule kinds:**

```bash
curl -X POST http://localhost:3737/_api/admin/recipes \
  -H "Content-Type: application/json" \
  -d '{
    "name": "petstore bounded",
    "spec_source": "<raw yaml here>",
    "selected_endpoints": "[{\"method\":\"get\",\"path\":\"/pet\"}]",
    "seed_count": 10,
    "rules": "[
      {\"kind\":\"range\",\"field\":\"Pet.id\",\"min\":1,\"max\":999},
      {\"kind\":\"choice\",\"field\":\"Pet.status\",\"options\":[\"available\",\"pending\",\"sold\"]},
      {\"kind\":\"const\",\"field\":\"Pet.name\",\"value\":\"Cosmo\"},
      {\"kind\":\"pattern\",\"field\":\"Tag.name\",\"regex\":\"[A-Z]{3}-[0-9]{4}\"},
      {\"kind\":\"compare\",\"left\":\"Pet.id\",\"op\":\"gt\",\"right\":50}
    ]"
  }'
```

On activate, rules apply to BOTH the seeded SQLite rows (via the seeder) AND any composed JSON response documents (via the composer / shared entity pools).

### Schemas graph

`GET /_api/admin/graph` returns the entity-relationship graph of the currently active spec + selected endpoints. The admin UI renders this graph on the **Schemas** page. Three node categories are distinguished:

- **Roots** — definitions that appear directly as the response (or body) type of at least one selected endpoint. Every selected operation contributes its response definition as a root, along with any `$ref`-typed body parameters.
- **Shared entities** — definitions reachable from more than one root via transitive `$ref` traversal. They are surfaced so that shared-pool configuration can target them explicitly.
- **Virtual roots** — endpoints whose response shape is not a named definition (primitive arrays, loose objects, etc.). They are tracked separately so the UI can still render them as graph nodes even though they are not backed by a definition.

All three sets, plus the edge list, are returned in a single payload; the UI uses Dagre to lay out the graph.

---

## Mock API Behavior

After a spec has been configured, mirage handles requests on the paths defined in that spec. All other paths return 404.

### Route patterns

Routes are matched against the registered path patterns. A segment enclosed in `{braces}` is a wildcard that captures a single path segment. Pattern matching is exact: `/pet/1/photos` does not match `/pet/{petId}` because the segment count differs.

### IDs and the rowid system

Mirage uses SQLite `rowid` as the record identifier. When you POST a new record, the `id` field in the response is the `rowid` assigned by SQLite. GET and DELETE by ID use this same `rowid`. IDs must be integers — passing a non-integer ID to a single-item route returns 400.

### Endpoint reference

| Method | Pattern | Behavior |
|---|---|---|
| GET | `/{resource}` | Returns all rows in the table as a JSON array. Always 200. |
| GET | `/{resource}/{id}` | Returns the single row whose `rowid` equals `id`. 200 on match, 404 if not found, 400 for non-integer `id`. |
| POST | `/{resource}` | Inserts a new row. Only fields that exist as columns in the table are used; unknown fields are silently ignored. Returns 201 with the inserted object plus the assigned `id`. Returns 400 if the body is missing, not a JSON object, or contains no recognized columns. |
| DELETE | `/{resource}/{id}` | Deletes the row whose `rowid` equals `id`. Returns 204 (No Content) on success, 404 if not found, 400 for non-integer `id`. |

Any registered method/path combination that does not match one of the four behaviors above (e.g., PUT or PATCH) returns 405.

### Table name derivation

The table name for a path is derived from its first path segment with the first letter uppercased. `/pet` and `/pet/{petId}` both map to the table `Pet`. `/store/inventory` maps to `Store`. This means all operations on the same first segment share a table.

### Type handling

- JSON strings are stored as SQLite TEXT.
- JSON numbers are stored as INTEGER or REAL depending on whether they have a fractional part.
- JSON booleans are stored as INTEGER (1 for true, 0 for false).
- JSON null is stored as NULL.
- Nested objects and arrays are serialized to a JSON string and stored as TEXT. On read, TEXT values that parse as a JSON object or array are returned as structured JSON rather than a string.

### Status code summary

| Status | Condition |
|---|---|
| 200 | Successful GET. |
| 201 | Successful POST (record created). |
| 204 | Successful DELETE (record removed). |
| 400 | Invalid ID format, missing or malformed request body, no recognized columns in POST body, no spec imported (configure only). |
| 404 | No matching route registered for this method+path, or row not found. |
| 405 | Method not supported for the matched route (e.g., PUT). |
| 500 | Internal database error. |

### Example session

```bash
# List all pets (returns seeded array)
curl http://localhost:3737/pet

# Fetch pet with rowid 1
curl http://localhost:3737/pet/1

# Create a new pet
curl -X POST http://localhost:3737/pet \
  -H "Content-Type: application/json" \
  -d '{"name": "Fido", "status": "available"}'
# -> 201: {"name":"Fido","status":"available","id":11}

# Delete pet with rowid 3
curl -X DELETE http://localhost:3737/pet/3
# -> 204 No Content

# Fetch deleted pet
curl http://localhost:3737/pet/3
# -> 404: {"error":"not found"}
```
