# rust-api-template

Starter project for building APIs with Axum, PostgreSQL, and OpenTelemetry. It exposes:

- `/health` as a liveness endpoint that returns `200 OK` with a JSON status payload.
- `/echo` to reflect the incoming request across all HTTP verbs with tracing spans per method when OTEL is enabled.
- `/mcp` as an optional MCP Streamable HTTP endpoint for `health_check` and `echo_request` tools when enabled.

## Template Bootstrap

After creating a new repository from this template, run:

- `./scripts/init-template.sh`

The script uses the current repository directory name as the new Cargo package/bin name, updates the main hardcoded references (`Cargo.toml`, Rust imports, README, `.env.example`, and `Dockerfile.artifact`), then runs `cargo build` and `cargo test`.

If you want to override the detected name, pass it explicitly:

- `./scripts/init-template.sh my-new-api`

## Getting Started

### Prerequisites

- Rust toolchain (stable).
- PostgreSQL access. You can use the included Docker Compose if you want a local DB.

### Quick start

1. Start a database (optional example using Compose):
   - `docker compose up -d postgres jaeger`
   - If you are upgrading from an older Postgres image and see a volume layout error, recreate the Postgres volume once:
   - `docker compose down -v`
   - `docker compose up -d postgres jaeger`
2. Set `DATABASE_URL` (example for the Compose service):
   - `export DATABASE_URL=postgres://postgres:postgres@localhost:5453/postgres`
3. Optionally set:
   - `APP_HOST` and `APP_PORT` (defaults: `127.0.0.1:8080`).
   - `APP_CORS_ALLOW_ORIGINS` (comma-separated or `*`) and `APP_BODY_LIMIT_BYTES`.
   - `OTEL_ENABLED=false` to disable OpenTelemetry export and HTTP tracing middleware while keeping structured logs.
   - `MCP_ENABLED=true` to expose the MCP endpoint.
   - `MCP_PATH=/mcp` to change the MCP path.
   - `MCP_ALLOWED_ORIGINS=*` to keep the MCP endpoint fully open, or provide a comma-separated allowlist if you want to restrict it.
4. Run:
   - `cargo run`

Migrations are managed by SQLx and executed on startup from `migrations/`.
Swagger UI is available at `/docs` with the generated OpenAPI contract.

### MCP HTTP

This template can expose an MCP server over Streamable HTTP in stateless JSON mode.

- It is disabled by default and only mounts when `MCP_ENABLED=true`.
- The MCP endpoint is intentionally not included in `/openapi.json`.
- CORS is permissive by default for both the REST API and MCP. Set `APP_CORS_ALLOW_ORIGINS` or `MCP_ALLOWED_ORIGINS` only if you want to restrict them.

The first version exposes two tools:

- `health_check` returns the service status and version.
- `echo_request` mirrors `method`, `path`, `headers`, and `body` from the tool input.
- When MCP is enabled, startup logs include the MCP endpoint URL.

To test with the inspector:

1. Start the API with MCP enabled:
   - `MCP_ENABLED=true cargo run`
2. Launch the inspector:
   - `npx @modelcontextprotocol/inspector`
3. Connect using Streamable HTTP:
   - `http://127.0.0.1:8080/mcp`

Current limitations:

- no resources or prompts
- no bearer auth
- no SSE/session mode; `GET /mcp` returns `405 Method Not Allowed`

### Artifact image

`Dockerfile.artifact` expects a prebuilt binary in `artifacts/<bin-name>/<arch>/` and accepts `BIN_NAME` as a build argument. Example:

`docker build -f Dockerfile.artifact --build-arg TARGETARCH=amd64 --build-arg BIN_NAME=rust-api-template .`

### SQLx note

The SQLx query macros use the database schema at compile time. Make sure `DATABASE_URL` is set when building. If you prefer offline builds, run `cargo sqlx prepare` and set `SQLX_OFFLINE=true`.

### Testing

- Unit tests: `cargo test`
- Integration tests: `cargo test --test integration`
  - Requires Docker; tests spin up `pgvector/pgvector:pg18` and, unless `OTEL_ENABLED=false`, a Jaeger collector via testcontainers.

## Architecture

This template is organized around four main layers:

- `routes/`: transport and protocol adapters for HTTP and MCP
- `services/`: business rules and use-case orchestration
- `repositories/`: persistence and external integration adapters
- `dto/`: request/response contracts, validation, and data transformation structs

Preferred flow:

- `route -> dto -> service -> repository -> service -> dto -> route`

Guidelines:

- keep HTTP and MCP details inside `routes/`
- keep business decisions inside `services/`
- keep SQLx, queues, and external API clients inside `repositories/`
- keep payload contracts and transformation structs inside `dto/`
- let `AppState` carry concrete repositories instead of exposing raw driver clients when possible

## Project Layout

```
src/
  config.rs         # environment loading
  db.rs             # connection pool + migrations
  dto/              # request/response contracts and shared transport payloads
  repositories/     # DB, queue, cache, and external integration adapters
  routes/           # HTTP and MCP transport handlers plus wiring
  services/         # business rules and use-case orchestration
```

Adjust the repositories and services to fit your application, then expand the router with new modules as needed.
