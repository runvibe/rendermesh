# Templates

RenderMesh supports Handlebars template rendering for HTML only.

## HTML-Only Rule

Files are compiled as templates when either condition is true:

- The object path ends with `.html` or `.htm`.
- The object metadata content type is `text/html` or compatible, such as `text/html; charset=utf-8`.

Non-HTML files are never compiled as templates.

## Compile Time

Templates are compiled after bucket sync:

- Once during startup sync.
- Again after every successful background sync for that origin.

Compiled templates are stored in an in-memory registry owned by the main runtime and shared through `Arc`.

## Render Time

RenderMesh renders a template only when an edge payload includes `params`.

No params:

```json
{
  "headers": {
    "x-edge": "static-pass-through"
  }
}
```

The file is served directly from the local mirror.

With params:

```json
{
  "params": {
    "title": "Hello"
  }
}
```

The resolved HTML file is rendered with Handlebars using the params.

## File Path With Params

```json
{
  "file_path": "/index.html",
  "params": {
    "title": "Hello"
  }
}
```

RenderMesh selects `/index.html` from the local mirror and renders the compiled template for that path.

## Error Cases

- Params for a non-HTML file return `415 Unsupported Media Type`.
- Template render failures return `502 Bad Gateway`.
- Missing template entries are treated as unsupported HTML rendering for the request.

## Static HTML Without Params

HTML files are not automatically rendered. If the edge payload does not include `params`, the HTML is returned exactly as stored in the bucket.
