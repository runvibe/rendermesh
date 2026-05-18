# Overview

RenderMesh is a Rust edge gateway for frontend applications stored in S3/R2-compatible buckets. It receives HTTP requests, resolves the request host to a configured origin, uses a local mirror of that origin's bucket, applies per-origin delivery rules, optionally calls programmable HTTP edge hooks, and returns the final response.

## What RenderMesh Solves

Static hosting is often too rigid for real frontend delivery. Applications may need domain-based routing, SPA fallbacks, redirects, rewrites, CORS, dynamic HTML customization, and observability. RenderMesh centralizes those concerns while keeping frontend artifacts portable in buckets.

## Core Concepts

- **Origin**: A named bucket-backed application.
- **Host mapping**: A global manifest mapping exact or wildcard hosts to origins.
- **Local mirror**: A local copy of each origin's bucket contents.
- **Origin edge config**: A YAML or JSON file inside a bucket that defines delivery behavior.
- **Edge hook**: An external HTTP endpoint called before static delivery.
- **Template store**: An in-memory Handlebars registry containing only HTML templates from the mirrored bucket.

## Request Lifecycle

1. The HTTP route receives a request.
2. RenderMesh extracts host, path, query, headers, and optional client IP headers.
3. The host resolver maps the request host to an origin.
4. CORS headers are derived from the manifest host map.
5. The origin's edge config is loaded from the in-memory edge config store.
6. Configured edge hooks are called in order.
7. Edge hook payloads can return a direct response, headers, params, or a file path.
8. Redirect and rewrite rules are applied when no terminal edge response exists.
9. Static files are served from the local mirror.
10. HTML templates render only when edge params are present.
11. Missing-file behavior is applied if no object is found.
12. The final response is emitted with OpenTelemetry spans across each stage.

## MVP Scope

The MVP includes:

- S3/R2-compatible origin configuration.
- Multiple origins and host mappings.
- Exact and wildcard host resolution.
- Local bucket mirroring.
- Periodic background sync.
- Per-origin edge config via `/_rendermesh/edge.yaml`, `edge.yml`, or `edge.json`.
- Redirects, rewrites, root object, auto-index, and missing-file behavior.
- Global per-origin edge hooks.
- HTML-only Handlebars templates compiled in memory.
- OpenTelemetry tracing.
- Local MinIO lab for manual testing.

The MVP intentionally excludes PostgreSQL, MCP, OpenAPI, Swagger UI, background render queues, cache invalidation APIs, and provider adapters beyond S3-compatible storage.
