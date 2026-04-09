# Mirage Command Reference

Mirage is a Swagger 2.0 mock API server. It reads a spec, generates an in-memory SQLite database seeded with fake data, and serves REST endpoints that respond to GET, POST, and DELETE requests. All state is ephemeral — the database resets on restart.

---

## CLI Reference

```
mirage [OPTIONS] [SPEC]
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
