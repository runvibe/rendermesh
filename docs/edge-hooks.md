# Edge Hooks

Edge hooks are programmable HTTP middleware endpoints called before static file delivery. They let external applications influence rendering without embedding application-specific logic inside RenderMesh.

## Request

RenderMesh sends a `POST` request with JSON:

```json
{
  "context": {
    "bucket": "bucket_my_app_123",
    "ip": "203.0.113.10",
    "origin": "my_app",
    "edge_context": {
      "tenant_id": "loja-123",
      "environment": "production"
    }
  },
  "request": {
    "url": "https://myapp.com/path?query=1",
    "path": "/path",
    "querystring": "query=1",
    "queryparams": {
      "query": "1"
    },
    "method": "GET",
    "headers": {
      "host": "myapp.com"
    },
    "body": ""
  }
}
```

## `context`

- `bucket`: Bucket name for S3 origins. For local origins, RenderMesh currently sends the origin id for compatibility with the existing edge DTO.
- `ip`: Client IP inferred from `x-forwarded-for` or `x-real-ip`; `null` when unavailable.
- `origin`: Origin id from the global manifest.
- `edge_context`: Optional static value from the resolved origin's global manifest config. Omitted when the origin does not define it.

## `request`

- `url`: Full request URL reconstructed by RenderMesh.
- `path`: Request path exactly as received by RenderMesh.
- `querystring`: Raw query string without the leading `?`; empty string when no query was sent.
- `queryparams`: Query string parsed as an object with percent-decoded string keys and values. If a key appears more than once, the last value wins.
- `method`: Original method. The MVP serves `GET`, `HEAD`, and `OPTIONS`.
- `headers`: Original request headers normalized to lowercase when they can be represented as UTF-8.
- `body`: Currently always an empty string in the MVP.

## Response Payloads

The HTTP status returned by the edge API is used as the response status for terminal edge responses.

### Continue With Headers

```json
{
  "headers": {
    "x-edge": "yes"
  }
}
```

RenderMesh stores safe response headers and continues normal delivery.

### Direct Body

```json
{
  "body": "Direct response from edge",
  "headers": {
    "x-edge": "direct"
  }
}
```

RenderMesh returns the edge body directly and does not read a file from the local mirror.

### Render Current Target With Params

```json
{
  "params": {
    "title": "Hello"
  }
}
```

RenderMesh resolves the current target file and renders it as a Handlebars template. This only works for HTML files compiled into the template store.

### Serve A Specific File

```json
{
  "file_path": "/static.html"
}
```

RenderMesh serves the selected file from the local mirror. Edge-selected `file_path` can point to any mirrored origin file, including internal files under `/.rendermesh`. Direct public requests to `/.rendermesh/*` remain blocked.

If the selected file does not exist, RenderMesh executes the origin's configured `missing` behavior.

`file_path` must be named exactly `file_path`, must start with `/`, must not contain `..`, and must not contain control characters. For example, a request to `https://example.com/data.json` can be intercepted by an edge hook and served from an internal mirrored file:

```json
{
  "file_path": "/.rendermesh/config/data.json"
}
```

The value `.rendermesh/config/data.json` is rejected because it does not start with `/`. The field name `file_Path` is ignored by the contract because edge payload fields are snake_case.

### Serve And Render A Specific File

```json
{
  "file_path": "/index.html",
  "params": {
    "title": "Hello"
  }
}
```

RenderMesh serves the selected file and renders it as HTML with the provided params.

## Header Safety

Unsafe edge headers are ignored:

- `connection`
- `content-encoding`
- `content-length`
- `host`
- `keep-alive`
- `proxy-authenticate`
- `proxy-authorization`
- `te`
- `trailer`
- `transfer-encoding`
- `upgrade`

## Failure Behavior

- Edge timeout returns `504 Gateway Timeout`.
- Edge connection or request failure returns `502 Bad Gateway`.
- Invalid edge payload returns `502 Bad Gateway`.
- Template params for a non-HTML file return `415 Unsupported Media Type`.

## Local Example

The local edge API is implemented in [examples/local/edge-api/server.mjs](../examples/local/edge-api/server.mjs).
