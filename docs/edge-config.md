# Origin Edge Config

Each origin can include an edge config inside its bucket. RenderMesh checks these paths in order:

1. `/_rendermesh/edge.yaml`
2. `/_rendermesh/edge.yml`
3. `/_rendermesh/edge.json`

The file controls request behavior for that origin. YAML and JSON use the same schema.

If the file is missing, RenderMesh uses safe defaults. If the file exists but is invalid, only that origin becomes unavailable and requests for it return `500` until a valid config is synced.

## Minimal Config

```yaml
version: 1

edge:
  root_object: /index.html
  auto_rewrite_index: true

missing:
  action: not_found
  page: /index.html
```

The same config in JSON:

```json
{
  "version": 1,
  "edge": {
    "root_object": "/index.html",
    "auto_rewrite_index": true
  },
  "missing": {
    "action": "not_found",
    "page": "/index.html"
  }
}
```

## `edge`

- `root_object`: Object served for `/`.
- `auto_rewrite_index`: When true, `/docs` can resolve to `/docs/index.html` if the direct object is missing.

## `missing`

`missing` controls what happens when no object can be served.

Return `404` using a page body:

```yaml
missing:
  action: not_found
  page: /index.html
```

Serve another file with `200`:

```yaml
missing:
  action: serve
  path: /index.html
```

Redirect:

```yaml
missing:
  action: redirect
  to: /
  status: 302
```

## Redirects

Redirects are evaluated after edge hooks continue and before static file lookup.

```yaml
redirects:
  - from: /legacy
    to: /static.html
    status: 302
```

Query strings are preserved when applicable.

## Rewrites

Rewrites change the internal object target without changing the browser URL.

```yaml
rewrites:
  - from: /home
    to: /index.html
```

## Edge Hooks

Edge hooks run before static delivery.

```yaml
edges:
  - name: local-lab-edge
    url: http://127.0.0.1:4010/edge
    timeout_ms: 800
```

The MVP supports global per-origin hooks. Per-route hooks and after-response hooks are intentionally outside the current MVP.

## Local Example

See [examples/local/bucket/_rendermesh/edge.yaml](../examples/local/bucket/_rendermesh/edge.yaml) for a working config with redirects, rewrites, auto-index, missing behavior, and an edge hook.
