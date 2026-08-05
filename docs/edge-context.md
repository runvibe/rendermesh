# Edge Context

`edge_context` lets an origin attach static, non-secret metadata to every edge hook request for that origin.

Use it for values that belong to the deployed application or tenant, not to an individual HTTP request. Good examples include tenant id, app name, environment, theme, deployment channel, or feature flags.

## Configure

Add `edge_context` under any `s3` or `local` origin in the global manifest.

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

JSON manifests use the same field:

```json
{
  "origins": {
    "storefront": {
      "type": "s3",
      "bucket": "storefront-prod",
      "endpoint_env": "STOREFRONT_STORAGE_ENDPOINT",
      "region_env": "STOREFRONT_STORAGE_REGION",
      "edge_context": {
        "tenant_id": "loja-123",
        "environment": "production",
        "feature_flags": {
          "checkout_v2": true
        }
      }
    }
  }
}
```

## Apply It With An Edge Hook

`edge_context` is configured in the global manifest, while the edge hook URL is configured in the origin edge config stored with the application files.

Global manifest:

```yaml
origins:
  storefront:
    type: s3
    bucket: storefront-prod
    endpoint_env: STOREFRONT_STORAGE_ENDPOINT
    region_env: STOREFRONT_STORAGE_REGION
    edge_context:
      tenant_id: loja-123
      locale: pt-BR
      currency: BRL
      theme: dark
      feature_flags:
        checkout_v2: true

hosts:
  loja.example.com:
    origin: storefront
```

Origin file `/.rendermesh/edge.yaml`:

```yaml
version: 1

edge:
  root_object: /index.html
  auto_rewrite_index: true

edges:
  - name: storefront-edge
    url: https://edge-api.example.com/render-context
    timeout_ms: 1000

missing:
  action: not_found
  page: /index.html
```

Then RenderMesh sends `context.edge_context` to `https://edge-api.example.com/render-context` for requests resolved to `loja.example.com`.

## Edge Hook Payload

When the resolved origin defines `edge_context`, RenderMesh sends it as `context.edge_context`.

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

If an origin does not configure `edge_context`, RenderMesh omits `context.edge_context` from the JSON payload.

## Edge API Examples

### Read Context In Node.js

```js
import express from "express";

const app = express();
app.use(express.json());

app.post("/render-context", (req, res) => {
  const edgeContext = req.body.context.edge_context;
  const request = req.body.request;

  if (edgeContext?.feature_flags?.checkout_v2) {
    return res.json({
      headers: { "x-checkout-version": "v2" },
      params: {
        tenantId: edgeContext.tenant_id,
        locale: edgeContext.locale,
        currency: edgeContext.currency,
        theme: edgeContext.theme,
        currentPath: request.path
      }
    });
  }

  res.json({});
});

app.listen(4000);
```

The response above tells RenderMesh to continue serving the resolved HTML file, but render it with the returned `params`. Those params are available to Handlebars templates.

### Route To A Tenant-Specific File

The edge can use `edge_context` to select an internal file from the local mirror:

```js
app.post("/render-context", (req, res) => {
  const edgeContext = req.body.context.edge_context;

  if (edgeContext?.tenant_id === "loja-123") {
    return res.json({
      file_path: "/tenants/loja-123/index.html",
      params: {
        theme: edgeContext.theme
      }
    });
  }

  res.json({});
});
```

`file_path` can point to mirrored origin files, including internal files under `/.rendermesh`, but it must start with `/` and must not contain `..`.

### Add Response Headers Without Rendering

The edge can also use context only to add headers and let normal static delivery continue:

```js
app.post("/render-context", (req, res) => {
  const edgeContext = req.body.context.edge_context;

  res.json({
    headers: {
      "x-tenant-id": edgeContext?.tenant_id ?? "unknown",
      "x-app-environment": edgeContext?.environment ?? "unknown"
    }
  });
});
```

### Return A Direct Response

For maintenance windows or feature gates, the edge can use context to stop the chain and return a direct body:

```js
app.post("/render-context", (req, res) => {
  const edgeContext = req.body.context.edge_context;

  if (edgeContext?.maintenance === true) {
    return res.status(503).json({
      body: "Store temporarily unavailable",
      headers: {
        "content-type": "text/plain"
      }
    });
  }

  res.json({});
});
```

RenderMesh uses the edge HTTP status for terminal edge responses.

## Rules

- `edge_context` accepts any valid YAML/JSON value, but an object is recommended.
- RenderMesh does not interpolate environment variables inside `edge_context`.
- The value is static per origin until the global manifest changes and RenderMesh is restarted or redeployed.
- The value is sent to every edge hook configured for the resolved origin.
- Existing origins remain compatible because the field is optional.

## Security

Do not put credentials, API tokens, private keys, session tokens, or end-user personal data in `edge_context`. The value is sent to external edge endpoints and may appear in logs.

If a value changes per request, pass it through request headers, query parameters, or application-side edge logic instead.
