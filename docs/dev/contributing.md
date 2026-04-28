# Contributing

## Prerequisites

- **Rust** edition 2024 (1.85+) — uses let-chain syntax
- **Node 20+ / pnpm 9+** — only if modifying the admin UI

`reqwest` is configured with `rustls-tls` (no native-tls) so cross-compile
builds do not require OpenSSL on the build host.

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

### QA shell scripts

Scenario-level scripts live alongside `tests/integration.rs` and drive the
running binary or the admin UI for end-to-end checks:

- `tests/qa-wizard.sh` — walks the admin UI wizard end-to-end.
- `tests/qa-per-table-seed.sh` — exercises the per-definition `seed_counts` map.
- `tests/qa-migrate-seed-counts.sh` — verifies the `seed_count` → `seed_counts` migration that fans the scalar count onto every spec definition.
- `tests/qa-endpoint-layer.sh` — drives the Schemas-page endpoint-layer toggle.
- `tests/qa-tsrs-toggle-roundtrip.sh` — round-trip on the schema-graph toggle.

The QA scripts are not run by `cargo test`; invoke them directly while a
mirage server is running.

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

3. **Seeder** (`seeder.rs`): Extend `fake_value_for_field()` to generate appropriate fake data for the new type/format. If document-based generation is affected, also update `composer.rs`.

4. **Server** (`server.rs`): Modify handler functions if the feature changes response shape or behavior.

## Adding a New Rule Kind

Constraint rules live in `src/rules.rs`. To add a new rule kind:

1. Add a variant to the `Rule` enum (serde tag = `"kind"`, rename_all `snake_case`).
2. Update `Rule::target_field()` and `Rule::is_field_level()` / `is_compare()` helpers.
3. Extend `validate_rules()` to reject invalid instances of the new kind (type mismatches, cycles, bad inputs).
4. If field-level: handle it in `generate_for_field_rule()` and ensure `fake_value_for_field_layered()` consults the rule map first.
5. If cross-field (like Compare): add an apply pass in `apply_compare_rules()` / `repair_left()` invoked after row generation.
6. Thread the new variant through both the SQLite seed path (`seeder::seed_table`) and the composer path (`composer::compose_documents`).
7. Add unit tests per variant + conflict/cycle/type-mismatch cases.
8. Update the UI `RuleEditor` in `ui/src/index.tsx` so users can author the new kind.

## CLI Subcommands

The `mirage` binary has three entry paths:

- **Default (serve)**: `mirage <spec>` boots the mock server on the configured port.
- **Inspect**: `mirage inspect <spec>` parses the spec and prints diagnostic info (definitions, roots, extension-only roots, virtual roots) without starting a server. Useful for debugging classification and ref-resolution issues.
- **Recipes**: `mirage recipes <subcommand>` is a thin reqwest client over the admin HTTP API (`MIRAGE_URL`, default `http://localhost:3737`). Subcommands: `list`, `show`, `create`, `delete`, `clone`, `activate`, `reset`, `import`, `export`, `config apply`, `learn`. See [Commands and API > Recipes CLI](../user/commands.md#recipes-cli) for usage.

CLI parsing lives in `src/main.rs`:

- The `Commands` enum (around line 30) declares the top-level subcommands via `clap`'s `#[derive(Subcommand)]`.
- `RecipesCommand` (around line 47) declares the recipes subtree, with the per-verb argument shapes inline.
- `run_inspect` implements the inspect path; `run_recipes` dispatches all `mirage recipes ...` verbs; `run_learn` drives the sample-synthesizer flow.

When adding a new flag or option, update the relevant entry path and any shared parser/entity-graph setup — the three paths share spec loading but diverge after that.

### Adding a new `mirage recipes` subcommand

1. Add a variant to `RecipesCommand` in `src/main.rs` with the per-verb argument shape.
2. Add a match arm in `run_recipes` that builds the right HTTP request via `reqwest` and prints the response.
3. If the verb writes server state, add a corresponding admin-API handler under `src/server.rs` and register it in `build_router`.
4. Update `docs/user/commands.md` with the verb, its flags, and the underlying admin-API call.

## Project Structure

```
src/
  main.rs          CLI parsing, startup wiring
  parser.rs        Swagger 2.0 spec types and $ref resolution
  schema.rs        DDL generation from definitions
  seeder.rs        Fake data generation (SQLite row path)
  composer.rs      Document-based generation; nested $ref samples drawn from SQLite backing tables
  rules.rs         Constraint rule enum, validation, field-level + compare-repair passes
  recipe.rs        Recipe storage: endpoints, seed_counts, faker rules, custom lists, constraint rules, frozen rows
  entity_graph.rs  Definition graph: nodes, edges, roots, shared entities, endpoint pseudo-edges
  learn.rs         Sample-driven deterministic rule synthesizer (mirage recipes learn)
  server.rs        Axum router, catch-all handler, admin API
tests/
  integration.rs   E2E tests (spawn real binary)
  fixtures/
    petstore.yaml  Reference spec for most tests
    mega.yaml      Larger spec covering edge cases (allOf, virtual roots)
    input_only.yaml  Spec where the only $ref usage is a body parameter
    pets.jsonl     Sample data fixture for the recipes-learn flow
ui/
  src/             SolidJS + Tailwind source
    dagreLayout.ts   Live schemas-graph layout engine (Dagre-backed, used by render path)
    dagLayout.ts     Legacy barycenter-sweep layout; retained for unit-test coverage only
  dist/            Built assets (embedded by rust-embed)
```

`ui/src/dagreLayout.ts` exports `computeDagrePositions`, called unconditionally from the schemas-graph render in `ui/src/index.tsx`. `ui/src/dagLayout.ts` is no longer wired into rendering — it stays in the tree so the pure barycenter logic can be unit-tested without pulling in Dagre. See the file header in `ui/src/dagLayout.ts` for details.
