# Mirage

Swagger 2.0 mock API server. Feed it a spec, get live CRUD endpoints backed by SQLite with realistic fake data.

## Features

- **Spec parsing** -- full `$ref` resolution and `allOf` merging
- **Automatic table generation** -- SQLite tables created from Swagger definitions
- **Smart fake data** -- 40+ faker strategies with layered resolution: `x-faker` > format > name heuristic > type fallback
- **Live CRUD endpoints** -- GET collection, GET by ID, POST create, DELETE
- **Shared entity pools** -- cross-definition referential integrity
- **Admin UI** -- SolidJS browser wizard with recipe management, request log, schema browser, entity graph
- **Recipe persistence** -- saved recipes survive restarts (SQLite-backed)
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

## Documentation

| Topic | Description |
|-------|-------------|
| [Getting Started](docs/user/getting-started.md) | First time setup, building, running |
| [Commands and API](docs/user/commands.md) | CLI flags, admin API reference, mock endpoint behavior |
| [Architecture](docs/dev/architecture.md) | Module relationships and data flow |
| [Contributing](docs/dev/contributing.md) | Adding features, running tests, code style |

## License

[MIT](LICENSE)
