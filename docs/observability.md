# Observability

RenderMesh uses `tracing` and OpenTelemetry to expose the request lifecycle. The local lab includes Jaeger for manual inspection.

## Environment

OpenTelemetry is enabled by default. Useful variables:

- `OTEL_ENABLED`: Set to `false` to disable OTEL export and keep local formatted logs.
- `OTEL_SERVICE_NAME`: Service name reported to OTEL. Defaults to the crate name.
- `OTEL_EXPORTER_OTLP_PROTOCOL`: `grpc`, `http/protobuf`, or `http/json`.
- `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint.
- `OTEL_USE_SIMPLE_EXPORTER`: Set to `true` to use the simple exporter instead of batch exporting.
- `RUST_LOG`: Standard tracing filter, for example `info` or `rendermesh=debug`.

## Local Jaeger

Start Jaeger with the local lab:

```bash
docker compose up -d jaeger
```

Jaeger UI:

```text
http://127.0.0.1:16686
```

Example HTTP OTLP config:

```bash
export OTEL_ENABLED=true
export OTEL_SERVICE_NAME=rendermesh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318/v1/traces
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

Example gRPC OTLP config:

```bash
export OTEL_ENABLED=true
export OTEL_SERVICE_NAME=rendermesh
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4317
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
```

## Span Names

RenderMesh emits structured spans across the full fallback path:

- `rendermesh.request`: Incoming fallback request, final status, and total duration.
- `rendermesh.read_body`: Request body read duration and byte count.
- `rendermesh.gateway`: Gateway orchestration with host, path, origin, status, and duration.
- `rendermesh.resolve_host`: Host resolution against the manifest.
- `rendermesh.cors`: CORS derivation from the host map.
- `rendermesh.edge_config`: Origin edge config lookup.
- `rendermesh.edge_chain`: Edge hook chain execution.
- `rendermesh.edge_hook`: One configured edge hook.
- `rendermesh.redirect`: Redirect rule lookup.
- `rendermesh.resolve_target`: Rewrite and root object resolution.
- `rendermesh.static`: Static path resolution and auto-index behavior.
- `rendermesh.object`: Local mirror object lookup.
- `rendermesh.template_render`: Handlebars render duration.
- `rendermesh.missing`: Missing-file behavior.
- `rendermesh.response_build`: Final response assembly.

## Edge HTTP Events

External edge calls also emit events:

- `edge_hook_request_start`
- `edge_hook_request_finish`
- `edge_http_request_start`
- `edge_http_response_received`
- `edge_http_payload_decoded`

Important fields include `duration_ms`, `status`, `origin_id`, `bucket`, `path`, `edge_url`, `timeout_ms`, `outcome`, `hit`, `rendered`, and `body_bytes`.
