# Edge Context Spec

## Status

Implemented.

## Summary

Add an optional per-origin `edge_context` object to the global RenderMesh manifest and include it in every edge hook request for that origin. This lets operators attach deployment, tenant, application, or feature metadata to an origin without encoding those values in request headers, query strings, or edge hook URLs.

## Goals

- Allow S3 and local origins to define static custom context values in the global manifest.
- Send those values to all configured edge hooks for the resolved origin.
- Preserve backward compatibility for existing manifests and edge integrations.
- Keep request-derived data under `request` and origin/runtime metadata under `context`.
- Support arbitrary JSON-compatible values so teams can model strings, booleans, numbers, arrays, and nested objects.

## Non-goals

- Do not add dynamic per-request expression evaluation in the manifest.
- Do not read `edge_context` from per-origin `/.rendermesh/edge.yaml`; it belongs to the global origin definition.
- Do not let edge hooks mutate or persist `edge_context`.
- Do not use `edge_context` for secrets. Values are sent to external edge endpoints and may appear in logs.

## Manifest Contract

`edge_context` is optional on both `s3` and `local` origins.

```yaml
version: 1

runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60

origins:
  storefront:
    type: s3
    bucket: storefront-prod
    endpoint_env: STOREFRONT_STORAGE_ENDPOINT
    region_env: STOREFRONT_STORAGE_REGION
    edge_context:
      tenant_id: loja-123
      app_name: storefront
      environment: production
      theme: dark
      feature_flags:
        checkout_v2: true
        recommendations: false

  docs:
    type: local
    path: ./docs
    edge_context:
      app_name: docs
      audience:
        - public
        - developers

hosts:
  loja.example.com:
    origin: storefront
  docs.example.com:
    origin: docs
```

Equivalent JSON manifest:

```json
{
  "version": 1,
  "runtime": {
    "local_store_dir": "./var/rendermesh/origins",
    "sync_interval_seconds": 60
  },
  "origins": {
    "storefront": {
      "type": "s3",
      "bucket": "storefront-prod",
      "endpoint_env": "STOREFRONT_STORAGE_ENDPOINT",
      "region_env": "STOREFRONT_STORAGE_REGION",
      "edge_context": {
        "tenant_id": "loja-123",
        "app_name": "storefront",
        "environment": "production",
        "theme": "dark",
        "feature_flags": {
          "checkout_v2": true,
          "recommendations": false
        }
      }
    }
  },
  "hosts": {
    "loja.example.com": {
      "origin": "storefront"
    }
  }
}
```

## Edge Hook Request Contract

When an origin defines `edge_context`, RenderMesh includes it under `context.edge_context`.

```json
{
  "context": {
    "bucket": "storefront-prod",
    "ip": "203.0.113.10",
    "origin": "storefront",
    "edge_context": {
      "tenant_id": "loja-123",
      "app_name": "storefront",
      "environment": "production",
      "theme": "dark",
      "feature_flags": {
        "checkout_v2": true,
        "recommendations": false
      }
    }
  },
  "request": {
    "url": "https://loja.example.com/products?sku=abc",
    "path": "/products",
    "querystring": "sku=abc",
    "queryparams": {
      "sku": "abc"
    },
    "method": "GET",
    "headers": {
      "host": "loja.example.com"
    },
    "body": ""
  }
}
```

When an origin does not define `edge_context`, RenderMesh omits `context.edge_context` from the serialized JSON payload.

## DTO Changes

Add `edge_context` to origin manifest DTOs:

```rust
pub struct S3OriginConfig {
    // existing fields
    pub edge_context: Option<serde_json::Value>,
}

pub struct LocalOriginConfig {
    // existing fields
    pub edge_context: Option<serde_json::Value>,
}
```

Add `edge_context` to the edge hook context DTO:

```rust
pub struct EdgeHookContext {
    pub bucket: String,
    pub ip: Option<String>,
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_context: Option<serde_json::Value>,
}
```

## Runtime Design

1. During startup, derive a `BTreeMap<String, serde_json::Value>` from `manifest.origins`.
2. Pass that map into `RenderGatewayService` alongside the existing origin bucket map.
3. Store it in `RenderGatewayService` as shared immutable runtime configuration.
4. When `edge_hook_request` is built for the resolved origin, look up and clone the origin's configured context.
5. Serialize it as `context.edge_context` only when present.

Suggested helper:

```rust
fn origin_edge_contexts(manifest: &RenderMeshManifest) -> BTreeMap<String, serde_json::Value> {
    manifest
        .origins
        .iter()
        .filter_map(|(origin_id, origin)| {
            origin
                .edge_context()
                .cloned()
                .map(|context| (origin_id.clone(), context))
        })
        .collect()
}
```

Add an `OriginConfig::edge_context(&self) -> Option<&serde_json::Value>` accessor so startup and future services do not pattern-match origin variants repeatedly.

## Validation Rules

- `edge_context` must be valid YAML/JSON data accepted by `serde_json::Value`.
- `edge_context` may be an object, array, string, number, boolean, or null, but object values are recommended.
- RenderMesh must not perform environment variable interpolation inside `edge_context`.
- RenderMesh must not reject unknown keys inside `edge_context`.
- Existing `deny_unknown_fields` behavior for origin config must continue rejecting unsupported top-level origin fields, except for the newly supported `edge_context`.

## Security And Operational Guidance

- Do not put credentials, API tokens, private keys, session tokens, or end-user personal data in `edge_context`.
- Treat `edge_context` as static configuration distributed to every edge hook configured for that origin.
- If the value differs per request, use request headers, query parameters, or application-side edge logic instead.
- If the value differs per deployment, update the global manifest and restart or redeploy RenderMesh using the normal configuration flow.

## Compatibility

- Existing manifests remain valid because `edge_context` is optional.
- Existing edge integrations remain valid because the new field is additive.
- Omitting absent `edge_context` avoids changing payload shape for origins that do not opt in.
- Edges that deserialize strictly should continue working as long as they ignore unknown fields or the origin does not enable `edge_context`.

## Tests

Required test coverage:

1. YAML manifest parses `edge_context` for S3 origins.
2. YAML manifest parses `edge_context` for local origins.
3. JSON manifest parses nested `edge_context`.
4. Existing manifests without `edge_context` continue parsing.
5. Edge hook request includes `context.edge_context` for configured origins.
6. Edge hook request omits `context.edge_context` for origins without it.
7. The same `edge_context` is sent to every edge hook in a chain for the resolved origin.

## Documentation Updates

Update:

- `README.md`: add a short example in the manifest section.
- `docs/configuration.md`: document the origin-level `edge_context` field.
- `docs/edge-hooks.md`: document `context.edge_context` in the request payload.

## Acceptance Criteria

- Operators can configure `edge_context` under any S3 or local origin in YAML or JSON manifests.
- Edge hooks receive the configured value at `context.edge_context`.
- Origins without `edge_context` keep the current serialized payload shape.
- `cargo test` passes with the repository's supported Rust toolchain.
