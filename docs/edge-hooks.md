# Edge Hooks

Edge hooks are programmable HTTP middleware endpoints called before static file delivery. They let external applications influence rendering without embedding application-specific logic inside RenderMesh.

## Request

RenderMesh sends a `POST` request with JSON:

```json
{
  "context": {
    "bucket": "bucket_my_app_123",
    "ip": "203.0.113.10",
    "origin": "my_app"
  },
  "request": {
    "url": "https://myapp.com/path?query=1",
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

## `request`

- `url`: Full request URL reconstructed by RenderMesh.
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

RenderMesh serves the selected file from the local mirror.

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
