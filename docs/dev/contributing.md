# Contributing

## Prerequisites

- **Rust** edition 2024 (1.85+) — uses let-chain syntax
- **Node 20+ / pnpm 9+** — only if modifying the admin UI

## Building

```bash
cargo build
```

After modifying the admin UI, rebuild assets then force a Rust recompile:

```bash
cd ui && pnpm build
cd .. && touch src/server.rs && cargo build
```

The `touch` is necessary because `rust-embed` embeds `ui/dist/` at compile time and Cargo doesn't track those files as dependencies.

## Running Tests

```bash
# All tests (unit + integration)
cargo test

# Unit tests only
cargo test --lib

# Integration tests only
cargo test --test integration

# Single test by name
cargo test test_e2e_get_single
```

### Integration test pattern

Tests in `tests/integration.rs` spawn the real compiled binary on an ephemeral port, poll until ready, run HTTP assertions, then kill the process on drop. They use `tests/fixtures/petstore.yaml` as the spec.

## Code Style

```bash
cargo fmt          # format
cargo clippy       # lint
```

### SQL table name quoting

All SQL statements **must** double-quote table names. Table names come from Swagger definition keys (e.g., `Order`) which can be SQL reserved words.

```rust
// Correct
format!("SELECT * FROM \"{table}\"")

// Wrong — panics on reserved words like "Order"
format!("SELECT * FROM {table}")
```

## Adding a New Handler

The mock API uses a single catch-all handler, not per-route registration. To add a new HTTP verb (e.g., PUT):

1. Add a handler function in `server.rs` following the existing pattern
2. Extend the match in `catch_all_handler`:

```rust
("put", true) => put_replace(table, db, param_value.unwrap(), body).await,
```

3. Add unit and integration tests

## Adding a New Swagger Feature

The pipeline: **parser types -> schema mapping -> seeder generation -> server handling**.

1. **Parser** (`parser.rs`): Add fields to `SchemaObject` or `Operation`. If the field can appear in a `$ref`-resolved schema, update `resolve_schema` to propagate it.

2. **Schema** (`schema.rs`): Extend `map_type()` if the feature affects column types. Extend `generate_table_sql()` if it affects table structure.

3. **Seeder** (`seeder.rs`): Extend `fake_value_for_field()` to generate appropriate fake data for the new type/format.

4. **Server** (`server.rs`): Modify handler functions if the feature changes response shape or behavior.

## Project Structure

```
src/
  main.rs          CLI parsing, startup wiring
  parser.rs        Swagger 2.0 spec types and $ref resolution
  schema.rs        DDL generation from definitions
  seeder.rs        Fake data generation
  server.rs        Axum router, catch-all handler, admin API
tests/
  integration.rs   E2E tests (spawn real binary)
  fixtures/
    petstore.yaml  Reference spec for all tests
ui/
  src/             SolidJS + Tailwind source
  dist/            Built assets (embedded by rust-embed)
```
