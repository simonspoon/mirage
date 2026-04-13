# Getting Started

Mirage is a Swagger 2.0 mock API server. Give it a spec and it creates an in-memory SQLite database with matching tables, seeds fake data, and serves mock CRUD endpoints. No backend code required.

## Build

```bash
cargo build --release
# binary: ./target/release/mirage
```

## Two Ways to Use

### CLI Mode

Pass a spec file directly. Mirage imports it, creates tables, seeds 10 rows per definition, and starts serving immediately.

```bash
mirage path/to/swagger.json
```

```
Mirage server running on port 3737
```

### Admin UI Mode

Run with no arguments, then use the browser wizard.

```bash
mirage
```

Open `http://localhost:3737/_admin/` and:

1. Paste your Swagger 2.0 spec (JSON or YAML), click **Import Spec**
2. Select which endpoints to activate, set seed count
3. Configure shared entity pools, faker strategies, and constraint rules
4. Name and save as a **recipe**, then **Activate** — the server drops tables, reseeds with your config, and starts serving

Recipes are stored in the embedded database and survive restarts. See [Commands and API > Recipes](commands.md#recipes) for the full recipe API, including the **constraint rules** system for bounded ranges, choices, constants, regex patterns, and cross-field compares.

## Port

Default is `3737`. Override with `--port` / `-p`:

```bash
mirage swagger.json --port 8080
```

## Try It

Assuming a spec with a Pet resource:

```bash
# List all
curl http://localhost:3737/pet

# Get one
curl http://localhost:3737/pet/1

# Create
curl -X POST http://localhost:3737/pet \
  -H "Content-Type: application/json" \
  -d '{"name": "Rex", "status": "available"}'

# Delete
curl -X DELETE http://localhost:3737/pet/1
```

| Task | Command |
|------|---------|
| CLI mode | `mirage swagger.json` |
| Custom port | `mirage swagger.json -p 8080` |
| Admin UI mode | `mirage` then open `http://localhost:3737/_admin/` |
