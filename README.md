# Mirage

Swagger 2.0 mock API server. Feed it a spec, get live CRUD endpoints backed by SQLite with realistic fake data.

## Features

- **Spec parsing** -- full `$ref` resolution and `allOf` merging
- **Automatic table generation** -- SQLite tables created from Swagger definitions
- **Smart fake data** -- 40+ faker strategies with layered resolution: `x-faker` > format > name heuristic > type fallback
- **Live CRUD endpoints** -- GET collection, GET by ID, POST create, DELETE
- **Implicit referential integrity** -- nested `$ref` samples are drawn from each definition's SQLite backing table at compose time
- **Constraint rules** -- bounded ranges, enum choices, constants, regex patterns, and cross-field compares (`gt`/`lt`/`eq`/...) applied during seeding
- **Custom lists** -- named string pools usable from faker rules; shadow built-in strategies that share their name
- **Admin UI** -- SolidJS browser wizard with recipe management, schema browser with optional endpoint-edge layer, request log
- **Recipe persistence** -- saved recipes (endpoints, per-table seed counts, faker rules, custom lists, constraint rules, frozen rows) survive restarts, SQLite-backed
- **Frozen rows** -- exact rows that are re-inserted on every activate / reset
- **Recipes CLI** -- `mirage recipes ...` thin client over the admin HTTP API: list/show/create/clone/activate/reset/export/import/config-apply
- **`recipes learn`** -- deterministic, LLM-free rule synthesizer driven by sample JSONL data
- **`inspect` subcommand** -- spec diagnostics without starting a server
- **Single binary** -- UI embedded via rust-embed

## Quick Start

### Build

```bash
# Build the UI first
cd ui && pnpm install && pnpm build && cd ..

# Build the server
cargo build --release
```

The binary is at `./target/release/mirage`.

### Run

```bash
# CLI mode: parse a spec and serve immediately
mirage petstore.yaml

# Admin UI mode: launch the browser wizard
mirage
```

The server starts on port 3737 by default. Override with `-p`:

```bash
mirage -p 8080 petstore.yaml
```

## Usage

### CLI Mode

Pass a Swagger 2.0 spec file. Mirage creates tables, seeds fake data, and starts serving mock endpoints.

```bash
mirage path/to/swagger.json
```

```
Mirage server running on port 3737
```

Endpoints follow the paths defined in your spec:

```bash
curl http://localhost:3737/pets        # GET collection
curl http://localhost:3737/pets/1      # GET single
curl -X POST http://localhost:3737/pets -d '{"name":"Rex"}'  # POST create
curl -X DELETE http://localhost:3737/pets/1                    # DELETE
```

### Admin UI Mode

Run with no arguments to launch the admin interface at [http://localhost:3737/_admin/](http://localhost:3737/_admin/).

```bash
mirage
```

Upload specs, configure recipes, browse schemas, and monitor requests from the browser.

### Inspect

Diagnose a spec without starting the server:

```bash
mirage inspect petstore.yaml
```

### Recipes

Manage saved recipes from the CLI against a running server:

```bash
mirage recipes list
mirage recipes activate 7
mirage recipes reset
mirage recipes learn --id 7 --def Pet --file pets.jsonl
```

The full subcommand tree (`create`, `clone`, `import`, `export`, `config apply`, …) is documented in [Commands and API > Recipes CLI](docs/user/commands.md#recipes-cli).

## Documentation

| Topic | Description |
|-------|-------------|
| [Getting Started](docs/user/getting-started.md) | First time setup, building, running |
| [Commands and API](docs/user/commands.md) | CLI flags, admin API reference, mock endpoint behavior |
| [Architecture](docs/dev/architecture.md) | Module relationships and data flow |
| [Contributing](docs/dev/contributing.md) | Adding features, running tests, code style |

## License

[MIT](LICENSE)
